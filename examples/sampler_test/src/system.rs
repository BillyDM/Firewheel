use firewheel::{
    diff::Memo,
    dsp::volume::{DbMeterNormalizer, Volume, DEFAULT_DB_EPSILON},
    error::UpdateError,
    node::NodeID,
    nodes::{
        peak_meter::{PeakMeterNode, PeakMeterSmoother, PeakMeterState},
        sampler::{RepeatMode, SamplerNode, SamplerState},
    },
    FirewheelContext,
};
use symphonium::SymphoniumLoader;

pub const SAMPLE_PATHS: [&'static str; 4] = [
    "assets/test_files/swosh-sword-swing.flac",
    "assets/test_files/bird-sound.wav",
    "assets/test_files/beep_up.wav",
    "assets/test_files/birds_detail_chirp_medium_far.ogg",
];

struct Sampler {
    pub params: Memo<SamplerNode>,
    pub node_id: NodeID,
}

pub struct AudioSystem {
    cx: FirewheelContext,

    samplers: Vec<Sampler>,

    peak_meter_id: NodeID,
    peak_meter_smoother: PeakMeterSmoother<2>,
    peak_meter_normalizer: DbMeterNormalizer,
}

impl AudioSystem {
    pub fn new() -> Self {
        let mut cx = FirewheelContext::new(Default::default());
        cx.start_stream(Default::default()).unwrap();

        let sample_rate = cx.stream_info().unwrap().sample_rate;

        let mut loader = SymphoniumLoader::new();

        let graph_out = cx.graph_out_node_id();

        let peak_meter_node = PeakMeterNode::<2> { enabled: true };
        let peak_meter_smoother = PeakMeterSmoother::<2>::new(Default::default());

        let peak_meter_id = cx.add_node(peak_meter_node.clone(), None);
        cx.connect(peak_meter_id, graph_out, &[(0, 0), (1, 1)], false)
            .unwrap();

        let samplers = SAMPLE_PATHS
            .iter()
            .map(|path| {
                let sample = firewheel::load_audio_file(
                    &mut loader,
                    path,
                    Some(sample_rate),
                    Default::default(),
                )
                .unwrap()
                .into_dyn_resource();

                let mut params = SamplerNode::default();
                params.set_sample(sample);

                let node_id = cx.add_node(params.clone(), None);

                cx.connect(node_id, peak_meter_id, &[(0, 0), (1, 1)], false)
                    .unwrap();

                Sampler {
                    params: Memo::new(params),
                    node_id,
                }
            })
            .collect();

        Self {
            cx,
            samplers,
            peak_meter_id,
            peak_meter_smoother,
            peak_meter_normalizer: DbMeterNormalizer::default(),
        }
    }

    pub fn is_activated(&self) -> bool {
        self.cx.is_audio_stream_running()
    }

    pub fn start_or_restart(&mut self, sampler_i: usize) {
        let sampler = &mut self.samplers[sampler_i];

        sampler.params.start_or_restart();

        sampler
            .params
            .update_memo(&mut self.cx.event_queue(sampler.node_id));
    }

    pub fn set_volume(&mut self, sampler_i: usize, percent_volume: f32) {
        let sampler = &mut self.samplers[sampler_i];

        sampler.params.volume = Volume::Linear(percent_volume / 100.0);

        sampler
            .params
            .update_memo(&mut self.cx.event_queue(sampler.node_id));
    }

    pub fn set_repeat_mode(&mut self, sampler_i: usize, repeat_mode: RepeatMode) {
        let sampler = &mut self.samplers[sampler_i];

        sampler.params.repeat_mode = repeat_mode;

        sampler
            .params
            .update_memo(&mut self.cx.event_queue(sampler.node_id));
    }

    pub fn pause(&mut self, sampler_i: usize) {
        let sampler = &mut self.samplers[sampler_i];

        sampler.params.pause();

        sampler
            .params
            .update_memo(&mut self.cx.event_queue(sampler.node_id));
    }

    pub fn resume(&mut self, sampler_i: usize) {
        let sampler = &mut self.samplers[sampler_i];

        sampler.params.resume();

        sampler
            .params
            .update_memo(&mut self.cx.event_queue(sampler.node_id));
    }

    pub fn stop(&mut self, sampler_i: usize) {
        let sampler = &mut self.samplers[sampler_i];

        sampler.params.stop();

        sampler
            .params
            .update_memo(&mut self.cx.event_queue(sampler.node_id));
    }

    pub fn set_speed(&mut self, speed: f64) {
        for s in self.samplers.iter_mut() {
            s.params.speed = speed;
            s.params.update_memo(&mut self.cx.event_queue(s.node_id));
        }
    }

    pub fn is_playing(&self, sampler_i: usize) -> bool {
        self.cx
            .node_state::<SamplerState>(self.samplers[sampler_i].node_id)
            .unwrap()
            .playing()
    }

    pub fn is_paused(&self, sampler_i: usize) -> bool {
        self.cx
            .node_state::<SamplerState>(self.samplers[sampler_i].node_id)
            .unwrap()
            .paused()
    }

    pub fn update(&mut self) {
        if let Err(e) = self.cx.update() {
            tracing::error!("{:?}", &e);

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

    pub fn update_meters(&mut self, delta_seconds: f32) {
        self.peak_meter_smoother.update(
            self.cx
                .node_state::<PeakMeterState<2>>(self.peak_meter_id)
                .unwrap()
                .peak_gain_db(DEFAULT_DB_EPSILON),
            delta_seconds,
        );
    }

    pub fn peak_meter_values(&self) -> [f32; 2] {
        self.peak_meter_smoother
            .smoothed_peaks_normalized(&self.peak_meter_normalizer)
    }

    pub fn peak_meter_has_clipped(&self) -> [bool; 2] {
        self.peak_meter_smoother.has_clipped()
    }
}
