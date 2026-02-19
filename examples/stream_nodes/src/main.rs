use core::{num::NonZeroU32, time::Duration};
// The use of `bevy_platform` is optional, but it is recommended for better
// compatibility with webassembly, no_std, and platforms without 64 bit atomics.
use bevy_platform::sync::Arc;
use firewheel::{
    channel_config::NonZeroChannelCount,
    error::UpdateError,
    nodes::stream::{
        reader::{StreamReaderConfig, StreamReaderNode, StreamReaderState},
        writer::{PushStatus, StreamWriterConfig, StreamWriterNode, StreamWriterState},
        ReadStatus, ResamplingChannelConfig,
    },
    FirewheelContext,
};

const CHANNEL_CAPACITY_SECONDS: f64 = 4.0;
const UPDATE_INTERVAL: Duration = Duration::from_millis(15);
const IN_SAMPLE_RATE: NonZeroU32 = NonZeroU32::new(44100).unwrap();
const OUT_SAMPLE_RATE: NonZeroU32 = NonZeroU32::new(48000).unwrap();
const NUM_CHANNELS: NonZeroChannelCount = NonZeroChannelCount::STEREO;

fn main() {
    tracing::subscriber::set_global_default(
        tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::DEBUG)
            .finish(),
    )
    .unwrap();

    let mut cx = FirewheelContext::new(Default::default());
    cx.start_stream(Default::default()).unwrap();
    let output_stream_sample_rate = cx.stream_info().unwrap().sample_rate;

    dbg!(output_stream_sample_rate);

    let graph_out_node_id = cx.graph_out_node_id();

    let stream_writer_id = cx.add_node(
        StreamWriterNode,
        Some(StreamWriterConfig {
            channels: NUM_CHANNELS,
            ..Default::default()
        }),
    );
    let stream_reader_id = cx.add_node(
        StreamReaderNode,
        Some(StreamReaderConfig {
            channels: NUM_CHANNELS,
        }),
    );

    cx.connect(
        stream_writer_id,
        graph_out_node_id,
        &[(0, 0), (1, 1)],
        false,
    )
    .unwrap();
    cx.connect(stream_writer_id, stream_reader_id, &[(0, 0), (1, 1)], false)
        .unwrap();

    let event = cx
        .node_state_mut::<StreamWriterState>(stream_writer_id)
        .unwrap()
        .start_stream(
            IN_SAMPLE_RATE,
            output_stream_sample_rate,
            ResamplingChannelConfig {
                // By default this is set to `0.4` (400 ms). You will probably want a larger
                // capacity buffer depending on your use case. Generally this value should
                // be at least twice as large as the size of packets you intend to send.
                capacity_seconds: CHANNEL_CAPACITY_SECONDS,
                // By default the channel will try to autocorrect underflows and overflows
                // by discarding samples and pushing zero samples if a certain threshold
                // is reached. Set this to `None` to disable this behavior.
                overflow_autocorrect_percent_threshold: None,
                underflow_autocorrect_percent_threshold: None,
                ..Default::default()
            },
        )
        .unwrap();
    // This event must be sent to the node's processor for the stream to take effect.
    cx.queue_event_for(stream_writer_id, event.into());

    let event = cx
        .node_state_mut::<StreamReaderState>(stream_reader_id)
        .unwrap()
        .start_stream(
            OUT_SAMPLE_RATE,
            output_stream_sample_rate,
            ResamplingChannelConfig {
                // For stream readers, the `latency_seconds` value should also be at least
                // the size of packets you intend to read. Here, we use twice that size to
                // be safe.
                latency_seconds: 0.3,
                // By default this is set to `0.4` (400 ms). You will probably want a larger
                // capacity buffer depending on your use case. Generally this value should
                // be at least twice as large as the size of packets you intend to send.
                //
                // This value should also be at least twice as large as `latency_seconds`.
                capacity_seconds: 0.6,
                // By default the channel will try to autocorrect underflows and overflows
                // by discarding samples and pushing zero samples if a certain threshold
                // is reached. Set this to `None` to disable this behavior.
                overflow_autocorrect_percent_threshold: None,
                underflow_autocorrect_percent_threshold: None,
                ..Default::default()
            },
        )
        .unwrap();
    // This event must be sent to the node's processor for the stream to take effect.
    cx.queue_event_for(stream_reader_id, event.into());

    // Wrap the handles in an `Arc<Mutex<T>>>` so that we can send them to other threads.
    let stream_writer_handle = Arc::new(
        cx.node_state::<StreamWriterState>(stream_writer_id)
            .unwrap()
            .handle(),
    );
    let stream_reader_handle = Arc::new(
        cx.node_state::<StreamReaderState>(stream_reader_id)
            .unwrap()
            .handle(),
    );

    std::thread::spawn(move || {
        let mut phasor: f32 = 0.0;
        let phasor_inc: f32 = 440.0 / IN_SAMPLE_RATE.get() as f32;

        // We will send packets of data that are 1 second long.
        let packet_frames = IN_SAMPLE_RATE.get() as usize;

        let mut in_buf = vec![0.0; packet_frames * NUM_CHANNELS.get().get() as usize];

        loop {
            let mut handle = stream_writer_handle.lock().unwrap();

            // If this happens excessively in Release mode, you may want to consider
            // increasing [`StreamWriterConfig::channel_config.latency_seconds`].
            if handle.underflow_occurred() {
                println!("Underflow occured in stream writer node!");
            }

            // If this happens excessively in Release mode, you may want to consider
            // increasing [`StreamWriterConfig::channel_config.capacity_seconds`]. For
            // example, if you are streaming data from a network, you may want to
            // increase the capacity to several seconds.
            if handle.overflow_occurred() {
                println!("Overflow occured in stream writer node!");
            }

            // Wait until the node's processor is ready to receive data.
            if handle.is_ready() {
                // Here, if the value drops below the size of a packet `1.0`, then we know we
                // should push a new packet of data.
                //
                // Alternatively you could do:
                //
                // while handle.occupied_seconds().unwrap() < handle.latency_seconds() {
                //
                // or
                //
                // while handle.available_frames() >= packet_frames {
                //
                while handle.occupied_seconds().unwrap() < 1.0 {
                    // Generate a sine wave on all channels.
                    for chunk in in_buf.chunks_exact_mut(NUM_CHANNELS.get().get() as usize) {
                        let val = (phasor * std::f32::consts::TAU).sin() * 0.5;
                        phasor = (phasor + phasor_inc).fract();

                        for s in chunk.iter_mut() {
                            *s = val;
                        }
                    }

                    let status = handle.push_interleaved(&in_buf);

                    match status {
                        PushStatus::Ok => {
                            println!("Successfully wrote data");
                        }
                        PushStatus::OutputNotReady => {
                            // The output stream is not ready yet.
                        }
                        PushStatus::OverflowOccurred { num_frames_pushed } => {
                            // An overflow occured. This may result in audible audio
                            // glitches.
                            println!(
                                "Overflow occured in stream writer node! Number of frames discarded: {}",
                                packet_frames - num_frames_pushed
                            );
                        }
                        PushStatus::UnderflowCorrected {
                            num_zero_frames_pushed,
                        } => {
                            // An underflow occured. This may result in audible audio
                            // glitches.
                            println!(
                                "Underflow occured in stream writer node! Number of frames dropped: {}",
                                packet_frames - num_zero_frames_pushed
                            );
                        }
                    }
                }
            }

            std::thread::sleep(UPDATE_INTERVAL);
        }
    });

    std::thread::spawn(move || {
        // We will read packets of data that are 15 ms long, this time in
        // de-interleaved format.
        let packet_frames =
            (OUT_SAMPLE_RATE.get() as f32 * UPDATE_INTERVAL.as_secs_f32()).round() as usize;
        let mut out_buf: Vec<Vec<f32>> = (0..NUM_CHANNELS.get().get())
            .map(|_| vec![0.0; packet_frames])
            .collect();

        loop {
            let mut handle = stream_reader_handle.lock().unwrap();

            // If this happens excessively in Release mode, you may want to consider
            // increasing [`StreamReaderConfig::channel_config.latency_seconds`].
            if handle.underflow_occurred() {
                println!("Underflow occured in stream reader node!");
            }

            // If this happens excessively in Release mode, you may want to consider
            // increasing [`StreamReaderConfig::channel_config.capacity_seconds`]. For
            // example, if you are streaming data from a network, you may want to
            // increase the capacity to several seconds.
            if handle.overflow_occurred() {
                println!("Overflow occured in stream reader node!");
            }

            // Wait until the node's processor is ready to read data.
            if handle.is_ready() {
                let status = handle.read(&mut out_buf, 0..packet_frames).unwrap();

                match status {
                    ReadStatus::Ok => {
                        println!("Successfully read data");
                    }
                    ReadStatus::InputNotReady => {
                        // The input stream is not ready yet.
                    }
                    ReadStatus::UnderflowOccurred { num_frames_read } => {
                        // An underflow occured. This may result in audible audio
                        // glitches.
                        println!(
                            "Underflow occured in stream reader node! Number of frames dropped: {}",
                            packet_frames - num_frames_read
                        );
                    }
                    ReadStatus::OverflowCorrected {
                        num_frames_discarded,
                    } => {
                        // An overflow occured. This may result in audible audio
                        // glitches.
                        println!(
                            "Overflow occured in stream reader node! Number of frames discarded: {}",
                            num_frames_discarded
                        );
                    }
                }

                // Alternatively, if you just wish to read all available frames in the
                // channel, then you could do:
                //
                // while handle.available_frames() >= packet_frames {
                //     let status = handle.read(&mut out_buf, 0..packet_frames).unwrap();
                //
                //     // Send data over the network, for example.
                // }
            }

            std::thread::sleep(UPDATE_INTERVAL);
        }
    });

    loop {
        if let Err(e) = cx.update() {
            tracing::error!("{:?}", &e);

            if let UpdateError::StreamStoppedUnexpectedly(_) = e {
                // Notify the stream node handles that the output stream has stopped.
                // This will automatically stop any active streams on the nodes.
                cx.node_state_mut::<StreamWriterState>(stream_writer_id)
                    .unwrap()
                    .stop_stream();
                cx.node_state_mut::<StreamReaderState>(stream_reader_id)
                    .unwrap()
                    .stop_stream();

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
