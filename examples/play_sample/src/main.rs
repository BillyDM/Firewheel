use std::time::Duration;

use clap::Parser;
use firewheel::{
    cpal::CpalStream,
    nodes::sampler::{SamplerNode, SamplerState},
    FirewheelContext,
};

const UPDATE_INTERVAL: Duration = Duration::from_millis(15);

#[derive(clap::Parser)]
struct Cli {
    /// The path to the audio file to play
    path: std::path::PathBuf,
}

fn main() {
    tracing::subscriber::set_global_default(
        tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::DEBUG)
            .finish(),
    )
    .unwrap();

    let args = Cli::parse();

    // --- Start the context and get the sample rate of the audio stream. ----------------

    let mut cx = FirewheelContext::new(Default::default());
    let mut stream = CpalStream::new(&mut cx, Default::default()).unwrap();

    let sample_rate = cx.stream_info().unwrap().sample_rate;

    // --- Create a sampler state, and add it as a node in the audio graph. --------------

    let mut sampler_node = SamplerNode::default();

    let sampler_id = cx
        .add_node(sampler_node, None)
        .expect("Sampler node should construct without error");

    let graph_out_id = cx.graph_out_node_id();

    cx.connect(sampler_id, graph_out_id, &[(0, 0), (1, 1)], false)
        .unwrap();

    // --- Load a sample into memory, and tell the node to use it and play it. -----------

    let probed = symphonium::probe_from_file(
        args.path, None, // Custom container probe
    )
    .unwrap();
    let sample = firewheel::dyn_symphonium_resource(
        symphonium::decode(
            probed,
            &symphonium::DecodeConfig::default(),
            Some(sample_rate), // target sample rate
            None,              // An optional cache
            None,              // Custom codec registry
        )
        .unwrap(),
    );

    cx.queue_event_for(sampler_id, SamplerNode::set_dyn_sample_event(sample));

    sampler_node.start_or_restart();
    cx.queue_event_for(sampler_id, sampler_node.sync_play_event());

    // Get the playback ID after calling `start_or_restart()` to detect when
    // this specific playback sequence has finished playing.
    //
    // Note, this ID becomes invalidated once the sampler node receives a
    // new "play" event.
    let playback_id = sampler_node.playback_id();

    // --- Simulated update loop ---------------------------------------------------------

    loop {
        // Update the firewheel context.
        // This must be called regularly (i.e. once every frame).
        if let Err(e) = cx.update() {
            tracing::error!("{:?}", &e);
        }

        // Using `playback_finished()` is more reliable than using
        // `currently_stopped()` since it takes into account the delay
        // between when the play event is created and when the sampler
        // node receives the event.
        if cx
            .node_state::<SamplerState>(sampler_id)
            .unwrap()
            .playback_finished(playback_id)
        {
            // Sample has finished playing.
            break;
        }

        // Log any stream errors/warnings that have occurred.
        stream.log_status();

        // The stream has stopped unexpectedly (i.e the user has
        // unplugged their headphones.)
        //
        // Typically you should start a new stream as soon as
        // possible to resume processing (even if it's a dummy
        // output device).
        //
        // In this example we just quit the application.
        if !stream.all_streams_ok() {
            break;
        }

        std::thread::sleep(UPDATE_INTERVAL);
    }

    println!("finished");
}
