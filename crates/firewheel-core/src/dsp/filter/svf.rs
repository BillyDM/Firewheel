#[cfg(not(feature = "std"))]
use num_traits::Float;

use core::f32::consts::PI;

use super::butterworth::{
    ORD4_Q_SCALE, ORD6_Q_SCALE, ORD8_Q_SCALE, Q_BUTTERWORTH_ORD2, Q_BUTTERWORTH_ORD4,
    Q_BUTTERWORTH_ORD6, Q_BUTTERWORTH_ORD8,
};

/// The coefficients for an SVF (state variable filter) model.
///
/// This is based on the filter model developed by Andrew Simper:
/// <https://cytomic.com/files/dsp/SvfLinearTrapOptimised2.pdf>
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct SvfCoeff {
    pub a1: f32,
    pub a2: f32,
    pub a3: f32,

    pub m0: f32,
    pub m1: f32,
    pub m2: f32,
}

impl SvfCoeff {
    pub const NO_OP: Self = Self {
        a1: 0.0,
        a2: 0.0,
        a3: 0.0,
        m0: 1.0,
        m1: 0.0,
        m2: 0.0,
    };

    pub fn lowpass_ord2(cutoff_hz: f32, q: f32, sample_rate_recip: f32) -> Self {
        let g = g(cutoff_hz, sample_rate_recip);
        let k = 1.0 / q;

        Self::from_g_and_k(g, k, 0.0, 0.0, 1.0)
    }

    pub fn lowpass_ord4(cutoff_hz: f32, q: f32, sample_rate_recip: f32) -> [Self; 2] {
        let g = g(cutoff_hz, sample_rate_recip);
        let q_norm = scale_q_norm_for_order(q_norm(q), ORD4_Q_SCALE);

        core::array::from_fn(|i| {
            let q = q_norm * Q_BUTTERWORTH_ORD4[i];
            let k = 1.0 / q;

            Self::from_g_and_k(g, k, 0.0, 0.0, 1.0)
        })
    }

    pub fn lowpass_ord6(cutoff_hz: f32, q: f32, sample_rate_recip: f32) -> [Self; 3] {
        let g = g(cutoff_hz, sample_rate_recip);
        let q_norm = scale_q_norm_for_order(q_norm(q), ORD4_Q_SCALE);

        core::array::from_fn(|i| {
            let q = q_norm * Q_BUTTERWORTH_ORD6[i];
            let k = 1.0 / q;

            Self::from_g_and_k(g, k, 0.0, 0.0, 1.0)
        })
    }

    pub fn lowpass_ord8(cutoff_hz: f32, q: f32, sample_rate_recip: f32) -> [Self; 4] {
        let g = g(cutoff_hz, sample_rate_recip);
        let q_norm = scale_q_norm_for_order(q_norm(q), ORD8_Q_SCALE);

        core::array::from_fn(|i| {
            let q = q_norm * Q_BUTTERWORTH_ORD8[i];
            let k = 1.0 / q;

            Self::from_g_and_k(g, k, 0.0, 0.0, 1.0)
        })
    }

    pub fn highpass_ord2(cutoff_hz: f32, q: f32, sample_rate_recip: f32) -> Self {
        let g = g(cutoff_hz, sample_rate_recip);
        let k = 1.0 / q;

        Self::from_g_and_k(g, k, 1.0, -k, -1.0)
    }

    pub fn highpass_ord4(cutoff_hz: f32, q: f32, sample_rate_recip: f32) -> [Self; 2] {
        let g = g(cutoff_hz, sample_rate_recip);
        let q_norm = scale_q_norm_for_order(q_norm(q), ORD4_Q_SCALE);

        core::array::from_fn(|i| {
            let q = q_norm * Q_BUTTERWORTH_ORD4[i];
            let k = 1.0 / q;

            Self::from_g_and_k(g, k, 1.0, -k, -1.0)
        })
    }

    pub fn highpass_ord6(cutoff_hz: f32, q: f32, sample_rate_recip: f32) -> [Self; 3] {
        let g = g(cutoff_hz, sample_rate_recip);
        let q_norm = scale_q_norm_for_order(q_norm(q), ORD6_Q_SCALE);

        core::array::from_fn(|i| {
            let q = q_norm * Q_BUTTERWORTH_ORD6[i];
            let k = 1.0 / q;

            Self::from_g_and_k(g, k, 1.0, -k, -1.0)
        })
    }

    pub fn highpass_ord8(cutoff_hz: f32, q: f32, sample_rate_recip: f32) -> [Self; 4] {
        let g = g(cutoff_hz, sample_rate_recip);
        let q_norm = scale_q_norm_for_order(q_norm(q), ORD8_Q_SCALE);

        core::array::from_fn(|i| {
            let q = q_norm * Q_BUTTERWORTH_ORD8[i];
            let k = 1.0 / q;

            Self::from_g_and_k(g, k, 1.0, -k, -1.0)
        })
    }

