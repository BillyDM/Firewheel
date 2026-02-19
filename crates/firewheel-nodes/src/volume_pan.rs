use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::{
        fade::FadeCurve,
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

pub use super::volume::VolumeNodeConfig;

/// A node that applies volume and panning to a stereo signal
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct VolumePanNode {
    /// The overall volume
    pub volume: Volume,
    /// The pan amount, where `0.0` is center, `-1.0` is fully left, and `1.0`
    /// is fully right.
    pub pan: f32,
    /// The algorithm used to map the normalized panning value in the range
    /// `[-1.0, 1.0]` to the corresponding gain values for the left and right
    /// channels.
    pub pan_law: FadeCurve,

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
    /// Construct a new `VolumePanNode` from the given volume and pan values.
    ///
    /// * `volume` - The overall volume.
    /// * `pan` - The pan amount, where `0.0` is center, `-1.0` is fully left,
    ///   and `1.0` is fully right.
    pub const fn from_volume_pan(volume: Volume, pan: f32) -> Self {
        Self {
            volume,
            pan,
            pan_law: FadeCurve::EqualPower3dB,
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
            min_gain: DEFAULT_AMP_EPSILON,
        }
    }

    /// Construct a new `VolumePanNode` from the given pan value.
    ///
    /// The volume will be set to unity gain.
    ///
    /// * `pan` - The pan amount, where `0.0` is center, `-1.0` is fully left,
    ///   and `1.0` is fully right.
    pub const fn from_pan(pan: f32) -> Self {
        Self {
            volume: Volume::UNITY_GAIN,
            pan,
            pan_law: FadeCurve::EqualPower3dB,
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
            min_gain: DEFAULT_AMP_EPSILON,
        }
    }

    /// Construct a new `VolumePanNode` from the given volume.
    ///
    /// The pan amount will be set to `0.0` (center).
    pub const fn from_volume(volume: Volume) -> Self {
        Self {
            volume,
            pan: 0.0,
            pan_law: FadeCurve::EqualPower3dB,
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
            min_gain: DEFAULT_AMP_EPSILON,
        }
    }

    /// Set the given volume in a linear scale, where `0.0` is silence and
    /// `1.0` is unity gain.
    ///
    /// These units are suitable for volume sliders (simply convert percent
    /// volume to linear volume by diving the percent volume by 100).
    pub const fn set_volume_linear(&mut self, linear: f32) {
        self.volume = Volume::Linear(linear);
    }

    /// Set the given volume in percentage, where `0.0` is silence and
    /// `100.0` is unity gain.
    ///
    /// These units are suitable for volume sliders.
    pub const fn set_volume_percent(&mut self, percent: f32) {
        self.volume = Volume::from_percent(percent);
    }

    /// Set the given volume in decibels, where `0.0` is unity gain and
    /// `f32::NEG_INFINITY` is silence.
    pub const fn set_volume_decibels(&mut self, decibels: f32) {
        self.volume = Volume::Decibels(decibels);
    }

    pub fn compute_gains(&self, amp_epsilon: f32) -> (f32, f32) {
        let global_gain = self.volume.amp_clamped(amp_epsilon);

        let (mut gain_l, mut gain_r) = self.pan_law.compute_gains_neg1_to_1(self.pan);

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
            pan_law: FadeCurve::default(),
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
            min_gain,
        }
    }
}

struct Processor {
    gain_l: SmoothedParam,
    gain_r: SmoothedParam,

    params: VolumePanNode,

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
                    self.min_gain = (*min_gain).max(0.0);
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

            if info.prev_output_was_silent {
                // Previous block was silent, so no need to smooth.
                self.gain_l.reset_to_target();
                self.gain_r.reset_to_target();
            }
        }

        if info.in_silence_mask.all_channels_silent(2) {
            self.gain_l.reset_to_target();
            self.gain_r.reset_to_target();

            return ProcessStatus::ClearAllOutputs;
        }

        let in1 = &buffers.inputs[0][..info.frames];
        let in2 = &buffers.inputs[1][..info.frames];
        let (out1, out2) = buffers.outputs.split_first_mut().unwrap();
        let out1 = &mut out1[..info.frames];
        let out2 = &mut out2[0][..info.frames];

        if self.gain_l.has_settled() && self.gain_r.has_settled() {
            if self.gain_l.target_value() <= self.min_gain
                && self.gain_r.target_value() <= self.min_gain
            {
                self.gain_l.reset_to_target();
                self.gain_r.reset_to_target();

                ProcessStatus::ClearAllOutputs
            } else {
                for i in 0..info.frames {
                    out1[i] = in1[i] * self.gain_l.target_value();
                    out2[i] = in2[i] * self.gain_r.target_value();
                }

                ProcessStatus::OutputsModifiedWithMask(MaskType::Silence(info.in_silence_mask))
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

            ProcessStatus::OutputsModified
        }
    }

    fn new_stream(
        &mut self,
        stream_info: &firewheel_core::StreamInfo,
        _context: &mut ProcStreamCtx,
    ) {
        self.gain_l.update_sample_rate(stream_info.sample_rate);
        self.gain_r.update_sample_rate(stream_info.sample_rate);
    }
}
