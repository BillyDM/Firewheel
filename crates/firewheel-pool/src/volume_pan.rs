#[cfg(not(feature = "std"))]
use bevy_platform::prelude::{vec, Vec};

#[cfg(feature = "scheduled_events")]
use firewheel_core::clock::EventInstant;

use firewheel_core::{
    channel_config::NonZeroChannelCount,
    diff::{Diff, PathBuilder},
    node::NodeID,
};
use firewheel_graph::{backend::AudioBackend, FirewheelCtx};

use crate::FxChain;

/// A default [`FxChain`] for 2D game audio.
///
/// This chain contains a single `VolumePan` node.
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct VolumePanChain {
    pub volume_pan: firewheel_nodes::volume_pan::VolumePanNode,
    pub config: firewheel_nodes::volume_pan::VolumeNodeConfig,
}

impl VolumePanChain {
    /// Set the parameters of the volume pan node.
    ///
    /// * `params` - The new parameters.
    /// * `time` - The instant these new parameters should take effect. If this
    /// is `None`, then the parameters will take effect as soon as the node receives
    /// the event.
    pub fn set_params<B: AudioBackend>(
        &mut self,
        params: firewheel_nodes::volume_pan::VolumePanNode,
        #[cfg(feature = "scheduled_events")] time: Option<EventInstant>,
        node_ids: &[NodeID],
        cx: &mut FirewheelCtx<B>,
    ) {
        let node_id = node_ids[0];

        self.volume_pan.diff(
            &params,
            PathBuilder::default(),
            #[cfg(not(feature = "scheduled_events"))]
            &mut cx.event_queue(node_id),
            #[cfg(feature = "scheduled_events")]
            &mut cx.event_queue_scheduled(node_id, time),
        );
    }
}

impl FxChain for VolumePanChain {
    fn construct_and_connect<B: AudioBackend>(
        &mut self,
        first_node_id: NodeID,
        first_node_num_out_channels: NonZeroChannelCount,
        dst_node_id: NodeID,
        dst_num_channels: NonZeroChannelCount,
        cx: &mut FirewheelCtx<B>,
    ) -> Vec<NodeID> {
        let volume_pan_params = firewheel_nodes::volume_pan::VolumePanNode::default();

        let volume_pan_node_id = cx.add_node(volume_pan_params, Some(self.config));

        cx.connect(
            first_node_id,
            volume_pan_node_id,
            if first_node_num_out_channels.get().get() == 1 {
                &[(0, 0), (0, 1)]
            } else {
                &[(0, 0), (1, 1)]
            },
            false,
        )
        .unwrap();

        cx.connect(
            volume_pan_node_id,
            dst_node_id,
            if dst_num_channels.get().get() == 1 {
                &[(0, 0), (1, 0)]
            } else {
                &[(0, 0), (1, 1)]
            },
            false,
        )
        .unwrap();

        vec![volume_pan_node_id]
    }
}
