use super::{MAX_HZ, MIN_HZ};
use firewheel_core::node::NodeError;
use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    dsp::{
        coeff_update::{CoeffUpdateFactor, CoeffUpdateMask},
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

    /// The time in seconds of the internal smoothing filter.
    ///
    /// By default this is set to `0.023` (23ms). This value is chosen to be
    /// roughly equal to a typical block size of 1024 samples (23 ms) to
    /// eliminate stair-stepping for most games.
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
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
            coeff_update_factor: CoeffUpdateFactor::default(),
        }
    }
}

impl<const CHANNELS: usize> FastLowpassNode<CHANNELS> {
    /// Construct a new `FastLowpassNode` from the given parameters.
    ///
    /// * `cutoff_hz` - The cutoff frequency in hertz in the range `[20.0, 20480.0]`
    pub const fn from_cutoff_hz(cutoff_hz: f32) -> Self {
        Self {
            cutoff_hz,
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
            coeff_update_factor: CoeffUpdateFactor::DEFAULT,
        }
    }
}

impl<const CHANNELS: usize> AudioNode for FastLowpassNode<CHANNELS> {
    type Configuration = EmptyConfig;

    fn info(&self, _config: &Self::Configuration) -> Result<AudioNodeInfo, NodeError> {
        Ok(AudioNodeInfo::new()
            .debug_name("fast_lowpass")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::new(CHANNELS as u32).unwrap(),
                num_outputs: ChannelCount::new(CHANNELS as u32).unwrap(),
            }))
        // TODO: Once the scheduler gets in-place processing support, use
        // in-place processing for this node.
    }

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> Result<impl AudioNodeProcessor, NodeError> {
        let sample_rate_recip = cx.stream_info.sample_rate_recip as f32;

        let cutoff_hz = self.cutoff_hz.clamp(MIN_HZ, MAX_HZ);

        Ok(Processor {
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
            coeff_update_mask: self.coeff_update_factor.mask(),
            cutoff_changed: false,
        })
    }
}

#[cold]
#[inline(never)]
fn calc_coeff<const CHANNELS: usize>(
    cutoff_hz: f32,
    sample_rate_recip: f32,
) -> OnePoleIirLPFCoeffSimd<CHANNELS> {
    OnePoleIirLPFCoeffSimd::splat(OnePoleIirLPFCoeff::new(cutoff_hz, sample_rate_recip))
}

struct Processor<const CHANNELS: usize> {
    filter: OnePoleIirLPFSimd<CHANNELS>,
    coeff: OnePoleIirLPFCoeffSimd<CHANNELS>,

    cutoff_hz: SmoothedParam,
    coeff_update_mask: CoeffUpdateMask,
    cutoff_changed: bool,
}

impl<const CHANNELS: usize> Processor<CHANNELS> {
    fn reset(&mut self) {
        self.cutoff_hz.reset_to_target();
        self.filter.reset();
    }
}

impl<const CHANNELS: usize> AudioNodeProcessor for Processor<CHANNELS> {
    fn events(&mut self, info: &ProcInfo, events: &mut ProcEvents, _extra: &mut ProcExtra) {
        for patch in events.drain_patches::<FastLowpassNode<CHANNELS>>() {
            match patch {
                FastLowpassNodePatch::CutoffHz(cutoff) => {
                    self.cutoff_changed = true;
                    self.cutoff_hz.set_value(cutoff.clamp(MIN_HZ, MAX_HZ));
                }
                FastLowpassNodePatch::SmoothSeconds(seconds) => {
                    self.cutoff_hz.set_smooth_seconds(seconds, info.sample_rate);
                }
                FastLowpassNodePatch::CoeffUpdateFactor(f) => {
                    self.coeff_update_mask = f.mask();
                }
            }
        }
    }

    fn bypassed(&mut self, _bypassed: bool) {
        self.reset();
    }

    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        _extra: &mut ProcExtra,
    ) -> ProcessStatus {
        if info.in_silence_mask.all_channels_silent(CHANNELS) {
            // Outputs will be silent, so no need to process.

            // Reset the smoothers and filters since they don't need to smooth any
            // output.
            self.reset();

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

                // Only recalculate coefficients every 2^coeff_update_factor frames
                if self.coeff_update_mask.do_update(i) {
                    self.coeff = calc_coeff(cutoff_hz, info.sample_rate_recip as f32);
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
                self.coeff =
                    calc_coeff(self.cutoff_hz.target_value(), info.sample_rate_recip as f32);
            }
        } else {
            // The cutoff parameter is not currently smoothing, so we can optimize by
            // only updating the filter coefficients once.
            if self.cutoff_changed {
                self.cutoff_changed = false;
                self.coeff =
                    calc_coeff(self.cutoff_hz.target_value(), info.sample_rate_recip as f32);
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

        ProcessStatus::OutputsModified
    }

    fn new_stream(&mut self, stream_info: &StreamInfo, _context: &mut ProcStreamCtx) {
        self.cutoff_hz.update_sample_rate(stream_info.sample_rate);
        self.coeff = calc_coeff(
            self.cutoff_hz.target_value(),
            stream_info.sample_rate_recip as f32,
        );
    }
}
