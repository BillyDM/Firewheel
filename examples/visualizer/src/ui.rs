use eframe::App;
use egui::{
    emath::RectTransform,
    epaint::{self, PathStroke},
    pos2, vec2, Pos2, Rect, Ui,
};
use std::f32::consts::PI;

use crate::system::AudioSystem;

pub struct DemoApp {
    audio_system: AudioSystem,
    window_size: u32,
}

impl DemoApp {
    pub fn new() -> Self {
        let window_size = 450;

        Self {
            window_size,
            audio_system: AudioSystem::new(window_size),
        }
    }

    // A basic oscilloscope, using the audio data from the triple buffer node.
    fn draw_oscilloscope(&mut self, ui: &mut Ui) {
        const NUM_POINTS: usize = 200;

        let size = ui.available_size();
        let (_id, rect) = ui.allocate_space(size);

        let mut output = self.audio_system.triple_buffer_state.output();
        let Some(audio_data) = output.channels() else {
            return;
        };
        let frames = audio_data[0].len();

        let mut left_rect = rect.clone();
        left_rect.set_height(left_rect.height() / 2.0);
        let mut right_rect = left_rect.clone();
        right_rect = right_rect.translate(vec2(0.0, left_rect.height()));

        let to_left_rect =
            RectTransform::from_to(Rect::from_x_y_ranges(0.0..=1.0, -1.0..=1.0), left_rect);
        let to_right_rect =
            RectTransform::from_to(Rect::from_x_y_ranges(0.0..=1.0, -1.0..=1.0), right_rect);

        let build_points = |audio_data: &[f32], rect_transform: RectTransform| -> Vec<Pos2> {
            (0..NUM_POINTS)
                .map(|i| {
                    let x = i as f32 / NUM_POINTS as f32;

                    let pos = x * frames as f32;
                    let index = pos.floor() as usize;
                    let fract_index = pos.fract();

                    let s0 = audio_data.get(index).copied().unwrap_or(0.0);
                    let s1 = audio_data.get(index + 1).copied().unwrap_or(0.0);

                    let value = s0 + ((s1 - s0) * fract_index);

                    // Apply a windowing function to make the oscilloscope look
                    // "more interesting".
                    let y = value * (x * PI).sin();

                    rect_transform * pos2(x, y)
                })
                .collect()
        };

        let left_points = build_points(&audio_data[0], to_left_rect);
        let right_points = build_points(&audio_data[1], to_right_rect);

        let color = if ui.style().visuals.dark_mode {
            egui::Color32::GREEN
        } else {
            egui::Color32::DARK_GREEN
        };

        ui.painter().extend([
            epaint::Shape::line(left_points, PathStroke::new(2.0, color)),
            epaint::Shape::line(right_points, PathStroke::new(2.0, color)),
        ]);
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
            });
        });

        egui::CentralPanel::default().show(cx, |ui| {
            if ui.button("Play Sample").clicked() {
                self.audio_system.play_sample();
            }

            let mut enabled = self.audio_system.triple_buffer_params.enabled;
            ui.checkbox(&mut enabled, "enabled").changed();
            if enabled != self.audio_system.triple_buffer_params.enabled {
                self.audio_system.set_enabled(enabled);
            }

            if ui
                .add(egui::Slider::new(&mut self.window_size, 1..=2000).text("window size"))
                .changed()
            {
                self.audio_system.set_window_size(self.window_size);
            }

            ui.separator();

            self.draw_oscilloscope(ui);
        });

        self.audio_system.update();

        if !self.audio_system.is_activated() {
            // TODO: Don't panic.
            panic!("Audio system disconnected");
        }

        cx.request_repaint();
    }
}
