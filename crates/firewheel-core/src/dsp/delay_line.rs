use core::num::NonZeroU32;

use bevy_platform::prelude::Vec;
#[cfg(not(feature = "std"))]
use num_traits::Float;

#[derive(Debug)]
pub struct DelayLine {
    buffer: Vec<f64>,
    index: usize,
}

impl DelayLine {
    pub fn new(length: usize) -> Self {
        // No need to carry extra capacity around.
        let mut buffer = Vec::new();
        buffer.reserve_exact(length);
        buffer.extend(core::iter::repeat_n(0.0, length));

        Self { buffer, index: 0 }
    }

    /// Read the least recent sample pushed to this delay line (the sample that
    /// will be replaced with the next [`Self::write_and_advance`]).
    pub fn read_last(&self) -> f64 {
        self.buffer[self.index]
    }

    pub fn read(&self, num_samples_delay: usize) -> Option<f64> {
        let buffer_len = self.buffer.len();

        // Ensure that requested samples of delay are not greater than our capacity and that the number of samples of delay is not zero.
        if buffer_len < num_samples_delay || num_samples_delay == 0 {
            return None;
        }

        // Wrap the requested delay if necessary
        let index = match num_samples_delay > self.index {
            // Wrapping is needed - wrap from the end of the vec.
            true => buffer_len - (num_samples_delay - self.index),
            // No wrapping required - just subtract from the index.
            false => self.index - num_samples_delay,
        };

        // Our index must be in range
        Some(self.buffer[index])
    }

    /// Read a sample at some delay of samples. Fractional delays will linearly
    /// interpolate between the two nearest samples.
    ///
    /// # Returns
    ///
    /// Returns the value of the delayed sample, if the delay samples is not
    /// greater than the delay line capacity, in which case `None` is returned.
    pub fn read_seconds(&self, seconds_delay: f32, sample_rate: NonZeroU32) -> Option<f64> {
        // Get the number of samples to delay. This number may be fractional and
        // will be interpolated. Add 1.0, as a delay of 0.0 is invalid and fractional delays
        // will always start at at least 1.0.
        let num_samples_delay_f = (seconds_delay * sample_rate.get() as f32) + 1f32;

        let buffer_len = self.buffer.len();

        // Ensure the requested delay is within bounds
        if buffer_len < num_samples_delay_f.ceil() as usize {
            return None;
        }

        // Get the actual index of the delay, as a fraction
        let mut index_f = self.index as f32 - num_samples_delay_f;

        // If negative, wrap to the end of the buffer
        if index_f.is_sign_negative() {
            index_f = buffer_len as f32 - index_f.abs();
        }

        // Find the two indices to interpolate between
        let index_a = index_f.floor() as usize % buffer_len;
        let index_b = (index_a + 1) % buffer_len;

        let sample_a = self.buffer[index_a];
        let sample_b = self.buffer[index_b];

        // Amount to interpolate
        let fract = index_f.fract() as f64;

        let mix_a = sample_a * (1.0 - fract);
        let mix_b = sample_b * fract;

        Some(mix_a + mix_b)
    }

    /// Overwrite the least recent sample.
    pub fn write_and_advance(&mut self, value: f64) {
        self.buffer[self.index] = value;

        if self.index == self.buffer.len() - 1 {
            self.index = 0;
        } else {
            self.index += 1;
        }
    }

    pub fn reset(&mut self) {
        self.buffer.fill(0.0);
    }

    pub fn resize(&mut self, size: usize) {
        // little point in messing around with the exact
        // capacity here
        self.buffer.resize(size, 0.0);
        self.index %= self.buffer.len();
    }
}

#[cfg(test)]
mod tests {
    macro_rules! delay_line_test {
        ($name:ident, $length:expr) => {
            #[test]
            fn $name() {
                let mut line = super::DelayLine::new($length);
                for i in 0..$length {
                    assert_eq!(line.read_last(), 0.0);
                    line.write_and_advance(i as f64);
                }
                for i in 0..$length {
                    assert_eq!(line.read_last(), i as f64);
                    line.write_and_advance(0.0);
                }
            }
        };
    }

    delay_line_test!(length_1, 1);
    delay_line_test!(length_3, 3);
    delay_line_test!(length_10, 10);

    #[test]
    fn read_delay_line() {
        let mut line = super::DelayLine::new(10);

        // Write enough times to overwrite some old values
        for i in 0..14 {
            line.write_and_advance(i as f64);
        }

        // 10, 11, 12, 13, 4, 5, 6, 7, 8, 9
        //                 └ Index

        assert_eq!(line.read_last(), 4.0);
        // Read without wrapping
        assert_eq!(line.read(1), Some(13.0));
        // Read with wrapping
        assert_eq!(line.read(8), Some(6.0));
        // The index should be equal to the maximum delay
        assert_eq!(line.read_last(), line.read(line.buffer.len()).unwrap());
        // Obtain nothing with invalid ranges
        assert_eq!(line.read(0), None);
        assert_eq!(line.read(11), None);
    }

    #[test]
    fn read_delay_line_fractional() {
        let mut line = super::DelayLine::new(10);

        // Write enough times to overwrite some old values
        for i in 0..14 {
            line.write_and_advance(i as f64);
        }

        let sample_rate = core::num::NonZeroU32::new(1u32).unwrap();

        // 10, 11, 12, 13, 4, 5, 6, 7, 8, 9
        //              │  └ Index
        //              └─── 0s

        // Read without interpolation
        assert_eq!(line.read_seconds(0.0, sample_rate), Some(13.0));
        // Read with interpolation, without wrapping
        assert_eq!(line.read_seconds(1.5, sample_rate), Some(11.5));
        // Read with interpolation, with wrapping
        assert_eq!(line.read_seconds(5.5, sample_rate), Some(7.5));
        // Obtain nothing with invalid ranges
        assert_eq!(line.read_seconds(9.5, sample_rate), None);
    }
}
