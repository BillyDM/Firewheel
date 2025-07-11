//! A 3D spatial positioning node using a basic (and naive) algorithm. It does
//! not make use of any fancy binaural algorithms, rather it just applies basic
//! panning and filtering.

use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::{
        filter::single_pole_iir::{OnePoleIirLPF, OnePoleIirLPFCoeff},
        pan_law::PanLaw,
        volume::{Volume, DEFAULT_AMP_EPSILON},
    },
    event::Vec3,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, GetChannels,
        ProcInfo, ProcessStatus, SimpleAudioProcessor,
    },
    param::smoother::{SmoothedParam, SmootherConfig},
    SilenceMask,
};

const DAMPING_CUTOFF_HZ_MIN: f32 = 20.0;
const DAMPING_CUTOFF_HZ_MAX: f32 = 20_480.0;
const CALC_FILTER_COEFF_INTERVAL: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct SpatialBasicConfig {
    /// The time in seconds of the internal smoothing filter.
    ///
    /// By default this is set to `0.01` (10ms).
    pub smooth_secs: f32,

    /// If the resutling amplitude of the volume is less than or equal to this
    /// value, then the amplitude will be clamped to `0.0` (silence).
    pub amp_epsilon: f32,
}

impl Default for SpatialBasicConfig {
    fn default() -> Self {
        Self {
            smooth_secs: 10.0 / 1_000.0,
            amp_epsilon: DEFAULT_AMP_EPSILON,
        }
    }
}

/// The parameters for a 3D spatial positioning node using a basic (and naive) algorithm.
/// It does not make use of any fancy binaural algorithms, rather it just applies basic
/// panning and filtering.
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct SpatialBasicNode {
    /// The overall volume. This is applied before the spatialization algorithm.
    pub volume: Volume,

    /// A 3D vector representing the offset between the listener and the
    /// sound source.
    ///
    /// The coordinates are `(x, y, z)`.
    ///
    /// * `-x` is to the left of the listener, and `+x` is the the right of the listener
    /// * `-y` is below the listener, and `+y` is above the listener.
    /// * `-z` is in front of the listener, and `+z` is behind the listener
    ///
    /// The origin `(0.0, 0.0, 0.0)` will have a volume equal to the original signal
    /// (with the `normalized_volume` paramter applied). A  distance  of `10.0`
    /// from the origin will have a volume equal to `-6dB`, a distance of `20.0` will
    /// have a volume equal to `-12dB`, a distance of `40.0` will have a volume equal
    /// to `-24dB`, and so on (every doubling of distance is a 6dB reduction in
    /// volume).
    ///
    /// 1 unit is roughly equal to 1 meter (if I did my math right), but you may wish
    /// to scale this unit as you see fit.
    ///
    /// By default this is set to `(0.0, 0.0, 0.0)`
    pub offset: Vec3,

    /// The distance at which the signal becomes fully dampened (lowpassed).
    ///
    /// Set to a negative value or NAN for no damping.
    ///
    /// By default this is set to `100`.
    pub damping_distance: f32,

    /// The amount of muffling (lowpass cutoff hin Hz) in the range `[20.0, 20_480.0]`,
    /// where `20_480.0` is no muffling and `20.0` is maximum muffling.
    ///
    /// This can be used to give the effect of a sound being played behind a wall
    /// or underwater.
    ///
    /// By default this is set to `20_480.0`.
    pub muffle_cutoff_hz: f32,

    /// The threshold for the maximum amount of panning that can occur, in the range
    /// `[0.0, 1.0]`, where `0.0` is no panning and `1.0` is full panning (where one
    /// of the channels is fully silent when panned hard left or right).
    ///
    /// Setting this to a value less than `1.0` can help remove some of the
    /// jarringness of having a sound playing in only one ear.
    ///
    /// By default this is set to `0.6`.
    pub panning_threshold: f32,
}

impl Default for SpatialBasicNode {
    fn default() -> Self {
        Self {
            volume: Volume::default(),
            offset: Vec3::new(0.0, 0.0, 0.0),
            damping_distance: 100.0,
            muffle_cutoff_hz: DAMPING_CUTOFF_HZ_MAX,
            panning_threshold: 0.6,
        }
    }
}

