//! A 3D spatial positioning node using a basic (and naive) algorithm. It does
//! not make use of any fancy binaural algorithms, rather it just applies basic
//! panning and filtering.

#[cfg(not(feature = "std"))]
use num_traits::Float;

use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::{
        filter::{
            single_pole_iir::{OnePoleIirLPF, OnePoleIirLPFCoeff},
            smoothing_filter::DEFAULT_SMOOTH_SECONDS,
        },
        pan_law::PanLaw,
        volume::Volume,
    },
    event::ProcEvents,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        ProcBuffers, ProcExtra, ProcInfo, ProcessStatus,
    },
    param::smoother::{SmoothedParam, SmootherConfig},
    vector::Vec3,
    ConnectedMask, SilenceMask,
};

const MUFFLE_CUTOFF_HZ_MIN: f32 = 20.0;
const MUFFLE_CUTOFF_HZ_MAX: f32 = 20_480.0;
const MUFFLE_CUTOFF_HZ_RANGE_RECIP: f32 = 1.0 / (MUFFLE_CUTOFF_HZ_MAX - MUFFLE_CUTOFF_HZ_MIN);
const CALC_FILTER_COEFF_INTERVAL: usize = 8;

/// The method in which to calculate the volume of a sound based on the distance from
/// the listener.
///
/// Based on <https://developer.mozilla.org/en-US/docs/Web/API/PannerNode/distanceModel>
///
/// Interactive graph of the different models: <https://www.desmos.com/calculator/g1pbsc5m9y>
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Diff, Patch)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
pub enum DistanceModel {
    #[default]
    /// A linear distance model calculates the gain by:
    ///
    /// `reference_distance / (reference_distance + rolloff_factor * (max(distance, reference_distance) - reference_distance))`
    ///
    /// This mostly closely matches how sound is attenuated in the real world, and is the default model.
    Inverse,
    /// A linear distance model calculates the gain by:
    ///
    /// `(1.0 - rolloff_factor * (distance - reference_distance) / (max_distance - reference_distance)).clamp(0.0, 1.0)`
    Linear,
    /// An exponential distance model calculates the gain by:
    ///
    /// `pow((max(distance, reference_distance) / reference_distance, -rolloff_factor)`
    ///
    /// This is equivalent to [`DistanceModel::Inverse`] when `rolloff_factor = 1.0`.
    Exponential,
}

impl DistanceModel {
    fn calculate_gain(
        &self,
        distance: f32,
        rolloff_factor: f32,
        reference_distance: f32,
        maximum_distance: f32,
    ) -> f32 {
        if distance <= reference_distance || rolloff_factor <= 0.00001 {
            return 1.0;
        }

        match self {
            DistanceModel::Inverse => {
                reference_distance
                    / (reference_distance + (rolloff_factor * (distance - reference_distance)))
            }
            DistanceModel::Linear => {
                if maximum_distance <= reference_distance {
                    1.0
                } else {
                    (1.0 - (rolloff_factor * (distance - reference_distance)
                        / (maximum_distance - reference_distance)))
                        .clamp(0.0, 1.0)
                }
            }
            DistanceModel::Exponential => (distance / reference_distance).powf(-rolloff_factor),
        }
    }
}

