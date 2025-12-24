use eframe::App;
use egui::{Color32, Id, Ui, UiKind};
use egui_snarl::{
    ui::{AnyPins, PinInfo, SnarlPin, SnarlStyle, SnarlViewer},
    InPin, InPinId, OutPin, OutPinId, Snarl,
};
use firewheel::{
    diff::Memo,
    dsp::{fade::FadeCurve, mix::Mix},
    event::NodeEventType,
    node::NodeID,
    nodes::{
        beep_test::BeepTestNode,
        convolution::{ConvolutionNode, ImpulseResponse},
        fast_filters::{
            bandpass::FastBandpassNode, highpass::FastHighpassNode, lowpass::FastLowpassNode,
            MAX_HZ, MIN_HZ,
        },
        freeverb::FreeverbNode,
        mix::MixNode,
        noise_generator::{pink::PinkNoiseGenNode, white::WhiteNoiseGenNode},
        sampler::{RepeatMode, SamplerNode},
        svf::{SvfNode, SvfType, DEFAULT_MAX_Q, DEFAULT_MIN_Q},
        volume::VolumeNode,
        volume_pan::VolumePanNode,
    },
    Volume,
};

use crate::system::{AudioSystem, NodeType, SAMPLE_PATHS};

const CABLE_COLOR: Color32 = Color32::from_rgb(0xb0, 0x00, 0xb0);

pub enum GuiAudioNode {
    #[allow(unused)]
    SystemIn,
    SystemOut,
    BeepTest {
        id: firewheel::node::NodeID,
        params: Memo<BeepTestNode>,
    },
    WhiteNoiseGen {
        id: firewheel::node::NodeID,
        params: Memo<WhiteNoiseGenNode>,
    },
    PinkNoiseGen {
        id: firewheel::node::NodeID,
        params: Memo<PinkNoiseGenNode>,
    },
    StereoToMono {
        id: firewheel::node::NodeID,
    },
    VolumeMono {
        id: firewheel::node::NodeID,
        params: Memo<VolumeNode>,
    },
    VolumeStereo {
        id: firewheel::node::NodeID,
        params: Memo<VolumeNode>,
    },
    VolumePan {
        id: firewheel::node::NodeID,
        params: Memo<VolumePanNode>,
    },
    FastLowpass {
        id: firewheel::node::NodeID,
        params: Memo<FastLowpassNode<2>>,
    },
    FastHighpass {
        id: firewheel::node::NodeID,
        params: Memo<FastHighpassNode<2>>,
    },
    FastBandpass {
        id: firewheel::node::NodeID,
        params: Memo<FastBandpassNode<2>>,
    },
    SVF {
        id: firewheel::node::NodeID,
        params: Memo<SvfNode<2>>,
    },
    MixMono {
        id: firewheel::node::NodeID,
        params: Memo<MixNode>,
    },
    MixStereo {
        id: firewheel::node::NodeID,
        params: Memo<MixNode>,
    },
    Sampler {
        id: firewheel::node::NodeID,
        params: Memo<SamplerNode>,
    },
    Freeverb {
        id: firewheel::node::NodeID,
        params: Memo<FreeverbNode>,
    },
    ConvolutionMono {
        id: firewheel::node::NodeID,
        params: Memo<ConvolutionNode<1>>,
    },
    ConvolutionStereo {
        id: firewheel::node::NodeID,
        params: Memo<ConvolutionNode<2>>,
    },
}

impl GuiAudioNode {
    fn node_id(&self, audio_system: &AudioSystem) -> firewheel::node::NodeID {
        match self {
            &Self::SystemIn => audio_system.graph_in_node_id(),
            &Self::SystemOut => audio_system.graph_out_node_id(),
            &Self::BeepTest { id, .. } => id,
            &Self::WhiteNoiseGen { id, .. } => id,
            &Self::PinkNoiseGen { id, .. } => id,
            &Self::StereoToMono { id } => id,
            &Self::VolumeMono { id, .. } => id,
            &Self::VolumeStereo { id, .. } => id,
            &Self::VolumePan { id, .. } => id,
            &Self::FastLowpass { id, .. } => id,
            &Self::FastHighpass { id, .. } => id,
            &Self::FastBandpass { id, .. } => id,
            &Self::SVF { id, .. } => id,
            &Self::MixMono { id, .. } => id,
            &Self::MixStereo { id, .. } => id,
            &Self::Sampler { id, .. } => id,
            &Self::Freeverb { id, .. } => id,
            &Self::ConvolutionMono { id, .. } => id,
            &Self::ConvolutionStereo { id, .. } => id,
        }
    }

