use firewheel_core::node::NodeError;
use firewheel_core::{
    channel_config::{ChannelConfig, NonZeroChannelCount},
    diff::{Diff, Patch},
    dsp::{
        filter::smoothing_filter::DEFAULT_SMOOTH_SECONDS,
        volume::{Volume, DEFAULT_AMP_EPSILON},
    },
    event::ProcEvents,
    mask::MaskType,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers,
        ProcExtra, ProcInfo, ProcStreamCtx, ProcessStatus,
    },
    param::smoother::{SmoothedParam, SmootherConfig},
};

/// The configuration of a [`VolumeNode`]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct VolumeNodeConfig {
    /// The number of input and output channels.
    pub channels: NonZeroChannelCount,
}

impl Default for VolumeNodeConfig {
    fn default() -> Self {
        Self {
            channels: NonZeroChannelCount::STEREO,
        }
    }
}

/// A node that changes the volume of a signal
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct VolumeNode {
    /// The volume to apply to the signal
    pub volume: Volume,

    /// The time in seconds of the internal smoothing filter.
    ///
    /// By default this is set to `0.015` (15ms).
    pub smooth_seconds: f32,
    /// If the resutling gain (in raw amplitude, not decibels) is less
    /// than or equal to this value, then the gain will be clamped to
    /// `0.0` (silence).
    ///
    /// By default this is set to `0.00001` (-100 decibels).
    pub min_gain: f32,
}

impl Default for VolumeNode {
    fn default() -> Self {
        Self {
            volume: Volume::default(),
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
            min_gain: DEFAULT_AMP_EPSILON,
        }
    }
}

impl VolumeNode {
    /// Construct a volume node from the given volume in a linear scale,
    /// where `0.0` is silence and `1.0` is unity gain.
    ///
    /// These units are suitable for volume sliders (simply convert percent
    /// volume to linear volume by diving the percent volume by 100).
    pub const fn from_linear(linear: f32) -> Self {
        Self {
            volume: Volume::Linear(linear),
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
            min_gain: DEFAULT_AMP_EPSILON,
        }
    }

    /// Construct a volume node from the given volume in percentage,
    /// where `0.0` is silence and `100.0` is unity gain.
    ///
    /// These units are suitable for volume sliders.
    pub const fn from_percent(percent: f32) -> Self {
        Self {
            volume: Volume::from_percent(percent),
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
            min_gain: DEFAULT_AMP_EPSILON,
        }
    }

    /// Construct a volume node from the given volume in decibels, where `0.0`
    /// is unity gain and `f32::NEG_INFINITY` is silence.
    pub const fn from_decibels(decibels: f32) -> Self {
        Self {
            volume: Volume::Decibels(decibels),
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
            min_gain: DEFAULT_AMP_EPSILON,
        }
    }

    /// Set the given volume in a linear scale, where `0.0` is silence and
    /// `1.0` is unity gain.
    ///
    /// These units are suitable for volume sliders (simply convert percent
    /// volume to linear volume by diving the percent volume by 100).
    pub const fn set_linear(&mut self, linear: f32) {
        self.volume = Volume::Linear(linear);
    }

    /// Set the given volume in percentage, where `0.0` is silence and
    /// `100.0` is unity gain.
    ///
    /// These units are suitable for volume sliders.
    pub const fn set_percent(&mut self, percent: f32) {
        self.volume = Volume::from_percent(percent);
    }

    /// Set the given volume in decibels, where `0.0` is unity gain and
    /// `f32::NEG_INFINITY` is silence.
    pub const fn set_decibels(&mut self, decibels: f32) {
        self.volume = Volume::Decibels(decibels);
    }
}

impl AudioNode for VolumeNode {
    type Configuration = VolumeNodeConfig;

