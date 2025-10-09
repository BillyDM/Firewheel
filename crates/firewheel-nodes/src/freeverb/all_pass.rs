use super::delay_line::DelayLine;

#[derive(Debug)]
pub struct AllPass {
    delay_line: DelayLine,
}

impl AllPass {
    pub fn new(delay_length: usize) -> Self {
        Self {
            delay_line: DelayLine::new(delay_length),
        }
    }

    pub fn tick(&mut self, input: f64) -> f64 {
        let delayed = self.delay_line.read();
        let output = -input + delayed;

        // in the original version of freeverb this is a member which is never modified
        let feedback = 0.5;

        self.delay_line
            .write_and_advance(input + delayed * feedback);

        output
    }

    pub fn reset(&mut self) {
        self.delay_line.reset();
    }

    pub fn resize(&mut self, delay_length: usize) {
        self.delay_line.resize(delay_length);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn basic_ticking() {
        let mut allpass = super::AllPass::new(2);
        assert_eq!(allpass.tick(1.0), -1.0);
        assert_eq!(allpass.tick(0.0), 0.0);
        assert_eq!(allpass.tick(0.0), 1.0);
        assert_eq!(allpass.tick(0.0), 0.0);
        assert_eq!(allpass.tick(0.0), 0.5);
        assert_eq!(allpass.tick(0.0), 0.0);
        assert_eq!(allpass.tick(0.0), 0.25);
    }
}
