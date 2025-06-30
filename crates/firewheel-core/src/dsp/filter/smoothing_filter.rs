use core::num::NonZeroU32;

pub const DEFAULT_SMOOTH_SECONDS: f32 = 10.0 / 1_000.0;
pub const DEFAULT_SETTLE_EPSILON: f32 = 0.00001f32;

/// The coefficients for a simple smoothing/declicking filter where:
///
/// `y[n] = (target_value * a) + (x[n-1] * b)`
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothingFilterCoeff {
    pub a0: f32,
    pub b1: f32,
}

impl SmoothingFilterCoeff {
    pub fn new(sample_rate: NonZeroU32, smooth_secs: f32) -> Self {
        assert!(smooth_secs > 0.0);

        let b1 = (-1.0f32 / (smooth_secs * sample_rate.get() as f32)).exp();
        let a0 = 1.0f32 - b1;

        Self { a0, b1 }
    }
}

/// The state of a simple smoothing/declicking filter where:
///
/// `y[n] = (target_value * a) + (x[n-1] * b)`
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothingFilter {
    pub z1: f32,
}

impl SmoothingFilter {
    pub fn new(value: f32) -> Self {
        Self { z1: value }
    }

    #[inline(always)]
    pub fn process(&mut self, target: f32, coeff: SmoothingFilterCoeff) -> f32 {
        self.z1 = (target * coeff.a0) + (self.z1 * coeff.b1);
        self.z1
    }

    #[inline(always)]
    pub fn process_sample_a(&mut self, target_times_a: f32, coeff_b: f32) -> f32 {
        self.z1 = target_times_a + (self.z1 * coeff_b);
        self.z1
    }

    pub fn process_into_buffer(
        &mut self,
        buffer: &mut [f32],
        target: f32,
        coeff: SmoothingFilterCoeff,
    ) {
        let target_times_a = target * coeff.a0;

        for s in buffer.iter_mut() {
            *s = self.process_sample_a(target_times_a, coeff.b1);
        }
    }

    /// Settle the filter if its state is close enough to the target value.
    ///
    /// Returns `true` if this filter is settled, `false` if not.
    pub fn settle(&mut self, target: f32, settle_epsilon: f32) -> bool {
        if (self.z1 - target).abs() < settle_epsilon {
            self.z1 = target;
            true
        } else {
            false
        }
    }

    pub fn has_settled(&self, target: f32) -> bool {
        self.z1 == target
    }
}
