use eframe::App;
use firewheel::{nodes::volume_pan::VolumePanNode, Volume};

use crate::system::AudioSystem;

pub struct DemoApp {
    audio_system: AudioSystem,
}

impl DemoApp {
    pub fn new() -> Self {
        Self {
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
            });
        });

        egui::CentralPanel::default().show(cx, |ui| {
            ui.label("Default VolumePan FX Chain");

            if ui.button("Play").clicked() {
                self.audio_system.sampler_node.start_or_restart();

                // The `worker_id` can be later used to reference this piece of work being done.
                let _worker_id = self.audio_system.sampler_pool_1.new_worker(
                    &self.audio_system.sampler_node,
                    None, // Apply the changes immediately
                    true, // Steal worker if pool is full
                    &mut self.audio_system.cx,
                    |fx_chain_state, cx| {
                        // While we don't change these parameters in this example, in a typical app
                        // you would want to reset the parameters to the desired state when playing
                        // a new sample.
                        fx_chain_state.fx_chain.set_params(
                            VolumePanNode::default(),
                            None, // Apply the changes immediately
                            &fx_chain_state.node_ids,
                            cx,
                        );
                    },
                );
            }

            self.audio_system.sampler_pool_1.poll(&self.audio_system.cx);

            let num_active_works = self.audio_system.sampler_pool_1.num_active_workers();
            ui.label(format!("Num active workers: {}", num_active_works));

            ui.separator();

            ui.label("Custom FX Chain");

            if ui.button("Play").clicked() {
                self.audio_system.sampler_node.start_or_restart();

                // The `worker_id` can be later used to reference this piece of work being done.
                let _worker_id = self.audio_system.sampler_pool_2.new_worker(
                    &self.audio_system.sampler_node,
                    None, // Apply the changes immediately
                    true, // Steal worker if pool is full
                    &mut self.audio_system.cx,
                    |fx_chain_state, cx| {
                        // While we don't change these parameters in this example, in a typical app
                        // you would want to reset the parameters to the desired state when playing
                        // a new sample.
                        fx_chain_state.fx_chain.volume.volume = Volume::UNITY_GAIN;

                        // The nodes IDs appear in the same order as what was returned in
                        // [`MyCustomChain::construct_and_connect`].
                        fx_chain_state.fx_chain.volume.update_memo(
                            &mut cx.event_queue_scheduled(fx_chain_state.node_ids[1], None),
                        );
                    },
                );
            }

            self.audio_system.sampler_pool_2.poll(&self.audio_system.cx);

            let num_active_works = self.audio_system.sampler_pool_2.num_active_workers();
            ui.label(format!("Num active workers: {}", num_active_works));
        });

        self.audio_system.update();

        cx.request_repaint();
    }
}
