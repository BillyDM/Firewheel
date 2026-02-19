use firewheel::{
    channel_config::NonZeroChannelCount,
    diff::Memo,
    error::UpdateError,
    node::NodeID,
    nodes::{
        sampler::SamplerNode,
        triple_buffer::{TripleBufferConfig, TripleBufferNode, TripleBufferState, WindowSize},
    },
    FirewheelContext,
};
use symphonium::SymphoniumLoader;

pub const SAMPLE_PATH: &'static str = "assets/test_files/bird-sound.wav";

pub struct AudioSystem {
    cx: FirewheelContext,

    sampler_params: Memo<SamplerNode>,
    sampler_node_id: NodeID,

    pub triple_buffer_params: Memo<TripleBufferNode>,
    pub triple_buffer_node_id: NodeID,
    pub triple_buffer_state: TripleBufferState,
}

impl AudioSystem {
    pub fn new(window_size: u32) -> Self {
        let mut cx = FirewheelContext::new(Default::default());
        cx.start_stream(Default::default()).unwrap();

        let sample_rate = cx.stream_info().unwrap().sample_rate;
        let mut loader = SymphoniumLoader::new();
        let graph_out = cx.graph_out_node_id();

        let sample = firewheel::load_audio_file(
            &mut loader,
            SAMPLE_PATH,
            Some(sample_rate),
            Default::default(),
        )
        .unwrap()
        .into_dyn_resource();

        let mut sampler_params = SamplerNode::default();
        sampler_params.set_sample(sample);
        let sampler_node_id = cx.add_node(sampler_params.clone(), None);

        let triple_buffer_params = TripleBufferNode {
            window_size: WindowSize::Samples(window_size),
            enabled: true,
        };
        let triple_buffer_node_id = cx.add_node(
            triple_buffer_params.clone(),
            Some(TripleBufferConfig {
                max_window_size: WindowSize::Samples(2048),
                channels: NonZeroChannelCount::STEREO,
            }),
        );

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
            sampler_params: Memo::new(sampler_params),
            sampler_node_id,
            triple_buffer_params: Memo::new(triple_buffer_params),
            triple_buffer_node_id,
            triple_buffer_state,
        }
    }

    pub fn play_sample(&mut self) {
        self.sampler_params.start_or_restart();
        self.sampler_params
            .update_memo(&mut self.cx.event_queue(self.sampler_node_id));
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.triple_buffer_params.enabled = enabled;
        self.triple_buffer_params
            .update_memo(&mut self.cx.event_queue(self.triple_buffer_node_id));
    }

    pub fn set_window_size(&mut self, window_size: u32) {
        self.triple_buffer_params.window_size = WindowSize::Samples(window_size);
        self.triple_buffer_params
            .update_memo(&mut self.cx.event_queue(self.triple_buffer_node_id));
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

    pub fn is_activated(&self) -> bool {
        self.cx.is_audio_stream_running()
    }
}
