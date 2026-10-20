//! Tessera — a macOS menu bar resource monitor with a disk map.

mod app;
mod debug;
mod disk;
mod macos;
mod sampler;
mod settings;
mod state;
mod statusbar;
mod ui;

use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};

fn main() -> eframe::Result {
    let background = std::env::args().any(|a| a == "--background");

    let shared = state::Shared::new(settings::Settings::load());
    shared.disk.load_cached();
    sampler::spawn(shared.clone());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Tessera")
            .with_app_id("com.gbvanrenswoude.tessera")
            .with_inner_size([1280.0, 860.0])
            .with_min_inner_size([1040.0, 700.0])
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            .with_visible(false),
        persist_window: true,
        persistence_path: Some(tessera_core::paths::support_dir()),
        centered: true,
        renderer: eframe::Renderer::Wgpu,
        // Start as a menu bar app: no Dock icon until the dashboard opens.
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_activation_policy(ActivationPolicy::Accessory);
        })),
        ..Default::default()
    };

    eframe::run_native(
        "Tessera",
        options,
        Box::new(move |cc| Ok(Box::new(app::TesseraApp::new(cc, shared, !background)))),
    )
}
