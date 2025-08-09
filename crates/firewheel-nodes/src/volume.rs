use firewheel_core::{
    channel_config::{ChannelConfig, NonZeroChannelCount},
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

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
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

#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
pub struct VolumeNode {
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

impl AudioNode for VolumeNode {
    type Configuration = VolumeNodeConfig;

    fn info(&self, config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("volume")
            .channel_config(ChannelConfig {
                num_inputs: config.channels.get(),
                num_outputs: config.channels.get(),
            })
    }

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        let min_gain = self.min_gain.max(0.0);
        let gain = self.volume.amp_clamped(min_gain);

        VolumeProcessor {
            gain: SmoothedParam::new(
                gain,
                SmootherConfig {
                    smooth_seconds: self.smooth_seconds,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            prev_block_was_silent: true,
            min_gain,
        }
    }
}

struct VolumeProcessor {
    gain: SmoothedParam,

    prev_block_was_silent: bool,
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

                    if self.prev_block_was_silent {
                        // Previous block was silent, so no need to smooth.
                        self.gain.reset();
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

        self.prev_block_was_silent = false;

        if info
            .in_silence_mask
            .all_channels_silent(buffers.inputs.len())
        {
            // All channels are silent, so there is no need to process. Also reset
            // the filter since it doesn't need to smooth anything.
            self.gain.reset();
            self.prev_block_was_silent = true;

            return ProcessStatus::ClearAllOutputs;
        }

        if !self.gain.is_smoothing() {
            if self.gain.target_value() == 0.0 {
                self.prev_block_was_silent = true;
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

                return ProcessStatus::OutputsModified {
                    out_silence_mask: info.in_silence_mask,
                };
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

        ProcessStatus::outputs_modified(SilenceMask::NONE_SILENT)
    }

    fn new_stream(&mut self, stream_info: &firewheel_core::StreamInfo) {
        self.gain.update_sample_rate(stream_info.sample_rate);
    }
}
