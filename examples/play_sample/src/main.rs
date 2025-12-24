use std::time::Duration;

use clap::Parser;
use firewheel::{
    error::UpdateError,
    nodes::sampler::{SamplerNode, SamplerState},
    FirewheelContext,
};
use symphonium::SymphoniumLoader;

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
    cx.start_stream(Default::default()).unwrap();

    let sample_rate = cx.stream_info().unwrap().sample_rate;

    // --- Create a sampler state, and add it as a node in the audio graph. --------------

    let mut sampler_node = SamplerNode::default();

    let sampler_id = cx.add_node(sampler_node.clone(), None);
    let graph_out_id = cx.graph_out_node_id();

    cx.connect(sampler_id, graph_out_id, &[(0, 0), (1, 1)], false)
        .unwrap();

    // --- Load a sample into memory, and tell the node to use it and play it. -----------

    let mut loader = SymphoniumLoader::new();
    let sample =
        firewheel::load_audio_file(&mut loader, args.path, sample_rate, Default::default())
            .unwrap()
            .into_dyn_resource();

    sampler_node.set_sample(sample);
    cx.queue_event_for(sampler_id, sampler_node.sync_sample_event());

    sampler_node.start_or_restart();
    cx.queue_event_for(sampler_id, sampler_node.sync_play_event());

    // Manually set the shared `stopped` flag. This is needed to account for the delay
    // between sending a play event and the node's processor receiving that event.
    cx.node_state::<SamplerState>(sampler_id)
        .unwrap()
        .mark_stopped();

    // --- Simulated update loop ---------------------------------------------------------

    loop {
        if cx.node_state::<SamplerState>(sampler_id).unwrap().stopped() {
            // Sample has finished playing.
            break;
        }

        if let Err(e) = cx.update() {
            tracing::error!("{:?}", &e);

            if let UpdateError::StreamStoppedUnexpectedly(_) = e {
                // The stream has stopped unexpectedly (i.e the user has
                // unplugged their headphones.)
                //
                // Typically you should start a new stream as soon as
                // possible to resume processing (event if it's a dummy
                // output device).
                //
                // In this example we just quit the application.
                break;
            }
        }

        std::thread::sleep(UPDATE_INTERVAL);
    }

    println!("finished");
}
