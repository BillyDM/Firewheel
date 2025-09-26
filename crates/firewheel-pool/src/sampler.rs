use firewheel_core::{
    channel_config::NonZeroChannelCount,
    diff::{Diff, PathBuilder},
    node::NodeID,
};
use firewheel_graph::{backend::AudioBackend, ContextQueue, FirewheelCtx};
use firewheel_nodes::sampler::{SamplerConfig, SamplerNode, SamplerState};

use crate::{PoolError, PoolableNode};

/// A struct which uses a [`SamplerNode`] as the first node in an
/// [`AudioNodePool`](crate::AudioNodePool).
pub struct SamplerPool;

impl PoolableNode for SamplerPool {
    type AudioNode = SamplerNode;

    /// Return the number of output channels for the given configuration.
    fn num_output_channels(config: Option<&SamplerConfig>) -> NonZeroChannelCount {
        config
            .map(|c| c.channels)
            .unwrap_or(SamplerConfig::default().channels)
    }

    /// Return `true` if the given parameters signify that the sequence is stopped,
    /// `false` otherwise.
    fn params_stopped(params: &SamplerNode) -> bool {
        params.stop_requested()
    }

    /// Return `true` if the node state of the given node is stopped.
    ///
    /// Return an error if the given `node_id` is invalid.
    fn node_is_stopped<B: AudioBackend>(
        node_id: NodeID,
        cx: &FirewheelCtx<B>,
    ) -> Result<bool, PoolError> {
        cx.node_state::<SamplerState>(node_id)
            .map(|s| s.stopped())
            .ok_or(PoolError::InvalidNodeID(node_id))
    }

    /// Return a score of how ready this node is to accept new work.
    ///
    /// The worker with the highest worker score will be chosen for the new work.
    ///
    /// Return an error if the given `node_id` is invalid.
    fn worker_score<B: AudioBackend>(
        params: &SamplerNode,
        node_id: NodeID,
        cx: &mut FirewheelCtx<B>,
    ) -> Result<u64, PoolError> {
        cx.node_state::<SamplerState>(node_id)
            .map(|s| s.worker_score(params))
            .ok_or(PoolError::InvalidNodeID(node_id))
    }

    /// Diff the new parameters and push the changes into the event queue.
    fn diff<B: AudioBackend>(
        baseline: &SamplerNode,
        new: &SamplerNode,
        event_queue: &mut ContextQueue<B>,
    ) {
        new.diff(baseline, PathBuilder::default(), event_queue);
    }

    /// Notify the node state that a sequence is playing.
    ///
    /// This is used to account for the delay between sending an event to the node
    /// and the node receiving the event.
    ///
    /// Return an error if the given `node_id` is invalid.
    fn mark_playing<B: AudioBackend>(
        node_id: NodeID,
        cx: &mut FirewheelCtx<B>,
    ) -> Result<(), PoolError> {
        cx.node_state_mut::<SamplerState>(node_id)
            .map(|s| s.mark_playing())
            .ok_or(PoolError::InvalidNodeID(node_id))
    }

    /// Pause the sequence in the node parameters
    fn pause(params: &mut SamplerNode) {
        params.pause();
    }
    /// Resume the sequence in the node parameters
    fn resume(params: &mut SamplerNode) {
        params.resume();
    }
    /// Stop the sequence in the node parameters
    fn stop(params: &mut SamplerNode) {
        params.stop();
    }
}
