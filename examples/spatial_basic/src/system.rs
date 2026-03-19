use firewheel::{
    cpal::CpalStream,
    diff::Memo,
    node::NodeID,
    nodes::{
        sampler::{RepeatMode, SamplerNode},
        spatial_basic::SpatialBasicNode,
    },
    FirewheelContext,
};
use symphonium::SymphoniumLoader;

pub struct AudioSystem {
    pub cx: FirewheelContext,
    pub stream: CpalStream,

    pub _sampler_node: SamplerNode,
    pub _sampler_node_id: NodeID,

    pub spatial_basic_node: Memo<SpatialBasicNode>,
    pub spatial_basic_node_id: NodeID,
}

impl AudioSystem {
    pub fn new() -> Self {
        let mut cx = FirewheelContext::new(Default::default());
        let stream = CpalStream::new(&mut cx, Default::default()).unwrap();

        let sample_rate = cx.stream_info().unwrap().sample_rate;

        let mut loader = SymphoniumLoader::new();
        let sample = firewheel::load_audio_file(
            &mut loader,
            "assets/test_files/dpren_very-lush-and-swag-loop.ogg",
            Some(sample_rate),
            Default::default(),
        )
        .unwrap()
        .into_dyn_resource();

        let graph_out_node_id = cx.graph_out_node_id();

        let mut sampler_node = SamplerNode::default();
        sampler_node.set_sample(sample);
        sampler_node.repeat_mode = RepeatMode::RepeatEndlessly;
        sampler_node.start_or_restart();

        let sampler_node_id = cx
            .add_node(sampler_node.clone(), None)
            .expect("Sampler node should construct without error");

        let spatial_basic_node = SpatialBasicNode::default();
        let spatial_basic_node_id = cx
            .add_node(spatial_basic_node, None)
            .expect("Spatial basic node should construct without error");

        cx.connect(
            sampler_node_id,
            spatial_basic_node_id,
            &[(0, 0), (1, 1)],
            false,
        )
        .unwrap();
        cx.connect(
            spatial_basic_node_id,
            graph_out_node_id,
            &[(0, 0), (1, 1)],
            false,
        )
        .unwrap();

        Self {
            cx,
            stream,
            _sampler_node: sampler_node,
            _sampler_node_id: sampler_node_id,
            spatial_basic_node: Memo::new(spatial_basic_node),
            spatial_basic_node_id,
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
