//! A simple node that demonstrates having a handle with shared state for
//! sending data back to the user.

// The use of `bevy_platform` is optional, but it is recommended for better
// compatibility with webassembly, no_std, and platforms without 64 bit atomics.
use bevy_platform::sync::atomic::{AtomicU32, Ordering};
use firewheel::node::NodeError;
use firewheel::{
    atomic_float::AtomicF32,
    channel_config::{ChannelConfig, ChannelCount},
    collector::ArcGc,
    diff::{Diff, Patch},
    event::ProcEvents,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        ProcBuffers, ProcExtra, ProcInfo, ProcStreamCtx, ProcessStatus,
    },
    StreamInfo,
};

#[derive(Debug)]
struct SharedState {
    rms_value: AtomicF32,
    // A simple counter used to keep track of when the processor should update
    // the RMS value.
    read_count: AtomicU32,
}

// The node struct holds all of the parameters of the node.
//
// # Notes about ECS
//
// In order to be friendlier to ECS's (entity component systems), it is encouraged
// that any struct deriving this trait be POD (plain ol' data). If you want your
// audio node to be usable in the Bevy game engine, also derive
// `bevy_ecs::prelude::Component`. (You can hide this derive behind a feature flag
// by using `#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]`).
//
// To keep this struct POD, this example makes use of the "custom state" API to
// send the rms value from the processor to the user.
//
// ------------------------------------------------------------------------------
/// This node roughly estimates the RMS (root-mean-square, a meaasure of loundess)
/// of a mono signal.
///
/// Note this node doesn't calculate the true RMS (That requires a much more
/// expensive algorithm using a sliding window.)
#[derive(Debug, Diff, Patch, Clone, Copy)]
pub struct FastRmsNode {
    /// Whether or not this node is enabled.
    pub enabled: bool,
    /// The size of the window used to measure the RMS value.
    ///
    /// Smaller values are better at detecting short bursts of loundess (transients),
    /// while larger values are better for measuring loudness on a broader time scale.
    ///
    /// By default this is set to `0.05` (50ms).
    pub window_size_secs: f32,
}

impl Default for FastRmsNode {
    fn default() -> Self {
        Self {
            enabled: true,
            window_size_secs: 50.0 / 1_000.0,
        }
    }
}

// The state struct is stored in the Firewheel context, and the user can retrieve
// it using `FirewheelCtx::node_state` and `FirewheelCtx::node_state_mut`.
#[derive(Clone)]
pub struct FastRmsState {
    // `ArcGc` is a simple wrapper around `Arc` that automatically collects
    // dropped resources from the audio thread and drops them on another
    // thread.
    shared_state: ArcGc<SharedState>,
}

impl FastRmsState {
    fn new() -> Self {
        Self {
            shared_state: ArcGc::new(SharedState {
                rms_value: AtomicF32::new(0.0),
                read_count: AtomicU32::new(1),
            }),
        }
    }

    /// Get the estimated RMS value.
    ///
    /// (Note, this is just a rough estimate. This node doesn't calculate the true
    /// RMS value).
    pub fn rms_value(&self) -> f32 {
        let rms = self.shared_state.rms_value.load(Ordering::Relaxed);
        self.shared_state.read_count.fetch_add(1, Ordering::Relaxed);
        rms
    }
}

// Implement the AudioNode type for your node.
impl AudioNode for FastRmsNode {
    // Since this node doesnt't need any configuration, we'll just
    // default to `EmptyConfig`.
    type Configuration = EmptyConfig;

