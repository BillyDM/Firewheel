use core::f32;

use fft_convolver::FFTConvolver;
use firewheel_core::channel_config::NonZeroChannelCount;
use firewheel_core::collector::ArcGc;
use firewheel_core::node::NodeError;
use firewheel_core::{
    channel_config::ChannelConfig,
    diff::{Diff, Patch},
    dsp::{
        declick::{DeclickFadeCurve, Declicker},
        fade::FadeCurve,
        filter::smoothing_filter::DEFAULT_SMOOTH_SECONDS,
        mix::{Mix, MixDSP},
        volume::{Volume, DEFAULT_AMP_EPSILON},
    },
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcessStatus,
    },
    param::smoother::{SmoothedParam, SmootherConfig},
    sample_resource::SampleResourceF32,
};

/// Node configuration for [`ConvolutionNode`].
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ConvolutionNodeConfig {
    /// The number of channels in this node.
    ///
    /// By default this is set to [`NonZeroChannelCount::STEREO`].
    pub channels: NonZeroChannelCount,

    /// The maximum length of an impulse response in seconds this node can
    /// hold.
    ///
    /// By default this is set to `4.0`.
    pub max_impulse_length_seconds: f64,

    /// Smaller blocks may reduce latency at the cost of increased CPU usage.
    ///
    /// By default this is set to `1024`.
    pub partition_size: usize,
}

/// The default partition size to use with a [`ConvolutionNode`].
///
/// Smaller blocks may reduce latency at the cost of increased CPU usage.
pub const DEFAULT_PARTITION_SIZE: usize = 1024;

impl Default for ConvolutionNodeConfig {
    fn default() -> Self {
        Self {
            channels: NonZeroChannelCount::STEREO,
            max_impulse_length_seconds: 4.0,
            partition_size: DEFAULT_PARTITION_SIZE,
        }
    }
}

/// Imparts characteristics of an [`ImpulseResponse`] to the input signal.
///
/// Convolution is often used to achieve reverb effects, but is more
/// computationally expensive than algorithmic reverb.
#[derive(Patch, Diff, Clone, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ConvolutionNode {
    /// The impulse response to use.
    #[cfg_attr(feature = "bevy_reflect", reflect(ignore))]
    #[cfg_attr(feature = "serde", serde(skip))]
    pub impulse: Option<ArcGc<dyn SampleResourceF32>>,

    /// Pause the convolution processing.
    ///
    /// This prevents a tail from ringing out when you want all sound to
    /// momentarily pause.
    pub pause: bool,

    /// The value representing the mix between the two audio signals
    ///
    /// This is a normalized value in the range `[0.0, 1.0]`, where `0.0` is
    /// fully the first signal, `1.0` is fully the second signal, and `0.5` is
    /// an equal mix of both.
    ///
    /// By default this is set to [`Mix::CENTER`].
    pub mix: Mix,

    /// The algorithm used to map the normalized mix value in the range `[0.0,
    /// 1.0]` to the corresponding gain values for the two signals.
    ///
    /// By default this is set to [`FadeCurve::EqualPower3dB`].
    pub fade_curve: FadeCurve,

    /// The gain applied to the resulting convolved signal.
    ///
    /// Defaults to -20dB to balance the volume increase likely to occur when
    /// convolving audio. Values closer to 1.0 may be very loud.
    pub wet_gain: Volume,

    /// Adjusts the time in seconds over which parameters are smoothed for `mix`
    /// and `wet_gain`.
    ///
    /// Defaults to `0.015` (15ms).
    pub smooth_seconds: f32,
}

impl Default for ConvolutionNode {
    fn default() -> Self {
        Self {
            impulse: None,
            mix: Mix::CENTER,
            fade_curve: FadeCurve::default(),
            wet_gain: Volume::Decibels(-20.0),
            pause: false,
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
        }
    }
}

impl core::fmt::Debug for ConvolutionNode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut f = f.debug_struct("SamplerNode");
        f.field(
            "impulse_len_frames",
            &self.impulse.as_ref().map(|i| i.len_frames()),
        );
        f.field("pause", &self.pause);
        f.field("mix", &self.mix);
        f.field("fade_curve", &self.fade_curve);
        f.field("wet_gain", &self.wet_gain);
        f.field("smooth_seconds", &self.smooth_seconds);
        f.finish()
    }
}

impl AudioNode for ConvolutionNode {
    type Configuration = ConvolutionNodeConfig;

