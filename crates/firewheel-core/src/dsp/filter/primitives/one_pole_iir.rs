//! Based on https://github.com/MeadowlarkDAW/meadow-dsp/tree/main/meadow-dsp-mit with permission
use std::f32::consts::PI;

use crate::dsp::filter::filter_trait::Filter;

/// The coefficients for a single-pole IIR filter.
#[derive(Default, Clone, Copy, PartialEq)]
pub struct OnePoleIirCoeff {
    pub a0: f32,
    pub b1: f32,

    pub m0: f32,
    pub m1: f32,
}

impl OnePoleIirCoeff {
    pub const NO_OP: Self = Self {
        a0: 0.0,
        b1: 0.0,
        m0: 1.0,
        m1: 0.0,
    };

    pub fn lowpass(cutoff_hz: f32, sample_rate_recip: f32) -> Self {
        let b1 = ((-2.0 * PI) * cutoff_hz * sample_rate_recip).exp();
        let a0 = 1.0 - b1;

        Self {
            a0,
            b1,
            m0: 0.0,
            m1: 1.0,
        }
    }

    pub fn highpass(cutoff_hz: f32, sample_rate_recip: f32) -> Self {
        let b1 = ((-2.0 * PI) * cutoff_hz * sample_rate_recip).exp();
        let a0 = 1.0 - b1;

        Self {
            a0,
            b1,
            m0: 1.0,
            m1: -1.0,
        }
    }
}

/// The state of a single-pole IIR filter.
#[derive(Default, Clone, Copy, PartialEq)]
pub struct OnePoleIirState {
    pub z1: f32,
}

impl Filter for OnePoleIirState {
    type Coeffs = OnePoleIirCoeff;

    #[inline(always)]
    fn process(&mut self, input: f32, coeff: &Self::Coeffs) -> f32 {
        self.z1 = (coeff.a0 * input) + (coeff.b1 * self.z1);
        coeff.m0 * input + coeff.m1 * self.z1
    }

    #[inline(always)]
    fn reset(&mut self) {
        self.z1 = 0.0;
    }

    #[inline(always)]
    fn is_silent(&self) -> bool {
        self.z1.abs() <= Self::SILENT_THRESHOLD
    }
}