    // Return information about your node. This method is only ever called
    // once.
    fn info(&self, _config: &Self::Configuration) -> Result<AudioNodeInfo, NodeError> {
        // The builder pattern is used for future-proofness as it is likely that
        // more fields will be added in the future.
        Ok(AudioNodeInfo::new()
            // A static name used for debugging purposes.
            .debug_name("example_fast_rms")
            // The configuration of the input/output ports.
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::MONO,
                num_outputs: ChannelCount::ZERO,
            })
            // Custom !Send state that can be stored in the Firewheel context and
            // accessed by the user.
            //
            // The user accesses this state via `FirewheelCtx::node_state` and
            // `FirewheelCtx::node_state_mut`.
            .custom_state(FastRmsState::new()))
    }

    // Construct the realtime processor counterpart using the given information
    // about the audio stream.
    //
    // This method is called before the node processor is sent to the realtime
    // thread, so it is safe to do non-realtime things here like allocating.
    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> Result<impl AudioNodeProcessor, NodeError> {
        let window_frames =
            (self.window_size_secs * cx.stream_info.sample_rate.get() as f32).round() as usize;

        // Extract the custom state so we can get a reference to the shared state.
        let custom_state = cx.custom_state::<FastRmsState>().unwrap();

        Ok(Processor {
            params: self.clone(),
            shared_state: ArcGc::clone(&custom_state.shared_state),
            squares: 0.0,
            num_squared_values: 0,
            window_frames,
            last_read_count: 0,
        })
    }
}

// The realtime processor counterpart to your node.
struct Processor {
    params: FastRmsNode,
    shared_state: ArcGc<SharedState>,
    squares: f32,
    num_squared_values: usize,
    window_frames: usize,
    last_read_count: u32,
}

impl AudioNodeProcessor for Processor {
    // The realtime process method.
    fn process(
        &mut self,
        // Information about the process block.
        info: &ProcInfo,
        // The buffers of data to process.
        buffers: ProcBuffers,
        // The list of events for our node to process.
        events: &mut ProcEvents,
        // Extra buffers and utilities.
        _extra: &mut ProcExtra,
    ) -> ProcessStatus {
        for patch in events.drain_patches::<FastRmsNode>() {
            match patch {
                FastRmsNodePatch::WindowSizeSecs(window_size_secs) => {
                    let window_frames =
                        (window_size_secs * info.sample_rate.get() as f32).round() as usize;

                    if self.window_frames != window_frames {
                        self.window_frames = window_frames;

                        self.squares = 0.0;
                        self.num_squared_values = 0;
                    }
                }
                _ => {}
            }

            self.params.apply(patch);
        }

        if !self.params.enabled {
            self.shared_state.rms_value.store(0.0, Ordering::Relaxed);

            self.squares = 0.0;
            self.num_squared_values = 0;

            return ProcessStatus::Bypass;
        }

        let mut frames_processed = 0;
        while frames_processed < info.frames {
            let process_frames =
                (info.frames - frames_processed).min(self.window_frames - self.num_squared_values);

            for &s in buffers.inputs[0][frames_processed..frames_processed + process_frames].iter()
            {
                self.squares += s * s;
            }

            self.num_squared_values += process_frames;
            frames_processed += process_frames;

            if self.num_squared_values == self.window_frames {
                let mean = self.squares / self.window_frames as f32;
                let rms = mean.sqrt();

                let latest_read_count = self.shared_state.read_count.load(Ordering::Relaxed);
                let previous_rms = self.shared_state.rms_value.load(Ordering::Relaxed);

                if latest_read_count != self.last_read_count || rms > previous_rms {
                    self.shared_state.rms_value.store(rms, Ordering::Relaxed);
                }

                self.squares = 0.0;
                self.num_squared_values = 0;
                self.last_read_count = latest_read_count;
            }
        }

        // There are no outputs in this node.
        ProcessStatus::Bypass
    }

    // Called when a new stream has been created. Because the new stream may have a
    // different sample rate from the old one, make sure to update any calculations
    // that depend on the sample rate.
    //
    // This gets called outside of the audio thread, so it is safe to allocate and
    // deallocate here.
    fn new_stream(&mut self, stream_info: &StreamInfo, _context: &mut ProcStreamCtx) {
        self.window_frames =
            (self.params.window_size_secs * stream_info.sample_rate.get() as f32).round() as usize;

        self.squares = 0.0;
        self.num_squared_values = 0;
    }
}