    fn title(&self) -> String {
        match self {
            &Self::SystemIn => "System In",
            &Self::SystemOut => "System Out",
            &Self::BeepTest { .. } => "Beep Test",
            &Self::WhiteNoiseGen { .. } => "White Noise Generator",
            &Self::PinkNoiseGen { .. } => "Pink Noise Generator",
            &Self::StereoToMono { .. } => "Stereo To Mono",
            &Self::VolumeMono { .. } => "Volume (Mono)",
            &Self::VolumeStereo { .. } => "Volume (Stereo)",
            &Self::VolumePan { .. } => "Volume & Pan",
            &Self::FastLowpass { .. } => "Fast Lowpass",
            &Self::FastHighpass { .. } => "Fast Highpass",
            &Self::FastBandpass { .. } => "Fast Bandpass",
            &Self::SVF { .. } => "SVF",
            &Self::MixMono { .. } => "Mix (Mono)",
            &Self::MixStereo { .. } => "Mix (Stereo)",
            &Self::Sampler { .. } => "Sampler",
            &Self::Freeverb { .. } => "Freeverb",
            &Self::ConvolutionMono { .. } => "Convolution (Mono)",
            &Self::ConvolutionStereo { .. } => "Convolution (Stereo)",
        }
        .into()
    }

    fn num_inputs(&self) -> usize {
        match self {
            &Self::SystemIn => 0,
            &Self::SystemOut => 2,
            &Self::BeepTest { .. } => 0,
            &Self::WhiteNoiseGen { .. } => 0,
            &Self::PinkNoiseGen { .. } => 0,
            &Self::StereoToMono { .. } => 2,
            &Self::VolumeMono { .. } => 1,
            &Self::VolumeStereo { .. } => 2,
            &Self::VolumePan { .. } => 2,
            &Self::FastLowpass { .. } => 2,
            &Self::FastHighpass { .. } => 2,
            &Self::FastBandpass { .. } => 2,
            &Self::SVF { .. } => 2,
            &Self::MixMono { .. } => 2,
            &Self::MixStereo { .. } => 4,
            &Self::Sampler { .. } => 0,
            &Self::Freeverb { .. } => 2,
            &Self::ConvolutionMono { .. } => 1,
            &Self::ConvolutionStereo { .. } => 2,
        }
    }

    fn num_outputs(&self) -> usize {
        match self {
            &Self::SystemIn => 1,
            &Self::SystemOut => 0,
            &Self::BeepTest { .. } => 1,
            &Self::WhiteNoiseGen { .. } => 1,
            &Self::PinkNoiseGen { .. } => 1,
            &Self::StereoToMono { .. } => 1,
            &Self::VolumeMono { .. } => 1,
            &Self::VolumeStereo { .. } => 2,
            &Self::VolumePan { .. } => 2,
            &Self::FastLowpass { .. } => 2,
            &Self::FastHighpass { .. } => 2,
            &Self::FastBandpass { .. } => 2,
            &Self::SVF { .. } => 2,
            &Self::MixMono { .. } => 1,
            &Self::MixStereo { .. } => 2,
            &Self::Sampler { .. } => 2,
            &Self::Freeverb { .. } => 2,
            &Self::ConvolutionMono { .. } => 1,
            &Self::ConvolutionStereo { .. } => 2,
        }
    }
}

struct DemoViewer<'a> {
    audio_system: &'a mut AudioSystem,
}

impl<'a> DemoViewer<'a> {
    fn remove_edge(&mut self, from: OutPinId, to: InPinId, snarl: &mut Snarl<GuiAudioNode>) {
        let Some(src_node) = snarl.get_node(from.node) else {
            return;
        };
        let Some(dst_node) = snarl.get_node(to.node) else {
            return;
        };
        let src_node = src_node.node_id(&self.audio_system);
        let dst_node = dst_node.node_id(&self.audio_system);

        self.audio_system
            .disconnect(src_node, dst_node, from.output as u32, to.input as u32);

        snarl.disconnect(from, to);
    }
}

