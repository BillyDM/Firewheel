use firewheel::{
    channel_config::NonZeroChannelCount,
    collector::ArcGc,
    cpal::CpalStream,
    error::AddEdgeError,
    node::NodeID,
    nodes::{
        beep_test::BeepTestNode,
        convolution::{ConvolutionNode, ConvolutionNodeConfig},
        fast_filters::{
            bandpass::FastBandpassNode, highpass::FastHighpassNode, lowpass::FastLowpassNode,
        },
        freeverb::FreeverbNode,
        mix::{MixNode, MixNodeConfig},
        noise_generator::{pink::PinkNoiseGenNode, white::WhiteNoiseGenNode},
        sampler::SamplerNode,
        svf::SvfNode,
        volume::{VolumeNode, VolumeNodeConfig},
        volume_pan::VolumePanNode,
        StereoToMonoNode,
    },
    sample_resource::{SampleResource, SampleResourceF32},
    ContextQueue, FirewheelContext,
};
use symphonium::SymphoniumLoader;

use crate::ui::GuiAudioNode;

pub const SAMPLE_PATHS: [&'static str; 4] = [
    "assets/test_files/swosh-sword-swing.flac",
    "assets/test_files/bird-sound.wav",
    "assets/test_files/beep_up.wav",
    "assets/test_files/birds_detail_chirp_medium_far.ogg",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    BeepTest,
    WhiteNoiseGen,
    PinkNoiseGen,
    StereoToMono,
    VolumeMono,
    VolumeStereo,
    VolumePan,
    FastLowpass,
    FastHighpass,
    FastBandpass,
    SVF,
    MixMono,
    MixStereo,
    Sampler,
    Freeverb,
    ConvolutionMono,
    ConvolutionStereo,
}

