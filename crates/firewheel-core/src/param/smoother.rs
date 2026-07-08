use core::num::NonZeroU32;

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::Vec;

use crate::{
    StreamInfo,
    dsp::filter::smoothing_filter::{
        self, MAX_SETTLE_RATIO, MIN_SETTLE_RATIO, SmoothingFilter, SmoothingFilterCoeff,
    },
};

/// A good default value to use for the `span` argument in [`SmoothedParam::new()`]
/// when constructing gain/volume parameters.
///
/// This causes maximum smoothing to occur when the volume immediately jumps
/// from 0% to 200% or vice versa.
pub const DEFAULT_GAIN_SPAN: f32 = 2.0;

/// The configuration for a [`SmoothedParam`]
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SmootherConfig {
    /// The amount of time in seconds it takes for the filter to smooth from one
    /// value to another.
    ///
    /// If less than 0.0, then 0.0 will be used.
    ///
    /// By default this is set to `0.062` (62ms). This value is chosen such that
    /// the stair-stepping effect isn't noticeable for a typical block size of 1024
    /// samples.
    pub smooth_seconds: f32,
    /// The threshold at which the filter is considered "settled".
    ///
    /// For example `0.01` means that the filter is considered settled if the value
    /// is within 1% of the parameter's range.
    ///
    /// Will be clamped to the range `[0.00075..0.9]`.
    ///
    /// By default this is set to `0.01`.
    pub settle_ratio: f32,
}

impl Default for SmootherConfig {
    fn default() -> Self {
        Self {
            smooth_seconds: smoothing_filter::DEFAULT_SMOOTH_SECONDS,
            settle_ratio: smoothing_filter::DEFAULT_SETTLE_RATIO,
        }
    }
}

/// A helper struct to smooth an f32 parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothedParam {
    target_value: f32,
    target_times_a: f32,
    filter: SmoothingFilter,
    coeff: SmoothingFilterCoeff,
    smooth_secs: f32,
    settle_ratio: f32,
    span: f32,
}

impl SmoothedParam {
    /// Construct a new smoothed f32 parameter with the given configuration.
    ///
    /// * `initial_value` - The initial target value.
    /// * `value_span` - The size of this parameter's range.
    ///   * If the minimum and maximum values of the parameter are known, then
    ///     typically `max_value - min_value` should be used.
    ///   * If the min and/or max values are not known, then use a span value that is
    ///     typical to be the worst-case-scenario (For example, if creating a "gain"
    ///     parameter, a good value to use is [`DEFAULT_GAIN_SPAN`] (`2.0`) since
    ///     immediately jumping from 0% volume to 200% volume or vice versa is typically
    ///     the worst-case-scenario).
    ///   * This value does not need to be positive.
    /// * `config` - Extra configuration options for the smoothing filter.
    /// * `sample_rate` - The sample rate of the stream.
    pub fn new(
        initial_value: f32,
        value_span: f32,
        config: SmootherConfig,
        sample_rate: NonZeroU32,
    ) -> Self {
        let smooth_secs = config.smooth_seconds.max(0.0);
        let settle_ratio = config
            .settle_ratio
            .clamp(MIN_SETTLE_RATIO, MAX_SETTLE_RATIO);

        let coeff = SmoothingFilterCoeff::new(sample_rate, smooth_secs, settle_ratio);

        Self {
            target_value: initial_value,
            target_times_a: initial_value * coeff.a0,
            filter: SmoothingFilter::new(initial_value),
            span: value_span.abs(),
            coeff,
            smooth_secs,
            settle_ratio,
        }
    }

    /// The target value of the parameter.
    pub fn target_value(&self) -> f32 {
        self.target_value
    }

    /// Set the target value of the parameter.
    pub fn set_value(&mut self, value: f32) {
        self.target_value = value;
        self.target_times_a = value * self.coeff.a0;
    }

    /// Settle the filter if its state is close enough to the target value.
    ///
    /// Returns `true` if this filter is settled, `false` if not.
    pub fn settle(&mut self) -> bool {
        self.filter
            .try_settle(self.target_value, self.span, self.settle_ratio)
    }

    /// Returns `true` if this parameter is currently smoothing this process cycle,
    /// `false` if not.
    pub fn is_smoothing(&self) -> bool {
        !self.filter.has_settled(self.target_value)
    }

    /// Returns `false` if this parameter is currently smoothing this process cycle,
    /// `true` if not.
    pub fn has_settled(&self) -> bool {
        self.filter.has_settled(self.target_value)
    }

    /// Returns `true` if this parameter has settled to the given value, `false`
    /// if not.
    pub fn has_settled_at(&self, value: f32) -> bool {
        self.target_value == value && self.filter.has_settled(self.target_value)
    }

    /// Returns `true` if this parameter has settled to a value less than or
    /// equal to the given value, `false` if not.
    pub fn has_settled_at_or_below(&self, value: f32) -> bool {
        self.target_value <= value && self.filter.has_settled(self.target_value)
    }

    /// Reset the internal smoothing filter to the current target value.
    pub fn reset_to_target(&mut self) {
        self.filter = SmoothingFilter::new(self.target_value);
    }

