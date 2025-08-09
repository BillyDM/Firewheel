//! A simple node that generates white noise.

use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::{
        filter::smoothing_filter::DEFAULT_SMOOTH_SECONDS,
        volume::{Volume, DEFAULT_AMP_EPSILON},
    },
    event::ProcEvents,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers,
        ProcExtra, ProcInfo, ProcessStatus,
    },
    param::smoother::{SmoothedParam, SmootherConfig},
    SilenceMask,
};

/// A simple node that generates white noise. (Mono output only)
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
pub struct WhiteNoiseGenNode {
    /// The overall volume.
    ///
    /// Note, white noise is really loud, so prefer to use a value like
    /// `Volume::Linear(0.4)` or `Volume::Decibels(-18.0)`.
    pub volume: Volume,
    /// Whether or not this node is enabled.
    pub enabled: bool,
    /// The time in seconds of the internal smoothing filter.
    ///
    /// By default this is set to `0.015` (15ms).
    pub smooth_seconds: f32,
}

impl Default for WhiteNoiseGenNode {
    fn default() -> Self {
        Self {
            volume: Volume::Linear(0.4),
            enabled: true,
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
        }
    }
}

/// The configuration for a [`WhiteNoiseGenNode`]
#[derive(Debug, Clone)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
pub struct WhiteNoiseGenConfig {
    /// The starting seed. This cannot be zero.
    pub seed: i32,
}

impl Default for WhiteNoiseGenConfig {
    fn default() -> Self {
        Self { seed: 17 }
    }
}

impl AudioNode for WhiteNoiseGenNode {
    type Configuration = WhiteNoiseGenConfig;

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("white_noise_gen")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::ZERO,
                num_outputs: ChannelCount::MONO,
            })
    }

    fn construct_processor(
        &self,
        config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        // Seed cannot be zero.
        let seed = if config.seed == 0 { 17 } else { config.seed };

        Processor {
            fpd: seed,
            gain: SmoothedParam::new(
                self.volume.amp_clamped(DEFAULT_AMP_EPSILON),
                SmootherConfig {
                    smooth_seconds: self.smooth_seconds,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            params: *self,
        }
    }
}

// The realtime processor counterpart to your node.
struct Processor {
    fpd: i32,
    params: WhiteNoiseGenNode,
    gain: SmoothedParam,
}

impl AudioNodeProcessor for Processor {
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        events: &mut ProcEvents,
        _extra: &mut ProcExtra,
    ) -> ProcessStatus {
        for patch in events.drain_patches::<WhiteNoiseGenNode>() {
            match patch {
                WhiteNoiseGenNodePatch::Volume(vol) => {
                    self.gain.set_value(vol.amp_clamped(DEFAULT_AMP_EPSILON));
                }
                WhiteNoiseGenNodePatch::SmoothSeconds(seconds) => {
                    self.gain.set_smooth_seconds(seconds, info.sample_rate);
                }
                _ => {}
            }

            self.params.apply(patch);
        }

        if !self.params.enabled || (self.gain.target_value() == 0.0 && !self.gain.is_smoothing()) {
            self.gain.reset();
            return ProcessStatus::ClearAllOutputs;
        }

        for s in buffers.outputs[0].iter_mut() {
            self.fpd ^= self.fpd << 13;
            self.fpd ^= self.fpd >> 17;
            self.fpd ^= self.fpd << 5;

            // Get a random normalized value in the range `[-1.0, 1.0]`.
            let r = self.fpd as f32 * (1.0 / 2_147_483_648.0);

            *s = r * self.gain.next_smoothed();
        }

        ProcessStatus::OutputsModified {
            out_silence_mask: SilenceMask::NONE_SILENT,
        }
    }
}
