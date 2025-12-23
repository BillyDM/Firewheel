use bevy_platform::sync::atomic::{AtomicU32, Ordering};
use firewheel_core::{
    atomic_float::AtomicF32,
    channel_config::{ChannelConfig, ChannelCount},
    collector::ArcGc,
    diff::{Diff, Patch},
    dsp::volume::amp_to_db,
    event::ProcEvents,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        ProcBuffers, ProcExtra, ProcInfo, ProcStreamCtx, ProcessStatus,
    },
    StreamInfo,
};

#[cfg(not(feature = "std"))]
use num_traits::Float;

/// A lightweight node that measures the loudness of a mono signal using a rough RMS
/// (root mean square) estimate.
///
/// Note this node doesn't calculate the true RMS (That requires a much more expensive
/// algorithm using a sliding window.) But it should be good enough for games that
/// simply wish to react to player audio.
#[derive(Debug, Diff, Patch, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FastRmsNode {
    /// Whether or not this node is enabled.
    pub enabled: bool,
    /// The size of the window used for measuring the RMS value.
    ///
    /// Smaller values are better at detecting short bursts of loudness (transients),
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

/// The state of a [`FastRmsNode`]. This contains the calculated RMS values.
#[derive(Clone)]
pub struct FastRmsState {
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

    /// Get the estimated RMS value in decibels.
    ///
    /// * `db_epsilon` - If the RMS value is less than or equal to this value, then it
    /// will be clamped to `f32::NEG_INFINITY` (silence). (You can use
    /// [firewheel_core::dsp::volume::DEFAULT_DB_EPSILON].)
    ///
    /// If the node is currently disabled, then this will return a value
    /// of `f32::NEG_INFINITY` (silence).
    ///
    /// Note this node doesn't calculate the true RMS (That requires a much more expensive
    /// algorithm using a sliding window.) But it should be good enough for games that
    /// simply wish to react to player audio.
    pub fn rms_db(&self, db_epsilon: f32) -> f32 {
        let rms = amp_to_db(self.shared_state.rms_value.load(Ordering::Relaxed));
        self.shared_state.read_count.fetch_add(1, Ordering::Relaxed);

        if rms <= db_epsilon {
            f32::NEG_INFINITY
        } else {
            rms
        }
    }
}

impl AudioNode for FastRmsNode {
    type Configuration = EmptyConfig;

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("fast_rms")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::MONO,
                num_outputs: ChannelCount::ZERO,
            })
            .custom_state(FastRmsState::new())
    }

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        let window_frames =
            (self.window_size_secs * cx.stream_info.sample_rate.get() as f32).round() as usize;

        let custom_state = cx.custom_state::<FastRmsState>().unwrap();

        Processor {
            params: self.clone(),
            shared_state: ArcGc::clone(&custom_state.shared_state),
            squares: 0.0,
            num_squared_values: 0,
            window_frames,
            last_read_count: 0,
        }
    }
}

struct Processor {
    params: FastRmsNode,
    shared_state: ArcGc<SharedState>,
    squares: f32,
    num_squared_values: usize,
    window_frames: usize,
    last_read_count: u32,
}

impl AudioNodeProcessor for Processor {
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        events: &mut ProcEvents,
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

            if !info.in_silence_mask.is_channel_silent(0) {
                for &s in
                    buffers.inputs[0][frames_processed..frames_processed + process_frames].iter()
                {
                    self.squares += s * s;
                }
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

    fn new_stream(&mut self, stream_info: &StreamInfo, _context: &mut ProcStreamCtx) {
        self.window_frames =
            (self.params.window_size_secs * stream_info.sample_rate.get() as f32).round() as usize;

        self.squares = 0.0;
        self.num_squared_values = 0;
    }
}

#[derive(Debug)]
struct SharedState {
    rms_value: AtomicF32,
    // A simple counter used to keep track of when the processor should update
    // the RMS value.
    read_count: AtomicU32,
}