pub struct AudioSystem {
    cx: FirewheelContext,
    pub stream: CpalStream,
    pub(crate) samples: Vec<ArcGc<dyn SampleResource>>,
    pub(crate) ir_samples: Vec<(&'static str, ArcGc<dyn SampleResourceF32>)>,
}

const IR_SAMPLE_PATHS: [&'static str; 2] = [
    "assets/test_files/ir_outside.wav",
    "assets/test_files/ir_hall.wav",
];

impl AudioSystem {
    pub fn new() -> Self {
        let mut cx = FirewheelContext::new(Default::default());
        let stream = CpalStream::new(&mut cx, Default::default()).unwrap();

        let sample_rate = cx.stream_info().unwrap().sample_rate;

        let mut loader = SymphoniumLoader::new();

        // Load all samples
        let samples = SAMPLE_PATHS
            .iter()
            .map(|path| {
                firewheel::load_audio_file(&mut loader, path, Some(sample_rate), Default::default())
                    .unwrap()
                    .into_dyn_resource()
            })
            .collect();

        let loaded = IR_SAMPLE_PATHS
            .iter()
            .map(|path| {
                firewheel::load_audio_file_f32(
                    &mut loader,
                    path,
                    Some(sample_rate),
                    Default::default(),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();

        // Process samples to get multiple channels from few files
        let ir_samples = vec![
            ("Outside (Mono)", vec![loaded[0][0].clone()].into()),
            ("Outside (Stereo)", loaded[0].clone().into()),
            ("Hall (Mono)", vec![loaded[1][0].clone()].into()),
            ("Hall (Stereo)", loaded[1].clone().into()),
        ];

        Self {
            cx,
            stream,
            ir_samples,
            samples,
        }
    }

    pub fn remove_node(&mut self, node_id: NodeID) {
        if let Err(_) = self.cx.remove_node(node_id) {
            tracing::error!("Node already removed!");
        }
    }

    pub fn add_node(&mut self, node_type: NodeType) -> GuiAudioNode {
        let id = match node_type {
            NodeType::BeepTest => self.cx.add_node(BeepTestNode::default(), None),
            NodeType::WhiteNoiseGen => self.cx.add_node(WhiteNoiseGenNode::default(), None),
            NodeType::PinkNoiseGen => self.cx.add_node(PinkNoiseGenNode::default(), None),
            NodeType::StereoToMono => self.cx.add_node(StereoToMonoNode, None),
            NodeType::VolumeMono => self.cx.add_node(
                VolumeNode::default(),
                Some(VolumeNodeConfig {
                    channels: NonZeroChannelCount::MONO,
                    ..Default::default()
                }),
            ),
            NodeType::VolumeStereo => self.cx.add_node(
                VolumeNode::default(),
                Some(VolumeNodeConfig {
                    channels: NonZeroChannelCount::STEREO,
                    ..Default::default()
                }),
            ),
            NodeType::VolumePan => self.cx.add_node(VolumePanNode::default(), None),
            NodeType::FastLowpass => self.cx.add_node(FastLowpassNode::<2>::default(), None),
            NodeType::FastHighpass => self.cx.add_node(FastHighpassNode::<2>::default(), None),
            NodeType::FastBandpass => self.cx.add_node(FastBandpassNode::<2>::default(), None),
            NodeType::SVF => self.cx.add_node(SvfNode::<2>::default(), None),
            NodeType::MixMono => self.cx.add_node(
                MixNode::default(),
                Some(MixNodeConfig {
                    channels: NonZeroChannelCount::MONO,
                }),
            ),
            NodeType::MixStereo => self.cx.add_node(
                MixNode::default(),
                Some(MixNodeConfig {
                    channels: NonZeroChannelCount::STEREO,
                }),
            ),
            NodeType::Sampler => self.cx.add_node(SamplerNode::default(), None),
            NodeType::Freeverb => self.cx.add_node(FreeverbNode::default(), None),
            NodeType::ConvolutionMono => self.cx.add_node(
                ConvolutionNode::default(),
                Some(ConvolutionNodeConfig {
                    channels: NonZeroChannelCount::MONO,
                    ..Default::default()
                }),
            ),
            NodeType::ConvolutionStereo => self.cx.add_node(ConvolutionNode::default(), None),
        }
        .expect("Failed to add node");

        match node_type {
            NodeType::BeepTest => GuiAudioNode::BeepTest {
                id,
                params: Default::default(),
            },
            NodeType::WhiteNoiseGen => GuiAudioNode::WhiteNoiseGen {
                id,
                params: Default::default(),
            },
            NodeType::PinkNoiseGen => GuiAudioNode::PinkNoiseGen {
                id,
                params: Default::default(),
            },
            NodeType::StereoToMono => GuiAudioNode::StereoToMono { id },
            NodeType::VolumeMono => GuiAudioNode::VolumeMono {
                id,
                params: Default::default(),
            },
            NodeType::VolumeStereo => GuiAudioNode::VolumeStereo {
                id,
                params: Default::default(),
            },
            NodeType::VolumePan => GuiAudioNode::VolumePan {
                id,
                params: Default::default(),
            },
            NodeType::FastLowpass => GuiAudioNode::FastLowpass {
                id,
                params: Default::default(),
            },
            NodeType::FastHighpass => GuiAudioNode::FastHighpass {
                id,
                params: Default::default(),
            },
            NodeType::FastBandpass => GuiAudioNode::FastBandpass {
                id,
                params: Default::default(),
            },
            NodeType::SVF => GuiAudioNode::SVF {
                id,
                params: Default::default(),
            },
            NodeType::MixMono => GuiAudioNode::MixMono {
                id,
                params: Default::default(),
            },
            NodeType::MixStereo => GuiAudioNode::MixStereo {
                id,
                params: Default::default(),
            },
            NodeType::Sampler => GuiAudioNode::Sampler {
                id,
                params: Default::default(),
            },
            NodeType::Freeverb => GuiAudioNode::Freeverb {
                id,
                params: Default::default(),
            },
            NodeType::ConvolutionMono => GuiAudioNode::ConvolutionMono {
                id,
                params: Default::default(),
            },
            NodeType::ConvolutionStereo => GuiAudioNode::ConvolutionStereo {
                id,
                params: Default::default(),
            },
        }
    }

    pub fn connect(
        &mut self,
        src_node: NodeID,
        dst_node: NodeID,
        src_port: u32,
        dst_port: u32,
    ) -> Result<(), AddEdgeError> {
        self.cx
            .connect(src_node, dst_node, &[(src_port, dst_port)], true)?;

        Ok(())
    }

    pub fn disconnect(&mut self, src_node: NodeID, dst_node: NodeID, src_port: u32, dst_port: u32) {
        self.cx
            .disconnect(src_node, dst_node, &[(src_port, dst_port)]);
    }

    pub fn graph_in_node_id(&self) -> NodeID {
        self.cx.graph_in_node_id()
    }

    pub fn graph_out_node_id(&self) -> NodeID {
        self.cx.graph_out_node_id()
    }

    pub fn is_activated(&self) -> bool {
        self.cx.is_active()
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

    pub fn reset(&mut self) {
        let nodes: Vec<NodeID> = self.cx.nodes().map(|n| n.id).collect();
        for node_id in nodes {
            let _ = self.cx.remove_node(node_id);
        }
    }

    pub fn event_queue(&mut self, node_id: NodeID) -> ContextQueue<'_> {
        self.cx.event_queue(node_id)
    }
}
