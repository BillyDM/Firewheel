use core::{array::from_fn, num::NonZeroU32};

use firewheel_core::{
    channel_config::{ChannelConfig, NonZeroChannelCount},
    diff::{Diff, Notify, Patch},
    dsp::{
        buffer::ChannelBuffer,
        declick::{DeclickFadeCurve, Declicker},
        fade::FadeCurve,
        filter::{
            single_pole_iir::{
                OnePoleIirHPF, OnePoleIirHPFCoeff, OnePoleIirLPF, OnePoleIirLPFCoeff,
            },
            smoothing_filter::DEFAULT_SMOOTH_SECONDS,
        },
        mix::{Mix, MixDSP},
        volume::Volume,
    },
    event::ProcEvents,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers,
        ProcExtra, ProcInfo, ProcessStatus,
    },
    param::smoother::{SmoothedParam, SmootherConfig},
};
use num_traits::Signed;

/// The configuration for an [`EchoNode`]
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EchoNodeConfig {
    /// The maximum amount of samples available per channel
    pub buffer_capacity: usize,
    /// The number of supported channels
    pub channels: NonZeroChannelCount,
}

impl EchoNodeConfig {
    /// Create a configuration that can hold up to a specified number of seconds
    /// of audio
    pub fn new(
        max_duration_seconds: f32,
        sample_rate: impl Into<NonZeroU32>,
        channels: impl Into<NonZeroChannelCount>,
    ) -> Self {
        Self {
            buffer_capacity: (max_duration_seconds * sample_rate.into().get() as f32).ceil()
                as usize,
            channels: channels.into(),
        }
    }
}

impl Default for EchoNodeConfig {
    fn default() -> Self {
        // Assume a common rate, as it cannot be known at compile time
        Self::new(
            5.0,
            NonZeroU32::new(44_100).unwrap(),
            NonZeroChannelCount::STEREO,
        )
    }
}

#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
pub struct EchoNode<const CHANNELS: usize> {
    /// The lowpass frequency in hertz in the range
    /// `[20.0, 20480.0]`.
    pub feedback_lpf: f32,
    /// The highpass frequency in hertz in the range `[20.0, 20480.0]`.
    pub feedback_hpf: f32,
    /// The value representing the mix between the dry and wet audio signals
    ///
    /// This is a normalized value in the range `[0.0, 1.0]`, where `0.0` is
    /// fully the dry signal, `1.0` is fully the wet signal, and `0.5` is an
    /// equal mix of both.
    ///
    /// By default this is set to [`Mix::CENTER`].
    pub mix: Mix,

    /// The delay time, in seconds.
    pub delay_seconds: [f32; CHANNELS],

    /// Feedback amplitude
    pub feedback: [Volume; CHANNELS],

    /// Crossfeed to the other channel. Unused in mono.
    ///
    /// Warning: crossfeed may lead to runaway feedback
    pub crossfeed: [Volume; CHANNELS],

    /// The algorithm used to map the normalized mix value in the range `[0.0,
    /// 1.0]` to the corresponding gain values for the two signals.
    ///
    /// By default this is set to [`FadeCurve::EqualPower3dB`].
    pub fade_curve: FadeCurve,

    /// Adjusts the time in seconds over which parameters are smoothed.
    ///
    /// Defaults to `0.015` (15ms).
    pub smooth_seconds: f32,

    pub stop: Notify<()>,
    pub paused: bool,
}

impl<const CHANNELS: usize> Default for EchoNode<CHANNELS> {
    fn default() -> Self {
        Self {
            feedback_lpf: 6_000.0,
            feedback_hpf: 70.0,
            mix: Mix::CENTER,
            fade_curve: FadeCurve::EqualPower3dB,
            stop: Notify::default(),
            paused: false,
            delay_seconds: [0.5; CHANNELS],
            feedback: [Volume::from_percent(30.0); CHANNELS],
            crossfeed: [Volume::from_percent(0.0); CHANNELS],
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
        }
    }
}

impl<const CHANNELS: usize> EchoNode<CHANNELS> {
    fn smoother_config(&self) -> SmootherConfig {
        SmootherConfig {
            smooth_seconds: self.smooth_seconds,
            ..Default::default()
        }
    }
}

