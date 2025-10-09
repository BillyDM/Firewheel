use super::delay_line::DelayLine;

#[derive(Debug)]
pub struct Comb {
    delay_line: DelayLine,
    feedback: f64,
    filter_state: f64,
    dampening: f64,
    dampening_inverse: f64,
}

impl Comb {
    pub fn new(delay_length: usize) -> Self {
        Self {
            delay_line: DelayLine::new(delay_length),
            feedback: 0.5,
            filter_state: 0.0,
            dampening: 0.5,
            dampening_inverse: 0.5,
        }
    }

    pub fn set_dampening(&mut self, value: f64) {
        self.dampening = value;
        self.dampening_inverse = 1.0 - value;
    }

    pub fn set_feedback(&mut self, value: f64) {
        self.feedback = value;
    }

    pub fn tick(&mut self, input: f64) -> f64 {
        let output = self.delay_line.read();

        self.filter_state = output * self.dampening_inverse + self.filter_state * self.dampening;

        self.delay_line
            .write_and_advance(input + self.filter_state * self.feedback);

        output
    }

    pub fn reset(&mut self) {
        self.filter_state = 0.0;
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
        let mut comb = super::Comb::new(2);
        assert_eq!(comb.tick(1.0), 0.0);
        assert_eq!(comb.tick(0.0), 0.0);
        assert_eq!(comb.tick(0.0), 1.0);
        assert_eq!(comb.tick(0.0), 0.0);
        assert_eq!(comb.tick(0.0), 0.25);
        assert_eq!(comb.tick(0.0), 0.125);
        assert_eq!(comb.tick(0.0), 0.125);
        assert_eq!(comb.tick(0.0), 0.09375);
    }
}