    pub fn notch(cutoff_hz: f32, q: f32, sample_rate_recip: f32) -> Self {
        let g = g(cutoff_hz, sample_rate_recip);
        let k = 1.0 / q;

        Self::from_g_and_k(g, k, 1.0, -k, 0.0)
    }

    pub fn bell(cutoff_hz: f32, q: f32, raw_gain: f32, sample_rate_recip: f32) -> Self {
        //let a = gain_db_to_a(gain_db);
        let a = raw_gain.sqrt();

        let g = g(cutoff_hz, sample_rate_recip);
        let k = 1.0 / (q * a);

        Self::from_g_and_k(g, k, 1.0, k * (raw_gain - 1.0), 0.0)
    }

    pub fn low_shelf(cutoff_hz: f32, q: f32, raw_gain: f32, sample_rate_recip: f32) -> Self {
        //let a = gain_db_to_a(gain_db);
        let a = raw_gain.sqrt();

        let g = (PI * cutoff_hz * sample_rate_recip).tan() / a.sqrt();
        let k = 1.0 / q;

        Self::from_g_and_k(g, k, 1.0, k * (a - 1.0), a * a - 1.0)
    }

    pub fn high_shelf(cutoff_hz: f32, q: f32, raw_gain: f32, sample_rate_recip: f32) -> Self {
        //let a = gain_db_to_a(gain_db);
        let a = raw_gain.sqrt();

        let g = (PI * cutoff_hz * sample_rate_recip).tan() / a.sqrt();
        let k = 1.0 / q;

        Self::from_g_and_k(g, k, raw_gain, k * (1.0 - a) * a, 1.0 - raw_gain)
    }

    pub fn allpass(cutoff_hz: f32, q: f32, sample_rate_recip: f32) -> Self {
        let g = g(cutoff_hz, sample_rate_recip);
        let k = 1.0 / q;

        Self::from_g_and_k(g, k, 1.0, -2.0 * k, 0.0)
    }

    pub fn from_g_and_k(g: f32, k: f32, m0: f32, m1: f32, m2: f32) -> Self {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;

        Self {
            a1,
            a2,
            a3,
            m0,
            m1,
            m2,
        }
    }
}

/// The state of an SVF (state variable filter) model.
///
/// This is based on the filter model developed by Andrew Simper:
/// <https://cytomic.com/files/dsp/SvfLinearTrapOptimised2.pdf>
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct SvfState {
    pub ic1eq: f32,
    pub ic2eq: f32,
}

impl SvfState {
    #[inline(always)]
    pub fn process(&mut self, input: f32, coeff: &SvfCoeff) -> f32 {
        let v3 = input - self.ic2eq;
        let v1 = coeff.a1 * self.ic1eq + coeff.a2 * v3;
        let v2 = self.ic2eq + coeff.a2 * self.ic1eq + coeff.a3 * v3;
        self.ic1eq = 2.0 * v1 - self.ic1eq;
        self.ic2eq = 2.0 * v2 - self.ic2eq;

        coeff.m0 * input + coeff.m1 * v1 + coeff.m2 * v2
    }

    #[inline(always)]
    pub fn reset(&mut self) {
        self.ic1eq = 0.0;
        self.ic2eq = 0.0;
    }
}

/// The coefficients for an SVF (state variable filter) model, optimized for
/// auto-vectorization.
///
/// This is based on the filter model developed by Andrew Simper:
/// <https://cytomic.com/files/dsp/SvfLinearTrapOptimised2.pdf>
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SvfCoeffSimd<const LANES: usize> {
    pub a1: [f32; LANES],
    pub a2: [f32; LANES],
    pub a3: [f32; LANES],

    pub m0: [f32; LANES],
    pub m1: [f32; LANES],
    pub m2: [f32; LANES],
}

impl<const LANES: usize> SvfCoeffSimd<LANES> {
    #[inline]
    pub fn new(coeff: &[SvfCoeff; LANES]) -> Self {
        Self {
            a1: core::array::from_fn(|i| coeff[i].a1),
            a2: core::array::from_fn(|i| coeff[i].a2),
            a3: core::array::from_fn(|i| coeff[i].a3),
            m0: core::array::from_fn(|i| coeff[i].m0),
            m1: core::array::from_fn(|i| coeff[i].m1),
            m2: core::array::from_fn(|i| coeff[i].m2),
        }
    }