impl<const CHANNELS: usize> AudioNode for EchoNode<CHANNELS> {
    type Configuration = EchoNodeConfig;

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("echo")
            .channel_config(ChannelConfig::new(CHANNELS, CHANNELS))
    }

    fn construct_processor(
        &self,
        config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        let max_frames = cx.stream_info.max_block_frames.get() as usize;
        let sample_rate = cx.stream_info.sample_rate;
        let smoother_config = self.smoother_config();
        Processor::<CHANNELS> {
            params: *self,
            declicker: Declicker::default(),
            delay_seconds_smoothed: self
                .delay_seconds
                .map(|channel| SmoothedParam::new(channel, smoother_config, sample_rate)),
            feedback_smoothed: self
                .feedback
                .map(|channel| SmoothedParam::new(channel.linear(), smoother_config, sample_rate)),
            crossfeed_smoothed: self
                .crossfeed
                .map(|channel| SmoothedParam::new(channel.linear(), smoother_config, sample_rate)),
            delay_seconds_smoothed_buffer: ChannelBuffer::<f32, CHANNELS>::new(max_frames),
            feedback_smoothed_buffer: ChannelBuffer::<f32, CHANNELS>::new(max_frames),
            crossfeed_smoothed_buffer: ChannelBuffer::<f32, CHANNELS>::new(max_frames),
            delay_buffers: from_fn(|_| DelayLine::<f32>::initialized(config.buffer_capacity)),
            mix_dsp: MixDSP::new(
                self.mix,
                self.fade_curve,
                smoother_config,
                cx.stream_info.sample_rate,
            ),
            feedback_lpf: [OnePoleIirLPF::default(); CHANNELS],
            feedback_hpf: [OnePoleIirHPF::default(); CHANNELS],
            prev_delay_seconds: [None; CHANNELS],
            next_delay_seconds: [None; CHANNELS],
            feedback_lpf_smoothed: SmoothedParam::new(
                self.feedback_lpf,
                smoother_config,
                sample_rate,
            ),
            feedback_hpf_smoothed: SmoothedParam::new(
                self.feedback_hpf,
                smoother_config,
                sample_rate,
            ),
        }
    }
}

struct Processor<const CHANNELS: usize> {
    params: EchoNode<CHANNELS>,
    feedback_lpf_smoothed: SmoothedParam,
    feedback_hpf_smoothed: SmoothedParam,
    feedback_lpf: [OnePoleIirLPF; CHANNELS],
    feedback_hpf: [OnePoleIirHPF; CHANNELS],
    mix_dsp: MixDSP,
    declicker: Declicker,
    // Set when transitioning delay seconds. When settled on the new value,
    // it is unset. We need this value to get the amount to mix the two
    // echos of different delay times.
    prev_delay_seconds: [Option<f32>; CHANNELS],
    // Represents the current amount of delay.
    delay_seconds_smoothed: [SmoothedParam; CHANNELS],
    // In order to smoothly mix without phase discontinuity, we must finish
    // mixing completely before moving on to another interpolation. For example,
    // imagine an interpolation is 50% complete before a new delay time is
    // requested. We would need to jump to 0% completion and move to the new
    // position, resulting in a click.
    //
    // To resolve this, the current interpolation will always run to completion
    // before the next requested target is considered, so only two delay lines
    // are mixed at any given time. This parameter is like a queue of a length
    // of 1. The latest request will replace any currently set value.
    //
    // This will be popped into `delay_seconds_smoothed` as the next target value
    // when `delay_seconds_smoothed` has settled.
    next_delay_seconds: [Option<f32>; CHANNELS],
    feedback_smoothed: [SmoothedParam; CHANNELS],
    crossfeed_smoothed: [SmoothedParam; CHANNELS],
    // Should always be the same count as `CHANNELS`
    delay_buffers: [DelayLine<f32>; CHANNELS],
    // We need to calculate all of these buffers at once, so scratch buffers may
    // not be enough depending on channels
    delay_seconds_smoothed_buffer: ChannelBuffer<f32, CHANNELS>,
    feedback_smoothed_buffer: ChannelBuffer<f32, CHANNELS>,
    crossfeed_smoothed_buffer: ChannelBuffer<f32, CHANNELS>,
}

