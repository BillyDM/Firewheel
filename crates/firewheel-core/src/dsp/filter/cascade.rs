use crate::dsp::filter::{
    filter_trait::Filter,
    primitives::{
        one_pole_iir::{OnePoleIirCoeff, OnePoleIirState},
        spec::FilterOrder,
        svf::{SvfCoeff, SvfState},
    },
};

/// A cascade of `N` state variable filters + a first order filter
#[derive(Clone, Copy)]
pub struct FilterCascade<const ORDER: FilterOrder> {
    one_pole: OnePoleIirState,
    svfs: [SvfState; ORDER],
}

pub struct FilterCascadeCoeffs<const ORDER: FilterOrder> {
    pub one_pole: OnePoleIirCoeff,
    pub svfs: [SvfCoeff; ORDER],
}

impl<const ORDER: FilterOrder> Default for FilterCascadeCoeffs<ORDER> {
    fn default() -> Self {
        Self {
            one_pole: OnePoleIirCoeff::NO_OP,
            svfs: [SvfCoeff::NO_OP; ORDER],
        }
    }
}

impl<const ORDER: FilterOrder> Default for FilterCascade<ORDER> {
    fn default() -> Self {
        Self {
            one_pole: Default::default(),
            svfs: [Default::default(); ORDER],
        }
    }
}

impl<const ORDER: FilterOrder> Filter for FilterCascade<ORDER> {
    type Coeffs = FilterCascadeCoeffs<ORDER>;

    #[inline(always)]
    fn reset(&mut self) {
        self.one_pole.reset();
        for svf in self.svfs.iter_mut() {
            svf.reset();
        }
    }

    #[inline(always)]
    fn process(&mut self, x: f32, coeffs: &Self::Coeffs) -> f32 {
        self.svfs
            .process(self.one_pole.process(x, &coeffs.one_pole), &coeffs.svfs)
    }

    #[inline(always)]
    fn is_silent(&self) -> bool {
        self.one_pole.is_silent() && self.svfs.is_silent()
    }
}

/// A cascade of up to `N` state variable filters + a first order filter
/// Supports redesigning of filters with different steepness up to `N` but also uses space for `N` filters regardless of current design
#[derive(Clone, Copy)]
pub struct FilterCascadeUpTo<const ORDER: FilterOrder> {
    pub one_pole: OnePoleIirState,
    pub svfs: [SvfState; ORDER],
    pub num_svfs: usize,
}

impl<const ORDER: FilterOrder> Default for FilterCascadeUpTo<ORDER> {
    fn default() -> Self {
        Self {
            one_pole: Default::default(),
            num_svfs: Default::default(),
            svfs: [Default::default(); ORDER],
        }
    }
}

impl<const ORDER: FilterOrder> Filter for FilterCascadeUpTo<ORDER> {
    type Coeffs = FilterCascadeCoeffs<ORDER>;

    #[inline(always)]
    fn reset(&mut self) {
        self.one_pole.reset();
        for svf in self.svfs.iter_mut() {
            svf.reset();
        }
    }

    #[inline(always)]
    fn process(&mut self, x: f32, coeffs: &Self::Coeffs) -> f32 {
        self.svfs
            .iter_mut()
            .zip(coeffs.svfs.iter())
            .take(self.num_svfs)
            .fold(
                self.one_pole.process(x, &coeffs.one_pole),
                |acc, (svf, coeffs)| svf.process(acc, coeffs),
            )
    }

    #[inline(always)]
    fn is_silent(&self) -> bool {
        self.one_pole.is_silent()
            && self
                .svfs
                .iter()
                .take(self.num_svfs)
                .all(|svf| svf.is_silent())
    }
}
