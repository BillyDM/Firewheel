use firewheel_core::{
    channel_config::{ChannelConfig, NonZeroChannelCount},
    diff::{Diff, Patch},
    dsp::volume::{Volume, DEFAULT_AMP_EPSILON},
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, GetChannels,
        ProcInfo, ProcessStatus, SimpleAudioProcessor,
    },
    param::smoother::{SmoothedParam, SmootherConfig},
};

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct VolumeNodeConfig {
    /// The time in seconds of the internal smoothing filter.
    ///
    /// By default this is set to `0.01` (10ms).
    pub smooth_secs: f32,

    /// The number of input and output channels.
    pub channels: NonZeroChannelCount,

    /// If the resutling amplitude of the volume is less than or equal to this
    /// value, then the amplitude will be clamped to `0.0` (silence).
    pub amp_epsilon: f32,
}

impl Default for VolumeNodeConfig {
    fn default() -> Self {
        Self {
            smooth_secs: 10.0 / 1_000.0,
            channels: NonZeroChannelCount::STEREO,
            amp_epsilon: DEFAULT_AMP_EPSILON,
        }
    }
}

#[derive(Default, Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct VolumeNode {
    pub volume: Volume,
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
            .uses_events(true)
    }

    fn construct_processor(
        &self,
        config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        let gain = self.volume.amp_clamped(config.amp_epsilon);

        VolumeProcessor {
            gain: SmoothedParam::new(
                gain,
                SmootherConfig {
                    smooth_secs: config.smooth_secs,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            prev_block_was_silent: true,
            amp_epsilon: config.amp_epsilon,
        }
    }
}

struct VolumeProcessor {
    gain: SmoothedParam,

    prev_block_was_silent: bool,
    amp_epsilon: f32,
}

impl SimpleAudioProcessor for VolumeProcessor {
    type Node = VolumeNode;

    fn can_skip_patch(&self, VolumeNodePatch::Volume(v): &VolumeNodePatch) -> bool {
        let gain = v.amp_clamped(self.amp_epsilon);

        // If the gain has not meaningfully changed, ignore.
        (gain - self.gain.target_value()).abs() < DEFAULT_AMP_EPSILON
    }

    fn apply_patch(&mut self, VolumeNodePatch::Volume(v): VolumeNodePatch) {
        let mut gain = v.amp_clamped(self.amp_epsilon);

        if (1. - DEFAULT_AMP_EPSILON..1. + DEFAULT_AMP_EPSILON).contains(&gain) {
            gain = 1.0;
        }

        self.gain.set_value(gain);

        if self.prev_block_was_silent {
            // Previous block was silent, so no need to smooth.
            self.gain.reset();
        }
    }

    fn process<B: GetChannels>(&mut self, mut buffers: B, proc_info: &ProcInfo) -> ProcessStatus {
        self.prev_block_was_silent = false;

        let mut buffers = buffers.channels();

        if proc_info
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
                for (ch_i, (out_ch, in_ch)) in buffers.outputs.zip(buffers.inputs).enumerate() {
                    if proc_info.in_silence_mask.is_channel_silent(ch_i) {
                        if !proc_info.out_silence_mask.is_channel_silent(ch_i) {
                            out_ch.fill(0.0);
                        }
                    } else {
                        for (os, &is) in out_ch.iter_mut().zip(in_ch.iter()) {
                            *os = is * self.gain.target_value();
                        }
                    }
                }

                return ProcessStatus::OutputsModified {
                    out_silence_mask: proc_info.in_silence_mask,
                };
            }
        }

        if buffers.inputs.len() == 1 {
            for (os, is) in buffers
                .outputs
                .next()
                .unwrap()
                .iter_mut()
                .zip(buffers.inputs.next().unwrap().iter())
            {
                *os = is * self.gain.next_smoothed();
            }
        } else if buffers.inputs.len() == 2 {
            let in0 = buffers.inputs.next().unwrap();
            let in1 = buffers.inputs.next().unwrap();
            let out0 = buffers.outputs.next().unwrap();
            let out1 = buffers.outputs.next().unwrap();

            for ((os0, ins0), (os1, ins1)) in out0.iter_mut().zip(in0).zip(out1.iter_mut().zip(in1))
            {
                let g = self.gain.next_smoothed();
                *os0 = *ins0 * g;
                *os1 = *ins1 * g;
            }
        } else {
            let scratch = buffers.scratch_buffers.next().unwrap();
            self.gain
                .process_into_buffer(&mut scratch[..buffers.frames]);

            for (ch_i, (out_ch, in_ch)) in buffers.outputs.zip(buffers.inputs).enumerate() {
                if proc_info.in_silence_mask.is_channel_silent(ch_i) {
                    if !proc_info.out_silence_mask.is_channel_silent(ch_i) {
                        out_ch.fill(0.0);
                    }
                    continue;
                }

                for ((os, &is), &g) in out_ch.iter_mut().zip(in_ch.iter()).zip(&*scratch) {
                    *os = is * g;
                }
            }
        }

        self.gain.settle();

        ProcessStatus::OutputsModified {
            out_silence_mask: proc_info.in_silence_mask,
        }
    }

    fn new_stream(&mut self, stream_info: &firewheel_core::StreamInfo) {
        self.gain.update_sample_rate(stream_info.sample_rate);
    }
}
