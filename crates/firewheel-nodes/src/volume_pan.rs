use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::{
        filter::smoothing_filter::DEFAULT_SMOOTH_SECONDS,
        pan_law::PanLaw,
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

pub use super::volume::VolumeNodeConfig;

// TODO: Option for true stereo panning?

#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
pub struct VolumePanNode {
    /// The overall volume.
    pub volume: Volume,
    /// The pan amount, where `0.0` is center, `-1.0` is fully left, and `1.0` is
    /// fully right.
    pub pan: f32,
    /// The algorithm to use to map a normalized panning value in the range `[-1.0, 1.0]`
    /// to the corresponding gain values for the left and right channels.
    pub pan_law: PanLaw,

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

impl VolumePanNode {
    pub fn compute_gains(&self, amp_epsilon: f32) -> (f32, f32) {
        let global_gain = self.volume.amp_clamped(amp_epsilon);

        let (mut gain_l, mut gain_r) = self.pan_law.compute_gains(self.pan);

        gain_l *= global_gain;
        gain_r *= global_gain;

        if gain_l > 0.99999 && gain_l < 1.00001 {
            gain_l = 1.0;
        }
        if gain_r > 0.99999 && gain_r < 1.00001 {
            gain_r = 1.0;
        }

        (gain_l, gain_r)
    }
}

impl Default for VolumePanNode {
    fn default() -> Self {
        Self {
            volume: Volume::default(),
            pan: 0.0,
            pan_law: PanLaw::default(),
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
            min_gain: DEFAULT_AMP_EPSILON,
        }
    }
}

impl AudioNode for VolumePanNode {
    type Configuration = VolumeNodeConfig;

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("volume_pan")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::STEREO,
                num_outputs: ChannelCount::STEREO,
            })
    }

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        let min_gain = self.min_gain.max(0.0);

        let (gain_l, gain_r) = self.compute_gains(self.min_gain);

        Processor {
            gain_l: SmoothedParam::new(
                gain_l,
                SmootherConfig {
                    smooth_seconds: self.smooth_seconds,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            gain_r: SmoothedParam::new(
                gain_r,
                SmootherConfig {
                    smooth_seconds: self.smooth_seconds,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            params: *self,
            prev_block_was_silent: true,
            min_gain,
        }
    }
}

struct Processor {
    gain_l: SmoothedParam,
    gain_r: SmoothedParam,

    params: VolumePanNode,

    prev_block_was_silent: bool,
    min_gain: f32,
}

impl AudioNodeProcessor for Processor {
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        events: &mut ProcEvents,
        _extra: &mut ProcExtra,
    ) -> ProcessStatus {
        let mut updated = false;
        for mut patch in events.drain_patches::<VolumePanNode>() {
            match &mut patch {
                VolumePanNodePatch::Pan(p) => {
                    *p = p.clamp(-1.0, 1.0);
                }
                VolumePanNodePatch::SmoothSeconds(seconds) => {
                    self.gain_l.set_smooth_seconds(*seconds, info.sample_rate);
                    self.gain_r.set_smooth_seconds(*seconds, info.sample_rate);
                }
                VolumePanNodePatch::MinGain(min_gain) => {
                    self.min_gain = min_gain.max(0.0);
                }
                _ => {}
            }

            self.params.apply(patch);
            updated = true;
        }

        if updated {
            let (gain_l, gain_r) = self.params.compute_gains(self.min_gain);
            self.gain_l.set_value(gain_l);
            self.gain_r.set_value(gain_r);

            if self.prev_block_was_silent {
                // Previous block was silent, so no need to smooth.
                self.gain_l.reset();
                self.gain_r.reset();
            }
        }

        self.prev_block_was_silent = false;

        if info.in_silence_mask.all_channels_silent(2) {
            self.gain_l.reset();
            self.gain_r.reset();
            self.prev_block_was_silent = true;

            return ProcessStatus::ClearAllOutputs;
        }

        let in1 = &buffers.inputs[0][..info.frames];
        let in2 = &buffers.inputs[1][..info.frames];
        let (out1, out2) = buffers.outputs.split_first_mut().unwrap();
        let out1 = &mut out1[..info.frames];
        let out2 = &mut out2[0][..info.frames];

        if !self.gain_l.is_smoothing() && !self.gain_r.is_smoothing() {
            if self.gain_l.target_value() == 0.0 && self.gain_r.target_value() == 0.0 {
                self.gain_l.reset();
                self.gain_r.reset();
                self.prev_block_was_silent = true;

                ProcessStatus::ClearAllOutputs
            } else {
                for i in 0..info.frames {
                    out1[i] = in1[i] * self.gain_l.target_value();
                    out2[i] = in2[i] * self.gain_r.target_value();
                }

                ProcessStatus::outputs_modified(info.in_silence_mask)
            }
        } else {
            for i in 0..info.frames {
                let gain_l = self.gain_l.next_smoothed();
                let gain_r = self.gain_r.next_smoothed();

                out1[i] = in1[i] * gain_l;
                out2[i] = in2[i] * gain_r;
            }

            self.gain_l.settle();
            self.gain_r.settle();

            ProcessStatus::outputs_modified(SilenceMask::NONE_SILENT)
        }
    }

    fn new_stream(&mut self, stream_info: &firewheel_core::StreamInfo) {
        self.gain_l.update_sample_rate(stream_info.sample_rate);
        self.gain_r.update_sample_rate(stream_info.sample_rate);
    }
}
