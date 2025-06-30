use core::num::NonZeroU32;

use crate::{
    dsp::filter::smoothing_filter::{self, SmoothingFilter, SmoothingFilterCoeff},
    StreamInfo,
};

/// The configuration for a [`SmoothedParam`]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmootherConfig {
    /// The amount of smoothing in seconds
    ///
    /// By default this is set to 5 milliseconds.
    pub smooth_secs: f32,
    /// The threshold at which the smoothing will complete
    ///
    /// By default this is set to `0.00001`.
    pub settle_epsilon: f32,
}

impl Default for SmootherConfig {
    fn default() -> Self {
        Self {
            smooth_secs: smoothing_filter::DEFAULT_SMOOTH_SECONDS,
            settle_epsilon: smoothing_filter::DEFAULT_SETTLE_EPSILON,
        }
    }
}

/// A helper struct to smooth an f32 parameter.
pub struct SmoothedParam {
    target_value: f32,
    target_times_a: f32,
    filter: SmoothingFilter,
    coeff: SmoothingFilterCoeff,
    smooth_secs: f32,
    settle_epsilon: f32,
}

impl SmoothedParam {
    /// Construct a new smoothed f32 parameter with the given configuration.
    pub fn new(value: f32, config: SmootherConfig, sample_rate: NonZeroU32) -> Self {
        assert!(config.smooth_secs > 0.0);
        assert!(config.settle_epsilon > 0.0);

        let coeff = SmoothingFilterCoeff::new(sample_rate, config.smooth_secs);

        Self {
            target_value: value,
            target_times_a: value * coeff.a0,
            filter: SmoothingFilter::new(value),
            coeff,
            smooth_secs: config.smooth_secs,
            settle_epsilon: config.settle_epsilon,
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
        self.filter.settle(self.target_value, self.settle_epsilon)
    }

    pub fn is_smoothing(&self) -> bool {
        !self.filter.has_settled(self.target_value)
    }

    /// Reset the smoother.
    pub fn reset(&mut self) {
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

            self.filter.settle(self.target_value, self.settle_epsilon);
        } else {
            buffer.fill(self.target_value);
        }
    }

    /// Update the sample rate.
    pub fn update_sample_rate(&mut self, sample_rate: NonZeroU32) {
        self.coeff = SmoothingFilterCoeff::new(sample_rate, self.smooth_secs);
    }
}

/// A helper struct to smooth an f32 parameter, along with a buffer of smoothed values.
pub struct SmoothedParamBuffer {
    smoother: SmoothedParam,
    buffer: Vec<f32>,
    buffer_is_constant: bool,
}

impl SmoothedParamBuffer {
    /// Construct a new smoothed f32 parameter with the given configuration.
    pub fn new(value: f32, config: SmootherConfig, stream_info: &StreamInfo) -> Self {
        let mut buffer = Vec::new();
        buffer.reserve_exact(stream_info.max_block_frames.get() as usize);
        buffer.resize(stream_info.max_block_frames.get() as usize, value);

        Self {
            smoother: SmoothedParam::new(value, config, stream_info.sample_rate),
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

        self.smoother.reset();
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
