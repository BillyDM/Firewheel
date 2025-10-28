use core::f32;

use fft_convolver::FFTConvolver;
use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    collector::OwnedGc,
    diff::{Diff, Patch},
    dsp::{
        declick::{DeclickFadeCurve, Declicker},
        fade::FadeCurve,
        filter::smoothing_filter::DEFAULT_SMOOTH_SECONDS,
        mix::{Mix, MixDSP},
        volume::Volume,
    },
    event::NodeEventType,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcessStatus,
    },
    param::smoother::{SmoothedParam, SmootherConfig},
    sample_resource::SampleResourceF32,
};

/// Imparts characteristics of an [`ImpulseResponse`] to the input signal.
///
/// Convolution is often used to achieve reverb effects, but is more
/// computationally expensive than algorithmic reverb.
#[derive(Clone, Copy, Patch, Diff, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct ConvolutionNode<const CHANNELS: usize> {
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

/// Node configuration for [`ConvolutionNode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
pub struct ConvolutionNodeConfig<const CHANNELS: usize> {
    /// The maximum number of supported IR channels (must be
    /// `ChannelCount::MONO` or `ChannelCount::STEREO`). This determines the
    /// number of buffers allocated. Loading an impulse response with more
    /// channels than supported will result in the remaining channels being
    /// removed.
    pub max_impulse_channel_count: ChannelCount,

    pub partition_size: usize,
}

/// The default partition size to use with a [`ConvolutionNode`].
///
/// Smaller blocks may reduce latency at the cost of increased CPU usage.
pub const DEFAULT_PARTITION_SIZE: usize = 1024;

/// A processed impulse response sample.
///
/// `ImpulseResponse`s are used in [`ConvolutionNode`]s.
pub struct ImpulseResponse(Vec<FFTConvolver<f32>>);

impl ImpulseResponse {
    /// Create a new `ImpulseResponse` with a custom partition size.
    ///
    /// Smaller blocks may reduce latency at the cost of increased CPU usage.
    pub fn new_with_partition_size(sample: impl SampleResourceF32, partition_size: usize) -> Self {
        let num_channels = sample.num_channels().get();
        Self(
            (0..num_channels)
                .map(|channel_index| {
                    let mut conv = FFTConvolver::default();
                    // The sample channel must exist, as our iterator is based
                    // on its length. The FFT may error, depending on several
                    // factors. Currently, this will result in a panic.
                    conv.init(partition_size, sample.channel(channel_index).unwrap())
                        .unwrap();
                    conv
                })
                .collect(),
        )
    }

    /// Create a new `ImpulseResponse` with a default partition size of `1024`.
    pub fn new(sample: impl SampleResourceF32) -> Self {
        Self::new_with_partition_size(sample, DEFAULT_PARTITION_SIZE)
    }
}

impl<const CHANNELS: usize> Default for ConvolutionNodeConfig<CHANNELS> {
    fn default() -> Self {
        Self {
            // A Convolution node with 0 `CHANNELS` is invalid and will panic.
            max_impulse_channel_count: ChannelCount::new(CHANNELS as u32).unwrap(),
            partition_size: DEFAULT_PARTITION_SIZE,
        }
    }
}

impl<const CHANNELS: usize> Default for ConvolutionNode<CHANNELS> {
    fn default() -> Self {
        Self {
            mix: Mix::CENTER,
            fade_curve: FadeCurve::default(),
            wet_gain: Volume::Decibels(-20.0),
            pause: false,
            smooth_seconds: DEFAULT_SMOOTH_SECONDS,
        }
    }
}

impl<const CHANNELS: usize> AudioNode for ConvolutionNode<CHANNELS> {
    type Configuration = ConvolutionNodeConfig<CHANNELS>;

    fn info(&self, _configuration: &Self::Configuration) -> AudioNodeInfo {
        if CHANNELS > 2 {
            panic!(
                "ConvolutionNode::CHANNELS cannot be greater than 2, got {}",
                CHANNELS
            );
        }
        AudioNodeInfo::new()
            .debug_name("convolution")
            .channel_config(ChannelConfig::new(CHANNELS, CHANNELS))
    }

    fn construct_processor(
        &self,
        _configuration: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        let sample_rate = cx.stream_info.sample_rate;
        let smooth_config = SmootherConfig {
            smooth_seconds: self.smooth_seconds,
            ..Default::default()
        };
        ConvolutionProcessor::<CHANNELS> {
            params: self.clone(),
            mix: MixDSP::new(self.mix, self.fade_curve, smooth_config, sample_rate),
            wet_gain_smoothed: SmoothedParam::new(self.wet_gain.amp(), smooth_config, sample_rate),
            declick: Declicker::default(),
            impulse_response: OwnedGc::new(None),
            next_impulse_response: OwnedGc::new(None),
        }
    }
}

pub enum ConvolutionNodeEvent {
    SetImpulseResponse(Option<ImpulseResponse>),
}

