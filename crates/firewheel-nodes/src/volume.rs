use std::{
    mem,
    ops::{self, Range},
};

use firewheel_core::{
    channel_config::{ChannelConfig, NonZeroChannelCount},
    diff::{Diff, Patch},
    dsp::volume::{Volume, DEFAULT_AMP_EPSILON},
    event::{NodeEventList, PatchEvent},
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers,
        ProcInfo, ProcessStatus,
    },
    param::smoother::{SmoothedParam, SmootherConfig},
    SilenceMask,
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

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
struct VolumeResult {
    // This should be a more-generic `ChannelMask` but we reuse `SilenceMask` for the MVP.
    unity: SilenceMask,
    silent: SilenceMask,
}

impl ops::BitAnd for VolumeResult {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self {
            unity: SilenceMask(self.unity.0 & rhs.unity.0),
            silent: SilenceMask(self.silent.0 & rhs.silent.0),
        }
    }
}

impl ops::BitAndAssign for VolumeResult {
    fn bitand_assign(&mut self, rhs: Self) {
        *self = *self & rhs;
    }
}

impl VolumeProcessor {
    fn process_range(
        &mut self,
        buffers: &mut ProcBuffers,
        proc_info: &ProcInfo,
        range: Range<usize>,
    ) -> VolumeResult {
        if proc_info
            .in_silence_mask
            .all_channels_silent(buffers.inputs.len())
        {
            // All channels are silent, so there is no need to process. Also reset
            // the filter since it doesn't need to smooth anything.
            self.gain.reset();
            self.prev_block_was_silent = true;

            return VolumeResult {
                unity: SilenceMask::NONE_SILENT,
                silent: SilenceMask::new_all_silent(buffers.outputs.len()),
            };
        }

        if !self.gain.is_smoothing() {
            if self.gain.target_value() == 0.0 {
                self.prev_block_was_silent = true;
                // Muted, so there is no need to process.
                return VolumeResult {
                    unity: SilenceMask::NONE_SILENT,
                    silent: SilenceMask::new_all_silent(buffers.outputs.len()),
                };
            } else if self.gain.target_value() == 1.0 {
                // Unity gain, there is no need to process.
                return VolumeResult {
                    unity: SilenceMask::new_all_silent(buffers.outputs.len()),
                    silent: SilenceMask::NONE_SILENT,
                };
            } else {
                for (ch_i, (out_ch, in_ch)) in buffers
                    .outputs
                    .iter_mut()
                    .zip(buffers.inputs.iter())
                    .enumerate()
                {
                    if proc_info.in_silence_mask.is_channel_silent(ch_i) {
                        if !proc_info.out_silence_mask.is_channel_silent(ch_i) {
                            out_ch[range.clone()].fill(0.0);
                        }
                    } else {
                        for (os, &is) in out_ch[range.clone()]
                            .iter_mut()
                            .zip(in_ch[range.clone()].iter())
                        {
                            *os = is * self.gain.target_value();
                        }
                    }
                }

                return VolumeResult {
                    unity: SilenceMask::NONE_SILENT,
                    silent: proc_info.in_silence_mask,
                };
            }
        }

        if buffers.inputs.len() == 1 {
            // Provide an optimized loop for mono.
            for (os, &is) in buffers.outputs[0][range.clone()]
                .iter_mut()
                .zip(buffers.inputs[0][range.clone()].iter())
            {
                *os = is * self.gain.next_smoothed();
            }
        } else if buffers.inputs.len() == 2 {
            // Provide an optimized loop for stereo.

            let in0 = &buffers.inputs[0][range.clone()];
            let in1 = &buffers.inputs[1][range.clone()];
            let (out0, out1) = buffers.outputs.split_first_mut().unwrap();
            let out0 = &mut out0[range.clone()];
            let out1 = &mut out1[0][range.clone()];

            for i in 0..proc_info.frames {
                let gain = self.gain.next_smoothed();

                out0[i] = in0[i] * gain;
                out1[i] = in1[i] * gain;
            }
        } else {
            self.gain
                .process_into_buffer(&mut buffers.scratch_buffers[0][range.clone()]);

            for (ch_i, (out_ch, in_ch)) in buffers
                .outputs
                .iter_mut()
                .zip(buffers.inputs.iter())
                .enumerate()
            {
                if proc_info.in_silence_mask.is_channel_silent(ch_i) {
                    if !proc_info.out_silence_mask.is_channel_silent(ch_i) {
                        out_ch.fill(0.0);
                    }
                    continue;
                }

                for ((os, &is), &g) in out_ch
                    .iter_mut()
                    .zip(in_ch.iter())
                    .zip(buffers.scratch_buffers[0][range.clone()].iter())
                {
                    *os = is * g;
                }
            }
        }

        self.gain.settle();

        VolumeResult {
            unity: SilenceMask::NONE_SILENT,
            silent: SilenceMask::NONE_SILENT,
        }
    }
}

impl AudioNodeProcessor for VolumeProcessor {
    fn process(
        &mut self,
        mut buffers: ProcBuffers,
        proc_info: &ProcInfo,
        mut events: NodeEventList,
    ) -> ProcessStatus {
        let mut result = VolumeResult {
            unity: SilenceMask::new_all_silent(buffers.outputs.len()),
            silent: SilenceMask::new_all_silent(buffers.outputs.len()),
        };

        let mut start = 0usize;

        events.for_each_patch::<VolumeNode>(
            |PatchEvent {
                 event: VolumeNodePatch::Volume(v),
                 time,
             }| {
                let mut gain = v.amp_clamped(self.amp_epsilon);

                // If the gain has not meaningfully changed, ignore.
                if (gain - self.gain.target_value()).abs() < 0.00001 {
                    return;
                }

                let prev_block_was_silent =
                    if let Some(time) = time.and_then(|time| time.to_samples(proc_info)) {
                        let end = time.0 - proc_info.clock_samples.start.0;
                        debug_assert!((start as i64..proc_info.frames as i64).contains(&end));
                        let range = start..end.max(start as i64) as usize;

                        // Just in case multiple events with the same time were scheduled.
                        if range.len() <= 0 {
                            return;
                        }

                        let prev_block_was_silent = mem::take(&mut self.prev_block_was_silent);

                        result &= self.process_range(&mut buffers, proc_info, range.clone());

                        start = range.end;

                        prev_block_was_silent
                    } else {
                        self.prev_block_was_silent
                    };

                if (0.99999..1.00001).contains(&gain) {
                    gain = 1.0;
                }

                self.gain.set_value(gain);

                if prev_block_was_silent {
                    // Previous block was silent, so no need to smooth.
                    self.gain.reset();
                }
            },
        );

        let range = start..proc_info.frames;
        result &= self.process_range(&mut buffers, proc_info, range);

        if result.silent.all_channels_silent(buffers.outputs.len()) {
            ProcessStatus::ClearAllOutputs
        } else if result.unity.all_channels_silent(buffers.outputs.len()) {
            ProcessStatus::Bypass
        } else {
            ProcessStatus::OutputsModified {
                out_silence_mask: result.silent,
            }
        }
    }

    fn new_stream(&mut self, stream_info: &firewheel_core::StreamInfo) {
        self.gain.update_sample_rate(stream_info.sample_rate);
    }
}
