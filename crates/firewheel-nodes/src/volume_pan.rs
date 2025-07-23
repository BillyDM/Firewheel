use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::{pan_law::PanLaw, volume::Volume},
    event::NodeEventList,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers,
        ProcInfo, ProcessStatus,
    },
    param::smoother::{SmoothedParam, SmootherConfig},
    SilenceMask,
};

pub use super::volume::VolumeNodeConfig;

// TODO: Option for true stereo panning?

#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct VolumePanNode {
    /// The overall volume.
    pub volume: Volume,
    /// The pan amount, where `0.0` is center, `-1.0` is fully left, and `1.0` is
    /// fully right.
    pub pan: f32,
    /// The algorithm to use to map a normalized panning value in the range `[-1.0, 1.0]`
    /// to the corresponding gain values for the left and right channels.
    pub pan_law: PanLaw,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolumePanNodeConfig {
    /// The time in seconds of the internal smoothing filter.
    ///
    /// By default this is set to `0.01` (10ms).
    pub smooth_secs: f32,
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
        config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        let (gain_l, gain_r) = self.compute_gains(config.amp_epsilon);

        Processor {
            gain_l: SmoothedParam::new(
                gain_l,
                SmootherConfig {
                    smooth_secs: config.smooth_secs,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            gain_r: SmoothedParam::new(
                gain_r,
                SmootherConfig {
                    smooth_secs: config.smooth_secs,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            params: *self,
            prev_block_was_silent: true,
            amp_epsilon: config.amp_epsilon,
        }
    }
}

struct Processor {
    gain_l: SmoothedParam,
    gain_r: SmoothedParam,

    params: VolumePanNode,

    prev_block_was_silent: bool,
    amp_epsilon: f32,
}

impl AudioNodeProcessor for Processor {
    fn process(
        &mut self,
        buffers: ProcBuffers,
        proc_info: &ProcInfo,
        events: &mut NodeEventList,
    ) -> ProcessStatus {
        let mut updated = false;
        for mut patch in events.drain_patches::<VolumePanNode>() {
            // here we selectively clamp the panning, leaving
            // other patches untouched
            if let VolumePanNodePatch::Pan(p) = &mut patch {
                *p = p.clamp(-1.0, 1.0);
            }

            self.params.apply(patch);
            updated = true;
        }

        if updated {
            let (gain_l, gain_r) = self.params.compute_gains(self.amp_epsilon);
            self.gain_l.set_value(gain_l);
            self.gain_r.set_value(gain_r);

            if self.prev_block_was_silent {
                // Previous block was silent, so no need to smooth.
                self.gain_l.reset();
                self.gain_r.reset();
            }
        }

        self.prev_block_was_silent = false;

        if proc_info.in_silence_mask.all_channels_silent(2) {
            self.gain_l.reset();
            self.gain_r.reset();
            self.prev_block_was_silent = true;

            return ProcessStatus::ClearAllOutputs;
        }

        let in1 = &buffers.inputs[0][..proc_info.frames];
        let in2 = &buffers.inputs[1][..proc_info.frames];
        let (out1, out2) = buffers.outputs.split_first_mut().unwrap();
        let out1 = &mut out1[..proc_info.frames];
        let out2 = &mut out2[0][..proc_info.frames];

        if !self.gain_l.is_smoothing() && !self.gain_r.is_smoothing() {
            if self.gain_l.target_value() == 0.0 && self.gain_r.target_value() == 0.0 {
                self.gain_l.reset();
                self.gain_r.reset();
                self.prev_block_was_silent = true;

                ProcessStatus::ClearAllOutputs
            } else {
                for i in 0..proc_info.frames {
                    out1[i] = in1[i] * self.gain_l.target_value();
                    out2[i] = in2[i] * self.gain_r.target_value();
                }

                ProcessStatus::outputs_modified(proc_info.in_silence_mask)
            }
        } else {
            for i in 0..proc_info.frames {
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
