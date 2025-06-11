use core::f32::consts::TAU;

use crate::dsp::filter::filter_trait::Filter;

/// The coefficients to a very basic single-pole IIR lowpass filter for
/// generic tasks. This filter is very computationally efficient.
///
/// This filter has the form: `y[n] = ax[n] + by[n−1]`
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct SinglePoleIirLPFCoeff {
    pub a0: f32,
    pub b1: f32,
}

impl SinglePoleIirLPFCoeff {
    #[inline]
    pub fn new(cutoff_hz: f32, sample_rate_recip: f32) -> Self {
        let b1 = (-TAU * cutoff_hz * sample_rate_recip).exp();
        let a0 = 1.0 - b1;

        Self { a0, b1 }
    }
}

/// The state of a very basic single-pole IIR lowpass filter for generic
/// tasks. This filter is very computationally efficient.
///
/// This filter has the form: `y[n] = ax[n] + by[n−1]`
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct SinglePoleIirLPF {
    pub z1: f32,
}

impl Filter for SinglePoleIirLPF {
    type Coeffs = SinglePoleIirLPFCoeff;

    #[inline(always)]
    fn reset(&mut self) {
        self.z1 = 0.0;
    }

    #[inline(always)]
    fn process(&mut self, x: f32, coeffs: &Self::Coeffs) -> f32 {
        self.z1 = (coeffs.a0 * x) + (coeffs.b1 * self.z1);
        self.z1
    }

    #[inline(always)]
    fn is_silent(&self) -> bool {
        self.z1 < Self::SILENT_THRESHOLD
    }
}

/// The coefficients to a very basic single-pole IIR highpass filter for
/// generic tasks. This filter is very computationally efficient.
///
/// This filter has the form: `y[n] = ax[n] + by[n−1]`
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct SinglePoleIirHPFCoeff {
    pub a0: f32,
    pub b1: f32,
}

impl SinglePoleIirHPFCoeff {
    #[inline]
    pub fn new(cutoff_hz: f32, sample_rate_recip: f32) -> Self {
        let b1 = (-TAU * cutoff_hz * sample_rate_recip).exp();
        let a0 = (1.0 + b1) * 0.5;

        Self { b1, a0 }
    }
}

/// The state of a very basic single-pole IIR highpass filter for generic
/// tasks. This filter is very computationally efficient.
///
/// This filter has the form: `y[n] = ax[n] + by[n−1]`
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct SinglePoleIirHPF {
    pub xz1: f32,
    pub yz1: f32,
}

impl SinglePoleIirHPF {
    pub fn reset(&mut self) {
        self.xz1 = 0.0;
        self.yz1 = 0.0;
    }

    #[inline(always)]
    pub fn process(&mut self, s: f32, coeff: SinglePoleIirHPFCoeff) -> f32 {
        self.yz1 = (coeff.a0 * s) + (coeff.b1 * self.yz1) - self.xz1;
        self.xz1 = s;
        self.yz1
    }
}
