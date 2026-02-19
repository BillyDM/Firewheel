#[cfg(not(feature = "std"))]
use num_traits::Float;

use core::num::NonZeroU32;

use firewheel_macros::{Diff, Patch};

use crate::{
    dsp::{
        coeff_update::{CoeffUpdateFactor, CoeffUpdateMask},
        filter::single_pole_iir::{OnePoleIirLPFCoeff, OnePoleIirLPFCoeffSimd, OnePoleIirLPFSimd},
    },
    param::smoother::{SmoothedParam, SmootherConfig},
};

pub const MUFFLE_CUTOFF_HZ_MIN: f32 = 20.0;
pub const MUFFLE_CUTOFF_HZ_MAX: f32 = 20_480.0;
const MUFFLE_CUTOFF_HZ_RANGE_RECIP: f32 = 1.0 / (MUFFLE_CUTOFF_HZ_MAX - MUFFLE_CUTOFF_HZ_MIN);

/// The method in which to calculate the volume of a sound based on the distance from
/// the listener.
///
/// Based on <https://developer.mozilla.org/en-US/docs/Web/API/PannerNode/distanceModel>
///
/// Interactive graph of the different models: <https://www.desmos.com/calculator/g1pbsc5m9y>
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Diff, Patch)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
        distance_gain_factor: f32,
        reference_distance: f32,
        maximum_distance: f32,
    ) -> f32 {
        if distance <= reference_distance || distance_gain_factor <= 0.00001 {
            return 1.0;
        }

        match self {
            DistanceModel::Inverse => {
                reference_distance
                    / (reference_distance
                        + (distance_gain_factor * (distance - reference_distance)))
            }
            DistanceModel::Linear => {
                if maximum_distance <= reference_distance {
                    1.0
                } else {
                    (1.0 - (distance_gain_factor * (distance - reference_distance)
                        / (maximum_distance - reference_distance)))
                        .clamp(0.0, 1.0)
                }
            }
            DistanceModel::Exponential => {
                (distance / reference_distance).powf(-distance_gain_factor)
            }
        }
    }
}

/// The parameters which describe how to attenuate a sound based on its distance from
/// the listener.
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DistanceAttenuation {
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
}

impl Default for DistanceAttenuation {
    fn default() -> Self {
        Self {
            distance_model: DistanceModel::Inverse,
            distance_gain_factor: 1.0,
            reference_distance: 5.0,
            max_distance: 200.0,
            distance_muffle_factor: 1.9,
            max_muffle_distance: 200.0,
            max_distance_muffle_cutoff_hz: 20.0,
        }
    }
}

pub struct DistanceAttenuatorStereoDsp {
    pub gain: SmoothedParam,
    pub muffle_cutoff_hz: SmoothedParam,
    pub damping_disabled: bool,

    pub filter: OnePoleIirLPFSimd<2>,
    coeff_update_mask: CoeffUpdateMask,
}

impl DistanceAttenuatorStereoDsp {
    pub fn new(
        smoother_config: SmootherConfig,
        sample_rate: NonZeroU32,
        coeff_update_factor: CoeffUpdateFactor,
    ) -> Self {
        Self {
            gain: SmoothedParam::new(1.0, smoother_config, sample_rate),
            muffle_cutoff_hz: SmoothedParam::new(
                MUFFLE_CUTOFF_HZ_MAX,
                smoother_config,
                sample_rate,
            ),
            damping_disabled: true,
            filter: OnePoleIirLPFSimd::default(),
            coeff_update_mask: coeff_update_factor.mask(),
        }
    }

    pub fn set_coeff_update_factor(&mut self, coeff_update_factor: CoeffUpdateFactor) {
        self.coeff_update_mask = coeff_update_factor.mask();
    }

    pub fn is_silent(&self) -> bool {
        self.gain.target_value() == 0.0 && !self.gain.is_smoothing()
    }

