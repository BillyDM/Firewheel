use std::time::Duration;

use firewheel::{
    error::UpdateError,
    nodes::{
        filter::{FilterNode, FilterType},
        sampler::{RepeatMode, SamplerNode, SamplerState},
    },
    FirewheelContext, Volume,
};
use symphonium::SymphoniumLoader;

const UPDATE_INTERVAL: Duration = Duration::from_millis(15);

fn play_sample_with_filter(
    sample_path: &str,
    filter_type: FilterType,
    cutoff_hz: f32,
    mix: f32,
    description: &str,
) {
    println!("\n--- {} ---", description);
    
    let mut cx = FirewheelContext::new(Default::default());
    cx.start_stream(Default::default()).unwrap();
    let sample_rate = cx.stream_info().unwrap().sample_rate;

    let mut sampler_node = SamplerNode::default();
    let filter_node = FilterNode {
        cutoff_hz,
        filter_type: filter_type as u32,
        mix,
        enabled: true,
    };

    let sampler_id = cx.add_node(sampler_node.clone(), None);
    let filter_id = cx.add_node(filter_node, None);
    let graph_out_id = cx.graph_out_node_id();

    // Connect: sampler -> filter -> output
    cx.connect(sampler_id, filter_id, &[(0, 0), (1, 1)], false).unwrap();
    cx.connect(filter_id, graph_out_id, &[(0, 0), (1, 1)], false).unwrap();

    // Load and play audio sample
    let mut loader = SymphoniumLoader::new();
    let sample = firewheel::load_audio_file(
        &mut loader,
        std::path::Path::new(sample_path),
        sample_rate,
        Default::default(),
    )
    .unwrap()
    .into_dyn_resource();

    sampler_node.set_sample(sample, Volume::Linear(0.7), RepeatMode::PlayOnce);
    cx.queue_event_for(sampler_id, sampler_node.sync_sequence_event());
    sampler_node.start_or_restart(None);
    cx.queue_event_for(sampler_id, sampler_node.sync_playback_event());
    cx.node_state::<SamplerState>(sampler_id).unwrap().mark_stopped(false);

    // Wait for playback to finish
    loop {
        if cx.node_state::<SamplerState>(sampler_id).unwrap().stopped() {
            break;
        }
        if let Err(e) = cx.update() {
            log::error!("{:?}", &e);
            if let UpdateError::StreamStoppedUnexpectedly(_) = e {
                break;
            }
        }
        std::thread::sleep(UPDATE_INTERVAL);
    }
}

fn main() {
    simple_log::quick!("info");

    println!("Firewheel Filter Node Demo");
    println!("This will demonstrate different filter types and settings using bird sound.");

    let sample_path = "assets/test_files/bird-sound.wav";

    // Test 1: No filter (bypass)
    {
        println!("\n--- Original (no filter) ---");
        
        let mut cx = FirewheelContext::new(Default::default());
        cx.start_stream(Default::default()).unwrap();
        let sample_rate = cx.stream_info().unwrap().sample_rate;

        let mut sampler_node = SamplerNode::default();
        let sampler_id = cx.add_node(sampler_node.clone(), None);
        let graph_out_id = cx.graph_out_node_id();

        // Connect directly: sampler -> output (no filter)
        cx.connect(sampler_id, graph_out_id, &[(0, 0), (1, 1)], false).unwrap();

        // Load and play bird sound
        let mut loader = SymphoniumLoader::new();
        let sample = firewheel::load_audio_file(
            &mut loader,
            std::path::Path::new(sample_path),
            sample_rate,
            Default::default(),
        )
        .unwrap()
        .into_dyn_resource();

        sampler_node.set_sample(sample, Volume::Linear(0.7), RepeatMode::PlayOnce);
        cx.queue_event_for(sampler_id, sampler_node.sync_sequence_event());
        sampler_node.start_or_restart(None);
        cx.queue_event_for(sampler_id, sampler_node.sync_playback_event());
        cx.node_state::<SamplerState>(sampler_id).unwrap().mark_stopped(false);

        // Wait for playback to finish
        loop {
            if cx.node_state::<SamplerState>(sampler_id).unwrap().stopped() {
                break;
            }
            if let Err(e) = cx.update() {
                log::error!("{:?}", &e);
                if let UpdateError::StreamStoppedUnexpectedly(_) = e {
                    break;
                }
            }
            std::thread::sleep(UPDATE_INTERVAL);
        }
    }

    std::thread::sleep(Duration::from_secs(1)); // Brief pause

    // Test 2: Low-pass filter
    play_sample_with_filter(
        sample_path,
        FilterType::LowPass,
        800.0,
        1.0,
        "Low-pass filter at 800Hz (removes high frequencies - should sound muffled)"
    );

    std::thread::sleep(Duration::from_secs(1));

    // Test 3: High-pass filter  
    play_sample_with_filter(
        sample_path,
        FilterType::HighPass,
        2000.0,
        1.0,
        "High-pass filter at 2000Hz (removes low frequencies - should sound thin)"
    );

    std::thread::sleep(Duration::from_secs(1));

    // Test 4: Band-pass filter
    play_sample_with_filter(
        sample_path,
        FilterType::BandPass,
        1500.0,
        1.0,
        "Band-pass filter at 1500Hz (removes low and high frequencies)"
    );

    std::thread::sleep(Duration::from_secs(1));

    // Test 5: Low-pass with mix
    play_sample_with_filter(
        sample_path,
        FilterType::LowPass,
        500.0,
        0.5,
        "Low-pass filter at 500Hz with 50% mix (blend of filtered and original)"
    );

    println!("\nDemo complete! You should have heard clear differences between each filter type:");
    println!("- Original: Full frequency range");
    println!("- Low-pass: Muffled, high frequencies removed"); 
    println!("- High-pass: Thin, low frequencies removed");
    println!("- Band-pass: Narrow frequency range, hollow sound");
    println!("- Mixed: Blend of filtered and original signal");
}