    fn info(&self, config: &Self::Configuration) -> Result<AudioNodeInfo, NodeError> {
        Ok(AudioNodeInfo::new()
            .debug_name("convolution")
            .channel_config(ChannelConfig::new(
                config.channels.get(),
                config.channels.get(),
            )))
    }

    fn construct_processor(
        &self,
        config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> Result<impl AudioNodeProcessor, NodeError> {
        let sample_rate = cx.stream_info.sample_rate;
        let smooth_config = SmootherConfig {
            smooth_seconds: self.smooth_seconds,
            ..Default::default()
        };

        let max_frames: usize =
            (config.max_impulse_length_seconds * (sample_rate.get() as f64)).ceil() as usize;

        // TODO: Ask the creator of `fft-convolver` to add a `with_capacity` method.
        let mut tmp_impulse = vec![0.0; max_frames];
        tmp_impulse[0] = 1.0;

        let mut convolver: Vec<FFTConvolver<f32>> = (0..config.channels.get().get())
            .map(|_| {
                let mut c = FFTConvolver::default();
                // TODO: Ask the creator of `fft-convolver` to add a `with_capacity` method.
                c.init(config.partition_size, &tmp_impulse).unwrap();
                c
            })
            .collect();

        let did_init_first_impulse = if let Some(s) = &self.impulse {
            if s.len_frames() > max_frames as u64 {
                return Err(ImpulseTooLongError {
                    got_len_seconds: s.len_frames() as f64 / cx.stream_info.sample_rate_recip,
                    max_len_seconds: config.max_impulse_length_seconds,
                }
                .into());
            }

            if s.num_channels().get() < config.channels.get().get() as usize {
                // Assume a mono impulse response and set it to all channels.
                let impulse_slice = s.channel(0).unwrap();

                for c in convolver.iter_mut() {
                    c.set_response(impulse_slice).unwrap();
                    c.reset();
                }
            } else {
                for (ch_i, c) in convolver.iter_mut().enumerate() {
                    c.set_response(s.channel(ch_i).unwrap()).unwrap();
                    c.reset();
                }
            }

            true
        } else {
            false
        };

        Ok(ConvolutionProcessor {
            params: self.clone(),
            mix: MixDSP::new(self.mix, self.fade_curve, smooth_config, sample_rate),
            wet_gain_smoothed: SmoothedParam::new(self.wet_gain.amp(), smooth_config, sample_rate),
            wet_declick: Declicker::SettledAt0,
            convolver,
            max_frames,
            did_init_first_impulse,
            has_impulse: did_init_first_impulse,
        })
    }
}

struct ConvolutionProcessor {
    params: ConvolutionNode,
    mix: MixDSP,
    wet_gain_smoothed: SmoothedParam,
    wet_declick: Declicker,
    convolver: Vec<FFTConvolver<f32>>,
    max_frames: usize,
    did_init_first_impulse: bool,
    has_impulse: bool,
}

impl AudioNodeProcessor for ConvolutionProcessor {
    fn process(
        &mut self,
        info: &firewheel_core::node::ProcInfo,
        buffers: firewheel_core::node::ProcBuffers,
        events: &mut firewheel_core::event::ProcEvents,
        extra: &mut firewheel_core::node::ProcExtra,
    ) -> ProcessStatus {
        let mut got_new_impulse = false;

        for patch in events.drain_patches::<ConvolutionNode>() {
            match patch {
                ConvolutionNodePatch::Impulse(_) => {
                    got_new_impulse = true;
                }
                ConvolutionNodePatch::Mix(mix) => {
                    self.mix.set_mix(mix, self.params.fade_curve);
                }
                ConvolutionNodePatch::FadeCurve(curve) => {
                    self.mix.set_mix(self.params.mix, curve);
                }
                ConvolutionNodePatch::WetGain(gain) => {
                    self.wet_gain_smoothed.set_value(gain.amp());
                }
                ConvolutionNodePatch::Pause(pause) => {
                    if self.has_impulse {
                        self.wet_declick
                            .fade_to_enabled(!pause, &extra.declick_values);
                    }
                }
                ConvolutionNodePatch::SmoothSeconds(smooth_seconds) => {
                    self.mix = MixDSP::new(
                        self.params.mix,
                        self.params.fade_curve,
                        SmootherConfig {
                            smooth_seconds,
                            ..Default::default()
                        },
                        info.sample_rate,
                    );
                    self.wet_gain_smoothed
                        .set_smooth_seconds(smooth_seconds, info.sample_rate);
                }
            }

            self.params.apply(patch);
        }

        if got_new_impulse {
            if let Some(s) = &self.params.impulse {
                let sample_len = s.len_frames();
                if sample_len > self.max_frames as u64 {
                    let _ = extra.logger.try_error("Impulse is too long, please increase ConvolutionNodeConfig::max_impulse_len_seconds");
                } else {
                    if s.num_channels().get() < self.convolver.len() {
                        // Assume a mono impulse response and set it to all channels.
                        let impulse_slice = s.channel(0).unwrap();

                        for c in self.convolver.iter_mut() {
                            c.set_response(impulse_slice).unwrap();

                            if !self.did_init_first_impulse {
                                c.reset();
                            }
                        }
                    } else {
                        for (ch_i, c) in self.convolver.iter_mut().enumerate() {
                            c.set_response(s.channel(ch_i).unwrap()).unwrap();

                            if !self.did_init_first_impulse {
                                c.reset();
                            }
                        }
                    }

                    self.did_init_first_impulse = true;
                    self.has_impulse = true;

                    if !self.params.pause {
                        self.wet_declick.fade_to_1(&extra.declick_values);
                    }
                }
            } else {
                self.wet_declick.fade_to_0(&extra.declick_values);
                self.has_impulse = false;
            }
        }

        let wet_output_silent = if self.wet_declick != Declicker::SettledAt0 {
            let mut scratch_buffers = extra.scratch_buffers.all_mut();
            let (wet_gain_buffer, wet_declick_buffer) = scratch_buffers.split_first_mut().unwrap();
            let wet_declick_buffer = &mut wet_declick_buffer[0];

            self.wet_gain_smoothed
                .process_into_buffer(&mut wet_gain_buffer[0..info.frames]);
            self.wet_declick.process_into_gain_buffer(
                &mut wet_declick_buffer[0..info.frames],
                false,
                &extra.declick_values,
                DeclickFadeCurve::EqualPower3dB,
            );

            for ((conv, input), output) in self
                .convolver
                .iter_mut()
                .zip(buffers.inputs.iter())
                .zip(buffers.outputs.iter_mut())
            {
                conv.process(input, output).unwrap();

                for ((out_s, &g1), &g2) in output
                    .iter_mut()
                    .zip(wet_gain_buffer.iter())
                    .zip(wet_declick_buffer.iter())
                {
                    *out_s *= g1 * g2;
                }
            }

            self.wet_gain_smoothed.settle();

            false
        } else {
            self.wet_gain_smoothed.reset_to_target();

            if self.mix.has_settled() {
                let gain = self.mix.first_gain_target();

                if (gain - 1.0).abs() <= DEFAULT_AMP_EPSILON {
                    return ProcessStatus::Bypass;
                }

                for (ch_i, (in_ch, out_ch)) in buffers
                    .inputs
                    .iter()
                    .zip(buffers.outputs.iter_mut())
                    .enumerate()
                {
                    if info.in_silence_mask.is_channel_silent(ch_i) {
                        if !info.out_silence_mask.is_channel_silent(ch_i) {
                            out_ch.fill(0.0);
                        }
                    } else {
                        for (in_s, out_s) in in_ch.iter().zip(out_ch.iter_mut()) {
                            *out_s = *in_s * gain;
                        }
                    }
                }

                return ProcessStatus::outputs_modified_with_silence_mask(info.in_silence_mask);
            } else {
                // Clear the wet output to zeros.
                for (ch_i, ch) in buffers.outputs.iter_mut().enumerate() {
                    if !info.out_silence_mask.is_channel_silent(ch_i) {
                        ch.fill(0.0);
                    }
                }
            }

            true
        };

        let mut scratch_buffers = extra.scratch_buffers.all_mut();
        let (scratch_buffer_0, scratch_buffer_1) = scratch_buffers.split_first_mut().unwrap();
        let scratch_buffer_1 = &mut scratch_buffer_1[0];

        self.mix.mix_dry_into_wet(
            info.frames,
            buffers.inputs,
            buffers.outputs,
            scratch_buffer_0,
            scratch_buffer_1,
        );

        if wet_output_silent {
            ProcessStatus::outputs_modified_with_silence_mask(info.in_silence_mask)
        } else {
            buffers.check_for_silence_on_outputs(DEFAULT_AMP_EPSILON)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpulseTooLongError {
    pub got_len_seconds: f64,
    pub max_len_seconds: f64,
}

impl core::error::Error for ImpulseTooLongError {}

impl core::fmt::Display for ImpulseTooLongError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "Impulse of length {} seconds is longer than Convolver with max length {} seconds. Please increase ConvolutionNodeConfig::max_impulse_len_seconds",
            self.got_len_seconds,
            self.max_len_seconds
        )
    }
}
