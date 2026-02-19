#[cfg(not(feature = "std"))]
use num_traits::Float;

use core::f32::consts::FRAC_PI_2;
use core::{num::NonZeroU32, ops::Range};

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::Vec;

use crate::dsp::filter::smoothing_filter::{SmoothingFilter, SmoothingFilterCoeff};

/// A struct used to declick audio signals using crossfading.
///
/// This approach is more SIMD-friendly than using a smoothing filter
/// or incrementing a gain value per-sample.
///
/// Used in conjunction with [`DeclickValues`].
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Declicker {
    SettledAt0,
    #[default]
    SettledAt1,
    FadingTo0 {
        frames_left: usize,
    },
    FadingTo1 {
        frames_left: usize,
    },
}

impl Declicker {
    pub fn from_enabled(enabled: bool) -> Self {
        if enabled {
            Self::SettledAt1
        } else {
            Self::SettledAt0
        }
    }

    pub fn has_settled(&self) -> bool {
        *self == Self::SettledAt0 || *self == Self::SettledAt1
    }

    pub fn disabled(&self) -> bool {
        *self == Self::SettledAt0
    }

    pub fn fade_to_enabled(&mut self, enabled: bool, declick_values: &DeclickValues) {
        if enabled {
            self.fade_to_1(declick_values);
        } else {
            self.fade_to_0(declick_values);
        }
    }

    pub fn fade_to_0(&mut self, declick_values: &DeclickValues) {
        match self {
            Self::SettledAt1 => {
                *self = Self::FadingTo0 {
                    frames_left: declick_values.frames(),
                }
            }
            Self::FadingTo1 { frames_left } => {
                let frames_left = if *frames_left <= declick_values.frames() {
                    declick_values.frames() - *frames_left
                } else {
                    declick_values.frames()
                };

                *self = Self::FadingTo0 { frames_left }
            }
            _ => {}
        }
    }

    pub fn fade_to_1(&mut self, declick_values: &DeclickValues) {
        match self {
            Self::SettledAt0 => {
                *self = Self::FadingTo1 {
                    frames_left: declick_values.frames(),
                }
            }
            Self::FadingTo0 { frames_left } => {
                let frames_left = if *frames_left <= declick_values.frames() {
                    declick_values.frames() - *frames_left
                } else {
                    declick_values.frames()
                };

                *self = Self::FadingTo1 { frames_left }
            }
            _ => {}
        }
    }

    /// Reset to the current target value.
    pub fn reset_to_target(&mut self) {
        *self = match &self {
            Self::FadingTo0 { .. } => Self::SettledAt0,
            Self::FadingTo1 { .. } => Self::SettledAt1,
            s => **s,
        }
    }

    pub fn reset_to_0(&mut self) {
        *self = Self::SettledAt0;
    }

    pub fn reset_to_1(&mut self) {
        *self = Self::SettledAt1;
    }

