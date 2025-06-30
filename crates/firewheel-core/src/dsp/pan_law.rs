use core::f32::consts::FRAC_PI_2;

use crate::diff::{Diff, Patch};

/// The algorithm to use to map a normalized panning value in the range `[-1.0, 1.0]`
/// to the corresponding gain values for the left and right channels.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Diff, Patch)]
#[repr(u32)]
pub enum PanLaw {
    /// This pan law makes the signal appear to play at a constant volume across
    /// the entire panning range.
    ///
    /// More specifically this a circular pan law with each channel at -3dB when
    /// panned center.
    #[default]
    EqualPower3dB = 0,
    /// Same as [`PanLaw::EqualPower3dB`], but each channel will be at -6dB when
    /// panned center which may be better for some signals.
    EqualPower6dB,
    /// This is cheaper to compute than `EqualPower3dB`, but is less accurate in
    /// its perception of constant volume.
    SquareRoot,
    /// The cheapest to compute, but is the least accurate in its perception of
    /// constant volume.
    Linear,
}

impl PanLaw {
    /// Compute the raw gain values for the `(left, right)` channels.
    ///
    /// * `pan` - The pan amount, where `0.0` is center, `-1.0` is fully left, and
    /// `1.0` is fully right.
    pub fn compute_gains(&self, pan: f32) -> (f32, f32) {
        if pan <= -1.0 {
            (1.0, 0.0)
        } else if pan >= 1.0 {
            (0.0, 1.0)
        } else {
            let pan = (pan + 1.0) * 0.5;

            match self {
                Self::EqualPower3dB => {
                    let pan = FRAC_PI_2 * pan;
                    let pan_cos = pan.cos();
                    let pan_sin = pan.sin();

                    (pan_cos, pan_sin)
                }
                Self::EqualPower6dB => {
                    let pan = FRAC_PI_2 * pan;
                    let pan_cos = pan.cos();
                    let pan_sin = pan.sin();

                    (pan_cos * pan_cos, pan_sin * pan_sin)
                }
                Self::SquareRoot => ((1.0 - pan).sqrt(), pan.sqrt()),
                Self::Linear => ((1.0 - pan), pan),
            }
        }
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
