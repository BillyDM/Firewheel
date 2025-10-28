use bevy_platform::prelude::Vec;
use firewheel_core::{
    channel_config::{ChannelConfig, NonZeroChannelCount},
    event::ProcEvents,
    mask::{MaskType, SilenceMask},
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers,
        ProcExtra, ProcInfo, ProcStreamCtx, ProcessStatus,
    },
};
use smallvec::{smallvec, SmallVec};

/// The configuration for a [`DelayCompensationNode`]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DelayCompNodeConfig {
    /// The number of input and output channels.
    pub channels: NonZeroChannelCount,
    /// The number of frames (samples in a single channel of audio) of
    /// delay compensation.
    pub delay_frames: usize,
}

impl Default for DelayCompNodeConfig {
    fn default() -> Self {
        Self {
            channels: NonZeroChannelCount::STEREO,
            delay_frames: 0,
        }
    }
}

/// A node which delays a signal by a given number samples.
///
/// This can be used to avoid phasing issues (comb filtering) caused by
/// parallel signal paths having differing latencies.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DelayCompensationNode;

impl AudioNode for DelayCompensationNode {
    type Configuration = DelayCompNodeConfig;

    fn info(&self, config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("stereo_to_mono")
            .channel_config(ChannelConfig {
                num_inputs: config.channels.get(),
                num_outputs: config.channels.get(),
            })
    }

    fn construct_processor(
        &self,
        config: &Self::Configuration,
        _cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        let channels = config.channels.get().get() as usize;
        let buffer_len = channels * config.delay_frames;

        let mut buffer: Vec<f32> = Vec::new();
        buffer.reserve_exact(buffer_len);
        buffer.resize(buffer_len, 0.0);

        Processor {
            buffer,
            delay_frames: config.delay_frames,
            ptr: 0,
            num_silent_frames_per_channel: smallvec![config.delay_frames; channels],
        }
    }
}

struct Processor {
    buffer: Vec<f32>,
    delay_frames: usize,
    ptr: usize,
    num_silent_frames_per_channel: SmallVec<[usize; 4]>,
}

impl AudioNodeProcessor for Processor {
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        _events: &mut ProcEvents,
        _extra: &mut ProcExtra,
    ) -> ProcessStatus {
        if self.delay_frames == 0 {
            return ProcessStatus::Bypass;
        }

        // TODO: Use constant mask instead
        let mut out_silence_mask = SilenceMask::NONE_SILENT;

        let extra_input_frames = if info.frames > self.delay_frames {
            info.frames - self.delay_frames
        } else {
            0
        };
        let first_copy_frames = info.frames.min(self.delay_frames - self.ptr);
        let second_copy_frames = (info.frames - first_copy_frames).min(self.ptr);

        for (ch_i, (((in_buf, out_buf), delay_buf), num_silent_frames)) in buffers
            .inputs
            .iter()
            .zip(buffers.outputs.iter_mut())
            .zip(self.buffer.chunks_exact_mut(self.delay_frames))
            .zip(self.num_silent_frames_per_channel.iter_mut())
            .enumerate()
        {
            let is_input_silent = info.in_silence_mask.is_channel_silent(ch_i);

            let clear_output = *num_silent_frames == self.delay_frames
                && (info.frames <= self.delay_frames || is_input_silent);

            if clear_output {
                if !info.out_silence_mask.is_channel_silent(ch_i) {
                    out_buf[..info.frames].fill(0.0);
                }

                out_silence_mask.set_channel(ch_i, true);
            } else {
                out_buf[..first_copy_frames]
                    .copy_from_slice(&delay_buf[self.ptr..self.ptr + first_copy_frames]);

                if second_copy_frames > 0 {
                    out_buf[first_copy_frames..first_copy_frames + second_copy_frames]
                        .copy_from_slice(&delay_buf[..second_copy_frames]);
                }

                if extra_input_frames > 0 {
                    if is_input_silent {
                        out_buf[self.delay_frames..info.frames].fill(0.0);
                    } else {
                        out_buf[self.delay_frames..info.frames]
                            .copy_from_slice(&in_buf[..extra_input_frames]);
                    }
                }
            }

            if !is_input_silent || *num_silent_frames < self.delay_frames {
                delay_buf[self.ptr..self.ptr + first_copy_frames].copy_from_slice(
                    &in_buf[extra_input_frames..extra_input_frames + first_copy_frames],
                );

                if second_copy_frames > 0 {
                    delay_buf[..second_copy_frames].copy_from_slice(
                        &in_buf[extra_input_frames + first_copy_frames..info.frames],
                    );
                }
            }

            *num_silent_frames = if is_input_silent {
                (*num_silent_frames + info.frames).min(self.delay_frames)
            } else {
                0
            };
        }

        if info.frames < self.delay_frames {
            self.ptr += info.frames;
            if self.ptr >= self.delay_frames {
                self.ptr -= self.delay_frames;
            }
        }

        ProcessStatus::OutputsModifiedWithMask(MaskType::Silence(out_silence_mask))
    }

    fn new_stream(
        &mut self,
        _stream_info: &firewheel_core::StreamInfo,
        _context: &mut ProcStreamCtx,
    ) {
        self.buffer.fill(0.0);
        self.num_silent_frames_per_channel.fill(self.delay_frames);
    }
}