/// The parameters for a 3D spatial positioning node using a basic (and naive) algorithm.
/// It does not make use of any fancy binaural algorithms, rather it just applies basic
/// panning and filtering.
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
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
    /// By default this is set to `(0.0, 0.0, 0.0)`
    pub offset: Vec3,

    /// The method in which to calculate the volume of a sound based on the distance from
    /// the listener.
    ///
    /// by default this is set to [`DistanceModel::Inverse`].
    ///
    /// Based on <https://developer.mozilla.org/en-US/docs/Web/API/PannerNode/distanceModel>
    ///
    /// Interactive graph of the different models: <https://www.desmos.com/calculator/g1pbsc5m9y>
    pub distance_model: DistanceModel,

    /// The factor by which the sound gets quieter the farther away it is from the
    /// listener.
    ///
    /// Values less than `1.0` will attenuate the sound less per unit distance, and values
    /// greater than `1.0` will attenuate the sound more per unit distance.
    ///
    /// Set to a value `<= 0.00001` to disable attenuating the sound.
    ///
    /// By default this is set to `1.0`.
    ///
    /// See <https://www.desmos.com/calculator/g1pbsc5m9y> for an interactive graph of
    /// how these parameters affect the final volume of a sound for each distance model.
    pub distance_gain_factor: f32,

    /// The minimum distance at which a sound is considered to be at the maximum volume.
    /// (Distances less than this value will be clamped at the maximum volume).
    ///
    /// If this value is `< 0.00001`, then it will be clamped to `0.00001`.
    ///
    /// By default this is set to `5.0`.
    ///
    /// See <https://www.desmos.com/calculator/g1pbsc5m9y> for an interactive graph of
    /// how these parameters affect the final volume of a sound for each distance model.
    pub reference_distance: f32,

    /// When using [`DistanceModel::Linear`], the maximum reference distance (at a
    /// rolloff factor of `1.0`) of a sound before it is considered to be "silent".
    /// (Distances greater than this value will be clamped to silence).
    ///
    /// If this value is `< 0.0`, then it will be clamped to `0.0`.
    ///
    /// By default this is set to `200.0`.
    ///
    /// See <https://www.desmos.com/calculator/g1pbsc5m9y> for an interactive graph of
    /// how these parameters affect the final volume of a sound for each distance model.
    pub max_distance: f32,

    /// The threshold for the maximum amount of panning that can occur, in the range
    /// `[0.0, 1.0]`, where `0.0` is no panning and `1.0` is full panning (where one
    /// of the channels is fully silent when panned hard left or right).
    ///
    /// Setting this to a value less than `1.0` can help remove some of the
    /// jarringness of having a sound playing in only one ear.
    ///
    /// By default this is set to `0.6`.
    pub panning_threshold: f32,

    /// The factor which determines the curve of the high frequency damping (lowpass)
    /// in relation to distance.
    ///
    /// Higher values dampen the high frequencies faster, while smaller values dampen
    /// the high frequencies slower.
    ///
    /// Set to a value `<= 0.00001` to disable muffling the sound based on distance.
    ///
    /// By default this is set to `1.9`.
    ///
    /// See <https://www.desmos.com/calculator/jxp8t9ero4> for an interactive graph of
    /// how these parameters affect the final lowpass cuttoff frequency.
    pub distance_muffle_factor: f32,

    /// The distance at which the high frequencies of a sound become fully muffled
    /// (lowpassed).
    ///
    /// Distances less than `reference_distance` will have no muffling.
    ///
    /// This has no effect if `muffle_factor` is `None`.
    ///
    /// By default this is set to `200.0`.
    ///
    /// See <https://www.desmos.com/calculator/jxp8t9ero4> for an interactive graph of
    /// how these parameters affect the final lowpass cuttoff frequency.
    pub max_muffle_distance: f32,

    /// The amount of muffling (lowpass) at `max_muffle_distance` in the range
    /// `[20.0, 20_480.0]`, where `20_480.0` is no muffling and `20.0` is maximum
    /// muffling.
    ///
    /// This has no effect if `muffle_factor` is `None`.
    ///
    /// By default this is set to `20.0`.
    ///
    /// See <https://www.desmos.com/calculator/jxp8t9ero4> for an interactive graph of
    /// how these parameters affect the final lowpass cuttoff frequency.
    pub max_distance_muffle_cutoff_hz: f32,

    /// The amount of muffling (lowpass) in the range `[20.0, 20_480.0]`,
    /// where `20_480.0` is no muffling and `20.0` is maximum muffling.
    ///
    /// This can be used to give the effect of a sound being played behind a wall
    /// or underwater.
    ///
    /// By default this is set to `20_480.0`.
    ///
    /// See <https://www.desmos.com/calculator/jxp8t9ero4> for an interactive graph of
    /// how these parameters affect the final lowpass cuttoff frequency.
    pub muffle_cutoff_hz: f32,

    /// If `true`, then any stereo input signals will be downmixed to mono before
    /// going throught the spatialization algorithm. If `false` then the left and
    /// right channels will be processed independently.
    ///
    /// This has no effect if only one input channel is connected.
    ///
    /// By default this is set to `true`.
    pub downmix: bool,

    /// The time in seconds of the internal smoothing filter.
    ///
    /// By default this is set to `0.015` (15ms).
    pub smooth_seconds: f32,
    /// If the resutling gain (in raw amplitude, not decibels) is less than or equal
    /// to this value, the the gain will be clamped to `0` (silence).
    ///
    /// By default this is set to "0.0001" (-80 dB).
    pub min_gain: f32,
}