impl<const CHANNELS: usize> AudioNodeProcessor for Processor<CHANNELS> {
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        events: &mut ProcEvents,
        extra: &mut ProcExtra,
    ) -> ProcessStatus {
        const SCRATCH_CHANNELS: usize = 2;
        const LPF_SCRATCH_INDEX: usize = 0;
        const HPF_SCRATCH_INDEX: usize = 1;

        let mut clear_buffers = false;
        for mut patch in events.drain_patches::<EchoNode<CHANNELS>>() {
            match &mut patch {
                EchoNodePatch::SmoothSeconds(seconds) => {
                    // Change all smoothed parameters to new smoothing
                    let update_smoothing = |param: &mut SmoothedParam| {
                        param.set_smooth_seconds(*seconds, info.sample_rate);
                    };
                    self.crossfeed_smoothed
                        .iter_mut()
                        .chain(self.feedback_smoothed.iter_mut())
                        .chain([self.feedback_hpf_smoothed, self.feedback_lpf_smoothed].iter_mut())
                        .chain(self.delay_seconds_smoothed.iter_mut())
                        .for_each(update_smoothing);
                }
                EchoNodePatch::FeedbackLpf(cutoff_hz) => {
                    self.feedback_lpf_smoothed.set_value(*cutoff_hz);
                }
                EchoNodePatch::FeedbackHpf(cutoff_hz) => {
                    self.feedback_hpf_smoothed.set_value(*cutoff_hz);
                }
                EchoNodePatch::Mix(mix) => {
                    self.mix_dsp.set_mix(*mix, self.params.fade_curve);
                }
                EchoNodePatch::DelaySeconds((index, delay_seconds)) => {
                    // TODO: make more robust
                    // Check to see if settled.
                    if self.prev_delay_seconds[*index].is_none() {
                        self.prev_delay_seconds[*index] =
                            Some(self.delay_seconds_smoothed[*index].target_value());
                        self.delay_seconds_smoothed[*index].set_value(*delay_seconds)
                    } else {
                        // If we're still transitioning, queue up the desired change.
                        self.next_delay_seconds[*index] = Some(*delay_seconds);
                    }
                }
                EchoNodePatch::Feedback((index, feedback)) => {
                    self.feedback_smoothed[*index].set_value(feedback.linear())
                }
                EchoNodePatch::Crossfeed((index, crossfeed)) => {
                    self.crossfeed_smoothed[*index].set_value(crossfeed.linear());
                }
                EchoNodePatch::Stop(_) => {
                    clear_buffers = true;
                    self.params.paused = true;
                    self.declicker.fade_to_enabled(false, &extra.declick_values);
                }
                EchoNodePatch::Paused(is_paused) => {
                    self.declicker
                        .fade_to_enabled(!*is_paused, &extra.declick_values);
                }
                EchoNodePatch::FadeCurve(fade_curve) => {
                    self.mix_dsp.set_mix(self.params.mix, *fade_curve);
                }
            }
            self.params.apply(patch);
        }

        if self.params.paused && self.declicker.has_settled() {
            return ProcessStatus::ClearAllOutputs;
        }

        // Zero outputs so that crossfeeds can be added to the output TODO: Is
        // there a more efficient way to do this that avoids clearing the
        // buffer?
        for output in buffers.outputs.iter_mut() {
            output.fill(0.0);
        }

        // Process smoothed values all at the same time

        // Smoothed cutoff values do not have to be calculated per channel.
        // Calculate smoothed filter values
        let mut scratch = extra.scratch_buffers.channels_mut::<SCRATCH_CHANNELS>();
        let scratch: [&mut [&mut [f32]]; 2] = scratch.split_at_mut(1).into();
        let lpf_smoothed = &mut scratch[LPF_SCRATCH_INDEX][0];
        self.feedback_lpf_smoothed.process_into_buffer(lpf_smoothed);

        let hpf_smoothed = &mut scratch[HPF_SCRATCH_INDEX][0];
        self.feedback_hpf_smoothed.process_into_buffer(hpf_smoothed);

        for channel_index in (0..CHANNELS).into_iter() {
            // Queue up delays if applicable
            if self.next_delay_seconds[channel_index].is_some() {
                // If there are no delays in progress...
                if self.prev_delay_seconds[channel_index] == None {
                    // Queue next value
                    self.delay_seconds_smoothed[channel_index]
                        .set_value(self.next_delay_seconds[channel_index].take().unwrap());
                }
            }
            self.delay_seconds_smoothed[channel_index].process_into_buffer(
                self.delay_seconds_smoothed_buffer
                    .channels_mut::<CHANNELS>()[channel_index],
            );
            self.feedback_smoothed[channel_index].process_into_buffer(
                self.feedback_smoothed_buffer.channels_mut::<CHANNELS>()[channel_index],
            );
            self.crossfeed_smoothed[channel_index].process_into_buffer(
                self.crossfeed_smoothed_buffer.channels_mut::<CHANNELS>()[channel_index],
            );
        }

        // The block diagram for this echo effect looks like this. (Declicking
        // has been omitted)
        /*
                              XFeed In
                                  ▼
                                  │
                                  │             ┌─► XFeed Out
                 ┌──────┐         │   ┌─────┐   │
              ┌─►│Filter├────►────┴──►│Delay├───┤
              │  └──────┘ Feedback    └─────┘   │
              │                               ┌─▼─┐
         In ●─┴──────────────────────────────►│Mix├───► Out
                                              └───┘
        */
        // Because we have crossfeed, everything must happen in lockstep. We'll
        // do each step of processing all channels at a time.
        for sample_index in 0..info.frames {
            // First, read delayed samples for all channels
            let delayed_samples: [f32; CHANNELS] = from_fn(|channel_index| {
                // The value of seconds delay that we wish to move towards (or have settled at).
                let next_secs_delay = self.delay_seconds_smoothed[channel_index].target_value();

                // Target delay, in fractional samples. This will act as the final state of our lerp (1.0).
                let mut next_delay_sample = self.delay_buffers[channel_index]
                    .read_delay_seconds(next_secs_delay, info.sample_rate);

                // If we aren't transitioning time, we're done at this point!
                // However, if the delay_seconds_smoothed is still settling,
                // that means we are still transitioning from a previous time
                // selection. We'll need the initial 0.0 state, with the completion of
                // the delay seconds from previous to next position (0.0 to 1.0) as the interpolator.

                if let Some(prev_secs_delay) = self.prev_delay_seconds[channel_index] {
                    // Get the sample that will act as position 0.0 of the interpolation
                    let prev_delay_sample = self.delay_buffers[channel_index]
                        .read_delay_seconds(prev_secs_delay, info.sample_rate);

                    // We can now calculate how much to interpolate, based on the completion of the delay smoother buffer
                    let current_secs_delay =
                        self.delay_seconds_smoothed_buffer.all()[channel_index][sample_index];

                    let denom = next_secs_delay - prev_secs_delay;
                    let interpolation_factor = {
                        match denom <= f32::EPSILON {
                            true => 1.0,
                            false => {
                                // assert!(current_secs_delay <= next_secs_delay);
                                // dbg!(current_secs_delay, prev_secs_delay);
                                // assert!(current_secs_delay >= prev_secs_delay); // this is failing and causing issues
                                ((current_secs_delay - prev_secs_delay) / denom).clamp(0.0, 1.0)
                            }
                        }
                    };

                    next_delay_sample *= interpolation_factor;
                    next_delay_sample += (1.0 - interpolation_factor) * prev_delay_sample;
                }

                if self.delay_seconds_smoothed[channel_index].has_settled()
                    && self.prev_delay_seconds[channel_index].is_some()
                {
                    // A `prev_delay_seconds` existing for this channel signals a delay change. If settled, remove it.
                    self.prev_delay_seconds[channel_index] = None;
                }

                next_delay_sample
            });

            // Process signal to find next samples to feed into the buffer (wet
            // signal)
            let next_buffer_samples: [f32; CHANNELS] = from_fn(|channel_index| {
                let lpf = &mut self.feedback_lpf[channel_index];
                let hpf = &mut self.feedback_hpf[channel_index];

                let input_sample = buffers.inputs[channel_index][sample_index];
                let feedback_sample = delayed_samples[channel_index]
                    * self.feedback_smoothed_buffer.all()[channel_index][sample_index];
                let crossfed_sample = (0..CHANNELS)
                    .filter(|i| i != &channel_index)
                    .map(|i| {
                        delayed_samples[i] * self.crossfeed_smoothed_buffer.all()[i][sample_index]
                    })
                    .sum::<f32>();

                let mut next = input_sample + feedback_sample + crossfed_sample;

                // Change filter coeffs based on smoothed values
                let scratch = extra.scratch_buffers.channels::<SCRATCH_CHANNELS>();

                // Filter samples through high and lowpass filter
                next = lpf.process(
                    next,
                    OnePoleIirLPFCoeff::new(
                        scratch[LPF_SCRATCH_INDEX][sample_index],
                        info.sample_rate_recip as f32,
                    ),
                );
                next = hpf.process(
                    next,
                    OnePoleIirHPFCoeff::new(
                        scratch[HPF_SCRATCH_INDEX][sample_index],
                        info.sample_rate_recip as f32,
                    ),
                );
                next
            });

            for channel_index in 0..CHANNELS {
                self.delay_buffers[channel_index]
                    .write_and_advance(next_buffer_samples[channel_index]);
                buffers.outputs[channel_index][sample_index] = delayed_samples[channel_index];
            }
        }

        // Mix the resultant signal
        match CHANNELS {
            1 => {
                self.mix_dsp.mix_dry_into_wet_mono(
                    buffers.inputs[0],
                    buffers.outputs[0],
                    info.frames,
                );
            }
            2 => {
                let (dry_l, dry_r) = (buffers.inputs[0], buffers.inputs[1]);
                let (wet_l, wet_r) = buffers.outputs.split_at_mut(1);
                self.mix_dsp
                    .mix_dry_into_wet_stereo(dry_l, dry_r, wet_l[0], wet_r[0], info.frames);
            }
            _ => {
                let mut scratch_buffers = extra.scratch_buffers.channels_mut::<2>();
                let (split_a, split_b) = scratch_buffers.split_at_mut(1);
                self.mix_dsp.mix_dry_into_wet(
                    info.frames,
                    buffers.inputs,
                    buffers.outputs,
                    split_a[0],
                    split_b[0],
                );
            }
        }

        // Declick when pausing or stopping

        self.declicker.process(
            buffers.outputs,
            0..info.frames,
            &extra.declick_values,
            1.0,
            DeclickFadeCurve::EqualPower3dB,
        );

        // Clear internal buffers if signaled, such as when stopping
        if clear_buffers && self.declicker.has_settled() {
            for buffer in self.delay_buffers.iter_mut() {
                buffer.buffer.fill(0.0);
            }
        }

        buffers.check_for_silence_on_outputs(f32::EPSILON)
    }
}

