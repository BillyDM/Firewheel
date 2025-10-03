use std::f32;

use fft_convolver::FFTConvolver;
use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    collector::ArcGc,
    diff::{Diff, Patch},
    dsp::{
        buffer::ChannelBuffer,
        declick::{DeclickFadeCurve, Declicker},
        fade::FadeCurve,
        mix::{Mix, MixDSP},
        volume::Volume,
    },
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcessStatus,
    },
    param::smoother::{SmoothedParam, SmootherConfig},
    sample_resource::SampleResourceF32,
};

/// Convolves inputs with a supplied impulse response, often used for reverb
/// effects.
#[derive(Diff, Patch, Clone, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
pub struct ConvolutionNode<const CHANNELS: usize> {
    /// Defaults to true. When true, the node is enabled and will convolve
    /// audio. When false, the state of the convolver will be paused and can be
    /// later resumed. This is useful for applications such as pausing a
    /// convolved sound during a game pause menu.
    pub paused: bool,
    /// The wet/dry mix.
    pub mix: Mix,
    pub fade_curve: FadeCurve,
    /// The impulse response.
    pub impulse_response: Option<ArcGc<dyn SampleResourceF32>>,
    /// Defaults to -20dB. The wet signal can potentially be much louder than
    /// the dry input, resulting in the wet signal overwhelming the mix early
    /// on. For this reason, it is best to attenuate. Values closer to 1.0 may
    /// be very loud.
    pub wet_gain: Volume,
}

/// Node configuration for [`ConvolutionNode`].
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
pub struct ConvolutionNodeConfig<const CHANNELS: usize> {
    /// The maximum number of supported IR channels (must be
    /// `ChannelCount::MONO` or `ChannelCount::STEREO`). This determines the
    /// number of buffers allocated. Loading an impulse response with more
    /// channels than supported will result in the remaining channels being
    /// removed.
    pub max_impulse_channel_count: ChannelCount,
}

impl<const CHANNELS: usize> Default for ConvolutionNodeConfig<CHANNELS> {
    fn default() -> Self {
        Self {
            max_impulse_channel_count: ChannelCount::STEREO,
        }
    }
}