impl Default for SpatialBasicNode {
    fn default() -> Self {
        Self {
            volume: Volume::default(),
            offset: Vec3::new(0.0, 0.0, 0.0),
            distance_model: DistanceModel::Inverse,
            distance_gain_factor: 1.0,
            reference_distance: 5.0,
            max_distance: 200.0,
            panning_threshold: 0.6,
            distance_muffle_factor: 1.9,
            max_muffle_distance: 200.0,
            max_distance_muffle_cutoff_hz: 20.0,
            muffle_cutoff_hz: MUFFLE_CUTOFF_HZ_MAX,
            downmix: true,
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
            min_gain: 0.0001,
        }
    }
}

impl SpatialBasicNode {
    pub fn compute_values(&self) -> ComputedValues {
        let x2_z2 = (self.offset.x * self.offset.x) + (self.offset.z * self.offset.z);
        let xyz_distance = (x2_z2 + (self.offset.y * self.offset.y)).sqrt();
        let xz_distance = x2_z2.sqrt();

        let distance_gain = self.distance_model.calculate_gain(
            xyz_distance,
            self.distance_gain_factor,
            self.reference_distance,
            self.max_distance,
        );

        let pan = if xz_distance > 0.0 {
            (self.offset.x / xz_distance) * self.panning_threshold.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (pan_gain_l, pan_gain_r) = PanLaw::EqualPower3dB.compute_gains(pan);

        let mut volume_gain = self.volume.amp();
        if volume_gain > 0.99999 && volume_gain < 1.00001 {
            volume_gain = 1.0;
        }
        if volume_gain <= self.min_gain {
            volume_gain = 0.0;
        }

        let distance_cutoff_norm = if self.distance_muffle_factor <= 0.00001
            || xyz_distance <= self.reference_distance
            || self.max_muffle_distance <= self.reference_distance
            || self.max_distance_muffle_cutoff_hz >= MUFFLE_CUTOFF_HZ_MAX
        {
            1.0
        } else {
            let num = xyz_distance - self.reference_distance;
            let den = self.max_muffle_distance - self.reference_distance;

            let norm = 1.0 - (num / den).powf(self.distance_muffle_factor.recip());

            let min_norm = (self.max_distance_muffle_cutoff_hz - MUFFLE_CUTOFF_HZ_MIN)
                * MUFFLE_CUTOFF_HZ_RANGE_RECIP;

            norm.max(min_norm)
        };

        let damping_cutoff_hz = if (self.muffle_cutoff_hz < MUFFLE_CUTOFF_HZ_MAX - 0.01)
            || distance_cutoff_norm < 1.0
        {
            let hz = if distance_cutoff_norm < 1.0 {
                let muffle_cutoff_norm =
                    (self.muffle_cutoff_hz - MUFFLE_CUTOFF_HZ_MIN) * MUFFLE_CUTOFF_HZ_RANGE_RECIP;
                let final_norm = muffle_cutoff_norm * distance_cutoff_norm;

                (final_norm * (MUFFLE_CUTOFF_HZ_MAX - MUFFLE_CUTOFF_HZ_MIN)) + MUFFLE_CUTOFF_HZ_MIN
            } else {
                self.muffle_cutoff_hz
            };

            Some(hz.clamp(MUFFLE_CUTOFF_HZ_MIN, MUFFLE_CUTOFF_HZ_MAX))
        } else {
            None
        };

        let mut gain_l = pan_gain_l * distance_gain * volume_gain;
        let mut gain_r = pan_gain_r * distance_gain * volume_gain;

        if gain_l <= self.min_gain {
            gain_l = 0.0;
        }
        if gain_r <= self.min_gain {
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
    type Configuration = EmptyConfig;

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("spatial_basic")
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
        let computed_values = self.compute_values();

        Processor {
            gain_l: SmoothedParam::new(
                computed_values.gain_l,
                SmootherConfig {
                    smooth_seconds: self.smooth_seconds,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            gain_r: SmoothedParam::new(
                computed_values.gain_r,
                SmootherConfig {
                    smooth_seconds: self.smooth_seconds,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            damping_cutoff_hz: SmoothedParam::new(
                computed_values
                    .damping_cutoff_hz
                    .unwrap_or(MUFFLE_CUTOFF_HZ_MAX),
                SmootherConfig {
                    smooth_seconds: self.smooth_seconds,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            damping_disabled: computed_values.damping_cutoff_hz.is_none(),
            filter_l: OnePoleIirLPF::default(),
            filter_r: OnePoleIirLPF::default(),
            params: *self,
            prev_block_was_silent: true,
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
}

impl AudioNodeProcessor for Processor {
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        events: &mut ProcEvents,
        extra: &mut ProcExtra,
    ) -> ProcessStatus {
        let mut updated = false;
        for mut patch in events.drain_patches::<SpatialBasicNode>() {
            match &mut patch {
                SpatialBasicNodePatch::Offset(offset) => {
                    if !(offset.x.is_finite() && offset.y.is_finite() && offset.z.is_finite()) {
                        *offset = Vec3::default();
                    }
                }
                SpatialBasicNodePatch::DistanceGainFactor(f) => {
                    *f = f.max(0.0);
                }
                SpatialBasicNodePatch::ReferenceDistance(d) => {
                    *d = d.max(0.00001);
                }
                SpatialBasicNodePatch::MaxDistance(d) => {
                    *d = d.max(0.0);
                }
                SpatialBasicNodePatch::DistanceMuffleFactor(f) => {
                    *f = f.max(0.0);
                }
                SpatialBasicNodePatch::MaxDistanceMuffleCutoffHz(cutoff) => {
                    *cutoff = cutoff.clamp(MUFFLE_CUTOFF_HZ_MIN, MUFFLE_CUTOFF_HZ_MAX);
                }
                SpatialBasicNodePatch::MuffleCutoffHz(cutoff) => {
                    *cutoff = cutoff.clamp(MUFFLE_CUTOFF_HZ_MIN, MUFFLE_CUTOFF_HZ_MAX);
                }
                SpatialBasicNodePatch::PanningThreshold(threshold) => {
                    *threshold = threshold.clamp(0.0, 1.0);
                }
                SpatialBasicNodePatch::SmoothSeconds(seconds) => {
                    self.gain_l.set_smooth_seconds(*seconds, info.sample_rate);
                    self.gain_r.set_smooth_seconds(*seconds, info.sample_rate);
                    self.damping_cutoff_hz
                        .set_smooth_seconds(*seconds, info.sample_rate);
                }
                SpatialBasicNodePatch::MinGain(g) => {
                    *g = g.clamp(0.0, 1.0);
                }
                _ => {}
            }

            self.params.apply(patch);
            updated = true;
        }

        if updated {
            let computed_values = self.params.compute_values();

            self.gain_l.set_value(computed_values.gain_l);
            self.gain_r.set_value(computed_values.gain_r);

            if let Some(cutoff_hz) = computed_values.damping_cutoff_hz {
                self.damping_cutoff_hz.set_value(cutoff_hz);
                self.damping_disabled = false;
            } else {
                self.damping_cutoff_hz.set_value(MUFFLE_CUTOFF_HZ_MAX);
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

        self.prev_block_was_silent = false;

        if info.in_silence_mask.all_channels_silent(2) {
            self.gain_l.reset();
            self.gain_r.reset();
            self.damping_cutoff_hz.reset();
            self.filter_l.reset();
            self.filter_r.reset();

            self.prev_block_was_silent = true;

            return ProcessStatus::ClearAllOutputs;
        }

        let scratch_buffer = extra.scratch_buffers.first_mut();

        let (in1, in2) = if info.in_connected_mask == ConnectedMask::STEREO_CONNECTED {
            if self.params.downmix {
                // Downmix the stereo signal to mono.
                for (out_s, (&in1, &in2)) in scratch_buffer[..info.frames].iter_mut().zip(
                    buffers.inputs[0][..info.frames]
                        .iter()
                        .zip(buffers.inputs[1][..info.frames].iter()),
                ) {
                    *out_s = (in1 + in2) * 0.5;
                }

                (
                    &scratch_buffer[..info.frames],
                    &scratch_buffer[..info.frames],
                )
            } else {
                (
                    &buffers.inputs[0][..info.frames],
                    &buffers.inputs[1][..info.frames],
                )
            }
        } else {
            // Only one (or none) channels are connected, so just use the first
            // channel as input.
            (
                &buffers.inputs[0][..info.frames],
                &buffers.inputs[0][..info.frames],
            )
        };

        // Make doubly sure that the compiler optimizes away the bounds checking
        // in the loop.
        let in1 = &in1[..info.frames];
        let in2 = &in2[..info.frames];

        let (out1, out2) = buffers.outputs.split_first_mut().unwrap();
        let out1 = &mut out1[..info.frames];
        let out2 = &mut out2[0][..info.frames];

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
                for i in 0..info.frames {
                    out1[i] = in1[i] * self.gain_l.target_value();
                    out2[i] = in2[i] * self.gain_r.target_value();
                }
            } else {
                // The cutoff parameter is not currently smoothing, so we can optimize by
                // only updating the filter coefficients once.
                let coeff = OnePoleIirLPFCoeff::new(
                    self.damping_cutoff_hz.target_value(),
                    info.sample_rate_recip as f32,
                );

                for i in 0..info.frames {
                    out1[i] = in1[i] * self.gain_l.target_value();
                    out2[i] = in2[i] * self.gain_r.target_value();

                    out1[i] = self.filter_l.process(out1[i], coeff);
                    out2[i] = self.filter_r.process(out2[i], coeff);
                }
            }

            ProcessStatus::outputs_modified(info.in_silence_mask);
        } else {
            if self.damping_disabled && !self.damping_cutoff_hz.is_smoothing() {
                for i in 0..info.frames {
                    let gain_l = self.gain_l.next_smoothed();
                    let gain_r = self.gain_r.next_smoothed();

                    out1[i] = in1[i] * gain_l;
                    out2[i] = in2[i] * gain_r;
                }
            } else {
                let mut coeff = OnePoleIirLPFCoeff::default();

                for i in 0..info.frames {
                    let cutoff_hz = self.damping_cutoff_hz.next_smoothed();
                    let gain_l = self.gain_l.next_smoothed();
                    let gain_r = self.gain_r.next_smoothed();

                    out1[i] = in1[i] * gain_l;
                    out2[i] = in2[i] * gain_r;

                    // Because recalculating filter coefficients is expensive, a trick like
                    // this can be use to only recalculate them every CALC_FILTER_COEFF_INTERVAL
                    // frames.
                    if i & (CALC_FILTER_COEFF_INTERVAL - 1) == 0 {
                        coeff = OnePoleIirLPFCoeff::new(cutoff_hz, info.sample_rate_recip as f32);
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
