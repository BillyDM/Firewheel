use core::num::NonZeroU32;
use core::ops::Range;

use crate::{
    diff::{Diff, EventQueue, Patch, PatchError, PathBuilder},
    dsp::fade::FadeCurve,
    event::ParamData,
    param::smoother::{SmoothedParam, SmootherConfig},
};

/// A value representing the mix between two audio signals (e.g. second/first mix)
///
/// This is a normalized value in the range `[0.0, 1.0]`, where `0.0` is fully
/// the first signal, `1.0` is fully the second signal, and `0.5` is an equal
/// mix of both.
#[repr(transparent)]
#[derive(Default, Debug, Clone, Copy, PartialEq, PartialOrd)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Mix(f32);

impl Mix {
    /// Only use the first (first) signal.
    pub const FULLY_FIRST: Self = Self(0.0);
    /// Only use the second (second) signal.
    pub const FULLY_SECOND: Self = Self(1.0);

    /// Only use the dry (first) signal.
    pub const FULLY_DRY: Self = Self(0.0);
    /// Only use the wet (second) signal.
    pub const FULLY_WET: Self = Self(1.0);

    /// An equal mix of both signals
    pub const CENTER: Self = Self(0.5);

    /// Construct a value representing the mix between two audio signals
    /// (e.g. wet/drt mix)
    ///
    /// `mix` is a normalized value in the range `[0.0, 1.0]`, where `0.0` is fully
    /// the first (dry) signal, `1.0` is fully the second (wet) signal, and `0.5` is
    /// an equal mix of both.
    pub const fn new(mix: f32) -> Self {
        Self(mix.clamp(0.0, 1.0))
    }

    /// Construct a value representing the mix between two audio signals
    /// (e.g. wet/dry mix)
    ///
    /// `percent` is a value in the range `[0.0, 100.0]`, where `0.0` is fully the
    /// first (dry) signal, `100.0` is fully the second (wet) signal, and `50.0` is an
    /// equal mix of both.
    pub const fn from_percent(percent: f32) -> Self {
        Self::new(percent / 100.0)
    }

    pub const fn get(&self) -> f32 {
        self.0
    }

    pub const fn to_percent(self) -> f32 {
        self.0 * 100.0
    }

    /// Compute the raw gain values for both inputs.
    pub fn compute_gains(&self, fade_curve: FadeCurve) -> (f32, f32) {
        fade_curve.compute_gains_0_to_1(self.0)
    }
}

impl From<f32> for Mix {
    fn from(value: f32) -> Self {
        Self::new(value)
    }
}

impl From<f64> for Mix {
    fn from(value: f64) -> Self {
        Self::new(value as f32)
    }
}

impl From<Mix> for f32 {
    fn from(value: Mix) -> Self {
        value.get()
    }
}

impl From<Mix> for f64 {
    fn from(value: Mix) -> Self {
        value.get() as f64
    }
}

impl Diff for Mix {
    fn diff<E: EventQueue>(&self, baseline: &Self, path: PathBuilder, event_queue: &mut E) {
        if self != baseline {
            event_queue.push_param(ParamData::F32(self.0), path);
        }
    }
}

impl Patch for Mix {
    type Patch = Self;

    fn patch(data: &ParamData, _: &[u32]) -> Result<Self::Patch, PatchError> {
        match data {
            ParamData::F32(v) => Ok(Self::new(*v)),
            _ => Err(PatchError::InvalidData),
        }
    }

    fn apply(&mut self, value: Self::Patch) {
        *self = value;
    }
}

/// A DSP helper struct that efficiently mixes two signals together.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MixDSP {
    gain_0: SmoothedParam,
    gain_1: SmoothedParam,
}

impl MixDSP {
    pub fn new(
        mix: Mix,
        fade_curve: FadeCurve,
        config: SmootherConfig,
        sample_rate: NonZeroU32,
    ) -> Self {
        let (gain_0, gain_1) = mix.compute_gains(fade_curve);

        Self {
            gain_0: SmoothedParam::new(gain_0, config, sample_rate),
            gain_1: SmoothedParam::new(gain_1, config, sample_rate),
        }
    }

