use std::ops::RangeInclusive;

use eframe::App;
use egui::{epaint::CircleShape, Color32, Pos2, Sense, Stroke, StrokeKind};
use firewheel::{nodes::spatial_basic::DistanceModel, Volume};

use crate::system::AudioSystem;

const RANGE: RangeInclusive<f32> = -80.0..=80.0;

pub struct DemoApp {
    audio_system: AudioSystem,
}

impl DemoApp {
    pub fn new() -> Self {
        let audio_system = AudioSystem::new();

        Self { audio_system }
    }
}

impl App for DemoApp {
    fn update(&mut self, cx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("top_panel").show(cx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("Menu", |ui| {
                    ui.hyperlink_to(
                        "interactive distance model graph",
                        "https://www.desmos.com/calculator/g1pbsc5m9y",
                    );
                    ui.hyperlink_to(
                        "interactive muffle model graph",
                        "https://www.desmos.com/calculator/jxp8t9ero4",
                    );

                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        if ui.button("Quit").clicked() {
                            cx.send_viewport_cmd(egui::ViewportCommand::Close)
                        }
                    }
                });
                ui.add_space(16.0);

                egui::widgets::global_theme_preference_switch(ui);
            });
        });

        egui::CentralPanel::default().show(cx, |ui| {
            let mut updated = false;

            let mut linear_volume = self.audio_system.spatial_basic_node.volume.linear();
            if ui
                .add(
                    egui::Slider::new(&mut linear_volume, 0.0..=2.0)
                        .step_by(0.0)
                        .text("volume"),
                )
                .changed()
            {
                self.audio_system.spatial_basic_node.volume = Volume::Linear(linear_volume);
                updated = true;
            }

            updated |= ui
                .add(
                    egui::Slider::new(
                        &mut self.audio_system.spatial_basic_node.offset.x,
                        RANGE.clone(),
                    )
                    .step_by(0.0)
                    .text("x"),
                )
                .changed();

            updated |= ui
                .add(
                    egui::Slider::new(
                        &mut self.audio_system.spatial_basic_node.offset.y,
                        RANGE.clone(),
                    )
                    .step_by(0.0)
                    .text("y"),
                )
                .changed();

            updated |= ui
                .add(
                    egui::Slider::new(
                        &mut self.audio_system.spatial_basic_node.offset.z,
                        RANGE.clone(),
                    )
                    .step_by(0.0)
                    .text("z"),
                )
                .changed();

            let before = self.audio_system.spatial_basic_node.distance_model;
            egui::ComboBox::from_label("distance model")
                .selected_text(match self.audio_system.spatial_basic_node.distance_model {
                    DistanceModel::Inverse => "Inverse",
                    DistanceModel::Linear => "Linear",
                    DistanceModel::Exponential => "Exponential",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.audio_system.spatial_basic_node.distance_model,
                        DistanceModel::Inverse,
                        "Inverse",
                    );
                    ui.selectable_value(
                        &mut self.audio_system.spatial_basic_node.distance_model,
                        DistanceModel::Linear,
                        "Linear",
                    );
                    ui.selectable_value(
                        &mut self.audio_system.spatial_basic_node.distance_model,
                        DistanceModel::Exponential,
                        "Exponential",
                    );
                });

            if self.audio_system.spatial_basic_node.distance_model != before {
                updated = true;
            }

            ui.horizontal(|ui| {
                updated |= ui
                    .add(
                        egui::Slider::new(
                            &mut self.audio_system.spatial_basic_node.distance_gain_factor,
                            0.0001..=10.0,
                        )
                        .step_by(0.0)
                        .text("distance gain factor"),
                    )
                    .changed();

                ui.label("(value of 0 = no damping)");
            });

            ui.horizontal(|ui| {
                updated |= ui
                    .add(
                        egui::Slider::new(
                            &mut self.audio_system.spatial_basic_node.reference_distance,
                            0.0001..=50.0,
                        )
                        .step_by(0.0)
                        .text("reference distance"),
                    )
                    .changed();
            });

            ui.horizontal(|ui| {
                updated |= ui
                    .add(
                        egui::Slider::new(
                            &mut self.audio_system.spatial_basic_node.max_distance,
                            1.0..=500.0,
                        )
                        .step_by(0.0)
                        .text("maximum distance"),
                    )
                    .changed();

                ui.label("(only has effect with linear distance model)");
            });

            updated |= ui
                .add(
                    egui::Slider::new(
                        &mut self.audio_system.spatial_basic_node.panning_threshold,
                        0.0..=1.0,
                    )
                    .step_by(0.0)
                    .text("panning threshold"),
                )
                .changed();

            ui.horizontal(|ui| {
                updated |= ui
                    .add(
                        egui::Slider::new(
                            &mut self.audio_system.spatial_basic_node.distance_muffle_factor,
                            0.0..=10.0,
                        )
                        .step_by(0.0)
                        .text("distance muffle factor"),
                    )
                    .changed();

                ui.label("(value of 0 = no muffling)");
            });

            updated |= ui
                .add(
                    egui::Slider::new(
                        &mut self.audio_system.spatial_basic_node.max_muffle_distance,
                        1.0..=500.0,
                    )
                    .step_by(0.0)
                    .text("maximum muffle distance"),
                )
                .changed();

            updated |= ui
                .add(
                    egui::Slider::new(
                        &mut self
                            .audio_system
                            .spatial_basic_node
                            .max_distance_muffle_cutoff_hz,
                        20.0..=20_480.0,
                    )
                    .step_by(0.0)
                    .logarithmic(true)
                    .text("max distance muffle cutoff hz"),
                )
                .changed();

            updated |= ui
                .add(
                    egui::Slider::new(
                        &mut self.audio_system.spatial_basic_node.muffle_cutoff_hz,
                        20.0..=20_480.0,
                    )
                    .step_by(0.0)
                    .logarithmic(true)
                    .text("muffle cutoff hz"),
                )
                .changed();

            updated |= ui
                .add(egui::Checkbox::new(
                    &mut self.audio_system.spatial_basic_node.downmix,
                    "downmix stereo to mono",
                ))
                .changed();

            updated |= ui
                .add(
                    egui::Slider::new(
                        &mut self.audio_system.spatial_basic_node.min_gain,
                        0.0..=1.0,
                    )
                    .logarithmic(true)
                    .step_by(0.0)
                    .text("minimum gain (raw amplitude, not decibels)"),
                )
                .changed();

            updated |= ui
                .add(
                    egui::Slider::new(
                        &mut self.audio_system.spatial_basic_node.smooth_seconds,
                        0.0..=0.15,
                    )
                    .step_by(0.0)
                    .text("smoothing filter seconds"),
                )
                .changed();

            let offset = &mut self.audio_system.spatial_basic_node.offset;
            let x = &mut offset.x;
            let z = &mut offset.z;

            updated |= ui
                .add(XYPad::new(x, z, RANGE.clone(), RANGE.clone(), 200.0))
                .changed();

            if updated {
                let mut queue = self
                    .audio_system
                    .cx
                    .event_queue(self.audio_system.spatial_basic_node_id);

                self.audio_system.spatial_basic_node.update_memo(&mut queue);
            }
        });

        self.audio_system.update();
    }
}

