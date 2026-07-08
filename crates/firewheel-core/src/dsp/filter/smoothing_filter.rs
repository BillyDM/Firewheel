#[cfg(not(feature = "std"))]
use num_traits::Float;

use core::num::NonZeroU32;

/// The default number of seconds for a [`Smoothing Filter`].
///
/// This value is chosen to be roughly equal to a typical block size
/// of 1024 samples (23 ms) to eliminate stair-stepping for most
/// games.
pub const DEFAULT_SMOOTH_SECONDS: f32 = 23.0 / 1_000.0;

/// The default epsilon value for a [`SmoothingFilter`].
pub const DEFAULT_SETTLE_EPSILON: f32 = 0.01;

/// The minimum supported epsilon value for a [`SmoothingFilter`].
///
/// Values smaller than this can tend to never settle correctly due to floating point
/// accumulation errors.
pub const MIN_SETTLE_EPSILON: f32 = 0.00075;
/// The minimum supported epsilon value for a [`SmoothingFilter`].
///
/// Values larger than this can tend to never settle correctly due to floating point
/// accumulation errors.
pub const MAX_SETTLE_EPSILON: f32 = 0.9;

/// The coefficients for a simple smoothing/declicking filter where:
///
/// `y[n] = (target_value * a) + (x[n-1] * b)`
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothingFilterCoeff {
    pub a0: f32,
    pub b1: f32,
}

impl SmoothingFilterCoeff {
    /// Calculate the coefficients for a [`SmoothingFilter`].
    ///
    /// * `sample_rate` - The sample rate of the signal.
    /// * `smooth_secs` - The amount of time it takes for the filter to smooth from one
    ///   end of the parameter's range to the other end.
    ///     * If less than 0.0, then it will be clamped to 0.0.
    /// * `settle_epsilon` - The threshold at which the filter is considered "settled".
    ///   For example `0.01` means that the filter is considered settled if the value is
    ///   within 1% of the total span of the parameter's range. Must be >=
    ///   [`MIN_SETTLE_EPSILON`] (0.00075) and <= [`MAX_SETTLE_EPSILON`] (0.9).
    ///     * Will be clamped to the range `[0.00075..0.9]`.
    /// Returns `true` if this filter is settled, `false` if not.
    pub fn new(sample_rate: NonZeroU32, smooth_secs: f32, epsilon: f32) -> Self {
        let smooth_secs = smooth_secs.max(0.0);
        let epsilon = epsilon.clamp(MIN_SETTLE_EPSILON, MAX_SETTLE_EPSILON);

        // The b1 coefficient of a one pole lp filter is given by:
        //
        // b1 = e ^ (-1 / t_to_1_over_e)
        //
        // where t_to_1_over_e is the amount of time for the filter to decay to 1/e
        // (about 36.8%).
        //
        // So, to get a filter which decays to a given "epsilon" value in
        // "t_to_epsilon" frames, we need to adjust the t_to_1_over_e value. Because
        // doubling the time is equivalent to decaying by another 36.8%, we can
        // relate epsilon and t_to_epsilon with:
        //
        // epsilon = (1/e) ^ c
        // t_to_epsilon = t_to_1_over_e * c
        //
        // where c is some unknown constant.
        //
        // Solve for c:
        //
        // c = log_(1/e)(epsilon)
        //   = -ln(epsilon)
        //
        // Solve for t_to_1_over_e:
        //
        // t_to_1_over_e = t_to_epsilon / c
        //               = t_to_epsilon * (1 / -ln(epsilon))
        //               = -ln(epsilon) / t_to_epsilon
        //
        // Which finally gives us:
        //
        // b1 = e ^ (-1 / (-ln(epsilon) / t_to_epsilon))
        //    = e ^ (ln(epsilon) / t_to_epsilon)
        //    = epsilon ^ (1 / t_to_epsilon)

        let t_to_epsilon = (smooth_secs * sample_rate.get() as f32).max(1.0);

        let b1 = epsilon.powf(t_to_epsilon.recip());
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
    /// * `target` - The target value that is being smoothed to.
    /// * `span` - The size of the parameter's range (equal to `(max_value - min_value).abs()`).
    /// * `settle_epsilon` - The threshold at which the filter is considered "settled".
    ///   For example `0.01` means that the filter is considered settled if the value is
    ///   within 1% of the total `span`.
    ///
    /// Returns `true` if this filter is settled, `false` if not.
    pub fn settle(&mut self, target: f32, span: f32, settle_epsilon: f32) -> bool {
        if self.z1 == target {
            true
        } else if span == 0.0 || (self.z1 - target).abs() < (span * settle_epsilon).abs() {
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

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn smoothing_filter_decay() {
        const TEST_SAMPLE_RATES: [u32; 2] = [44100, 48000];
        const TEST_SECONDS: [f32; 6] = [
            0.0,
            1.0 / 1_000.0,
            5.0 / 1_000.0,
            23.0 / 1_000.0,
            100.0 / 1_000.0,
            500.0 / 1_000.0,
        ];
        const TEST_EPSILONS: [f32; 6] = [
            MIN_SETTLE_EPSILON,
            0.001,
            0.01,
            0.1,
            0.5,
            MAX_SETTLE_EPSILON,
        ];
        const TEST_VALUES: [(f32, f32); 5] = [
            (1.0, 0.0),
            (0.0, 1.0),
            (1.0, -1.0),
            (20.0, 20480.0),
            (20480.0, 20.0),
        ];

        let mut max_error = 0.0;

        for sr in TEST_SAMPLE_RATES {
            for secs in TEST_SECONDS {
                for eps in TEST_EPSILONS {
                    for (start_value, end_value) in TEST_VALUES {
                        let coeff =
                            SmoothingFilterCoeff::new(NonZeroU32::new(sr).unwrap(), secs, eps);
                        let mut filter = SmoothingFilter::new(start_value);

                        let mut frames: u32 = 0;
                        while !filter.settle(end_value, (start_value - end_value).abs(), eps)
                            && frames < sr * 4
                        {
                            let _ = filter.process(end_value, coeff);
                            frames += 1;
                        }

                        let expected_frames = ((secs * sr as f32).round() as u32).max(1);

                        let diff = (frames as f32 - expected_frames as f32).abs();

                        // Don't consider off by one frame to be an error.
                        let diff = if diff < 2.0 { 0.0 } else { diff };

                        let error = diff / expected_frames as f32;

                        max_error = error.max(max_error);

                        // Give some leeway for floating point accumulation errors.
                        const ERROR_TOLERANCE: f32 = 0.02;

                        assert!(
                            error < ERROR_TOLERANCE,
                            "error {} is >= the maximum accepted error {} | expected_frames {}, got frames {}",
                            error,
                            ERROR_TOLERANCE,
                            expected_frames,
                            frames
                        );
                    }
                }
            }
        }

        dbg!(max_error);
    }
}
