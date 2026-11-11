pub mod app;
pub mod auth;
pub mod client;
pub mod discovery;
pub mod state;
pub mod ui;

use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn main() -> eframe::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ntx_viewer=info,warn".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime"),
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([800.0, 600.0])
            .with_title("NTX Panel"),
        ..Default::default()
    };

    eframe::run_native(
        "NTX Panel",
        options,
        Box::new(|cc| Ok(Box::new(app::NtxApp::new(cc, rt)))),
    )
}
