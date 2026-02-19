use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::{
        coeff_update::{CoeffUpdateFactor, CoeffUpdateMask},
        declick::{DeclickFadeCurve, Declicker},
        filter::{
            single_pole_iir::{OnePoleIirLPFCoeff, OnePoleIirLPFCoeffSimd, OnePoleIirLPFSimd},
            smoothing_filter::DEFAULT_SMOOTH_SECONDS,
        },
    },
    event::ProcEvents,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        ProcBuffers, ProcExtra, ProcInfo, ProcStreamCtx, ProcessStatus,
    },
    param::smoother::{SmoothedParam, SmootherConfig},
    StreamInfo,
};

use super::{MAX_HZ, MIN_HZ};

pub type FastLowpassMonoNode = FastLowpassNode<1>;
pub type FastLowpassStereoNode = FastLowpassNode<2>;

/// A simple single-pole IIR lowpass filter that is computationally efficient
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FastLowpassNode<const CHANNELS: usize = 2> {
    /// The cutoff frequency in hertz in the range `[20.0, 20480.0]`.
    pub cutoff_hz: f32,
    /// Whether or not this node is enabled.
    pub enabled: bool,

    /// The time in seconds of the internal smoothing filter.
    ///
    /// By default this is set to `0.015` (15ms).
    pub smooth_seconds: f32,

    /// An exponent representing the rate at which DSP coefficients are
    /// updated when parameters are being smoothed.
    ///
    /// Smaller values will produce less "stair-stepping" artifacts,
    /// but will also consume more CPU.
    ///
    /// The resulting number of frames (samples in a single channel of audio)
    /// that will elapse between each update is calculated as
    /// `2^coeff_update_factor`.
    ///
    /// By default this is set to `5`.
    pub coeff_update_factor: CoeffUpdateFactor,
}

impl<const CHANNELS: usize> Default for FastLowpassNode<CHANNELS> {
    fn default() -> Self {
        Self {
            cutoff_hz: 1_000.0,
            enabled: true,
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
            coeff_update_factor: CoeffUpdateFactor::default(),
        }
    }
}

impl<const CHANNELS: usize> FastLowpassNode<CHANNELS> {
    /// Construct a new `FastLowpassNode` from the given parameters.
    ///
    /// * `cutoff_hz` - The cutoff frequency in hertz in the range `[20.0, 20480.0]`
    /// * `enabled` - Whether or not this node is enabled
    pub const fn from_cutoff_hz(cutoff_hz: f32, enabled: bool) -> Self {
        Self {
            cutoff_hz,
            enabled,
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
            coeff_update_factor: CoeffUpdateFactor::DEFAULT,
        }
    }
}

impl<const CHANNELS: usize> AudioNode for FastLowpassNode<CHANNELS> {
    type Configuration = EmptyConfig;

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("fast_lowpass")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::new(CHANNELS as u32).unwrap(),
                num_outputs: ChannelCount::new(CHANNELS as u32).unwrap(),
            })
    }

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        let sample_rate_recip = cx.stream_info.sample_rate_recip as f32;

        let cutoff_hz = self.cutoff_hz.clamp(MIN_HZ, MAX_HZ);

        Processor {
            filter: OnePoleIirLPFSimd::default(),
            coeff: OnePoleIirLPFCoeffSimd::<CHANNELS>::splat(OnePoleIirLPFCoeff::new(
                cutoff_hz,
                sample_rate_recip,
            )),
            cutoff_hz: SmoothedParam::new(
                cutoff_hz,
                SmootherConfig {
                    smooth_seconds: self.smooth_seconds,
                    ..Default::default()
                },
                cx.stream_info.sample_rate,
            ),
            enable_declicker: Declicker::from_enabled(self.enabled),
            coeff_update_mask: self.coeff_update_factor.mask(),
        }
    }
}

struct Processor<const CHANNELS: usize> {
    filter: OnePoleIirLPFSimd<CHANNELS>,
    coeff: OnePoleIirLPFCoeffSimd<CHANNELS>,

    cutoff_hz: SmoothedParam,
    enable_declicker: Declicker,
    coeff_update_mask: CoeffUpdateMask,
}

