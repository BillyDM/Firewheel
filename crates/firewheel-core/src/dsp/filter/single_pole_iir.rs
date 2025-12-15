#[cfg(not(feature = "std"))]
use num_traits::Float;

use core::f32::consts::TAU;

/// The coefficients to a very basic single-pole IIR lowpass filter for
/// generic tasks. This filter is very computationally efficient.
///
/// This filter has the form: `y[n] = ax[n] + by[n−1]`
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct OnePoleIirLPFCoeff {
    pub a0: f32,
    pub b1: f32,
}

impl OnePoleIirLPFCoeff {
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
pub struct OnePoleIirLPF {
    pub z1: f32,
}

impl OnePoleIirLPF {
    pub fn reset(&mut self) {
        self.z1 = 0.0;
    }

    #[inline(always)]
    pub fn process(&mut self, s: f32, coeff: OnePoleIirLPFCoeff) -> f32 {
        self.z1 = (coeff.a0 * s) + (coeff.b1 * self.z1);
        self.z1
    }
}

/// The coefficients to a very basic single-pole IIR highpass filter for
/// generic tasks. This filter is very computationally efficient.
///
/// This filter has the form: `y[n] = ax[n] + by[n−1]`
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct OnePoleIirHPFCoeff {
    pub a0: f32,
    pub b1: f32,
}

impl OnePoleIirHPFCoeff {
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
pub struct OnePoleIirHPF {
    pub xz1: f32,
    pub yz1: f32,
}

impl OnePoleIirHPF {
    pub fn reset(&mut self) {
        self.xz1 = 0.0;
        self.yz1 = 0.0;
    }

    #[inline(always)]
    pub fn process(&mut self, s: f32, coeff: OnePoleIirHPFCoeff) -> f32 {
        self.yz1 = (coeff.b1 * self.yz1) + (coeff.a0 * (s - self.xz1));
        self.xz1 = s;
        self.yz1
    }
}

/// The coefficients to a very basic single-pole IIR lowpass filter for
/// generic tasks, optimized for auto-vectorization. This filter is very
/// computationally efficient.
///
/// This filter has the form: `y[n] = ax[n] + by[n−1]`
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OnePoleIirLPFCoeffSimd<const LANES: usize> {
    pub a0: [f32; LANES],
    pub b1: [f32; LANES],
}

impl<const LANES: usize> OnePoleIirLPFCoeffSimd<LANES> {
    #[inline]
    pub fn new(coeff: &[OnePoleIirLPFCoeff; LANES]) -> Self {
        Self {
            a0: core::array::from_fn(|i| coeff[i].a0),
            b1: core::array::from_fn(|i| coeff[i].b1),
        }
    }

    #[inline]
    pub const fn splat(coeff: OnePoleIirLPFCoeff) -> Self {
        Self {
            a0: [coeff.a0; LANES],
            b1: [coeff.b1; LANES],
        }
    }

    #[inline]
    pub fn set_lane(&mut self, i: usize, coeff: OnePoleIirLPFCoeff) {
        assert!(i < LANES);

        self.a0[i] = coeff.a0;
        self.b1[i] = coeff.b1;
    }

    #[inline]
    pub fn get_lane(&mut self, i: usize) -> Option<OnePoleIirLPFCoeff> {
        if i >= LANES {
            return None;
        }

        Some(OnePoleIirLPFCoeff {
            a0: self.a0[i],
            b1: self.b1[i],
        })
    }
}

impl<const LANES: usize> Default for OnePoleIirLPFCoeffSimd<LANES> {
    fn default() -> Self {
        Self::splat(OnePoleIirLPFCoeff::default())
    }
}

/// The state of a very basic single-pole IIR lowpass filter for generic
/// tasks, optimized for auto-vectorization. This filter is very
/// computationally efficient.
///
/// This filter has the form: `y[n] = ax[n] + by[n−1]`
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OnePoleIirLPFSimd<const LANES: usize> {
    pub z1: [f32; LANES],
}

impl<const LANES: usize> OnePoleIirLPFSimd<LANES> {
    #[inline]
    pub fn new(state: &[OnePoleIirLPF; LANES]) -> Self {
        Self {
            z1: core::array::from_fn(|i| state[i].z1),
        }
    }

    #[inline]
    pub const fn splat(state: OnePoleIirLPF) -> Self {
        Self {
            z1: [state.z1; LANES],
        }
    }

    #[inline(always)]
    pub fn reset(&mut self) {
        self.z1 = [0.0; LANES];
    }

    #[inline]
    pub fn set_lane(&mut self, i: usize, state: OnePoleIirLPF) {
        assert!(i < LANES);
        self.z1[i] = state.z1;
    }

