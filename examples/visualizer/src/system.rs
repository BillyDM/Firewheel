use firewheel::{
    channel_config::NonZeroChannelCount,
    cpal::CpalStream,
    diff::Memo,
    node::NodeID,
    nodes::{
        sampler::SamplerNode,
        triple_buffer::{TripleBufferConfig, TripleBufferNode, TripleBufferState, WindowSize},
    },
    FirewheelContext,
};

pub const SAMPLE_PATH: &'static str = "assets/test_files/bird-sound.wav";

pub struct AudioSystem {
    cx: FirewheelContext,
    stream: CpalStream,

    sampler_params: Memo<SamplerNode>,
    sampler_node_id: NodeID,

    pub triple_buffer_params: Memo<TripleBufferNode>,
    pub triple_buffer_node_id: NodeID,
    pub triple_buffer_state: TripleBufferState,
    pub triple_buffer_bypassed: bool,
}

impl AudioSystem {
    pub fn new(window_size: u32) -> Self {
        let mut cx = FirewheelContext::new(Default::default());
        let stream = CpalStream::new(&mut cx, Default::default()).unwrap();

        let sample_rate = cx.stream_info().unwrap().sample_rate;
        let graph_out = cx.graph_out_node_id();

        let probed = symphonium::probe_from_file(
            SAMPLE_PATH,
            None, // Custom container probe
        )
        .unwrap();
        let sample = firewheel::dyn_sample_resource(
            symphonium::decode(
                probed,
                &symphonium::DecodeConfig::default(),
                Some(sample_rate), // target sample rate
                None,              // An optional cache
                None,              // Custom codec registry
            )
            .unwrap(),
        );

        let mut sampler_params = SamplerNode::default();
        sampler_params.set_sample(sample);
        let sampler_node_id = cx
            .add_node(sampler_params.clone(), None)
            .expect("Sampler node should construct without error");

        let triple_buffer_params = TripleBufferNode {
            window_size: WindowSize::Samples(window_size),
        };
        let triple_buffer_node_id = cx
            .add_node(
                triple_buffer_params.clone(),
                Some(TripleBufferConfig {
                    max_window_size: WindowSize::Samples(2048),
                    channels: NonZeroChannelCount::STEREO,
                }),
            )
            .expect("Triple buffer node should construct without error");

        cx.connect(sampler_node_id, graph_out, &[(0, 0), (1, 1)], false)
            .unwrap();
        cx.connect(
            sampler_node_id,
            triple_buffer_node_id,
            &[(0, 0), (1, 1)],
            false,
        )
        .unwrap();

        let triple_buffer_state = cx
            .node_state::<TripleBufferState>(triple_buffer_node_id)
            .unwrap()
            .clone();

        Self {
            cx,
            stream,
            sampler_params: Memo::new(sampler_params),
            sampler_node_id,
            triple_buffer_params: Memo::new(triple_buffer_params),
            triple_buffer_node_id,
            triple_buffer_state,
            triple_buffer_bypassed: false,
        }
    }

    pub fn play_sample(&mut self) {
        self.sampler_params.start_or_restart();
        self.sampler_params
            .update_memo(&mut self.cx.event_queue(self.sampler_node_id));
    }

    pub fn set_bypassed(&mut self, bypassed: bool) {
        self.triple_buffer_bypassed = bypassed;
        self.cx
            .queue_bypassed_for(self.triple_buffer_node_id, bypassed);
    }

    pub fn set_window_size(&mut self, window_size: u32) {
        self.triple_buffer_params.window_size = WindowSize::Samples(window_size);
        self.triple_buffer_params
            .update_memo(&mut self.cx.event_queue(self.triple_buffer_node_id));
    }

    pub fn update(&mut self) {
        // Update the firewheel context.
        // This must be called reguarly (i.e. once every frame).
        if let Err(e) = self.cx.update() {
            tracing::error!("{:?}", &e);
        }

        // Log any stream errors/warnings that have occurred.
        self.stream.log_status();

        // The stream has stopped unexpectedly (i.e the user has
        // unplugged their headphones.)
        //
        // Typically you should start a new stream as soon as
        // possible to resume processing (even if it's a dummy
        // output device).
        //
        // In this example we just quit the application.
        if !self.stream.is_running() {
            panic!("Stream stopped unexpectedly!");
        }
    }

    pub fn is_activated(&self) -> bool {
        self.cx.is_active()
    }
}
