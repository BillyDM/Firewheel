mod system;
mod ui;

// When compiling natively:
#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    #[cfg(debug_assertions)]
    simple_log::quick!("debug");
    #[cfg(not(debug_assertions))]
    simple_log::quick!("info");

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 600.0])
            .with_min_inner_size([300.0, 220.0]),
        vsync: true,
        ..Default::default()
    };

    eframe::run_native(
        "firewheel visual node graph demo",
        native_options,
        Box::new(|_| Ok(Box::new(ui::DemoApp::new()))),
    )
}

// When compiling to web using trunk:
#[cfg(target_arch = "wasm32")]
fn main() {
    let web_options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async {
        eframe::WebRunner::new()
            .start(
                "firewheel_visual_node_graph_demo",
                web_options,
                Box::new(|cx| Ok(Box::new(ui::DemoApp::new(cx)))),
            )
            .await
            .expect("failed to start eframe");
    });
}