    #[inline]
    pub fn get_lane(&mut self, i: usize) -> Option<OnePoleIirLPF> {
        if i >= LANES {
            return None;
        }

        Some(OnePoleIirLPF { z1: self.z1[i] })
    }

    #[inline(always)]
    pub fn process(
        &mut self,
        input: [f32; LANES],
        coeff: &OnePoleIirLPFCoeffSimd<LANES>,
    ) -> [f32; LANES] {
        core::array::from_fn(|i| {
            self.z1[i] = (coeff.a0[i] * input[i]) + (coeff.b1[i] * self.z1[i]);
            self.z1[i]
        })
    }
}

impl<const LANES: usize> Default for OnePoleIirLPFSimd<LANES> {
    fn default() -> Self {
        Self::splat(OnePoleIirLPF::default())
    }
}

/// The coefficients to a very basic single-pole IIR highpass filter for
/// generic tasks, optimized for auto-vectorization. This filter is very
/// computationally efficient.
///
/// This filter has the form: `y[n] = ax[n] + by[n−1]`
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OnePoleIirHPFCoeffSimd<const LANES: usize> {
    pub a0: [f32; LANES],
    pub b1: [f32; LANES],
}

impl<const LANES: usize> OnePoleIirHPFCoeffSimd<LANES> {
    #[inline]
    pub fn new(coeff: &[OnePoleIirHPFCoeff; LANES]) -> Self {
        Self {
            a0: core::array::from_fn(|i| coeff[i].a0),
            b1: core::array::from_fn(|i| coeff[i].b1),
        }
    }

    #[inline]
    pub const fn splat(coeff: OnePoleIirHPFCoeff) -> Self {
        Self {
            a0: [coeff.a0; LANES],
            b1: [coeff.b1; LANES],
        }
    }

    #[inline]
    pub fn set_lane(&mut self, i: usize, coeff: OnePoleIirHPFCoeff) {
        assert!(i < LANES);

        self.a0[i] = coeff.a0;
        self.b1[i] = coeff.b1;
    }

    #[inline]
    pub fn get_lane(&mut self, i: usize) -> Option<OnePoleIirHPFCoeff> {
        if i >= LANES {
            return None;
        }

        Some(OnePoleIirHPFCoeff {
            a0: self.a0[i],
            b1: self.b1[i],
        })
    }
}

impl<const LANES: usize> Default for OnePoleIirHPFCoeffSimd<LANES> {
    fn default() -> Self {
        Self::splat(OnePoleIirHPFCoeff::default())
    }
}

/// The state of a very basic single-pole IIR highpass filter for generic
/// tasks, optimized for auto-vectorization. This filter is very
/// computationally efficient.
///
/// This filter has the form: `y[n] = ax[n] + by[n−1]`
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OnePoleIirHPFSimd<const LANES: usize> {
    pub xz1: [f32; LANES],
    pub yz1: [f32; LANES],
}

impl<const LANES: usize> OnePoleIirHPFSimd<LANES> {
    #[inline]
    pub fn new(state: &[OnePoleIirHPF; LANES]) -> Self {
        Self {
            xz1: core::array::from_fn(|i| state[i].xz1),
            yz1: core::array::from_fn(|i| state[i].yz1),
        }
    }

    #[inline]
    pub const fn splat(state: OnePoleIirHPF) -> Self {
        Self {
            xz1: [state.xz1; LANES],
            yz1: [state.yz1; LANES],
        }
    }

    #[inline(always)]
    pub fn reset(&mut self) {
        self.xz1 = [0.0; LANES];
        self.yz1 = [0.0; LANES];
    }

    #[inline]
    pub fn set_lane(&mut self, i: usize, state: OnePoleIirHPF) {
        assert!(i < LANES);
        self.xz1[i] = state.xz1;
        self.yz1[i] = state.yz1;
    }

    #[inline]
    pub fn get_lane(&mut self, i: usize) -> Option<OnePoleIirHPF> {
        if i >= LANES {
            return None;
        }

        Some(OnePoleIirHPF {
            xz1: self.xz1[i],
            yz1: self.yz1[i],
        })
    }

    #[inline(always)]
    pub fn process(
        &mut self,
        input: [f32; LANES],
        coeff: &OnePoleIirHPFCoeffSimd<LANES>,
    ) -> [f32; LANES] {
        core::array::from_fn(|i| {
            self.yz1[i] = (coeff.b1[i] * self.yz1[i]) + (coeff.a0[i] * (input[i] - self.xz1[i]));
            self.xz1[i] = input[i];
            self.yz1[i]
        })
    }
}

impl<const LANES: usize> Default for OnePoleIirHPFSimd<LANES> {
    fn default() -> Self {
        Self::splat(OnePoleIirHPF::default())
    }
}