    /// Crossfade between the two buffers, where [`Declicker::SettledAt0`] is fully
    /// `buffers_a`, and [`Declicker::SettledAt1`] is fully `buffers_b`.
    pub fn process_crossfade<VA: AsRef<[f32]>, VB: AsMut<[f32]>>(
        &mut self,
        buffers_a: &[VA],
        buffers_b: &mut [VB],
        frames: usize,
        declick_values: &DeclickValues,
        fade_curve: DeclickFadeCurve,
    ) {
        let mut crossfade_buffers =
            |declick_frames_left: &mut usize, values_a: &[f32], values_b: &[f32]| -> usize {
                let process_frames = frames.min(*declick_frames_left);

                let values_start = values_a.len() - *declick_frames_left;
                let values_a = &values_a[values_start..values_start + process_frames];
                let values_b = &values_b[values_start..values_start + process_frames];

                for (ch_a, ch_b) in buffers_a.iter().zip(buffers_b.iter_mut()) {
                    let slice_a = &ch_a.as_ref()[..process_frames];
                    let slice_b = &mut ch_b.as_mut()[..process_frames];

                    for i in 0..process_frames {
                        slice_b[i] = (slice_a[i] * values_a[i]) + (slice_b[i] * values_b[i]);
                    }
                }

                *declick_frames_left -= process_frames;

                process_frames
            };

        match self {
            Self::SettledAt0 => {
                for (ch_a, ch_b) in buffers_a.iter().zip(buffers_b.iter_mut()) {
                    let slice_a = &ch_a.as_ref()[..frames];
                    let slice_b = &mut ch_b.as_mut()[..frames];

                    slice_b.copy_from_slice(slice_a);
                }
            }
            Self::FadingTo0 { frames_left } => {
                let (values_a, values_b) = match fade_curve {
                    DeclickFadeCurve::Linear => (
                        &declick_values.linear_0_to_1_values,
                        &declick_values.linear_1_to_0_values,
                    ),
                    DeclickFadeCurve::EqualPower3dB => (
                        &declick_values.circular_0_to_1_values,
                        &declick_values.circular_1_to_0_values,
                    ),
                };

                let frames_processed = crossfade_buffers(frames_left, values_a, &values_b);

                if frames_processed < frames {
                    for (ch_a, ch_b) in buffers_a.iter().zip(buffers_b.iter_mut()) {
                        let slice_a = &ch_a.as_ref()[frames_processed..frames];
                        let slice_b = &mut ch_b.as_mut()[frames_processed..frames];

                        slice_b.copy_from_slice(slice_a);
                    }
                }

                if *frames_left == 0 {
                    *self = Self::SettledAt0;
                }
            }
            Self::FadingTo1 { frames_left } => {
                let (values_a, values_b) = match fade_curve {
                    DeclickFadeCurve::Linear => (
                        &declick_values.linear_1_to_0_values,
                        &declick_values.linear_0_to_1_values,
                    ),
                    DeclickFadeCurve::EqualPower3dB => (
                        &declick_values.circular_1_to_0_values,
                        &declick_values.circular_0_to_1_values,
                    ),
                };

                crossfade_buffers(frames_left, values_a, values_b);

                if *frames_left == 0 {
                    *self = Self::SettledAt1;
                }
            }
            _ => {}
        }
    }

    pub fn process<V: AsMut<[f32]>>(
        &mut self,
        buffers: &mut [V],
        range_in_buffer: Range<usize>,
        declick_values: &DeclickValues,
        gain: f32,
        fade_curve: DeclickFadeCurve,
    ) {
        let mut fade_buffers = |declick_frames_left: &mut usize, values: &[f32]| -> usize {
            let buffer_frames = range_in_buffer.end - range_in_buffer.start;
            let process_frames = buffer_frames.min(*declick_frames_left);
            let start_frame = values.len() - *declick_frames_left;

            if gain == 1.0 {
                for b in buffers.iter_mut() {
                    let b = &mut b.as_mut()
                        [range_in_buffer.start..range_in_buffer.start + process_frames];

                    for (s, &g) in b
                        .iter_mut()
                        .zip(values[start_frame..start_frame + process_frames].iter())
                    {
                        *s *= g;
                    }
                }
            } else {
                for b in buffers.iter_mut() {
                    let b = &mut b.as_mut()
                        [range_in_buffer.start..range_in_buffer.start + process_frames];

                    for (s, &g) in b
                        .iter_mut()
                        .zip(values[start_frame..start_frame + process_frames].iter())
                    {
                        *s *= g * gain;
                    }
                }
            }

            *declick_frames_left -= process_frames;

            process_frames
        };

        match self {
            Self::SettledAt0 => {
                for b in buffers.iter_mut() {
                    let b = &mut b.as_mut();
                    b[range_in_buffer.clone()].fill(0.0);
                }
            }
            Self::FadingTo0 { frames_left } => {
                let values = match fade_curve {
                    DeclickFadeCurve::Linear => &declick_values.linear_1_to_0_values,
                    DeclickFadeCurve::EqualPower3dB => &declick_values.circular_1_to_0_values,
                };

                let frames_processed = fade_buffers(frames_left, values);

                if frames_processed < range_in_buffer.end - range_in_buffer.start {
                    for b in buffers.iter_mut() {
                        let b = &mut b.as_mut()
                            [range_in_buffer.start + frames_processed..range_in_buffer.end];
                        b.fill(0.0);
                    }
                }

                if *frames_left == 0 {
                    *self = Self::SettledAt0;
                }
            }
            Self::FadingTo1 { frames_left } => {
                let values = match fade_curve {
                    DeclickFadeCurve::Linear => &declick_values.linear_0_to_1_values,
                    DeclickFadeCurve::EqualPower3dB => &declick_values.circular_0_to_1_values,
                };

                let frames_processed = fade_buffers(frames_left, values);

                if frames_processed < range_in_buffer.end - range_in_buffer.start && gain != 1.0 {
                    for b in buffers.iter_mut() {
                        let b = &mut b.as_mut()
                            [range_in_buffer.start + frames_processed..range_in_buffer.end];
                        for s in b.iter_mut() {
                            *s *= gain;
                        }
                    }
                }

                if *frames_left == 0 {
                    *self = Self::SettledAt1;
                }
            }
            _ => {}
        }
    }

