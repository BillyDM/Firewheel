use bevy_platform::sync::atomic::Ordering;
use firewheel_core::{
    atomic_float::AtomicF32,
    channel_config::{ChannelConfig, ChannelCount},
    collector::ArcGc,
    diff::{Diff, Patch},
    dsp::volume::{amp_to_db, DbMeterNormalizer},
    event::NodeEventList,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        ProcBuffers, ProcInfo, ProcessStatus,
    },
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeakMeterSmootherConfig {
    /// The rate of decay in seconds.
    ///
    /// By default this is set to `0.3` (300ms).
    pub decay_rate: f32,
    /// The rate at which this meter will refresh. This will typically
    /// match the display's frame rate.
    ///
    /// By default this is set to `60.0`.
    pub refresh_rate: f32,
    /// The number of frames that the values in `has_clipped` will
    /// hold their values before resetting to `false`.
    ///
    /// By default this is set to `60`.
    pub clip_hold_frames: usize,
}

impl Default for PeakMeterSmootherConfig {
    fn default() -> Self {
        Self {
            decay_rate: 0.3,
            refresh_rate: 60.0,
            clip_hold_frames: 60,
        }
    }
}

/// A helper struct to smooth out the output of [`PeakMeterNode`]. This
/// can be used to drive the animation of a peak meter in a GUI.
#[derive(Debug, Clone, Copy)]
pub struct PeakMeterSmoother<const NUM_CHANNELS: usize> {
    /// The current smoothed peak value of each channel, in decibels.
    smoothed_peaks: [f32; NUM_CHANNELS],
    clipped_frames_left: [usize; NUM_CHANNELS],
    level_decay: f32,
    frame_interval: f32,
    frame_counter: f32,
    clip_hold_frames: usize,
}

impl<const NUM_CHANNELS: usize> PeakMeterSmoother<NUM_CHANNELS> {
    pub fn new(config: PeakMeterSmootherConfig) -> Self {
        assert!(config.decay_rate > 0.0);
        assert!(config.refresh_rate > 0.0);
        assert!(config.clip_hold_frames > 0);

        Self {
            smoothed_peaks: [-100.0; NUM_CHANNELS],
            clipped_frames_left: [0; NUM_CHANNELS],
            level_decay: 1.0 - (-1.0 / (config.refresh_rate * config.decay_rate)).exp(),
            frame_interval: config.refresh_rate.recip(),
            frame_counter: 0.0,
            clip_hold_frames: config.clip_hold_frames,
        }
    }

    pub fn reset(&mut self) {
        self.smoothed_peaks = [-100.0; NUM_CHANNELS];
        self.clipped_frames_left = [0; NUM_CHANNELS];
    }

    pub fn update(&mut self, peaks_db: [f32; NUM_CHANNELS], delta_seconds: f32) {
        for ((smoothed_peak, &in_peak), clipped_frames_left) in self
            .smoothed_peaks
            .iter_mut()
            .zip(peaks_db.iter())
            .zip(self.clipped_frames_left.iter_mut())
        {
            if in_peak >= *smoothed_peak {
                *smoothed_peak = in_peak;

                if in_peak > 0.0 {
                    *clipped_frames_left = self.clip_hold_frames;
                }
            }
        }

        self.frame_counter += delta_seconds;

        // Correct for cumulative errors.
        if (self.frame_counter - self.frame_interval).abs() < 0.0001 {
            self.frame_counter = self.frame_interval;
        }

        while self.frame_counter >= self.frame_interval {
            self.frame_counter -= self.frame_interval;

            // Correct for cumulative errors.
            if (self.frame_counter - self.frame_interval).abs() < 0.0001 {
                self.frame_counter = self.frame_interval;
            }

            for ((smoothed_peak, &in_peak), clipped_frames_left) in self
                .smoothed_peaks
                .iter_mut()
                .zip(peaks_db.iter())
                .zip(self.clipped_frames_left.iter_mut())
            {
                if in_peak + 0.001 < *smoothed_peak {
                    *smoothed_peak += ((in_peak - *smoothed_peak) * self.level_decay).max(-100.0);
                }

                if *smoothed_peak > 0.0 {
                    *clipped_frames_left = self.clip_hold_frames;
                } else if *clipped_frames_left > 0 {
                    *clipped_frames_left -= 1;
                }
            }
        }
    }

    pub fn has_clipped(&self) -> [bool; NUM_CHANNELS] {
        core::array::from_fn(|i| self.clipped_frames_left[i] > 0)
    }

