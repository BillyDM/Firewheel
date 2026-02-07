#[cfg(not(feature = "std"))]
use num_traits::Float;

use core::f32::consts::FRAC_PI_2;

use crate::{
    diff::{Diff, Patch},
    dsp::volume::{amp_to_db, Volume},
};

/// The algorithm used to map a normalized crossfade/panning value in the
/// range `[0.0, 1.0]` or `[-1.0, 1.0]` to the corresponding gain values
/// for two inputs.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Diff, Patch)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u32)]
pub enum FadeCurve {
    /// This curve makes the combined signal appear to play at a constant volume
    /// across the entire fade range for most signals.
    ///
    /// More specifically this a circular curve with each input at -3dB at
    /// center.
    #[default]
    EqualPower3dB = 0,
    /// Same as [`FadeCurve::EqualPower3dB`], but each input will be at -6dB
    /// at center which may be better for some signals.
    EqualPower6dB,
    /// This is cheaper to compute than [`FadeCurve::EqualPower3dB`], but is less
    /// accurate in its perception of constant volume.
    SquareRoot,
    /// The cheapest to compute, but is the least accurate in its perception of
    /// constant volume for some signals (though if the signals are highly
    /// correlated such as a wet/dry mix, then this mode may actually provide
    /// better results.)
    Linear,
}

impl FadeCurve {
    /// Compute the raw gain values for both inputs.
    ///
    /// * `fade` - The fade amount, where `0.5` is center, `0.0` is fully the
    /// first input, and `1.0` is fully the second input.
    pub fn compute_gains_0_to_1(&self, fade: f32) -> (f32, f32) {
        if fade <= 0.00001 {
            (1.0, 0.0)
        } else if fade >= 0.99999 {
            (0.0, 1.0)
        } else {
            match self {
                Self::EqualPower3dB => {
                    let fade = FRAC_PI_2 * fade;
                    let fade_cos = fade.cos();
                    let fade_sin = fade.sin();

                    (fade_cos, fade_sin)
                }
                Self::EqualPower6dB => {
                    let fade = FRAC_PI_2 * fade;
                    let fade_cos = fade.cos();
                    let fade_sin = fade.sin();

                    (fade_cos * fade_cos, fade_sin * fade_sin)
                }
                Self::SquareRoot => ((1.0 - fade).sqrt(), fade.sqrt()),
                Self::Linear => ((1.0 - fade), fade),
            }
        }
    }

    /// Compute the raw gain values for both inputs.
    ///
    /// * `fade` - The fade amount, where `0.0` is center, `-1.0` is fully the
    /// first input, and `1.0` is fully the second input.
    pub fn compute_gains_neg1_to_1(&self, fade: f32) -> (f32, f32) {
        if fade <= -0.99999 {
            (1.0, 0.0)
        } else if fade >= 0.99999 {
            (0.0, 1.0)
        } else {
            let fade = (fade + 1.0) * 0.5;

            match self {
                Self::EqualPower3dB => {
                    let fade = FRAC_PI_2 * fade;
                    let fade_cos = fade.cos();
                    let fade_sin = fade.sin();

                    (fade_cos, fade_sin)
                }
                Self::EqualPower6dB => {
                    let fade = FRAC_PI_2 * fade;
                    let fade_cos = fade.cos();
                    let fade_sin = fade.sin();

                    (fade_cos * fade_cos, fade_sin * fade_sin)
                }
                Self::SquareRoot => ((1.0 - fade).sqrt(), fade.sqrt()),
                Self::Linear => ((1.0 - fade), fade),
            }
        }
    }

    /// Compute the decibel values for both inputs for crossfading.
    ///
    /// * `fade` - The fade amount, where `0.5` is center, `0.0` is fully the
    /// first input, and `1.0` is fully the second input.
    pub fn crossfade_decibels(&self, fade: f32) -> (f32, f32) {
        let (a1, a2) = self.compute_gains_0_to_1(fade);

        (amp_to_db(a1), amp_to_db(a2))
    }

    /// Compute the [`Volume`]s for both inputs for crossfading.
    ///
    /// (Both volumes will be of type [`Volume::Decibels`]).
    ///
    /// * `fade` - The fade amount, where `0.5` is center, `0.0` is fully the
    /// first input, and `1.0` is fully the second input.
    pub fn crossfade_volumes(&self, fade: f32) -> (Volume, Volume) {
        let (d1, d2) = self.crossfade_decibels(fade);

        (Volume::Decibels(d1), Volume::Decibels(d2))
    }

    pub fn from_u32(val: u32) -> Self {
        match val {
            1 => Self::EqualPower6dB,
            2 => Self::SquareRoot,
            3 => Self::Linear,
            _ => Self::EqualPower3dB,
        }
    }
}
