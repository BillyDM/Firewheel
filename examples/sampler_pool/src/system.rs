use firewheel::{
    channel_config::NonZeroChannelCount,
    diff::Memo,
    error::UpdateError,
    nodes::{
        sampler::SamplerNode,
        volume::{VolumeNode, VolumeNodeConfig},
        StereoToMonoNode,
    },
    sampler_pool::{FxChain, SamplerPool, VolumePanChain},
    FirewheelContext,
};
use symphonium::SymphoniumLoader;

/// The maximum number of samples that can be played in parallel in the `SamplerPool`.
///
/// A lower number was chosen to better showcase how work is stolen from the oldest
/// playing sample in the case when there are no more free workers left. A typical
/// game would probably want something a bit higher like `16` (Though keep in mind
/// that the higher the number, the more processing overhead there will be.)
pub const NUM_WORKERS: usize = 4;

pub struct AudioSystem {
    pub cx: FirewheelContext,

    pub sampler_pool_1: SamplerPool<VolumePanChain>,
    pub sampler_pool_2: SamplerPool<MyCustomChain>,
    pub sampler_node: SamplerNode,
}

impl AudioSystem {
    pub fn new() -> Self {
        let mut cx = FirewheelContext::new(Default::default());
        cx.start_stream(Default::default()).unwrap();

        let graph_out = cx.graph_out_node_id();

        let sampler_pool_1 = SamplerPool::new(
            NUM_WORKERS,                 // The number of workers to create in this pool.
            Default::default(),          // Use the default configuration.
            graph_out, // The ID of the node that the last effect in each fx chain instance will connect to.
            NonZeroChannelCount::STEREO, // The number of input channels in `graph_out`.
            &mut cx,   // The firewheel context.
        );

        let sampler_pool_2 = SamplerPool::new(
            NUM_WORKERS,                 // The number of workers to create in this pool.
            Default::default(),          // Use the default configuration.
            graph_out, // The ID of the node that the last effect in each fx chain instance will connect to.
            NonZeroChannelCount::STEREO, // The number of input channels in `graph_out`.
            &mut cx,   // The firewheel context.
        );

        let sample_rate = cx.stream_info().unwrap().sample_rate;
        let mut loader = SymphoniumLoader::new();

        let sample = firewheel::load_audio_file(
            &mut loader,
            "assets/test_files/bird-sound.wav",
            sample_rate,
            Default::default(),
        )
        .unwrap()
        .into_dyn_resource();

        let mut sampler_node = SamplerNode::default();
        sampler_node.set_sample(sample);

        Self {
            cx,
            sampler_pool_1,
            sampler_pool_2,
            sampler_node,
        }
    }

    pub fn update(&mut self) {
        if let Err(e) = self.cx.update() {
            log::error!("{:?}", &e);

            if let UpdateError::StreamStoppedUnexpectedly(_) = e {
                // The stream has stopped unexpectedly (i.e the user has
                // unplugged their headphones.)
                //
                // Typically you should start a new stream as soon as
                // possible to resume processing (event if it's a dummy
                // output device).
                //
                // In this example we just quit the application.
                panic!("Stream stopped unexpectedly!");
            }
        }
    }
}

/// An example of a custom FX chain for a sampler pool.
#[derive(Default)]
pub struct MyCustomChain {
    pub _stereo_to_mono: StereoToMonoNode,
    pub volume: Memo<VolumeNode>,
}

impl FxChain for MyCustomChain {
    fn construct_and_connect(
        &mut self,
        // The ID of the sampler node in this worker instance.
        sampler_node_id: firewheel::node::NodeID,
        // The number of channels in the sampler node.
        sampler_num_channels: NonZeroChannelCount,
        // The ID of the node that the last node in this FX chain should
        // connect to.
        dst_node_id: firewheel::node::NodeID,
        // The number of input channels on `dst_node_id`.
        dst_num_channels: NonZeroChannelCount,
        // The firewheel context.
        cx: &mut FirewheelContext,
    ) -> Vec<firewheel::node::NodeID> {
        // In this example we only support stereo, but you can have your FX
        // chain support multiple channel configurations.
        assert_eq!(sampler_num_channels, NonZeroChannelCount::STEREO);
        assert_eq!(dst_num_channels, NonZeroChannelCount::STEREO);

        let stereo_to_mono_node_id = cx.add_node(StereoToMonoNode, None);

        let volume_params = VolumeNode::default();
        let volume_node_id = cx.add_node(
            volume_params,
            Some(VolumeNodeConfig {
                channels: NonZeroChannelCount::MONO,
                ..Default::default()
            }),
        );

        // Connect the sampler node to the stereo_to_mono node.
        cx.connect(
            sampler_node_id,
            stereo_to_mono_node_id,
            &[(0, 0), (1, 1)],
            false,
        )
        .unwrap();

        // Connect the stereo_to_mono node to the volume node.
        cx.connect(stereo_to_mono_node_id, volume_node_id, &[(0, 0)], false)
            .unwrap();

        // Connect the volume node to the destination node.
        cx.connect(volume_node_id, dst_node_id, &[(0, 0), (0, 1)], false)
            .unwrap();

        // Return the list of node IDs in this FX chain.
        vec![stereo_to_mono_node_id, volume_node_id]
    }
}