#[derive(Clone, Debug)]
pub struct DelayLine<T> {
    buffer: Vec<T>,
    index: usize,
}

impl<T> DelayLine<T> {
    #[inline(always)]
    pub fn write_and_advance(&mut self, value: T) {
        if self.index == self.buffer.len() - 1 {
            self.index = 0;
        } else {
            self.index += 1;
        }
        self.buffer[self.index] = value;
    }

    #[inline(always)]
    pub fn read(&self) -> &T {
        // Buffer is non-zero, so this will never panic.
        &self.buffer[self.index]
    }

    #[inline(always)]
    pub fn read_delay_samples(&self, num_samples_delay: usize) -> &T {
        // Len: 6
        //
        // 0, 1, 2, 3, 4, 5
        //          ^
        //        Index
        let buffer_len = self.buffer.len();

        // Delay: 5 samples
        // 3  2  1  0  5  4  <-- Relative offset
        // 0, 1, 2, 3, 4, 5
        //          ^
        //        Index
        let index = match num_samples_delay > self.index {
            // Wrap
            true => buffer_len - (num_samples_delay - self.index),
            false => self.index - num_samples_delay,
        };

        &self.buffer[index]
    }
}

// TODO: Move to delayline in DSP
impl<T> DelayLine<T>
where
    T: num_traits::Float,
{
    /// Create an initialized delayline filled with zeros
    pub fn initialized(capacity: usize) -> Self {
        Self {
            buffer: vec![T::zero(); capacity],
            index: 0,
        }
    }

    #[inline(always)]
    pub fn read_delay_seconds(&self, seconds: f32, sample_rate: NonZeroU32) -> T {
        // TODO: Ensure seconds is within bounds

        // Seconds will likely be a fractional number
        let samples_delay_frac = seconds * sample_rate.get() as f32;
        let mut index_f = self.index as f32 - samples_delay_frac;

        if index_f.is_negative() {
            // If negative, wrap to the end of the buffer
            index_f = ((self.buffer.len() - 1) as f32) - index_f.abs();
        }

        let index_a = index_f.floor() as usize;
        let mut index_b = index_a + 1;

        // Should only ever reach up to, never greater than length
        if index_b == self.buffer.len() {
            // If setting to len, wrap
            index_b = 0;
        }

        let sample_a = self.buffer[index_a];
        let sample_b = self.buffer[index_b];

        let frac = T::from(index_f - index_f.floor()).unwrap();

        let mix_a = sample_a.mul(T::one() - frac);
        let mix_b = sample_b.mul(frac);

        mix_a + mix_b
    }
}
