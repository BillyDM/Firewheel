use bevy_platform::prelude::Vec;

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

    pub fn read(&self) -> f64 {
        self.buffer[self.index]
    }

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
                    assert_eq!(line.read(), 0.0);
                    line.write_and_advance(i as f64);
                }
                for i in 0..$length {
                    assert_eq!(line.read(), i as f64);
                    line.write_and_advance(0.0);
                }
            }
        };
    }

    delay_line_test!(length_1, 1);
    delay_line_test!(length_3, 3);
    delay_line_test!(length_10, 10);
}
