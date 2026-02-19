use std::time::{Duration, Instant};

use firewheel::{
    dsp::volume::Volume, error::UpdateError, nodes::beep_test::BeepTestNode, FirewheelContext,
};

const BEEP_FREQUENCY_HZ: f32 = 440.0;
const BEEP_VOLUME: Volume = Volume::Linear(0.45);
const BEEP_DURATION: Duration = Duration::from_secs(4);
const UPDATE_INTERVAL: Duration = Duration::from_millis(15);

fn main() {
    tracing::subscriber::set_global_default(
        tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::DEBUG)
            .finish(),
    )
    .unwrap();

    println!("Firewheel RtAudio beep test...");

    let mut cx = FirewheelContext::new(Default::default());
    cx.start_stream(Default::default()).unwrap();

    let beep_test_node = BeepTestNode {
        freq_hz: BEEP_FREQUENCY_HZ,
        volume: BEEP_VOLUME,
        enabled: true,
    };

    let beep_test_id = cx.add_node(beep_test_node, None);
    let graph_out_id = cx.graph_out_node_id();

    cx.connect(beep_test_id, graph_out_id, &[(0, 0), (0, 1)], false)
        .unwrap();

    let start = Instant::now();
    while start.elapsed() < BEEP_DURATION {
        if let Err(e) = cx.update() {
            tracing::error!("{:?}", &e);

            if let UpdateError::StreamStoppedUnexpectedly(_) = e {
                // The stream has stopped unexpectedly (i.e the user has
                // unplugged their headphones.)
                //
                // Typically you should start a new stream as soon as
                // possible to resume processing (even if it's a dummy
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
