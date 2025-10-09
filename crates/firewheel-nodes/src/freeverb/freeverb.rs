use super::{all_pass::AllPass, comb::Comb};

const FIXED_GAIN: f64 = 0.015;

const SCALE_WET: f64 = 3.0;
const SCALE_DAMPENING: f64 = 0.4;

const SCALE_ROOM: f64 = 0.28;
const OFFSET_ROOM: f64 = 0.7;

const STEREO_SPREAD: usize = 23;

const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING: [usize; 4] = [556, 441, 341, 225];

#[derive(Debug)]
pub struct Freeverb {
    combs: [(Comb, Comb); 8],
    allpasses: [(AllPass, AllPass); 4],
    wet_gains: (f64, f64),
    wet: f64,
    width: f64,
    dry: f64,
    input_gain: f64,
    dampening: f64,
    room_size: f64,
    frozen: bool,
}

fn adjust_length(length: usize, sr: usize) -> usize {
    (length as f64 * sr as f64 / 44100.0) as usize
}

impl Freeverb {
    pub fn new(sample_rate: usize) -> Self {
        let mut freeverb = Freeverb {
            combs: core::array::from_fn(|i| {
                (
                    Comb::new(adjust_length(COMB_TUNING[i], sample_rate)),
                    Comb::new(adjust_length(COMB_TUNING[i] + STEREO_SPREAD, sample_rate)),
                )
            }),
            allpasses: core::array::from_fn(|i| {
                (
                    AllPass::new(adjust_length(ALLPASS_TUNING[i], sample_rate)),
                    AllPass::new(adjust_length(
                        ALLPASS_TUNING[i] + STEREO_SPREAD,
                        sample_rate,
                    )),
                )
            }),
            wet_gains: (0.0, 0.0),
            wet: 0.0,
            dry: 0.0,
            input_gain: 0.0,
            width: 0.0,
            dampening: 0.0,
            room_size: 0.0,
            frozen: false,
        };

        freeverb.set_wet(1.0);
        freeverb.set_width(0.5);
        freeverb.set_dampening(0.5);
        freeverb.set_room_size(0.5);
        freeverb.set_frozen(false);

        freeverb
    }

    pub fn tick(&mut self, input: (f64, f64)) -> (f64, f64) {
        let input_mixed = (input.0 + input.1) * FIXED_GAIN * self.input_gain;

        let mut out = (0.0, 0.0);

        for combs in self.combs.iter_mut() {
            out.0 += combs.0.tick(input_mixed);
            out.1 += combs.1.tick(input_mixed);
        }

        for allpasses in self.allpasses.iter_mut() {
            out.0 = allpasses.0.tick(out.0);
            out.1 = allpasses.1.tick(out.1);
        }

        (
            out.0 * self.wet_gains.0 + out.1 * self.wet_gains.1 + input.0 * self.dry,
            out.1 * self.wet_gains.0 + out.0 * self.wet_gains.1 + input.1 * self.dry,
        )
    }

    pub fn set_dampening(&mut self, value: f64) {
        self.dampening = value * SCALE_DAMPENING;
    }

    pub fn set_wet(&mut self, value: f64) {
        self.wet = value * SCALE_WET;
        self.update_wet_gains();
    }

    pub fn set_width(&mut self, value: f64) {
        self.width = value;
        self.update_wet_gains();
    }

    fn update_wet_gains(&mut self) {
        self.wet_gains = (
            self.wet * (self.width / 2.0 + 0.5),
            self.wet * ((1.0 - self.width) / 2.0),
        )
    }

    fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
        self.input_gain = if frozen { 0.0 } else { 1.0 };
        self.update_combs();
    }

    pub fn set_room_size(&mut self, value: f64) {
        self.room_size = value * SCALE_ROOM + OFFSET_ROOM;
    }

    pub fn update_combs(&mut self) {
        let (feedback, dampening) = if self.frozen {
            (1.0, 0.0)
        } else {
            (self.room_size, self.dampening)
        };

        for combs in self.combs.iter_mut() {
            combs.0.set_feedback(feedback);
            combs.1.set_feedback(feedback);

            combs.0.set_dampening(dampening);
            combs.1.set_dampening(dampening);
        }
    }

    pub fn reset(&mut self) {
        for (l, r) in self.combs.iter_mut() {
            l.reset();
            r.reset();
        }

        for (l, r) in self.allpasses.iter_mut() {
            l.reset();
            r.reset();
        }
    }

    pub fn resize(&mut self, sample_rate: usize) {
        for (i, (l, r)) in self.combs.iter_mut().enumerate() {
            l.resize(adjust_length(COMB_TUNING[i], sample_rate));
            r.resize(adjust_length(COMB_TUNING[i] + STEREO_SPREAD, sample_rate));
        }

        for (i, (l, r)) in self.allpasses.iter_mut().enumerate() {
            l.resize(adjust_length(ALLPASS_TUNING[i], sample_rate));
            r.resize(adjust_length(
                ALLPASS_TUNING[i] + STEREO_SPREAD,
                sample_rate,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::STEREO_SPREAD;

    #[test]
    fn ticking_does_something() {
        let mut freeverb = super::Freeverb::new(44100);
        assert_eq!(freeverb.tick((1.0, 1.0)), (0.0, 0.0));
        for _ in 0..(super::COMB_TUNING[7] + STEREO_SPREAD) * 2 {
            freeverb.tick((0.0, 0.0));
        }
        assert_ne!(freeverb.tick((0.0, 0.0)), (0.0, 0.0));
    }
}
