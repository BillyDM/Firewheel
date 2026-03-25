use firewheel_core::node::NodeError;
use firewheel_core::{
    channel_config::{ChannelConfig, ChannelCount},
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        ProcBuffers, ProcExtra, ProcInfo, ProcessStatus,
    },
};

/// A node that converts a stereo signal into a mono signal
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Component))]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StereoToMonoNode;

impl AudioNode for StereoToMonoNode {
    type Configuration = EmptyConfig;

    fn info(&self, _config: &Self::Configuration) -> Result<AudioNodeInfo, NodeError> {
        Ok(AudioNodeInfo::new()
            .debug_name("stereo_to_mono")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::STEREO,
                num_outputs: ChannelCount::MONO,
            }))
    }

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        _cx: ConstructProcessorContext,
    ) -> Result<impl AudioNodeProcessor, NodeError> {
        Ok(StereoToMonoProcessor)
    }
}

struct StereoToMonoProcessor;

impl AudioNodeProcessor for StereoToMonoProcessor {
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        _extra: &mut ProcExtra,
    ) -> ProcessStatus {
        if info.in_silence_mask.all_channels_silent(2) {
            return ProcessStatus::ClearAllOutputs;
        }

        for (out_s, (&in1, &in2)) in buffers.outputs[0]
            .iter_mut()
            .zip(buffers.inputs[0].iter().zip(buffers.inputs[1].iter()))
        {
            *out_s = (in1 + in2) * 0.5;
        }

        ProcessStatus::OutputsModified
    }
}