impl<const CHANNELS: usize> AudioNodeProcessor for Processor<CHANNELS> {
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        events: &mut ProcEvents,
        extra: &mut ProcExtra,
    ) -> ProcessStatus {
        let mut cutoff_changed = false;

        for patch in events.drain_patches::<FastLowpassNode<CHANNELS>>() {
            match patch {
                FastLowpassNodePatch::CutoffHz(cutoff) => {
                    cutoff_changed = true;
                    self.cutoff_hz.set_value(cutoff.clamp(MIN_HZ, MAX_HZ));
                }
                FastLowpassNodePatch::Enabled(enabled) => {
                    // Tell the declicker to crossfade.
                    self.enable_declicker
                        .fade_to_enabled(enabled, &extra.declick_values);
                }
                FastLowpassNodePatch::SmoothSeconds(seconds) => {
                    self.cutoff_hz.set_smooth_seconds(seconds, info.sample_rate);
                }
                FastLowpassNodePatch::CoeffUpdateFactor(f) => {
                    self.coeff_update_mask = f.mask();
                }
            }
        }

        if self.enable_declicker.disabled() {
            // Disabled. Bypass this node.
            return ProcessStatus::Bypass;
        }

        if info.in_silence_mask.all_channels_silent(CHANNELS) && self.enable_declicker.has_settled()
        {
            // Outputs will be silent, so no need to process.

            // Reset the smoothers and filters since they don't need to smooth any
            // output.
            self.cutoff_hz.reset_to_target();
            self.filter.reset();
            self.enable_declicker.reset_to_target();

            return ProcessStatus::ClearAllOutputs;
        }

        assert!(buffers.inputs.len() == CHANNELS);
        assert!(buffers.outputs.len() == CHANNELS);
        for ch in buffers.inputs.iter() {
            assert!(ch.len() >= info.frames);
        }
        for ch in buffers.outputs.iter() {
            assert!(ch.len() >= info.frames);
        }

        if self.cutoff_hz.is_smoothing() {
            for i in 0..info.frames {
                let cutoff_hz = self.cutoff_hz.next_smoothed();

                // Because recalculating filter coefficients is expensive, a trick like
                // this can be used to only recalculate them every few frames.
                //
                // TODO: use core::hint::cold_path() once that stabilizes
                // TODO: instead, we can extract a function and use #[cold] + #[inline(always)] today
                //
                // TODO: Alternatively, this could be optimized using a lookup table
                if self.coeff_update_mask.do_update(i) {
                    self.coeff = OnePoleIirLPFCoeffSimd::splat(OnePoleIirLPFCoeff::new(
                        cutoff_hz,
                        info.sample_rate_recip as f32,
                    ));
                }

                let s: [f32; CHANNELS] = core::array::from_fn(|ch_i| {
                    // Safety: These bounds have been checked above.
                    unsafe { *buffers.inputs.get_unchecked(ch_i).get_unchecked(i) }
                });

                let out = self.filter.process(s, &self.coeff);

                for ch_i in 0..CHANNELS {
                    // Safety: These bounds have been checked above.
                    unsafe {
                        *buffers.outputs.get_unchecked_mut(ch_i).get_unchecked_mut(i) = out[ch_i];
                    }
                }
            }

            if self.cutoff_hz.settle() {
                self.coeff = OnePoleIirLPFCoeffSimd::splat(OnePoleIirLPFCoeff::new(
                    self.cutoff_hz.target_value(),
                    info.sample_rate_recip as f32,
                ));
            }
        } else {
            // The cutoff parameter is not currently smoothing, so we can optimize by
            // only updating the filter coefficients once.
            if cutoff_changed {
                self.coeff = OnePoleIirLPFCoeffSimd::splat(OnePoleIirLPFCoeff::new(
                    self.cutoff_hz.target_value(),
                    info.sample_rate_recip as f32,
                ));
            }

            for i in 0..info.frames {
                let s: [f32; CHANNELS] = core::array::from_fn(|ch_i| {
                    // Safety: These bounds have been checked above.
                    unsafe { *buffers.inputs.get_unchecked(ch_i).get_unchecked(i) }
                });

                let out = self.filter.process(s, &self.coeff);

                for ch_i in 0..CHANNELS {
                    // Safety: These bounds have been checked above.
                    unsafe {
                        *buffers.outputs.get_unchecked_mut(ch_i).get_unchecked_mut(i) = out[ch_i];
                    }
                }
            }
        }

        // Crossfade between the wet and dry signals to declick enabling/disabling.
        self.enable_declicker.process_crossfade(
            buffers.inputs,
            buffers.outputs,
            info.frames,
            &extra.declick_values,
            DeclickFadeCurve::Linear,
        );

        ProcessStatus::OutputsModified
    }

    fn new_stream(&mut self, stream_info: &StreamInfo, _context: &mut ProcStreamCtx) {
        self.cutoff_hz.update_sample_rate(stream_info.sample_rate);
        self.coeff = OnePoleIirLPFCoeffSimd::splat(OnePoleIirLPFCoeff::new(
            self.cutoff_hz.target_value(),
            stream_info.sample_rate_recip as f32,
        ));
    }
}