struct XYPad<'a> {
    x_value: &'a mut f32,
    y_value: &'a mut f32,
    x_range: RangeInclusive<f32>,
    y_range: RangeInclusive<f32>,
    width: f32,
}

impl<'a> XYPad<'a> {
    pub fn new(
        x_value: &'a mut f32,
        y_value: &'a mut f32,
        x_range: RangeInclusive<f32>,
        y_range: RangeInclusive<f32>,
        width: f32,
    ) -> Self {
        Self {
            x_value,
            y_value,
            x_range,
            y_range,
            width,
        }
    }
}

impl<'a> egui::Widget for XYPad<'a> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let (mut response, painter) = ui.allocate_painter(
            egui::Vec2::new(self.width, self.width),
            Sense::click_and_drag(),
        );

        let x_range_span = *self.x_range.end() - *self.x_range.start();
        let y_range_span = *self.y_range.end() - *self.y_range.start();

        let x_normal = (*self.x_value - *self.x_range.start()) / x_range_span;
        let y_normal = (*self.y_value - *self.y_range.start()) / y_range_span;

        let handle_pos = Pos2::new(
            response.rect.left() + (response.rect.size().x * x_normal),
            response.rect.top() + (response.rect.size().y * y_normal),
        );

        painter.rect_stroke(
            response.rect.expand(-1.0),
            0.0,
            Stroke::new(1.0, Color32::DARK_GRAY),
            StrokeKind::Middle,
        );

        painter.add(CircleShape::filled(
            response.rect.center(),
            3.0,
            Color32::DARK_GRAY,
        ));

        painter.add(CircleShape::filled(handle_pos, 3.0, Color32::DARK_GREEN));

        let point_id = response.id.with(0);
        let point_response = ui.interact(response.rect, point_id, Sense::click_and_drag());

        if let Some(point_pos) = point_response.interact_pointer_pos() {
            let x_normal = (point_pos.x - response.rect.left()) / response.rect.size().x;
            let y_normal = (point_pos.y - response.rect.top()) / response.rect.size().y;

            *self.x_value = (*self.x_range.start() + (x_range_span * x_normal))
                .clamp(*self.x_range.start(), *self.x_range.end());
            *self.y_value = (*self.y_range.start() + (y_range_span * y_normal))
                .clamp(*self.y_range.start(), *self.y_range.end());

            response.mark_changed();
        }

        response
    }
}