impl<'a> SnarlViewer<GuiAudioNode> for DemoViewer<'a> {
    fn drop_inputs(&mut self, pin: &InPin, snarl: &mut Snarl<GuiAudioNode>) {
        for from in pin.remotes.iter() {
            self.remove_edge(*from, pin.id, snarl);
        }
    }

    fn drop_outputs(&mut self, pin: &OutPin, snarl: &mut Snarl<GuiAudioNode>) {
        for to in pin.remotes.iter() {
            self.remove_edge(pin.id, *to, snarl);
        }
    }

    fn disconnect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<GuiAudioNode>) {
        self.remove_edge(from.id, to.id, snarl);
    }

    fn connect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<GuiAudioNode>) {
        let src_node = snarl
            .get_node(from.id.node)
            .unwrap()
            .node_id(&self.audio_system);
        let dst_node = snarl
            .get_node(to.id.node)
            .unwrap()
            .node_id(&self.audio_system);

        if let Err(e) = self.audio_system.connect(
            src_node,
            dst_node,
            from.id.output as u32,
            to.id.input as u32,
        ) {
            tracing::error!("{}", e);
            return;
        }

        snarl.connect(from.id, to.id);
    }

    fn title(&mut self, node: &GuiAudioNode) -> String {
        node.title()
    }

    fn show_header(
        &mut self,
        node: egui_snarl::NodeId,
        inputs: &[InPin],
        outputs: &[OutPin],
        ui: &mut Ui,
        snarl: &mut Snarl<GuiAudioNode>,
    ) {
        // Override header style to prevent text from being selected when
        // dragging windows
        let _ = (inputs, outputs);
        ui.ctx()
            .style_mut(|style| style.interaction.selectable_labels = false);
        ui.label(self.title(&snarl[node]));
    }

    fn inputs(&mut self, node: &GuiAudioNode) -> usize {
        node.num_inputs()
    }

    fn outputs(&mut self, node: &GuiAudioNode) -> usize {
        node.num_outputs()
    }

    fn show_input(
        &mut self,
        _pin: &InPin,
        _ui: &mut Ui,
        _snarl: &mut Snarl<GuiAudioNode>,
    ) -> impl SnarlPin + 'static {
        PinInfo::square().with_fill(CABLE_COLOR)
    }

    fn show_output(
        &mut self,
        _pin: &OutPin,
        _ui: &mut Ui,
        _snarl: &mut Snarl<GuiAudioNode>,
    ) -> impl SnarlPin + 'static {
        PinInfo::square().with_fill(CABLE_COLOR)
    }

    fn has_graph_menu(&mut self, _pos: egui::Pos2, _snarl: &mut Snarl<GuiAudioNode>) -> bool {
        true
    }

    fn show_graph_menu(&mut self, pos: egui::Pos2, ui: &mut Ui, snarl: &mut Snarl<GuiAudioNode>) {
        ui.label("Add node");
        if ui.button("Beep Test").clicked() {
            let node = self.audio_system.add_node(NodeType::BeepTest);
            snarl.insert_node(pos, node);
            ui.close_kind(UiKind::Menu);
        }
        if ui.button("White Noise Generator").clicked() {
            let node = self.audio_system.add_node(NodeType::WhiteNoiseGen);
            snarl.insert_node(pos, node);
            ui.close_kind(UiKind::Menu);
        }
        if ui.button("Pink Noise Generator").clicked() {
            let node = self.audio_system.add_node(NodeType::PinkNoiseGen);
            snarl.insert_node(pos, node);
            ui.close_kind(UiKind::Menu);
        }
        if ui.button("Stereo To Mono").clicked() {
            let node = self.audio_system.add_node(NodeType::StereoToMono);
            snarl.insert_node(pos, node);
            ui.close_kind(UiKind::Menu);
        }
        ui.menu_button("Volume", |ui| {
            if ui.button("Volume (mono)").clicked() {
                let node = self.audio_system.add_node(NodeType::VolumeMono);
                snarl.insert_node(pos, node);
                ui.close_kind(UiKind::Menu);
            }
            if ui.button("Volume (stereo)").clicked() {
                let node = self.audio_system.add_node(NodeType::VolumeStereo);
                snarl.insert_node(pos, node);
                ui.close_kind(UiKind::Menu);
            }
        });
        if ui.button("Volume & Pan").clicked() {
            let node = self.audio_system.add_node(NodeType::VolumePan);
            snarl.insert_node(pos, node);
            ui.close_kind(UiKind::Menu);
        }
        if ui.button("Fast Lowpass").clicked() {
            let node = self.audio_system.add_node(NodeType::FastLowpass);
            snarl.insert_node(pos, node);
            ui.close_kind(UiKind::Menu);
        }
        if ui.button("Fast Highpass").clicked() {
            let node = self.audio_system.add_node(NodeType::FastHighpass);
            snarl.insert_node(pos, node);
            ui.close_kind(UiKind::Menu);
        }
        if ui.button("Fast Bandpass").clicked() {
            let node = self.audio_system.add_node(NodeType::FastBandpass);
            snarl.insert_node(pos, node);
            ui.close_kind(UiKind::Menu);
        }
        if ui.button("SVF").clicked() {
            let node = self.audio_system.add_node(NodeType::SVF);
            snarl.insert_node(pos, node);
            ui.close_kind(UiKind::Menu);
        }
        if ui.button("Mix (Mono)").clicked() {
            let node = self.audio_system.add_node(NodeType::MixMono);
            snarl.insert_node(pos, node);
            ui.close_kind(UiKind::Menu);
        }
        if ui.button("Mix (Stereo)").clicked() {
            let node = self.audio_system.add_node(NodeType::MixStereo);
            snarl.insert_node(pos, node);
            ui.close_kind(UiKind::Menu);
        }
        if ui.button("Sampler").clicked() {
            let node = self.audio_system.add_node(NodeType::Sampler);
            snarl.insert_node(pos, node);
            ui.close_kind(UiKind::Menu);
        }
        if ui.button("Freeverb").clicked() {
            let node = self.audio_system.add_node(NodeType::Freeverb);
            snarl.insert_node(pos, node);
            ui.close_kind(UiKind::Menu);
        }
        // Mono section
        ui.menu_button("Mix", |ui| {
            if ui.button("Mix (Mono)").clicked() {
                let node = self.audio_system.add_node(NodeType::MixMono);
                snarl.insert_node(pos, node);
                ui.close_kind(UiKind::Menu);
            }
            if ui.button("Mix (Stereo)").clicked() {
                let node = self.audio_system.add_node(NodeType::MixStereo);
                snarl.insert_node(pos, node);
                ui.close_kind(UiKind::Menu);
            }
        });
        ui.menu_button("Convolution", |ui| {
            if ui.button("Convolution (Mono)").clicked() {
                let node = self.audio_system.add_node(NodeType::ConvolutionMono);
                snarl.insert_node(pos, node);
                ui.close_kind(UiKind::Menu);
            }
            if ui.button("Convolution (Stereo)").clicked() {
                let node = self.audio_system.add_node(NodeType::ConvolutionStereo);
                snarl.insert_node(pos, node);
                ui.close_kind(UiKind::Menu);
            }
        });
    }

    fn has_dropped_wire_menu(
        &mut self,
        _src_pins: AnyPins,
        _snarl: &mut Snarl<GuiAudioNode>,
    ) -> bool {
        false
    }

    fn has_node_menu(&mut self, _node: &GuiAudioNode) -> bool {
        true
    }

    fn show_node_menu(
        &mut self,
        node: egui_snarl::NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut Ui,
        snarl: &mut Snarl<GuiAudioNode>,
    ) {
        let n = snarl.get_node(node).unwrap();

        match &n {
            GuiAudioNode::SystemIn | GuiAudioNode::SystemOut => {}
            _ => {
                ui.label("Node menu");
                if ui.button("Remove").clicked() {
                    self.audio_system.remove_node(n.node_id(&self.audio_system));
                    snarl.remove_node(node);
                    ui.close_kind(UiKind::Menu);
                }
            }
        }
    }

    fn has_on_hover_popup(&mut self, _: &GuiAudioNode) -> bool {
        false
    }

    fn has_body(&mut self, node: &GuiAudioNode) -> bool {
        match node {
            GuiAudioNode::SystemIn { .. }
            | GuiAudioNode::SystemOut { .. }
            | GuiAudioNode::StereoToMono { .. } => false,
            _ => true,
        }
    }

    fn show_body(
        &mut self,
        node_id: egui_snarl::NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut Ui,
        snarl: &mut Snarl<GuiAudioNode>,
    ) {
        match snarl.get_node_mut(node_id).unwrap() {
            GuiAudioNode::BeepTest { id, params } => {
                ui.vertical(|ui| {
                    let mut linear_volume = params.volume.linear();
                    if ui
                        .add(egui::Slider::new(&mut linear_volume, 0.0..=1.0).text("volume"))
                        .changed()
                    {
                        params.volume = Volume::Linear(linear_volume);
                    }

                    ui.add(
                        egui::Slider::new(&mut params.freq_hz, 20.0..=20_000.0)
                            .logarithmic(true)
                            .text("frequency"),
                    );

                    ui.checkbox(&mut params.enabled, "enabled");

                    params.update_memo(&mut self.audio_system.event_queue(*id));
                });
            }
            GuiAudioNode::WhiteNoiseGen { id, params } => {
                ui.vertical(|ui| {
                    let mut linear_volume = params.volume.linear();
                    if ui
                        .add(egui::Slider::new(&mut linear_volume, 0.0..=0.5).text("volume"))
                        .changed()
                    {
                        params.volume = Volume::Linear(linear_volume);
                    }

                    ui.checkbox(&mut params.enabled, "enabled");

                    params.update_memo(&mut self.audio_system.event_queue(*id));
                });
            }
            GuiAudioNode::PinkNoiseGen { id, params } => {
                ui.vertical(|ui| {
                    let mut linear_volume = params.volume.linear();
                    if ui
                        .add(egui::Slider::new(&mut linear_volume, 0.0..=0.5).text("volume"))
                        .changed()
                    {
                        params.volume = Volume::Linear(linear_volume);
                    }

                    ui.checkbox(&mut params.enabled, "enabled");

                    params.update_memo(&mut self.audio_system.event_queue(*id));
                });
            }
            GuiAudioNode::VolumeMono { id, params } | GuiAudioNode::VolumeStereo { id, params } => {
                let mut linear_volume = params.volume.linear();
                if ui
                    .add(egui::Slider::new(&mut linear_volume, 0.0..=2.0).text("volume"))
                    .changed()
                {
                    params.volume = Volume::Linear(linear_volume);
                    params.update_memo(&mut self.audio_system.event_queue(*id));
                }
            }
            GuiAudioNode::VolumePan { id, params } => {
                ui.vertical(|ui| {
                    let mut linear_volume = params.volume.linear();
                    if ui
                        .add(egui::Slider::new(&mut linear_volume, 0.0..=2.0).text("volume"))
                        .changed()
                    {
                        params.volume = Volume::Linear(linear_volume);
                    }

                    ui.add(egui::Slider::new(&mut params.pan, -1.0..=1.0).text("pan"));

                    params.update_memo(&mut self.audio_system.event_queue(*id));
                });
            }
            GuiAudioNode::FastLowpass { id, params } => {
                ui.vertical(|ui| {
                    ui.add(
                        egui::Slider::new(&mut params.cutoff_hz, MIN_HZ..=MAX_HZ)
                            .logarithmic(true)
                            .text("cutoff hz"),
                    );

                    ui.checkbox(&mut params.enabled, "enabled");

                    params.update_memo(&mut self.audio_system.event_queue(*id));
                });
            }
            GuiAudioNode::FastHighpass { id, params } => {
                ui.vertical(|ui| {
                    ui.add(
                        egui::Slider::new(&mut params.cutoff_hz, MIN_HZ..=MAX_HZ)
                            .logarithmic(true)
                            .text("cutoff hz"),
                    );

                    ui.checkbox(&mut params.enabled, "enabled");

                    params.update_memo(&mut self.audio_system.event_queue(*id));
                });
            }
            GuiAudioNode::FastBandpass { id, params } => {
                ui.vertical(|ui| {
                    ui.add(
                        egui::Slider::new(&mut params.cutoff_hz, MIN_HZ..=MAX_HZ)
                            .logarithmic(true)
                            .text("cutoff hz"),
                    );

                    ui.checkbox(&mut params.enabled, "enabled");

                    params.update_memo(&mut self.audio_system.event_queue(*id));
                });
            }
            GuiAudioNode::SVF { id, params } => {
                ui.vertical(|ui| {
                    egui::ComboBox::from_label("filter type")
                        .selected_text(match params.filter_type {
                            SvfType::Lowpass => "Lowpass",
                            SvfType::LowpassX2 => "Lowpass x2",
                            SvfType::Highpass => "Highpass",
                            SvfType::HighpassX2 => "Highpass X2",
                            SvfType::Bandpass => "Bandpass",
                            SvfType::LowShelf => "Low Shelf",
                            SvfType::HighShelf => "High Shelf",
                            SvfType::Bell => "Bell",
                            SvfType::Notch => "Notch",
                            SvfType::Allpass => "Allpass",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut params.filter_type,
                                SvfType::Lowpass,
                                "Lowpass",
                            );
                            ui.selectable_value(
                                &mut params.filter_type,
                                SvfType::LowpassX2,
                                "Lowpass X2",
                            );
                            ui.selectable_value(
                                &mut params.filter_type,
                                SvfType::Highpass,
                                "Highpass",
                            );
                            ui.selectable_value(
                                &mut params.filter_type,
                                SvfType::HighpassX2,
                                "HighpassX2",
                            );
                            ui.selectable_value(
                                &mut params.filter_type,
                                SvfType::Bandpass,
                                "Bandpass",
                            );
                            ui.selectable_value(
                                &mut params.filter_type,
                                SvfType::LowShelf,
                                "Low Shelf",
                            );
                            ui.selectable_value(
                                &mut params.filter_type,
                                SvfType::HighShelf,
                                "HighShelf",
                            );
                            ui.selectable_value(&mut params.filter_type, SvfType::Bell, "Bell");
                            ui.selectable_value(&mut params.filter_type, SvfType::Notch, "Notch");
                            ui.selectable_value(
                                &mut params.filter_type,
                                SvfType::Allpass,
                                "Allpass",
                            );
                        });

                    ui.add(
                        egui::Slider::new(&mut params.cutoff_hz, MIN_HZ..=MAX_HZ)
                            .logarithmic(true)
                            .text("cutoff hz"),
                    );

                    ui.add(
                        egui::Slider::new(&mut params.q_factor, DEFAULT_MIN_Q..=DEFAULT_MAX_Q)
                            .logarithmic(true)
                            .text("q factor"),
                    );

                    let mut db_gain = params.gain.decibels();
                    if ui
                        .add(egui::Slider::new(&mut db_gain, -24.0..=24.0).text("gain"))
                        .changed()
                    {
                        params.gain = Volume::Decibels(db_gain);
                    }

                    ui.checkbox(&mut params.enabled, "enabled");

                    params.update_memo(&mut self.audio_system.event_queue(*id));
                });
            }
            GuiAudioNode::MixMono { id, params } | GuiAudioNode::MixStereo { id, params } => {
                ui.vertical(|ui| {
                    let mut linear_volume = params.volume.linear();
                    if ui
                        .add(egui::Slider::new(&mut linear_volume, 0.0..=2.0).text("volume"))
                        .changed()
                    {
                        params.volume = Volume::Linear(linear_volume);
                        params.update_memo(&mut self.audio_system.event_queue(*id));
                    }

                    let mut mix = params.mix.get();
                    ui.add(egui::Slider::new(&mut mix, 0.0..=1.0).text("mix"));
                    params.mix = Mix::new(mix);

                    fade_curve_ui(ui, &mut params.fade_curve);

                    params.update_memo(&mut self.audio_system.event_queue(*id));
                });
            }
            GuiAudioNode::Sampler { id, params } => {
                let mem_id = id.0.to_bits().to_string().into();
                let selection = ui
                    .memory(|mem| mem.data.get_temp::<Option<usize>>(mem_id))
                    .flatten();

                ui.vertical(|ui| {
                    egui::ComboBox::from_label("sample")
                        .selected_text(match selection {
                            Some(sample_index) => {
                                SAMPLE_PATHS[sample_index].rsplit("/").next().unwrap()
                            }
                            None => "None",
                        })
                        .wrap_mode(egui::TextWrapMode::Truncate)
                        .show_ui(ui, |ui| {
                            for sample_index in 0..SAMPLE_PATHS.len() {
                                if ui
                                    .selectable_value(
                                        &mut params.sample,
                                        Some(self.audio_system.samples[sample_index].clone()),
                                        SAMPLE_PATHS[sample_index].rsplit("/").next().unwrap(),
                                    )
                                    .clicked()
                                {
                                    ui.memory_mut(|mem| {
                                        mem.data.insert_temp::<Option<usize>>(
                                            mem_id,
                                            Some(sample_index),
                                        );
                                    });
                                    params.set_sample(
                                        self.audio_system.samples[sample_index].clone(),
                                    );
                                }
                            }
                        });

                    let mut volume = params.volume.linear();
                    if ui
                        .add(egui::Slider::new(&mut volume, 0.0..=1.0).text("volume"))
                        .changed()
                    {
                        params.volume = Volume::Linear(volume);
                    }

                    let mut repeat = matches!(params.repeat_mode, RepeatMode::RepeatEndlessly);
                    if ui.checkbox(&mut repeat, "repeat").clicked() {
                        params.repeat_mode = match repeat {
                            true => RepeatMode::RepeatEndlessly,
                            false => RepeatMode::PlayOnce,
                        };
                    }

                    ui.horizontal(|ui| {
                        if ui.button("Stop").clicked() {
                            params.stop();
                        }
                        if ui.button("Play").clicked() {
                            params.start_or_restart();
                        }
                    });
                });

                params.update_memo(&mut self.audio_system.event_queue(*id));
            }
            GuiAudioNode::Freeverb { id, params } => {
                ui.vertical(|ui| {
                    ui.add(egui::Slider::new(&mut params.room_size, 0.0..=1.0).text("room size"));
                    ui.add(egui::Slider::new(&mut params.damping, 0.0..=1.0).text("damping"));
                    ui.add(egui::Slider::new(&mut params.width, 0.0..=1.0).text("width"));

                    ui.horizontal(|ui| {
                        if ui.button("Reset").clicked() {
                            params.reset.notify();
                        }
                        if !params.pause {
                            if ui.button("Pause").clicked() {
                                params.pause = true;
                            }
                        } else {
                            if ui.button("Unpause").clicked() {
                                params.pause = false;
                            }
                        }
                    });
                });

                params.update_memo(&mut self.audio_system.event_queue(*id));
            }
            GuiAudioNode::ConvolutionMono { id, params } => {
                convolution_ui(ui, params, self.audio_system, *id);
                params.update_memo(&mut self.audio_system.event_queue(*id));
            }
            GuiAudioNode::ConvolutionStereo { id, params } => {
                convolution_ui(ui, params, self.audio_system, *id);
                params.update_memo(&mut self.audio_system.event_queue(*id));
            }
            _ => {}
        }
    }
}