    pub fn smoothed_peaks_db(&self) -> &[f32; NUM_CHANNELS] {
        &self.smoothed_peaks
    }

    pub fn smoothed_peak_db_mono(&self) -> f32 {
        let mut max_value: f32 = -100.0;
        for ch in self.smoothed_peaks {
            max_value = max_value.max(ch);
        }

        max_value
    }

    /// Get the peak values as a normalized value in the range `[0.0, 1.0]`.
    pub fn smoothed_peaks_normalized(&self, normalizer: &DbMeterNormalizer) -> [f32; NUM_CHANNELS] {
        core::array::from_fn(|i| normalizer.normalize(self.smoothed_peaks[i]))
    }

    pub fn smoothed_peaks_normalized_mono(&self, normalizer: &DbMeterNormalizer) -> f32 {
        normalizer.normalize(self.smoothed_peak_db_mono())
    }
}

#[derive(Diff, Patch, Debug, Clone, Copy)]
pub struct PeakMeterNode<const NUM_CHANNELS: usize> {
    pub enabled: bool,
}

#[derive(Clone)]
pub struct PeakMeterState<const NUM_CHANNELS: usize> {
    shared_state: ArcGc<SharedState<NUM_CHANNELS>>,
}

impl<const NUM_CHANNELS: usize> PeakMeterState<NUM_CHANNELS> {
    fn new() -> Self {
        assert_ne!(NUM_CHANNELS, 0);
        assert!(NUM_CHANNELS <= 64);

        Self {
            shared_state: ArcGc::new(SharedState {
                peak_gains: core::array::from_fn(|_| AtomicF32::new(0.0)),
            }),
        }
    }

    /// Get the latest peak values for each channel in decibels.
    ///
    /// * `db_epsilon` - If a peak value is less than or equal to this value, then it
    /// will be clamped to `f32::NEG_INFINITY` (silence).
    ///
    /// If the node is currently disabled, then this will return a value
    /// of `f32::NEG_INFINITY` (silence) for all channels.
    pub fn peak_gain_db(&self, db_epsilon: f32) -> [f32; NUM_CHANNELS] {
        core::array::from_fn(|i| {
            let db = amp_to_db(self.shared_state.peak_gains[i].load(Ordering::Relaxed));
            if db <= db_epsilon {
                f32::NEG_INFINITY
            } else {
                db
            }
        })
    }
}

impl<const NUM_CHANNELS: usize> AudioNode for PeakMeterNode<NUM_CHANNELS> {
    type Configuration = EmptyConfig;

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("peak_meter")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::new(NUM_CHANNELS as u32).unwrap(),
                num_outputs: ChannelCount::new(NUM_CHANNELS as u32).unwrap(),
            })
            .uses_events(true)
            .custom_state(PeakMeterState::<NUM_CHANNELS>::new())
    }

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        Processor {
            params: self.clone(),
            shared_state: ArcGc::clone(
                &cx.custom_state::<PeakMeterState<NUM_CHANNELS>>()
                    .unwrap()
                    .shared_state,
            ),
        }
    }
}

struct SharedState<const NUM_CHANNELS: usize> {
    peak_gains: [AtomicF32; NUM_CHANNELS],
}

struct Processor<const NUM_CHANNELS: usize> {
    params: PeakMeterNode<NUM_CHANNELS>,
    shared_state: ArcGc<SharedState<NUM_CHANNELS>>,
}

impl<const NUM_CHANNELS: usize> AudioNodeProcessor for Processor<NUM_CHANNELS> {
    fn process(
        &mut self,
        buffers: ProcBuffers,
        proc_info: &ProcInfo,
        mut events: NodeEventList,
    ) -> ProcessStatus {
        let was_enabled = self.params.enabled;

        events.for_each_patch::<PeakMeterNode<NUM_CHANNELS>>(|p| self.params.apply(p.event));

        if was_enabled && !self.params.enabled {
            for ch in self.shared_state.peak_gains.iter() {
                ch.store(0.0, Ordering::Relaxed);
            }
        }

        if !self.params.enabled {
            return ProcessStatus::Bypass;
        }

        for (i, (in_ch, peak_shared)) in buffers
            .inputs
            .iter()
            .zip(self.shared_state.peak_gains.iter())
            .enumerate()
        {
            if proc_info.in_silence_mask.is_channel_silent(i) {
                peak_shared.store(0.0, Ordering::Relaxed);
            } else {
                peak_shared.store(
                    firewheel_core::dsp::algo::max_peak(in_ch),
                    Ordering::Relaxed,
                );
            }
        }

        ProcessStatus::Bypass
    }
}
