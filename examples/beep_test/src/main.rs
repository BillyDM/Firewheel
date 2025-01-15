use std::time::{Duration, Instant};

use firewheel::{error::UpdateError, nodes::BeepTestParams, FirewheelContext};

const BEEP_FREQUENCY_HZ: f32 = 440.0;
const BEEP_NORMALIZED_VOLUME: f32 = 0.45;
const BEEP_DURATION: Duration = Duration::from_secs(4);
const UPDATE_INTERVAL: Duration = Duration::from_millis(15);

fn main() {
    simple_log::quick!("info");

    println!("Firewheel beep test...");

    let mut cx = FirewheelContext::new(Default::default());
    cx.start_stream(Default::default()).unwrap();

    let beep_test_params = BeepTestParams {
        freq_hz: BEEP_FREQUENCY_HZ,
        normalized_volume: BEEP_NORMALIZED_VOLUME,
        enabled: true,
    };

    let beep_test_id = cx.add_node(beep_test_params.clone());
    let graph_out_id = cx.graph_out_node();

    cx.connect(beep_test_id, graph_out_id, &[(0, 0), (0, 1)], false)
        .unwrap();

    let start = Instant::now();
    while start.elapsed() < BEEP_DURATION {
        if let Err(e) = cx.update() {
            log::error!("{:?}", &e);

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