impl<const CHANNELS: usize> Default for ConvolutionNode<CHANNELS> {
    fn default() -> Self {
        Self {
            mix: Mix::CENTER,
            fade_curve: FadeCurve::default(),
            impulse_response: None,
            wet_gain: Volume::Decibels(-20.0),
            paused: false,
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
        configuration: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        let convolvers: Vec<FFTConvolver<f32>> = Vec::from_iter({
            // Determine how many convolution buffers are needed
            let max_impulse_channels = configuration.max_impulse_channel_count.get() as usize;

            // Create a separate convolver buffer for each channel of the IR
            // sample. FFTConvolver does not implement `Clone` or `Copy`,
            // preventing usual `vec![]` initialization
            (0..max_impulse_channels)
                .map(|_| FFTConvolver::default())
                .collect::<Vec<_>>()
        });

        let block_frames = cx.stream_info.max_block_frames.get() as usize;
        ConvolutionProcessor::<CHANNELS> {
            params: self.clone(),
            // Response samples must be n-1 samples maximum to fit within the
            // given tail buffer.
            convolvers,
            mix: MixDSP::new(
                self.mix,
                self.fade_curve,
                SmootherConfig::default(),
                cx.stream_info.sample_rate,
            ),
            input_buffers: ChannelBuffer::new(block_frames),
            wet_gain_smoothed: SmoothedParam::new(
                self.wet_gain.amp(),
                Default::default(),
                cx.stream_info.sample_rate,
            ),
            wet_gain_buffer: vec![0.0; block_frames],
            declick: Declicker::default(),
        }
    }
}

struct ConvolutionProcessor<const CHANNELS: usize> {
    params: ConvolutionNode<CHANNELS>,
    convolvers: Vec<fft_convolver::FFTConvolver<f32>>,
    // Convolution needs a block to process, therefore we must store each input
    // buffer to use the following loop
    input_buffers: ChannelBuffer<f32, CHANNELS>,
    mix: MixDSP,
    wet_gain_smoothed: SmoothedParam,
    wet_gain_buffer: Vec<f32>,
    declick: Declicker,
}

impl<const CHANNELS: usize> AudioNodeProcessor for ConvolutionProcessor<CHANNELS> {
    fn process(
        &mut self,
        info: &firewheel_core::node::ProcInfo,
        buffers: firewheel_core::node::ProcBuffers,
        events: &mut firewheel_core::event::ProcEvents,
        extra: &mut firewheel_core::node::ProcExtra,
    ) -> ProcessStatus {
        // Determines if processing will pause next block
        let mut will_pause = false;
        for patch in events.drain_patches::<ConvolutionNode<CHANNELS>>() {
            match patch {
                ConvolutionNodePatch::Mix(mix) => {
                    self.mix.set_mix(mix, self.params.fade_curve);
                }
                ConvolutionNodePatch::FadeCurve(curve) => {
                    self.mix.set_mix(self.params.mix, curve);
                }
                ConvolutionNodePatch::ImpulseResponse(impulse_response) => {
                    self.params.impulse_response = impulse_response;

                    if let Some(impulse_response) = self.params.impulse_response.as_ref() {
                        // Initialize convolution buffers, depending on the
                        // count of channels in the currently loaded IR. There
                        // will be at least as many buffers as `CHANNEL`s, even
                        // if the loaded IR has less. Limit IR channels to the
                        // maximum channels of the node to handle stereo IR with
                        // mono inputs.
                        let ir_num_channels: usize =
                            impulse_response.num_channels().get().min(CHANNELS);
                        for ir_channel_id in 0..(ir_num_channels).max(CHANNELS) {
                            self.convolvers[ir_channel_id]
                                .init(
                                    info.frames,
                                    impulse_response
                                        .channel(ir_channel_id as usize)
                                        // If the desired channel doesn't exist
                                        // (i.e., a stereo node with a mono IR),
                                        // fallback to channel 0.
                                        .unwrap_or_else(|| impulse_response.channel(0).unwrap()),
                                )
                                .unwrap();
                        }
                    }
                }
                ConvolutionNodePatch::WetGain(gain) => {
                    self.wet_gain_smoothed.set_value(gain.amp());
                }
                ConvolutionNodePatch::Paused(paused) => {
                    // Immediately remove pause and start processing again if playing. Otherwise,
                    // save the value for the end of the processing block, and finish the current block when pausing
                    if !paused {
                        self.params.paused = false;
                    } else {
                        will_pause = true;
                    }
                    self.declick.fade_to_enabled(!paused, &extra.declick_values);
                }
            }
        }

        if self.params.paused {
            return ProcessStatus::ClearAllOutputs;
        }

        // Bypass if no impulse response is supplied
        if self.params.impulse_response.is_none() {
            return ProcessStatus::Bypass;
        }

        if !self.params.paused {
            // Amount to scale based on wet signal gain
            self.wet_gain_smoothed
                .process_into_buffer(&mut self.wet_gain_buffer);

            for (input_index, input) in buffers.inputs.iter().enumerate() {
                self.convolvers[input_index]
                    .process(input, buffers.outputs[input_index])
                    .unwrap();

                // Apply wet signal gain
                for (output_sample, gain) in buffers.outputs[input_index]
                    .iter_mut()
                    .zip(self.wet_gain_buffer.iter())
                {
                    *output_sample *= gain;
                }
            }

            match CHANNELS {
                // Use the stored buffers to mix back into the signal a block later
                1 => {
                    self.mix.mix_dry_into_wet_mono(
                        self.input_buffers.channels::<CHANNELS>()[0],
                        buffers.outputs[0],
                        info.frames,
                    );
                }
                2 => {
                    let (left, right) = buffers.outputs.split_at_mut(1);
                    self.mix.mix_dry_into_wet_stereo(
                        self.input_buffers.channels::<CHANNELS>()[0],
                        self.input_buffers.channels::<CHANNELS>()[1],
                        left[0],
                        right[0],
                        info.frames,
                    );
                }
                _ => panic!("Only Mono and Stereo are supported"),
            }

            // Copy the input to the processor's internal buffers Surely there is a
            // better way to do this, right?
            for (internal_buffer, input) in self
                .input_buffers
                .channels_mut::<CHANNELS>()
                .iter_mut()
                .zip(buffers.inputs.iter())
            {
                for (copy_into, copy_from) in internal_buffer.iter_mut().zip(input.iter()) {
                    *copy_into = *copy_from;
                }
            }
        }

        self.declick.process(
            buffers.outputs,
            0..info.frames,
            &extra.declick_values,
            1.0,
            DeclickFadeCurve::EqualPower3dB,
        );

        if will_pause {
            self.params.paused = true;
        }

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
