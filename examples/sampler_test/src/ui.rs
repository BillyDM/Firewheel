use std::time::Instant;

use eframe::App;
use egui::{Color32, ProgressBar};
use firewheel::nodes::sampler::RepeatMode;

use crate::system::{AudioSystem, SAMPLE_PATHS};

pub struct DemoApp {
    audio_system: AudioSystem,
    sampler_views: Vec<SamplerUIState>,
    prev_frame_instant: Instant,
}

impl DemoApp {
    pub fn new() -> Self {
        Self {
            audio_system: AudioSystem::new(),
            sampler_views: (0..SAMPLE_PATHS.len())
                .map(|i| SamplerUIState {
                    text: match i {
                        0 => "swish",
                        1 => "birds",
                        2 => "beep",
                        _ => "bird ambiance",
                    },
                    percent_volume: 100.0,
                    repeat: i == 3,
                })
                .collect(),
            prev_frame_instant: Instant::now(),
        }
    }
}

impl App for DemoApp {
    fn update(&mut self, cx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = (now - self.prev_frame_instant).as_secs_f32();
        self.prev_frame_instant = now;

        self.audio_system.update_meters(dt);

        egui::TopBottomPanel::top("top_panel").show(cx, |ui| {
            egui::menu::bar(ui, |ui| {
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
            for (i, sampler_state) in self.sampler_views.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(sampler_state.text);

                    if ui.button("Start or Restart").clicked() {
                        self.audio_system.start_or_restart(
                            i,
                            sampler_state.percent_volume / 100.0,
                            if sampler_state.repeat {
                                RepeatMode::RepeatEndlessly
                            } else {
                                RepeatMode::PlayOnce
                            },
                        );
                    }

                    if self.audio_system.playback_state(i).is_playing() {
                        if ui.button("Pause").clicked() {
                            self.audio_system.pause(i);
                        }
                    } else {
                        if ui.button("Resume").clicked() {
                            self.audio_system.resume(i);
                        }
                    }

                    if ui.button("Stop").clicked() {
                        self.audio_system.stop(i);
                    }

                    ui.checkbox(&mut sampler_state.repeat, "repeat");

                    ui.add(
                        egui::Slider::new(&mut sampler_state.percent_volume, 0.0..=100.0)
                            .text("volume"),
                    );
                });

                ui.separator();
            }

            ui.label("Note, \"repeat\" and \"volume\" are only applied when started/restarted.");

            let peak_values = self.audio_system.peak_meter_values();
            let peak_has_clipped = self.audio_system.peak_meter_has_clipped();

            ui.add(
                ProgressBar::new(peak_values[0]).fill(if peak_has_clipped[0] {
                    Color32::RED
                } else {
                    Color32::DARK_GREEN
                }),
            );
            ui.add(
                ProgressBar::new(peak_values[1]).fill(if peak_has_clipped[1] {
                    Color32::RED
                } else {
                    Color32::DARK_GREEN
                }),
            );
        });

        self.audio_system.update();

        if !self.audio_system.is_activated() {
            // TODO: Don't panic.
            panic!("Audio system disconnected");
        }

        cx.request_repaint();
    }
}

struct SamplerUIState {
    text: &'static str,
    percent_volume: f32,
    repeat: bool,
}