    pub fn set_mix(&mut self, mix: Mix, fade_curve: FadeCurve) {
        let (gain_0, gain_1) = mix.compute_gains(fade_curve);

        self.gain_0.set_value(gain_0);
        self.gain_1.set_value(gain_1);
    }

    /// Reset the internal smoothing filter to the current target value.
    pub fn reset_to_target(&mut self) {
        self.gain_0.reset_to_target();
        self.gain_1.reset_to_target();
    }

    pub fn update_sample_rate(&mut self, sample_rate: NonZeroU32) {
        self.gain_0.update_sample_rate(sample_rate);
        self.gain_1.update_sample_rate(sample_rate);
    }

    pub fn is_smoothing(&self) -> bool {
        self.gain_0.is_smoothing() || self.gain_1.is_smoothing()
    }

    pub fn has_settled(&self) -> bool {
        self.gain_0.has_settled() && self.gain_1.has_settled()
    }

    pub fn mix_dry_into_wet_mono(&mut self, dry: &[f32], wet: &mut [f32], frames: usize) {
        self.mix_first_into_second_mono(dry, wet, frames);
    }

    pub fn first_gain_target(&self) -> f32 {
        self.gain_0.target_value()
    }

    pub fn second_gain_target(&self) -> f32 {
        self.gain_1.target_value()
    }

    pub fn mix_dry_into_wet_stereo(
        &mut self,
        dry_l: &[f32],
        dry_r: &[f32],
        wet_l: &mut [f32],
        wet_r: &mut [f32],
        frames: usize,
    ) {
        self.mix_first_into_second_stereo(dry_l, dry_r, wet_l, wet_r, frames);
    }

    pub fn mix_dry_into_wet<VF: AsRef<[f32]>, VS: AsMut<[f32]>>(
        &mut self,
        dry: &[VF],
        wet: &mut [VS],
        dry_range: Range<usize>,
        wet_range: Range<usize>,
        scratch_buffer_0: &mut [f32],
        scratch_buffer_1: &mut [f32],
    ) {
        self.mix_first_into_second(
            dry,
            wet,
            dry_range,
            wet_range,
            scratch_buffer_0,
            scratch_buffer_1,
        );
    }

    pub fn mix_first_into_second_mono(&mut self, first: &[f32], second: &mut [f32], frames: usize) {
        let first = &first[..frames];
        let second = &mut second[..frames];

        if self.is_smoothing() {
            for (first_s, second_s) in first.iter().zip(second.iter_mut()) {
                let gain_first = self.gain_0.next_smoothed();
                let gain_second = self.gain_1.next_smoothed();

                *second_s = first_s * gain_first + *second_s * gain_second;
            }

            self.gain_0.settle();
            self.gain_1.settle();
        } else if self.gain_1.target_value() <= 0.00001 && self.gain_0.target_value() >= 0.99999 {
            // Simply copy first signal to output.
            second.copy_from_slice(first);
        } else if self.gain_0.target_value() <= 0.00001 && self.gain_1.target_value() >= 0.99999 {
            // Signal is already fully second
        } else {
            for (first_s, second_s) in first.iter().zip(second.iter_mut()) {
                *second_s =
                    first_s * self.gain_0.target_value() + *second_s * self.gain_1.target_value();
            }
        }
    }