    pub fn trending_towards_zero(&self) -> bool {
        match self {
            Declicker::SettledAt0 | Declicker::FadingTo0 { .. } => true,
            _ => false,
        }
    }

    pub fn trending_towards_one(&self) -> bool {
        match self {
            Declicker::SettledAt1 | Declicker::FadingTo1 { .. } => true,
            _ => false,
        }
    }

    pub fn frames_left(&self) -> usize {
        match *self {
            Declicker::FadingTo0 { frames_left } => frames_left,
            Declicker::FadingTo1 { frames_left } => frames_left,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DeclickFadeCurve {
    /// Linear fade.
    Linear,
    /// Equal power fade (circular).
    EqualPower3dB,
}

/// A buffer of values that linearly ramp up/down between `0.0` and `1.0`.
///
/// This approach is more SIMD-friendly than using a smoothing filter or
/// incrementing the gain per-sample.
pub struct DeclickValues {
    pub linear_0_to_1_values: Vec<f32>,
    pub linear_1_to_0_values: Vec<f32>,
    pub circular_0_to_1_values: Vec<f32>,
    pub circular_1_to_0_values: Vec<f32>,
}

impl DeclickValues {
    pub const DEFAULT_FADE_SECONDS: f32 = 10.0 / 1_000.0;

    pub fn new(frames: NonZeroU32) -> Self {
        let frames = frames.get() as usize;
        let frames_recip = (frames as f32).recip();

        let mut linear_0_to_1_values = Vec::new();
        let mut linear_1_to_0_values = Vec::new();
        let mut circular_0_to_1_values = Vec::new();
        let mut circular_1_to_0_values = Vec::new();

        linear_0_to_1_values.reserve_exact(frames);
        linear_1_to_0_values.reserve_exact(frames);
        circular_0_to_1_values.reserve_exact(frames);
        circular_1_to_0_values.reserve_exact(frames);

        linear_0_to_1_values = (0..frames).map(|i| i as f32 * frames_recip).collect();
        linear_1_to_0_values = (0..frames).rev().map(|i| i as f32 * frames_recip).collect();

        circular_0_to_1_values = linear_0_to_1_values
            .iter()
            .map(|x| (x * FRAC_PI_2).sin())
            .collect();
        circular_1_to_0_values = circular_0_to_1_values.iter().rev().copied().collect();

        Self {
            linear_0_to_1_values,
            linear_1_to_0_values,
            circular_0_to_1_values,
            circular_1_to_0_values,
        }
    }

    pub fn frames(&self) -> usize {
        self.linear_0_to_1_values.len()
    }
}

/// A struct used to declick audio signals using a lowpass filter.
///
/// Note, this method of declicking does not have as good quality or
/// performance as the crossfading method used by [`Declicker`]. But
/// this can be used in situations where crossfading two signals is
/// infeasible or too expensive.
#[derive(Debug)]
pub struct LowpassDeclicker<const MAX_CHANNELS: usize> {
    filters: [SmoothingFilter; MAX_CHANNELS],
    coeff: SmoothingFilterCoeff,
    smooth_secs: f32,
    smooth_frames: usize,
    smooth_frames_recip: f32,
    frames_left: usize,
}

impl<const MAX_CHANNELS: usize> LowpassDeclicker<MAX_CHANNELS> {
    pub fn new(sample_rate: NonZeroU32, smooth_secs: f32) -> Self {
        let smooth_frames = ((smooth_secs * sample_rate.get() as f32).round() as usize).max(1);
        let smooth_frames_recip = (smooth_frames as f32).recip();

        Self {
            filters: [SmoothingFilter::new(0.0); MAX_CHANNELS],
            coeff: SmoothingFilterCoeff::new(sample_rate, smooth_secs),
            smooth_secs,
            smooth_frames,
            smooth_frames_recip,
            frames_left: 0,
        }
    }

    pub fn is_declicking(&self) -> bool {
        self.frames_left > 0
    }

    pub fn update_sample_rate(&mut self, sample_rate: NonZeroU32) {
        self.coeff = SmoothingFilterCoeff::new(sample_rate, self.smooth_secs);
        self.smooth_frames =
            ((self.smooth_secs * sample_rate.get() as f32).round() as usize).max(1);
        self.smooth_frames_recip = (self.smooth_frames as f32).recip();
    }

    pub fn begin(&mut self) {
        self.frames_left = self.smooth_frames;
    }

    pub fn reset(&mut self) {
        self.frames_left = 0;
    }

    pub fn process<V: AsMut<[f32]>>(&mut self, buffers: &mut [V], frames: usize) {
        if frames == 0 {
            return;
        }

        if self.frames_left == 0 {
            for (buf, f) in buffers.iter_mut().zip(self.filters.iter_mut()) {
                f.z1 = buf.as_mut()[frames - 1];
            }

            return;
        }

        let proc_frames = self.frames_left.min(frames);

        if buffers.len().min(MAX_CHANNELS) == 2 {
            // Provide an optimized loop for stereo.

            let (buf_l, buf_r) = buffers.split_first_mut().unwrap();
            let buf_l = &mut buf_l.as_mut()[..proc_frames];
            let buf_r = &mut buf_r[0].as_mut()[..proc_frames];

            let (f_l, f_r) = self.filters.split_first_mut().unwrap();
            let f_r = &mut f_r[0];

            for i in 0..proc_frames {
                let filtered_l = f_l.process(buf_l[i], self.coeff);
                let filtered_r = f_r.process(buf_r[i], self.coeff);

                let filtered_mix = (self.frames_left - i) as f32 * self.smooth_frames_recip;

                buf_l[i] = (filtered_l * filtered_mix) + (buf_l[i] * (1.0 - filtered_mix));
                buf_r[i] = (filtered_r * filtered_mix) + (buf_r[i] * (1.0 - filtered_mix));
            }
        } else {
            for (buf, f) in buffers.iter_mut().zip(self.filters.iter_mut()) {
                for (i, s) in buf.as_mut()[..proc_frames].iter_mut().enumerate() {
                    let filtered_s = f.process(*s, self.coeff);

                    let filtered_mix = (self.frames_left - i) as f32 * self.smooth_frames_recip;

                    *s = (filtered_s * filtered_mix) + (*s * (1.0 - filtered_mix));
                }
            }
        }

        self.frames_left -= proc_frames;
    }
}
