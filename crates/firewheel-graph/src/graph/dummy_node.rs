use firewheel_core::node::NodeError;
use firewheel_core::{
    channel_config::ChannelConfig,
    event::ProcEvents,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, ProcBuffers,
        ProcExtra, ProcInfo, ProcessStatus,
    },
};

/// A "dummy" [`AudioNode`], a node which does nothing.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DummyNode;

/// The configuration for a [`DummyNode`], a node which does nothing.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DummyNodeConfig {
    pub channel_config: ChannelConfig,
}

impl AudioNode for DummyNode {
    type Configuration = DummyNodeConfig;

    fn info(&self, config: &Self::Configuration) -> Result<AudioNodeInfo, NodeError> {
        Ok(AudioNodeInfo::new()
            .debug_name("dummy")
            .channel_config(config.channel_config))
    }

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        _cx: ConstructProcessorContext,
    ) -> Result<impl AudioNodeProcessor, NodeError> {
        Ok(DummyProcessor)
    }
}

struct DummyProcessor;

impl AudioNodeProcessor for DummyProcessor {
    fn process(
        &mut self,
        _info: &ProcInfo,
        _buffers: Option<ProcBuffers>,
        _events: &mut ProcEvents,
        _extra: &mut ProcExtra,
    ) -> ProcessStatus {
        ProcessStatus::Bypass
    }
}