    pub fn mix_first_into_second_stereo(
        &mut self,
        first_l: &[f32],
        first_r: &[f32],
        second_l: &mut [f32],
        second_r: &mut [f32],
        frames: usize,
    ) {
        let first_l = &first_l[..frames];
        let first_r = &first_r[..frames];
        let second_l = &mut second_l[..frames];
        let second_r = &mut second_r[..frames];

        if self.is_smoothing() {
            for i in 0..frames {
                let gain_0 = self.gain_0.next_smoothed();
                let gain_1 = self.gain_1.next_smoothed();

                second_l[i] = first_l[i] * gain_0 + second_l[i] * gain_1;
                second_r[i] = first_r[i] * gain_0 + second_r[i] * gain_1;
            }

            self.gain_0.settle();
            self.gain_1.settle();
        } else if self.gain_1.target_value() <= 0.00001 && self.gain_0.target_value() >= 0.99999 {
            // Simply copy first signal to output.
            second_l.copy_from_slice(first_l);
            second_r.copy_from_slice(first_r);
        } else if self.gain_0.target_value() <= 0.00001 && self.gain_1.target_value() >= 0.99999 {
            // Signal is already fully second
        } else {
            for i in 0..frames {
                second_l[i] = first_l[i] * self.gain_0.target_value()
                    + second_l[i] * self.gain_1.target_value();
                second_r[i] = first_r[i] * self.gain_0.target_value()
                    + second_r[i] * self.gain_1.target_value();
            }
        }
    }

    pub fn mix_first_into_second<VF: AsRef<[f32]>, VS: AsMut<[f32]>>(
        &mut self,
        first: &[VF],
        second: &mut [VS],
        first_range: Range<usize>,
        second_range: Range<usize>,
        scratch_buffer_0: &mut [f32],
        scratch_buffer_1: &mut [f32],
    ) {
        let frames =
            (first_range.end - first_range.start).min(second_range.end - second_range.start);

        if second.len() == 1 {
            self.mix_first_into_second_mono(
                &first[0].as_ref()[first_range.clone()],
                &mut second[0].as_mut()[second_range.clone()],
                frames,
            );
        } else if second.len() == 2 {
            let (second_l, second_r) = second.split_first_mut().unwrap();
            self.mix_first_into_second_stereo(
                &first[0].as_ref()[first_range.clone()],
                &first[1].as_ref()[first_range.clone()],
                &mut second_l.as_mut()[second_range.clone()],
                &mut second_r[0].as_mut()[second_range.clone()],
                frames,
            );
        } else if self.is_smoothing() {
            self.gain_0
                .process_into_buffer(&mut scratch_buffer_0[..frames]);
            self.gain_1
                .process_into_buffer(&mut scratch_buffer_1[..frames]);

            for (first_ch, second_ch) in first[..second.len()].iter().zip(second.iter_mut()) {
                for (((&first_s, &g0), &g1), second_s) in first_ch.as_ref()
                    [first_range.start..first_range.start + frames]
                    .iter()
                    .zip(scratch_buffer_0[..frames].iter())
                    .zip(scratch_buffer_1[..frames].iter())
                    .zip(
                        second_ch.as_mut()[second_range.start..second_range.start + frames]
                            .iter_mut(),
                    )
                {
                    *second_s = first_s * g0 + *second_s * g1;
                }
            }

            self.gain_0.settle();
            self.gain_1.settle();
        } else if self.gain_1.target_value() <= 0.00001 && self.gain_0.target_value() >= 0.99999 {
            // Simply copy input 0 to output.
            for (first_ch, second_ch) in first[..second.len()].iter().zip(second.iter_mut()) {
                second_ch.as_mut()[second_range.start..second_range.start + frames]
                    .copy_from_slice(
                        &first_ch.as_ref()[first_range.start..first_range.start + frames],
                    );
            }
        } else if self.gain_0.target_value() <= 0.00001 && self.gain_1.target_value() >= 0.99999 {
            // Signal is already fully second
        } else {
            for (first_ch, second_ch) in first[..second.len()].iter().zip(second.iter_mut()) {
                for (&first_s, second_s) in first_ch.as_ref()
                    [first_range.start..first_range.start + frames]
                    .iter()
                    .zip(
                        second_ch.as_mut()[second_range.start..second_range.start + frames]
                            .iter_mut(),
                    )
                {
                    *second_s = first_s * self.gain_0.target_value()
                        + *second_s * self.gain_1.target_value();
                }
            }
        }
    }
}