// Reusable ui to show a fade curve
fn fade_curve_ui(ui: &mut Ui, curve: &mut FadeCurve) {
    egui::ComboBox::from_label("fade curve")
        .selected_text(match curve {
            FadeCurve::EqualPower3dB => "Equal Power 3dB",
            FadeCurve::EqualPower6dB => "Equal Power 6dB",
            FadeCurve::SquareRoot => "Square Root",
            FadeCurve::Linear => "Linear",
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(curve, FadeCurve::EqualPower3dB, "Equal Power 3dB");
            ui.selectable_value(curve, FadeCurve::EqualPower6dB, "Equal Power 6dB");
            ui.selectable_value(curve, FadeCurve::SquareRoot, "Square Root");
            ui.selectable_value(curve, FadeCurve::Linear, "Linear");
        });
}

// Channel-independent UI for convolution
fn convolution_ui<const CHANNELS: usize>(
    ui: &mut Ui,
    params: &mut Memo<ConvolutionNode<CHANNELS>>,
    audio_system: &mut AudioSystem,
    node_id: NodeID,
) {
    ui.vertical(|ui| {
        ui.add(
            egui::Slider::from_get_set(0.0..=1.0, |val: Option<f64>| {
                if let Some(val) = val {
                    params.mix = Mix::new(val as f32);
                }
                params.mix.get() as f64
            })
            .text("mix"),
        );
        fade_curve_ui(ui, &mut params.fade_curve);

        let ir_sample_id = format!("ir_sample_id_{}", ui.id().value());
        let current_ir_sample_index: Option<usize> = ui
            .memory(|mem| {
                mem.data
                    .get_temp::<Option<usize>>(ir_sample_id.clone().into())
            })
            .flatten();

        egui::ComboBox::from_label("Impulse response")
            .selected_text(match current_ir_sample_index {
                Some(sample_index) => audio_system.ir_samples[sample_index].0,
                None => "None",
            })
            .show_ui(ui, |ui| {
                let mut temp_current_ir = current_ir_sample_index.clone();
                let events = audio_system
                    .ir_samples
                    .iter()
                    .enumerate()
                    .filter_map(|(sample_index, (name, sample))| {
                        ui.selectable_value(&mut temp_current_ir, Some(sample_index), *name)
                            .clicked()
                            .then_some(|| {
                                let ir = ImpulseResponse::new(sample.clone());
                                NodeEventType::custom(Some(ir))
                            })
                    })
                    .next();

                if let Some(event) = events {
                    audio_system.queue_event(node_id, event());
                    ui.memory_mut(|mem| {
                        mem.data
                            .insert_temp(ir_sample_id.clone().into(), temp_current_ir);
                    });
                }
            });

        let mut linear_volume = params.wet_gain.linear();
        if ui
            .add(egui::Slider::new(&mut linear_volume, 0.0..=1.0).text("wet gain"))
            .changed()
        {
            params.wet_gain = Volume::Linear(linear_volume);
        }

        ui.horizontal(|ui| {
            if !params.pause {
                if ui.button("Pause").clicked() {
                    params.pause = true;
                }
            } else {
                if ui.button("Play").clicked() {
                    params.pause = false;
                }
            }
        });
    });
}