struct ConvolutionProcessor<const CHANNELS: usize> {
    params: ConvolutionNode<CHANNELS>,
    mix: MixDSP,
    wet_gain_smoothed: SmoothedParam,
    declick: Declicker,
    impulse_response: OwnedGc<Option<ImpulseResponse>>,
    // We cannot be certain that the transition to a new impulse response will
    // happen within one block, so we must store the old impulse response until
    // the declicker settles.
    next_impulse_response: OwnedGc<Option<ImpulseResponse>>,
}

impl<const CHANNELS: usize> AudioNodeProcessor for ConvolutionProcessor<CHANNELS> {
    fn process(
        &mut self,
        info: &firewheel_core::node::ProcInfo,
        buffers: firewheel_core::node::ProcBuffers,
        events: &mut firewheel_core::event::ProcEvents,
        extra: &mut firewheel_core::node::ProcExtra,
    ) -> ProcessStatus {
        for mut event in events.drain() {
            match event {
                NodeEventType::Param { data, path } => {
                    if let Ok(patch) = ConvolutionNode::<CHANNELS>::patch(&data, &path) {
                        // You can match on the patch directly
                        match patch {
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
                                self.declick.fade_to_enabled(!pause, &extra.declick_values);
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
                }
                NodeEventType::Custom(_) => {
                    if event.downcast_into_owned(&mut self.next_impulse_response) {
                        // Disable the audio stream while changing IRs
                        self.declick.fade_to_0(&extra.declick_values);
                    }
                }
                _ => (),
            }
        }

        // Check to see if there is a new IR waiting. If there is, and the audio
        // has stopped, swap the IR, and continue
        if self.next_impulse_response.is_some() && self.declick == Declicker::SettledAt0 {
            // The next impulse result must exist due to the check in this block
            let next_impulse_response = self.next_impulse_response.take().unwrap();
            self.impulse_response.replace(next_impulse_response);
            // Don't unpause if we're paused manually
            if !self.params.pause {
                self.declick.fade_to_1(&extra.declick_values);
            }
            // Begin mixing back in with the new impulse response next block
            return ProcessStatus::ClearAllOutputs;
        }

        // Only process if an impulse response is supplied
        if self.impulse_response.is_some() {
            const WET_GAIN_BUFFER: usize = 0;
            let wet_gain_buffer = &mut extra.scratch_buffers.channels_mut::<1>()[WET_GAIN_BUFFER];

            // Amount to scale based on wet signal gain
            self.wet_gain_smoothed.process_into_buffer(wet_gain_buffer);

            // If paused, return early after processing wet gain buffers to
            // avoid clicking
            if self.params.pause && self.declick == Declicker::SettledAt0 {
                return ProcessStatus::ClearAllOutputs;
            }

            for (input_index, input) in buffers.inputs.iter().enumerate() {
                // We unfortunately can't add more buffers to the convolution
                // struct, as we don't own it. This means we can't do stereo
                // with a mono impulse response. In this case, we'll just pass
                // the input through if we can't get a channel.

                // We already checked that the impulse response must exist, so
                // we can safely unwrap.
                if let Some(conv) = self
                    .impulse_response
                    .get_mut()
                    .as_mut()
                    .unwrap()
                    .0
                    .get_mut(input_index)
                {
                    conv.process(input, buffers.outputs[input_index]).unwrap();

                    // Apply wet signal gain
                    for (output_sample, gain) in buffers.outputs[input_index]
                        .iter_mut()
                        .zip(wet_gain_buffer.iter())
                    {
                        *output_sample *= gain;
                    }
                }
            }
        }

        if self.impulse_response.is_some() {
            match CHANNELS {
                1 => {
                    self.mix.mix_dry_into_wet_mono(
                        buffers.inputs[0],
                        buffers.outputs[0],
                        info.frames,
                    );
                }
                2 => {
                    let (left, right) = buffers.outputs.split_at_mut(1);
                    self.mix.mix_dry_into_wet_stereo(
                        buffers.inputs[0],
                        buffers.inputs[1],
                        left[0],
                        right[0],
                        info.frames,
                    );
                }
                _ => panic!("Only Mono and Stereo are supported"),
            }
        } else {
            // Pass through audio if no impulse provided
            for (input, output) in buffers.inputs.iter().zip(buffers.outputs.iter_mut()) {
                output.copy_from_slice(input);
            }
        }

        self.declick.process(
            buffers.outputs,
            0..info.frames,
            &extra.declick_values,
            1.0,
            DeclickFadeCurve::EqualPower3dB,
        );

        buffers.check_for_silence_on_outputs(f32::EPSILON)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Behave as expected up to stereo
    #[test]
    fn mono_stereo_ok() {
        ConvolutionNode::<1>::default().info(&ConvolutionNodeConfig::default());
        ConvolutionNode::<2>::default().info(&ConvolutionNodeConfig::default());
    }

    // Error when 3+ channels are requested
    #[test]
    #[should_panic]
    fn fail_above_stereo() {
        ConvolutionNode::<3>::default().info(&ConvolutionNodeConfig::default());
    }
}
