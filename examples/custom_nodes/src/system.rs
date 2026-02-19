use firewheel::{diff::Memo, error::UpdateError, node::NodeID, FirewheelContext};

use crate::nodes::{filter::FilterNode, noise_gen::NoiseGenNode, rms::FastRmsNode};

pub struct AudioSystem {
    pub cx: FirewheelContext,

    pub noise_gen_node: Memo<NoiseGenNode>,
    pub filter_node: Memo<FilterNode>,
    pub rms_node: Memo<FastRmsNode>,

    pub noise_gen_node_id: NodeID,
    pub filter_node_id: NodeID,
    pub rms_node_id: NodeID,
}

impl AudioSystem {
    pub fn new() -> Self {
        let mut cx = FirewheelContext::new(Default::default());
        cx.start_stream(Default::default()).unwrap();

        let noise_gen_node = NoiseGenNode::default();
        let filter_node = FilterNode::default();
        let rms_node = FastRmsNode::default();

        let noise_gen_node_id = cx.add_node(noise_gen_node, None);
        let filter_node_id = cx.add_node(filter_node, None);
        let rms_node_id = cx.add_node(rms_node.clone(), None);

        let graph_out_node_id = cx.graph_out_node_id();

        cx.connect(noise_gen_node_id, filter_node_id, &[(0, 0)], false)
            .unwrap();
        cx.connect(filter_node_id, rms_node_id, &[(0, 0)], false)
            .unwrap();
        cx.connect(filter_node_id, graph_out_node_id, &[(0, 0), (0, 1)], false)
            .unwrap();

        Self {
            cx,
            noise_gen_node: Memo::new(noise_gen_node),
            filter_node: Memo::new(filter_node),
            rms_node: Memo::new(rms_node),
            noise_gen_node_id,
            filter_node_id,
            rms_node_id,
        }
    }

    pub fn update(&mut self) {
        if let Err(e) = self.cx.update() {
            tracing::error!("{:?}", &e);

            if let UpdateError::StreamStoppedUnexpectedly(_) = e {
                // The stream has stopped unexpectedly (i.e the user has
                // unplugged their headphones.)
                //
                // Typically you should start a new stream as soon as
                // possible to resume processing (even if it's a dummy
                // output device).
                //
                // In this example we just quit the application.
                panic!("Stream stopped unexpectedly!");
            }
        }
    }
}