impl SpatialBasicNode {
    pub fn compute_values(&self, amp_epsilon: f32) -> ComputedValues {
        let x2_z2 = (self.offset[0] * self.offset[0]) + (self.offset[2] * self.offset[2]);
        let xyz_distance = (x2_z2 + (self.offset[1] * self.offset[1])).sqrt();
        let xz_distance = x2_z2.sqrt();

        let distance_gain = 10.0f32.powf(-0.03 * xyz_distance);

        let pan = if xz_distance > 0.0 {
            (self.offset[0] / xz_distance) * self.panning_threshold.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (pan_gain_l, pan_gain_r) = PanLaw::EqualPower3dB.compute_gains(pan);

        let mut volume_gain = self.volume.amp();
        if volume_gain > 0.99999 && volume_gain < 1.00001 {
            volume_gain = 1.0;
        }
        if volume_gain <= amp_epsilon {
            volume_gain = 0.0;
        }

        let muffle_cutoff_hz = if self.muffle_cutoff_hz > DAMPING_CUTOFF_HZ_MAX - 0.00001 {
            DAMPING_CUTOFF_HZ_MAX
        } else {
            self.muffle_cutoff_hz
                .clamp(DAMPING_CUTOFF_HZ_MIN, DAMPING_CUTOFF_HZ_MAX)
        };

        let damping_cutoff_hz = if self.damping_distance.is_finite() && self.damping_distance >= 0.0
        {
            if self.damping_distance < 0.00001 {
                Some(DAMPING_CUTOFF_HZ_MIN)
            } else {
                let damp_normal =
                    1.0 - (xyz_distance.min(self.damping_distance) / self.damping_distance);
                Some(
                    (DAMPING_CUTOFF_HZ_MIN
                        + ((muffle_cutoff_hz - DAMPING_CUTOFF_HZ_MIN) * damp_normal))
                        .clamp(DAMPING_CUTOFF_HZ_MIN, muffle_cutoff_hz),
                )
            }
        } else {
            if muffle_cutoff_hz == DAMPING_CUTOFF_HZ_MAX {
                None
            } else {
                Some(muffle_cutoff_hz)
            }
        };

        let mut gain_l = pan_gain_l * distance_gain * volume_gain;
        let mut gain_r = pan_gain_r * distance_gain * volume_gain;

        if gain_l <= amp_epsilon {
            gain_l = 0.0;
        }
        if gain_r <= amp_epsilon {
            gain_r = 0.0;
        }

        ComputedValues {
            gain_l,
            gain_r,
            damping_cutoff_hz,
        }
    }
}

pub struct ComputedValues {
    pub gain_l: f32,
    pub gain_r: f32,
    pub damping_cutoff_hz: Option<f32>,
}

impl AudioNode for SpatialBasicNode {
    type Configuration = SpatialBasicConfig;

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("spatial_basic")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::STEREO,
                num_outputs: ChannelCount::STEREO,
            })
            .uses_events(true)
    }

    fn construct_processor(
        &self,
        config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        let computed_values = self.compute_values(config.amp_epsilon);

        Processor {
            gain_l: SmoothedParam::new(
                computed_values.gain_l,
                SmootherConfig {
                    smooth_secs: config.smooth_secs,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            gain_r: SmoothedParam::new(
                computed_values.gain_r,
                SmootherConfig {
                    smooth_secs: config.smooth_secs,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            damping_cutoff_hz: SmoothedParam::new(
                computed_values
                    .damping_cutoff_hz
                    .unwrap_or(DAMPING_CUTOFF_HZ_MAX),
                SmootherConfig {
                    smooth_secs: config.smooth_secs,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            damping_disabled: computed_values.damping_cutoff_hz.is_none(),
            filter_l: OnePoleIirLPF::default(),
            filter_r: OnePoleIirLPF::default(),
            params: *self,
            prev_block_was_silent: true,
            amp_epsilon: config.amp_epsilon,
            updated: false,
        }
    }
}

struct Processor {
    gain_l: SmoothedParam,
    gain_r: SmoothedParam,
    damping_cutoff_hz: SmoothedParam,
    damping_disabled: bool,

    filter_l: OnePoleIirLPF,
    filter_r: OnePoleIirLPF,

    params: SpatialBasicNode,

    prev_block_was_silent: bool,
    amp_epsilon: f32,
    updated: bool,
}

impl SimpleAudioProcessor for Processor {
    type Params = SpatialBasicNode;

    fn apply_patch(&mut self, mut patch: SpatialBasicNodePatch) {
        match &mut patch {
            SpatialBasicNodePatch::Offset(offset) => {
                if !offset.is_finite() {
                    *offset = Vec3::default();
                }
            }
            SpatialBasicNodePatch::MuffleCutoffHz(cutoff) => {
                *cutoff = cutoff.clamp(DAMPING_CUTOFF_HZ_MIN, DAMPING_CUTOFF_HZ_MAX);
            }
            SpatialBasicNodePatch::PanningThreshold(threshold) => {
                *threshold = threshold.clamp(0.0, 1.0);
            }
            _ => {}
        }

        self.params.apply(patch);
        self.updated = true;
    }

    fn can_skip_patch(&self, patch: &SpatialBasicNodePatch) -> bool {
        match patch {
            &SpatialBasicNodePatch::Offset(mut offset) => {
                if !offset.is_finite() {
                    offset = Vec3::default();
                }

                (offset - self.params.offset).length_squared() < f32::EPSILON
            }
            SpatialBasicNodePatch::MuffleCutoffHz(cutoff) => {
                let cutoff = cutoff.clamp(DAMPING_CUTOFF_HZ_MIN, DAMPING_CUTOFF_HZ_MAX);

                (cutoff - self.params.muffle_cutoff_hz).abs() < f32::EPSILON
            }
            SpatialBasicNodePatch::PanningThreshold(threshold) => {
                let threshold = threshold.clamp(0.0, 1.0);

                (threshold - self.params.panning_threshold).abs() < f32::EPSILON
            }
            SpatialBasicNodePatch::Volume(vol) => {
                (vol.amp() - self.params.volume.amp()).abs() < f32::EPSILON
            }
            SpatialBasicNodePatch::DampingDistance(dist) => {
                (*dist - self.params.damping_distance).abs() < f32::EPSILON
            }
        }
    }

    fn process<B: GetChannels>(&mut self, mut buffers: B, proc_info: &ProcInfo) -> ProcessStatus {
        if self.updated {
            let computed_values = self.params.compute_values(self.amp_epsilon);

            self.gain_l.set_value(computed_values.gain_l);
            self.gain_r.set_value(computed_values.gain_r);

            if let Some(cutoff_hz) = computed_values.damping_cutoff_hz {
                self.damping_cutoff_hz.set_value(cutoff_hz);
                self.damping_disabled = false;
            } else {
                self.damping_cutoff_hz.set_value(DAMPING_CUTOFF_HZ_MAX);
                self.damping_disabled = true;
            }

            if self.prev_block_was_silent {
                // Previous block was silent, so no need to smooth.
                self.gain_l.reset();
                self.gain_r.reset();
                self.damping_cutoff_hz.reset();
                self.filter_l.reset();
                self.filter_r.reset();
            }
        }

        self.updated = false;

        self.prev_block_was_silent = false;

        if proc_info.in_silence_mask.all_channels_silent(2) {
            self.gain_l.reset();
            self.gain_r.reset();
            self.damping_cutoff_hz.reset();
            self.filter_l.reset();
            self.filter_r.reset();

            self.prev_block_was_silent = true;

            return ProcessStatus::ClearAllOutputs;
        }

        let mut buffers = buffers.channels();

        let in1 = buffers.inputs.next().unwrap();
        let in2 = buffers.inputs.next().unwrap();
        let out1 = buffers.outputs.next().unwrap();
        let out2 = buffers.outputs.next().unwrap();

        if !self.gain_l.is_smoothing()
            && !self.gain_r.is_smoothing()
            && !self.damping_cutoff_hz.is_smoothing()
        {
            if self.gain_l.target_value() == 0.0 && self.gain_r.target_value() == 0.0 {
                self.gain_l.reset();
                self.gain_r.reset();
                self.damping_cutoff_hz.reset();
                self.filter_l.reset();
                self.filter_r.reset();

                self.prev_block_was_silent = true;

                return ProcessStatus::ClearAllOutputs;
            } else if self.damping_disabled {
                for i in 0..proc_info.frames {
                    out1[i] = in1[i] * self.gain_l.target_value();
                    out2[i] = in2[i] * self.gain_r.target_value();
                }
            } else {
                // The cutoff parameter is not currently smoothing, so we can optimize by
                // only updating the filter coefficients once.
                let coeff = OnePoleIirLPFCoeff::new(
                    self.damping_cutoff_hz.target_value(),
                    proc_info.sample_rate_recip as f32,
                );

                for i in 0..proc_info.frames {
                    out1[i] = in1[i] * self.gain_l.target_value();
                    out2[i] = in2[i] * self.gain_r.target_value();

                    out1[i] = self.filter_l.process(out1[i], coeff);
                    out2[i] = self.filter_r.process(out2[i], coeff);
                }
            }

            ProcessStatus::outputs_modified(proc_info.in_silence_mask);
        } else {
            if self.damping_disabled && !self.damping_cutoff_hz.is_smoothing() {
                for i in 0..proc_info.frames {
                    let gain_l = self.gain_l.next_smoothed();
                    let gain_r = self.gain_r.next_smoothed();

                    out1[i] = in1[i] * gain_l;
                    out2[i] = in2[i] * gain_r;
                }
            } else {
                let mut coeff = OnePoleIirLPFCoeff::default();

                for i in 0..proc_info.frames {
                    let cutoff_hz = self.damping_cutoff_hz.next_smoothed();
                    let gain_l = self.gain_l.next_smoothed();
                    let gain_r = self.gain_r.next_smoothed();

                    out1[i] = in1[i] * gain_l;
                    out2[i] = in2[i] * gain_r;

                    // Because recalculating filter coefficients is expensive, a trick like
                    // this can be use to only recalculate them every CALC_FILTER_COEFF_INTERVAL
                    // frames.
                    if i & (CALC_FILTER_COEFF_INTERVAL - 1) == 0 {
                        coeff =
                            OnePoleIirLPFCoeff::new(cutoff_hz, proc_info.sample_rate_recip as f32);
                    }

                    out1[i] = self.filter_l.process(out1[i], coeff);
                    out2[i] = self.filter_r.process(out2[i], coeff);
                }
            }

            self.gain_l.settle();
            self.gain_r.settle();
            self.damping_cutoff_hz.settle();
        }

        ProcessStatus::outputs_modified(SilenceMask::NONE_SILENT)
    }

    fn new_stream(&mut self, stream_info: &firewheel_core::StreamInfo) {
        self.gain_l.update_sample_rate(stream_info.sample_rate);
        self.gain_r.update_sample_rate(stream_info.sample_rate);
        self.damping_cutoff_hz
            .update_sample_rate(stream_info.sample_rate);
    }
}