    fn info(&self, config: &Self::Configuration) -> Result<AudioNodeInfo, NodeError> {
        Ok(AudioNodeInfo::new()
            .debug_name("volume")
            .channel_config(ChannelConfig {
                num_inputs: config.channels.get(),
                num_outputs: config.channels.get(),
            }))
    }

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> Result<impl AudioNodeProcessor, NodeError> {
        let min_gain = self.min_gain.max(0.0);
        let gain = self.volume.amp_clamped(min_gain);

        Ok(VolumeProcessor {
            gain: SmoothedParam::new(
                gain,
                SmootherConfig {
                    smooth_seconds: self.smooth_seconds,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            min_gain,
        })
    }
}

struct VolumeProcessor {
    gain: SmoothedParam,

    min_gain: f32,
}

impl AudioNodeProcessor for VolumeProcessor {
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        events: &mut ProcEvents,
        extra: &mut ProcExtra,
    ) -> ProcessStatus {
        for patch in events.drain_patches::<VolumeNode>() {
            match patch {
                VolumeNodePatch::Volume(v) => {
                    let mut gain = v.amp_clamped(self.min_gain);
                    if gain > 0.99999 && gain < 1.00001 {
                        gain = 1.0;
                    }
                    self.gain.set_value(gain);

                    if info.prev_output_was_silent {
                        // Previous block was silent, so no need to smooth.
                        self.gain.reset_to_target();
                    }
                }
                VolumeNodePatch::SmoothSeconds(seconds) => {
                    self.gain.set_smooth_seconds(seconds, info.sample_rate);
                }
                VolumeNodePatch::MinGain(min_gain) => {
                    self.min_gain = min_gain.max(0.0);
                }
            }
        }

        if info
            .in_silence_mask
            .all_channels_silent(buffers.inputs.len())
        {
            // All channels are silent, so there is no need to process. Also reset
            // the filter since it doesn't need to smooth anything.
            self.gain.reset_to_target();

            return ProcessStatus::ClearAllOutputs;
        }

        if self.gain.has_settled() {
            if self.gain.target_value() <= self.min_gain {
                // Muted, so there is no need to process.
                return ProcessStatus::ClearAllOutputs;
            } else if self.gain.target_value() == 1.0 {
                // Unity gain, there is no need to process.
                return ProcessStatus::Bypass;
            } else {
                for (ch_i, (out_ch, in_ch)) in buffers
                    .outputs
                    .iter_mut()
                    .zip(buffers.inputs.iter())
                    .enumerate()
                {
                    if info.in_silence_mask.is_channel_silent(ch_i) {
                        if !info.out_silence_mask.is_channel_silent(ch_i) {
                            out_ch.fill(0.0);
                        }
                    } else {
                        for (os, &is) in out_ch.iter_mut().zip(in_ch.iter()) {
                            *os = is * self.gain.target_value();
                        }
                    }
                }

                return ProcessStatus::OutputsModifiedWithMask(MaskType::Silence(
                    info.in_silence_mask,
                ));
            }
        }

        if buffers.inputs.len() == 1 {
            // Provide an optimized loop for mono.
            for (os, &is) in buffers.outputs[0].iter_mut().zip(buffers.inputs[0].iter()) {
                *os = is * self.gain.next_smoothed();
            }
        } else if buffers.inputs.len() == 2 {
            // Provide an optimized loop for stereo.

            let in0 = &buffers.inputs[0][..info.frames];
            let in1 = &buffers.inputs[1][..info.frames];
            let (out0, out1) = buffers.outputs.split_first_mut().unwrap();
            let out0 = &mut out0[..info.frames];
            let out1 = &mut out1[0][..info.frames];

            for i in 0..info.frames {
                let gain = self.gain.next_smoothed();

                out0[i] = in0[i] * gain;
                out1[i] = in1[i] * gain;
            }
        } else {
            let scratch_buffer = extra.scratch_buffers.first_mut();

            self.gain
                .process_into_buffer(&mut scratch_buffer[..info.frames]);

            for (ch_i, (out_ch, in_ch)) in buffers
                .outputs
                .iter_mut()
                .zip(buffers.inputs.iter())
                .enumerate()
            {
                if info.in_silence_mask.is_channel_silent(ch_i) {
                    if !info.out_silence_mask.is_channel_silent(ch_i) {
                        out_ch.fill(0.0);
                    }
                    continue;
                }

                for ((os, &is), &g) in out_ch
                    .iter_mut()
                    .zip(in_ch.iter())
                    .zip(scratch_buffer[..info.frames].iter())
                {
                    *os = is * g;
                }
            }
        }

        self.gain.settle();

        ProcessStatus::OutputsModified
    }

    fn new_stream(
        &mut self,
        stream_info: &firewheel_core::StreamInfo,
        _context: &mut ProcStreamCtx,
    ) {
        self.gain.update_sample_rate(stream_info.sample_rate);
    }
}