pub struct DemoApp {
    snarl: Snarl<GuiAudioNode>,
    style: SnarlStyle,
    snarl_ui_id: Option<Id>,
    audio_system: AudioSystem,
}

impl DemoApp {
    pub fn new() -> Self {
        let mut snarl = Snarl::new();
        let style = SnarlStyle {
            max_scale: Some(1.0),
            ..Default::default()
        };

        snarl.insert_node(egui::Pos2 { x: 0.0, y: 0.0 }, GuiAudioNode::SystemOut);

        DemoApp {
            snarl,
            style,
            snarl_ui_id: None,
            audio_system: AudioSystem::new(),
        }
    }
}

impl App for DemoApp {
    fn update(&mut self, cx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("top_panel").show(cx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    ui.menu_button("Menu", |ui| {
                        if ui.button("Quit").clicked() {
                            cx.send_viewport_cmd(egui::ViewportCommand::Close)
                        }
                    });
                    ui.add_space(16.0);
                }

                egui::widgets::global_theme_preference_switch(ui);

                if ui.button("Clear All").clicked() {
                    self.audio_system.reset();

                    self.snarl = Default::default();
                    self.snarl
                        .insert_node(egui::Pos2 { x: 0.0, y: 0.0 }, GuiAudioNode::SystemOut);
                }
            });
        });

        egui::CentralPanel::default().show(cx, |ui| {
            self.snarl_ui_id = Some(ui.id());

            self.snarl.show(
                &mut DemoViewer {
                    audio_system: &mut self.audio_system,
                },
                &self.style,
                "snarl",
                ui,
            );
        });

        self.audio_system.update();

        if !self.audio_system.is_activated() {
            // TODO: Don't panic.
            panic!("Audio system disconnected");
        }
    }
}