    /// Return the next smoothed value.
    #[inline(always)]
    pub fn next_smoothed(&mut self) -> f32 {
        self.filter
            .process_sample_a(self.target_times_a, self.coeff.b1)
    }

    /// Fill the given buffer with the smoothed values.
    pub fn process_into_buffer(&mut self, buffer: &mut [f32]) {
        if self.is_smoothing() {
            self.filter
                .process_into_buffer(buffer, self.target_value, self.coeff);

            self.filter
                .try_settle(self.target_value, self.span, self.settle_ratio);
        } else {
            buffer.fill(self.target_value);
        }
    }

    pub fn set_smooth_seconds(&mut self, seconds: f32, sample_rate: NonZeroU32) {
        self.coeff = SmoothingFilterCoeff::new(sample_rate, seconds, self.settle_ratio);
        self.smooth_secs = seconds;
    }

    /// Update the sample rate.
    pub fn update_sample_rate(&mut self, sample_rate: NonZeroU32) {
        self.coeff = SmoothingFilterCoeff::new(sample_rate, self.smooth_secs, self.settle_ratio);
    }
}

/// A helper struct to smooth an f32 parameter, along with a buffer of smoothed values.
#[derive(Debug, Clone)]
pub struct SmoothedParamBuffer {
    smoother: SmoothedParam,
    buffer: Vec<f32>,
    buffer_is_constant: bool,
}

impl SmoothedParamBuffer {
    /// Construct a new smoothed f32 parameter with an internal buffer with the given
    /// configuration.
    ///
    /// * `initial_value` - The initial target value.
    /// * `value_span` - The size of this parameter's range.
    ///   * If the minimum and maximum values of the parameter are known, then
    ///     typically `max_value - min_value` should be used.
    ///   * If the min and/or max values are not known, then use a span value that is
    ///     typical to be the worst-case-scenario (For example, if creating a "gain"
    ///     parameter, a good value to use is [`DEFAULT_GAIN_SPAN`] (`2.0`) since
    ///     immediately jumping from 0% volume to 200% volume or vice versa is typically
    ///     the worst-case-scenario).
    ///   * This value does not need to be positive.
    /// * `config` - Extra configuration options for the smoothing filter.
    /// * `stream_info` - The information about the current stream.
    pub fn new(
        initial_value: f32,
        value_span: f32,
        config: SmootherConfig,
        stream_info: &StreamInfo,
    ) -> Self {
        let mut buffer = Vec::new();
        buffer.reserve_exact(stream_info.max_block_frames.get() as usize);
        buffer.resize(stream_info.max_block_frames.get() as usize, initial_value);

        Self {
            smoother: SmoothedParam::new(
                initial_value,
                value_span,
                config,
                stream_info.sample_rate,
            ),
            buffer,
            buffer_is_constant: true,
        }
    }

    /// The current target value that is being smoothed to.
    pub fn target_value(&self) -> f32 {
        self.smoother.target_value()
    }

    /// Set the target value of the parameter.
    pub fn set_value(&mut self, value: f32) {
        self.smoother.set_value(value);
    }

    /// Reset the smoother.
    pub fn reset(&mut self) {
        if self.smoother.is_smoothing() || !self.buffer_is_constant {
            self.buffer.fill(self.smoother.target_value);
            self.buffer_is_constant = true;
        }

        self.smoother.reset_to_target();
    }

    /// Get the buffer of smoothed samples.
    ///
    /// The second value is `true` if all the values in the buffer are the same.
    pub fn get_buffer(&mut self, frames: usize) -> (&[f32], bool) {
        self.buffer_is_constant = !self.smoother.is_smoothing();

        self.smoother
            .process_into_buffer(&mut self.buffer[..frames]);

        (
            &self.buffer[..frames],
            self.buffer_is_constant || frames < 2,
        )
    }

    /// Returns `true` if this parameter is currently smoothing this process cycle,
    /// `false` if not.
    pub fn is_smoothing(&self) -> bool {
        self.smoother.is_smoothing()
    }

    /// Returns `false` if this parameter is currently smoothing this process cycle,
    /// `true` if not.
    pub fn has_settled(&self) -> bool {
        self.smoother.has_settled()
    }

    /// Returns `true` if this parameter has settled to the given value, `false`
    /// if not.
    pub fn has_settled_at(&self, value: f32) -> bool {
        self.smoother.has_settled_at(value)
    }

    /// Returns `true` if this parameter has settled to a value less than or
    /// equal to the given value, `false` if not.
    pub fn has_settled_at_or_below(&self, value: f32) -> bool {
        self.smoother.has_settled_at_or_below(value)
    }

    /// Update the stream information.
    pub fn update_stream(&mut self, stream_info: &StreamInfo) {
        self.smoother.update_sample_rate(stream_info.sample_rate);

        let max_block_frames = stream_info.max_block_frames.get() as usize;

        if self.buffer.len() > max_block_frames {
            self.buffer.resize(max_block_frames, 0.0);
        } else if self.buffer.len() < max_block_frames {
            self.buffer
                .reserve_exact(max_block_frames - self.buffer.len());
            self.buffer
                .resize(max_block_frames, self.smoother.target_value());
        }
    }
}
