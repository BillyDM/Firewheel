mod system;
mod ui;

// When compiling natively:
#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    tracing::subscriber::set_global_default(
        tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::DEBUG)
            .finish(),
    )
    .unwrap();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([575.0, 300.0])
            .with_min_inner_size([575.0, 220.0]),
        vsync: true,
        ..Default::default()
    };

    eframe::run_native(
        "firewheel sampler pool test",
        native_options,
        Box::new(|_| Ok(Box::new(ui::DemoApp::new()))),
    )
}

// When compiling to web using trunk:
#[cfg(target_arch = "wasm32")]
fn main() {
    tracing::subscriber::set_global_default(
        tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::DEBUG)
            .finish(),
    )
    .unwrap();

    let web_options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async {
        eframe::WebRunner::new()
            .start(
                "firewheel sampler pool test",
                web_options,
                Box::new(|cx| Ok(Box::new(ui::DemoApp::new(cx)))),
            )
            .await
            .expect("failed to start eframe");
    });
}
