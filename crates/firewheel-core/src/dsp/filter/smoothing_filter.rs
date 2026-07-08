#[cfg(not(feature = "std"))]
use num_traits::Float;

use core::num::NonZeroU32;

/// The default number of seconds for a [`Smoothing Filter`].
///
/// This value is chosen to where the halfway decay point is roughly equal to a
/// typical block size of 1024 samples (23 ms), which should eliminate the stair-stepping
/// for most games.
pub const DEFAULT_SMOOTH_SECONDS: f32 = 46.0 / 1_000.0;

/// The default settle ratio value for a [`SmoothingFilter`].
pub const DEFAULT_SETTLE_RATIO: f32 = 0.01;

/// The minimum supported settle ratio value for a [`SmoothingFilter`].
///
/// Values smaller than this can tend to never settle correctly due to floating point
/// accumulation errors.
pub const MIN_SETTLE_RATIO: f32 = 0.00075;
/// The maximum supported settle ratio value for a [`SmoothingFilter`].
///
/// Values larger than this can tend to never settle correctly due to floating point
/// accumulation errors.
pub const MAX_SETTLE_RATIO: f32 = 0.9;

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
    /// * `smooth_secs` - The amount of time in seconds it takes for the filter to smooth
    ///   from one value to another.
    ///     * If less than 0.0, then 0.0 will be used.
    /// * `settle_ratio` - The threshold at which the filter is considered "settled".
    ///   For example `0.01` means that the filter is considered settled if the value is
    ///   within 1% of the total span of the parameter's range. Must be >=
    ///   [`MIN_SETTLE_RATIO`] (0.00075) and <= [`MAX_SETTLE_RATIO`] (0.9).
    ///     * Will be clamped to the range `[0.00075..0.9]`.
    ///
    /// Returns `true` if this filter is settled, `false` if not.
    pub fn new(sample_rate: NonZeroU32, smooth_secs: f32, settle_ratio: f32) -> Self {
        let smooth_secs = smooth_secs.max(0.0);
        let ratio = settle_ratio.clamp(MIN_SETTLE_RATIO, MAX_SETTLE_RATIO);

        // The b1 coefficient of a one pole lp filter is given by:
        //
        // b1 = e ^ (-1 / t_to_1_over_e)
        //
        // where t_to_1_over_e is the amount of time in frames for an impulse signal
        // to decay to 1/e (about 36.8%).
        //
        // So, to find the coefficients where an impulse signal decays to a "ratio"
        // in "t_to_ratio" frames, we need to adjust the t_to_1_over_e value.
        // Because doubling the time is equivalent to decaying by another 36.8%, we
        // can relate ratio and t_to_ratio with:
        //
        // ratio = (1/e) ^ c
        // t_to_ratio = t_to_1_over_e * c
        //
        // where c is some unknown constant.
        //
        // Solve for c:
        //
        // c = log_(1/e)(ratio)
        //   = -ln(ratio)
        //
        // Solve for t_to_1_over_e:
        //
        // t_to_1_over_e = t_to_ratio / c
        //               = t_to_ratio * (1 / -ln(ratio))
        //               = -ln(ratio) / t_to_ratio
        //
        // Which finally gives us:
        //
        // b1 = e ^ (-1 / (-ln(ratio) / t_to_ratio))
        //    = e ^ (ln(ratio) / t_to_ratio)
        //    = ratio ^ (1 / t_to_ratio)

        let t_to_ratio = (smooth_secs * sample_rate.get() as f32).max(1.0);

        let b1 = ratio.powf(t_to_ratio.recip());
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
    /// * `value_span` - The size of this parameter's range.
    ///   * If the minimum and maximum values of the parameter are known, then
    ///     typically `max_value - min_value` should be used.
    ///   * If the min and/or max values are not known, then use a span value that is
    ///     typical to be the worst-case-scenario (For example, if creating a "gain"
    ///     parameter, a good value to use is [`DEFAULT_GAIN_SPAN`] (`2.0`) since
    ///     immediately jumping from 0% volume to 200% volume or vice versa is typically
    ///     the worst-case-scenario).
    ///   * This value does not need to be positive.
    /// * `settle_ratio` - The threshold at which the filter is considered "settled".
    ///   For example `0.01` means that the filter is considered settled if the value is
    ///   within 1% of the total `value_span`.
    ///
    /// Returns `true` if this filter is settled, `false` if not.
    ///
    /// [`DEFAULT_GAIN_SPAN`]: crate::param::smoother::DEFAULT_GAIN_SPAN
    pub fn try_settle(&mut self, target: f32, value_span: f32, settle_ratio: f32) -> bool {
        if self.z1 == target {
            true
        } else if value_span == 0.0 || (self.z1 - target).abs() < (value_span * settle_ratio).abs()
        {
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
            DEFAULT_SMOOTH_SECONDS,
            100.0 / 1_000.0,
            500.0 / 1_000.0,
        ];
        const TEST_RATIOS: [f32; 6] = [MIN_SETTLE_RATIO, 0.001, 0.01, 0.1, 0.5, MAX_SETTLE_RATIO];
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
                for ratio in TEST_RATIOS {
                    for (start_value, end_value) in TEST_VALUES {
                        let coeff =
                            SmoothingFilterCoeff::new(NonZeroU32::new(sr).unwrap(), secs, ratio);
                        let mut filter = SmoothingFilter::new(start_value);

                        let mut frames: u32 = 0;
                        while !filter.try_settle(end_value, (start_value - end_value).abs(), ratio)
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
