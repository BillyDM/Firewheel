use firewheel::{cpal::CpalStream, diff::Memo, node::NodeID, FirewheelContext};

use crate::nodes::{
    filter::FilterNode,
    noise_gen::NoiseGenNode,
    rms::{FastRmsNode, FastRmsState},
};

pub struct AudioSystem {
    pub cx: FirewheelContext,
    pub stream: CpalStream,

    pub noise_gen_node: Memo<NoiseGenNode>,
    pub filter_node: Memo<FilterNode>,
    pub rms_node: Memo<FastRmsNode>,
    pub rms_node_state: FastRmsState,

    pub noise_gen_node_id: NodeID,
    pub filter_node_id: NodeID,
    pub rms_node_id: NodeID,

    pub noise_gen_bypassed: bool,
    pub filter_bypassed: bool,
    pub rms_bypassed: bool,
}

impl AudioSystem {
    pub fn new() -> Self {
        let mut cx = FirewheelContext::new(Default::default());
        let stream = CpalStream::new(&mut cx, Default::default()).unwrap();

        let noise_gen_node = NoiseGenNode::default();
        let filter_node = FilterNode::default();
        let rms_node = FastRmsNode::default();

        let noise_gen_node_id = cx
            .add_node(noise_gen_node, None)
            .expect("Noise gen node should construct without error");
        let filter_node_id = cx
            .add_node(filter_node, None)
            .expect("Filter node should construct without error");
        let rms_node_id = cx
            .add_node(rms_node, None)
            .expect("RMS node should construct without error");

        let graph_out_node_id = cx.graph_out_node_id();

        cx.connect(noise_gen_node_id, filter_node_id, &[(0, 0)], false)
            .unwrap();
        cx.connect(filter_node_id, rms_node_id, &[(0, 0)], false)
            .unwrap();
        cx.connect(filter_node_id, graph_out_node_id, &[(0, 0), (0, 1)], false)
            .unwrap();

        let rms_node_state = cx.node_state::<FastRmsState>(rms_node_id).unwrap().clone();

        Self {
            cx,
            stream,
            noise_gen_node: Memo::new(noise_gen_node),
            filter_node: Memo::new(filter_node),
            rms_node: Memo::new(rms_node),
            rms_node_state,
            noise_gen_node_id,
            filter_node_id,
            rms_node_id,
            noise_gen_bypassed: false,
            filter_bypassed: false,
            rms_bypassed: false,
        }
    }

    pub fn update(&mut self) {
        // Update the firewheel context.
        // This must be called reguarly (i.e. once every frame).
        if let Err(e) = self.cx.update() {
            tracing::error!("{:?}", &e);
        }

        if let Err(e) = self.stream.poll_status() {
            tracing::error!("{:?}", &e);

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
