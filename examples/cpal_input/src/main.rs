use std::time::Duration;

use firewheel::{
    channel_config::ChannelCount, error::UpdateError, CpalConfig, FirewheelConfig, FirewheelContext,
};

const UPDATE_INTERVAL: Duration = Duration::from_millis(15);

fn main() {
    tracing::subscriber::set_global_default(
        tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::DEBUG)
            .finish(),
    )
    .unwrap();

    let mut cx = FirewheelContext::new(FirewheelConfig {
        num_graph_inputs: ChannelCount::new(1).unwrap(),
        ..Default::default()
    });
    cx.start_stream(CpalConfig {
        output: Default::default(),
        input: Some(Default::default()),
    })
    .unwrap();

    dbg!(cx.stream_info());

    let graph_in_node_id = cx.graph_in_node_id();
    let graph_out_node_id = cx.graph_out_node_id();

    cx.connect(
        graph_in_node_id,
        graph_out_node_id,
        &[(0, 0), (0, 1)],
        false,
    )
    .unwrap();

    loop {
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
}
