use firewheel_core::{
    diff::{Diff, PathBuilder},
    node::NodeID,
};
use firewheel_graph::{ContextQueue, FirewheelContext};
use firewheel_nodes::sampler::{SamplerNode, SamplerState};

use crate::{PoolError, PoolableNode};

/// A struct which uses a [`SamplerNode`] as the first node in an
/// [`AudioNodePool`](crate::AudioNodePool).
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamplerPool;

impl PoolableNode for SamplerPool {
    type AudioNode = SamplerNode;
    type AdditionalNodeState = ();

    /// Return `true` if the given parameters signify that the sequence is stopped,
    /// `false` otherwise.
    fn params_stopped(params: &SamplerNode) -> bool {
        params.stop_requested()
    }

    /// Return `true` if the node state of the given node is stopped.
    ///
    /// Return an error if the given `node_id` is invalid.
    fn node_is_stopped(
        node_id: NodeID,
        params: &SamplerNode,
        _additional_state: &mut (),
        cx: &mut FirewheelContext,
    ) -> Result<bool, PoolError> {
        cx.node_state::<SamplerState>(node_id)
            .map(|s| s.playback_id_has_finished(params.playback_id()))
            .ok_or(PoolError::InvalidNodeID(node_id))
    }

    /// Return a score of how ready this node is to accept new work.
    ///
    /// The worker with the highest worker score will be chosen for the new work.
    ///
    /// Return an error if the given `node_id` is invalid.
    fn worker_score(
        params: &SamplerNode,
        node_id: NodeID,
        _additional_state: &mut (),
        cx: &mut FirewheelContext,
    ) -> Result<u64, PoolError> {
        cx.node_state::<SamplerState>(node_id)
            .map(|s| s.worker_score(params))
            .ok_or(PoolError::InvalidNodeID(node_id))
    }

    /// Diff the new parameters and push the changes into the event queue.
    fn diff(
        baseline: &SamplerNode,
        new: &SamplerNode,
        _additional_state: &mut (),
        event_queue: &mut ContextQueue,
    ) {
        new.diff(baseline, PathBuilder::default(), event_queue);
    }

    /// Notify the node state that a sequence is playing.
    ///
    /// This is used to account for the delay between sending an event to the node
    /// and the node receiving the event.
    ///
    /// Return an error if the given `node_id` is invalid.
    fn mark_playing(
        _node_id: NodeID,
        _params: &Self::AudioNode,
        _additional_state: &mut (),
        _cx: &mut FirewheelContext,
    ) -> Result<(), PoolError> {
        Ok(())
    }

    /// Pause the sequence in the node parameters
    fn pause(params: &mut SamplerNode, _additional_state: &mut ()) {
        params.pause();
    }
    /// Resume the sequence in the node parameters
    fn resume(params: &mut SamplerNode, _additional_state: &mut ()) {
        params.resume();
    }
    /// Stop the sequence in the node parameters
    fn stop(params: &mut SamplerNode, _additional_state: &mut ()) {
        params.stop();
    }
}
