use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::{
        coeff_update::{CoeffUpdateFactor, CoeffUpdateMask},
        declick::{DeclickFadeCurve, Declicker},
        filter::{
            single_pole_iir::{
                OnePoleIirHPFCoeff, OnePoleIirHPFCoeffSimd, OnePoleIirHPFSimd, OnePoleIirLPFCoeff,
                OnePoleIirLPFCoeffSimd, OnePoleIirLPFSimd,
            },
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

pub type FastBandpassMonoNode = FastBandpassNode<1>;
pub type FastBandpassStereoNode = FastBandpassNode<2>;

/// A simple single-pole IIR bandpass filter node that is computationally
/// efficient
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FastBandpassNode<const CHANNELS: usize = 2> {
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

impl<const CHANNELS: usize> Default for FastBandpassNode<CHANNELS> {
    fn default() -> Self {
        Self {
            cutoff_hz: 1_000.0,
            enabled: true,
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
            coeff_update_factor: CoeffUpdateFactor::default(),
        }
    }
}

impl<const CHANNELS: usize> FastBandpassNode<CHANNELS> {
    /// Construct a new `FastBandpassNode` from the given parameters.
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

impl<const CHANNELS: usize> AudioNode for FastBandpassNode<CHANNELS> {
    type Configuration = EmptyConfig;

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("fast_bandpass")
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
            lpf: OnePoleIirLPFSimd::default(),
            lpf_coeff: OnePoleIirLPFCoeffSimd::<CHANNELS>::splat(OnePoleIirLPFCoeff::new(
                cutoff_hz,
                sample_rate_recip,
            )),
            hpf: OnePoleIirHPFSimd::default(),
            hpf_coeff: OnePoleIirHPFCoeffSimd::<CHANNELS>::splat(OnePoleIirHPFCoeff::new(
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
    lpf: OnePoleIirLPFSimd<CHANNELS>,
    hpf: OnePoleIirHPFSimd<CHANNELS>,
    lpf_coeff: OnePoleIirLPFCoeffSimd<CHANNELS>,
    hpf_coeff: OnePoleIirHPFCoeffSimd<CHANNELS>,

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

        for patch in events.drain_patches::<FastBandpassNode<CHANNELS>>() {
            match patch {
                FastBandpassNodePatch::CutoffHz(cutoff) => {
                    cutoff_changed = true;
                    self.cutoff_hz.set_value(cutoff.clamp(MIN_HZ, MAX_HZ));
                }
                FastBandpassNodePatch::Enabled(enabled) => {
                    // Tell the declicker to crossfade.
                    self.enable_declicker
                        .fade_to_enabled(enabled, &extra.declick_values);
                }
                FastBandpassNodePatch::SmoothSeconds(seconds) => {
                    self.cutoff_hz.set_smooth_seconds(seconds, info.sample_rate);
                }
                FastBandpassNodePatch::CoeffUpdateFactor(f) => {
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
            self.lpf.reset();
            self.hpf.reset();
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
                //
                // TODO: Alternatively, this could be optimized using a lookup table
                if self.coeff_update_mask.do_update(i) {
                    self.lpf_coeff = OnePoleIirLPFCoeffSimd::splat(OnePoleIirLPFCoeff::new(
                        cutoff_hz,
                        info.sample_rate_recip as f32,
                    ));
                    self.hpf_coeff = OnePoleIirHPFCoeffSimd::splat(OnePoleIirHPFCoeff::new(
                        cutoff_hz,
                        info.sample_rate_recip as f32,
                    ));
                }

                let s: [f32; CHANNELS] = core::array::from_fn(|ch_i| {
                    // Safety: These bounds have been checked above.
                    unsafe { *buffers.inputs.get_unchecked(ch_i).get_unchecked(i) }
                });

                let out = self.lpf.process(s, &self.lpf_coeff);
                let out = self.hpf.process(out, &self.hpf_coeff);

                for ch_i in 0..CHANNELS {
                    // Safety: These bounds have been checked above.
                    unsafe {
                        *buffers.outputs.get_unchecked_mut(ch_i).get_unchecked_mut(i) = out[ch_i];
                    }
                }
            }

            if self.cutoff_hz.settle() {
                self.lpf_coeff = OnePoleIirLPFCoeffSimd::splat(OnePoleIirLPFCoeff::new(
                    self.cutoff_hz.target_value(),
                    info.sample_rate_recip as f32,
                ));
                self.hpf_coeff = OnePoleIirHPFCoeffSimd::splat(OnePoleIirHPFCoeff::new(
                    self.cutoff_hz.target_value(),
                    info.sample_rate_recip as f32,
                ));
            }
        } else {
            // The cutoff parameter is not currently smoothing, so we can optimize by
            // only updating the filter coefficients once.
            if cutoff_changed {
                self.lpf_coeff = OnePoleIirLPFCoeffSimd::splat(OnePoleIirLPFCoeff::new(
                    self.cutoff_hz.target_value(),
                    info.sample_rate_recip as f32,
                ));
                self.hpf_coeff = OnePoleIirHPFCoeffSimd::splat(OnePoleIirHPFCoeff::new(
                    self.cutoff_hz.target_value(),
                    info.sample_rate_recip as f32,
                ));
            }

            for i in 0..info.frames {
                let s: [f32; CHANNELS] = core::array::from_fn(|ch_i| {
                    // Safety: These bounds have been checked above.
                    unsafe { *buffers.inputs.get_unchecked(ch_i).get_unchecked(i) }
                });

                let out = self.lpf.process(s, &self.lpf_coeff);
                let out = self.hpf.process(out, &self.hpf_coeff);

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
        self.lpf_coeff = OnePoleIirLPFCoeffSimd::splat(OnePoleIirLPFCoeff::new(
            self.cutoff_hz.target_value(),
            stream_info.sample_rate_recip as f32,
        ));
        self.hpf_coeff = OnePoleIirHPFCoeffSimd::splat(OnePoleIirHPFCoeff::new(
            self.cutoff_hz.target_value(),
            stream_info.sample_rate_recip as f32,
        ));
    }
}