    #[inline]
    pub const fn splat(coeff: SvfCoeff) -> Self {
        Self {
            a1: [coeff.a1; LANES],
            a2: [coeff.a2; LANES],
            a3: [coeff.a3; LANES],
            m0: [coeff.m0; LANES],
            m1: [coeff.m1; LANES],
            m2: [coeff.m2; LANES],
        }
    }

    #[inline]
    pub fn set_lane(&mut self, i: usize, coeff: SvfCoeff) {
        assert!(i < LANES);

        self.a1[i] = coeff.a1;
        self.a2[i] = coeff.a2;
        self.a3[i] = coeff.a3;
        self.m0[i] = coeff.m0;
        self.m1[i] = coeff.m1;
        self.m2[i] = coeff.m2;
    }

    #[inline]
    pub fn get_lane(&mut self, i: usize) -> Option<SvfCoeff> {
        if i >= LANES {
            return None;
        }

        Some(SvfCoeff {
            a1: self.a1[i],
            a2: self.a2[i],
            a3: self.a3[i],
            m0: self.m0[i],
            m1: self.m1[i],
            m2: self.m2[i],
        })
    }
}

impl<const LANES: usize> Default for SvfCoeffSimd<LANES> {
    fn default() -> Self {
        Self::splat(SvfCoeff::default())
    }
}

/// The state of an SVF (state variable filter) model, optimized
/// for auto-vectorization.
///
/// This is based on the filter model developed by Andrew Simper:
/// <https://cytomic.com/files/dsp/SvfLinearTrapOptimised2.pdf>
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SvfStateSimd<const LANES: usize> {
    pub ic1eq: [f32; LANES],
    pub ic2eq: [f32; LANES],
}

impl<const LANES: usize> SvfStateSimd<LANES> {
    #[inline]
    pub fn new(state: &[SvfState; LANES]) -> Self {
        Self {
            ic1eq: core::array::from_fn(|i| state[i].ic1eq),
            ic2eq: core::array::from_fn(|i| state[i].ic2eq),
        }
    }

    #[inline]
    pub const fn splat(state: SvfState) -> Self {
        Self {
            ic1eq: [state.ic1eq; LANES],
            ic2eq: [state.ic2eq; LANES],
        }
    }

    #[inline]
    pub fn set_lane(&mut self, i: usize, state: SvfState) {
        assert!(i < LANES);

        self.ic1eq[i] = state.ic1eq;
        self.ic2eq[i] = state.ic2eq;
    }

    #[inline]
    pub fn get_lane(&mut self, i: usize) -> Option<SvfState> {
        if i >= LANES {
            return None;
        }

        Some(SvfState {
            ic1eq: self.ic1eq[i],
            ic2eq: self.ic2eq[i],
        })
    }

    #[inline(always)]
    pub fn reset(&mut self) {
        self.ic1eq = [0.0; LANES];
        self.ic2eq = [0.0; LANES];
    }

    #[inline(always)]
    pub fn reset_lane(&mut self, i: usize) {
        assert!(i < LANES);

        self.ic1eq[i] = 0.0;
        self.ic2eq[i] = 0.0;
    }

    #[inline(always)]
    pub fn process(&mut self, input: [f32; LANES], coeff: &SvfCoeffSimd<LANES>) -> [f32; LANES] {
        core::array::from_fn(|i| {
            let v3 = input[i] - self.ic2eq[i];
            let v1 = coeff.a1[i] * self.ic1eq[i] + coeff.a2[i] * v3;
            let v2 = self.ic2eq[i] + coeff.a2[i] * self.ic1eq[i] + coeff.a3[i] * v3;
            self.ic1eq[i] = 2.0 * v1 - self.ic1eq[i];
            self.ic2eq[i] = 2.0 * v2 - self.ic2eq[i];

            coeff.m0[i] * input[i] + coeff.m1[i] * v1 + coeff.m2[i] * v2
        })
    }
}

impl<const LANES: usize> Default for SvfStateSimd<LANES> {
    fn default() -> Self {
        Self::splat(SvfState::default())
    }
}

#[inline]
fn g(cutoff_hz: f32, sample_rate_recip: f32) -> f32 {
    (PI * cutoff_hz * sample_rate_recip).tan()
}

#[inline]
fn q_norm(q: f32) -> f32 {
    q * (1.0 / Q_BUTTERWORTH_ORD2)
}

/*
#[inline]
fn gain_db_to_a(gain_db: f32) -> f32 {
    10.0f32.powf(gain_db * (1.0 / 40.0))
}
*/

#[inline]
fn scale_q_norm_for_order(q_norm: f32, scale: f32) -> f32 {
    if q_norm > 1.0 {
        1.0 + ((q_norm - 1.0) * scale)
    } else {
        q_norm
    }
}