    pub fn compute_values(
        &mut self,
        distance: f32,
        params: &DistanceAttenuation,
        muffle_cutoff_hz: f32,
        min_gain: f32,
    ) {
        let reference_distance = params.reference_distance.max(0.00001);
        let max_distance = params.max_distance.max(0.0);
        let max_distance_muffle_cutoff_hz = params
            .max_distance_muffle_cutoff_hz
            .max(MUFFLE_CUTOFF_HZ_MIN);

        let distance_gain = params.distance_model.calculate_gain(
            distance,
            params.distance_gain_factor,
            reference_distance,
            max_distance,
        );

        let gain = if distance_gain <= min_gain {
            0.0
        } else {
            distance_gain
        };

        let distance_cutoff_norm = if params.distance_muffle_factor <= 0.00001
            || distance <= reference_distance
            || params.max_muffle_distance <= reference_distance
            || max_distance_muffle_cutoff_hz >= MUFFLE_CUTOFF_HZ_MAX
        {
            1.0
        } else {
            let num = distance - reference_distance;
            let den = params.max_muffle_distance - reference_distance;

            let norm = 1.0 - (num / den).powf(params.distance_muffle_factor.recip());

            let min_norm = (max_distance_muffle_cutoff_hz - MUFFLE_CUTOFF_HZ_MIN)
                * MUFFLE_CUTOFF_HZ_RANGE_RECIP;

            norm.max(min_norm)
        };

        let muffle_cutoff_hz = if (muffle_cutoff_hz < MUFFLE_CUTOFF_HZ_MAX - 0.01)
            || distance_cutoff_norm < 1.0
        {
            let hz = if distance_cutoff_norm < 1.0 {
                let muffle_cutoff_norm =
                    (muffle_cutoff_hz - MUFFLE_CUTOFF_HZ_MIN) * MUFFLE_CUTOFF_HZ_RANGE_RECIP;
                let final_norm = muffle_cutoff_norm * distance_cutoff_norm;

                (final_norm * (MUFFLE_CUTOFF_HZ_MAX - MUFFLE_CUTOFF_HZ_MIN)) + MUFFLE_CUTOFF_HZ_MIN
            } else {
                muffle_cutoff_hz
            };

            Some(hz.clamp(MUFFLE_CUTOFF_HZ_MIN, MUFFLE_CUTOFF_HZ_MAX))
        } else {
            None
        };

        self.gain.set_value(gain);

        if let Some(cutoff_hz) = muffle_cutoff_hz {
            self.muffle_cutoff_hz.set_value(cutoff_hz);
            self.damping_disabled = false;
        } else {
            self.muffle_cutoff_hz.set_value(MUFFLE_CUTOFF_HZ_MAX);
            self.damping_disabled = true;
        }
    }

    /// Returns `true` if the output buffers should be cleared with silence, `false`
    /// otherwise.
    pub fn process(
        &mut self,
        frames: usize,
        out1: &mut [f32],
        out2: &mut [f32],
        sample_rate_recip: f64,
    ) -> bool {
        // Make doubly sure that the compiler optimizes away the bounds checking
        // in the loop.
        let out1 = &mut out1[..frames];
        let out2 = &mut out2[..frames];

        if !self.gain.is_smoothing() && !self.muffle_cutoff_hz.is_smoothing() {
            if !self.gain.is_smoothing() && self.gain.target_value() == 0.0 {
                self.gain.reset_to_target();
                self.muffle_cutoff_hz.reset_to_target();
                self.filter.reset();

                return true;
            } else if self.damping_disabled {
                for i in 0..frames {
                    out1[i] = out1[i] * self.gain.target_value();
                    out2[i] = out2[i] * self.gain.target_value();
                }
            } else {
                // The cutoff parameter is not currently smoothing, so we can optimize by
                // only updating the filter coefficients once.
                let coeff = OnePoleIirLPFCoeffSimd::splat(OnePoleIirLPFCoeff::new(
                    self.muffle_cutoff_hz.target_value(),
                    sample_rate_recip as f32,
                ));

                for i in 0..frames {
                    let s = [
                        out1[i] * self.gain.target_value(),
                        out2[i] * self.gain.target_value(),
                    ];

                    let [l, r] = self.filter.process(s, &coeff);

                    out1[i] = l;
                    out2[i] = r;
                }
            }
        } else {
            if self.damping_disabled && !self.muffle_cutoff_hz.is_smoothing() {
                for i in 0..frames {
                    let gain = self.gain.next_smoothed();

                    out1[i] = out1[i] * gain;
                    out2[i] = out2[i] * gain;
                }
            } else {
                let mut coeff = OnePoleIirLPFCoeffSimd::default();

                for i in 0..frames {
                    let cutoff_hz = self.muffle_cutoff_hz.next_smoothed();
                    let gain = self.gain.next_smoothed();

                    // Because recalculating filter coefficients is expensive, a trick like
                    // this can be used to only recalculate them every few frames.
                    //
                    // TODO: use core::hint::cold_path() once that stabilizes
                    // TODO: instead, we can extract a function and use #[cold] + #[inline(always)] today
                    //
                    // TODO: Alternatively, this could be optimized using a lookup table
                    if self.coeff_update_mask.do_update(i) {
                        coeff = OnePoleIirLPFCoeffSimd::splat(OnePoleIirLPFCoeff::new(
                            cutoff_hz,
                            sample_rate_recip as f32,
                        ));
                    }

                    let s = [out1[i] * gain, out2[i] * gain];

                    let [l, r] = self.filter.process(s, &coeff);

                    out1[i] = l;
                    out2[i] = r;
                }
            }

            self.gain.settle();
            self.muffle_cutoff_hz.settle();
        }

        false
    }

    pub fn reset(&mut self) {
        self.gain.reset_to_target();
        self.muffle_cutoff_hz.reset_to_target();
        self.filter.reset();
    }

    pub fn set_smooth_seconds(&mut self, seconds: f32, sample_rate: NonZeroU32) {
        self.gain.set_smooth_seconds(seconds, sample_rate);
        self.muffle_cutoff_hz
            .set_smooth_seconds(seconds, sample_rate);
    }

    pub fn update_sample_rate(&mut self, sample_rate: NonZeroU32) {
        self.gain.update_sample_rate(sample_rate);
        self.muffle_cutoff_hz.update_sample_rate(sample_rate);
    }
}
