#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(clippy::result_large_err)]

pub mod app;
pub mod auth;
pub mod client;
pub mod discovery;
pub mod state;
pub mod terminal_emu;
pub mod tls;
pub mod ui;
#[cfg(target_os = "windows")]
pub mod win_dpi_fix;

use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn main() -> eframe::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

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

    let _guard = rt.enter();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1160.0, 720.0])
            .with_min_inner_size([720.0, 520.0])
            .with_title("NTX Panel"),
        ..Default::default()
    };

    eframe::run_native(
        "NTX Panel",
        options,
        Box::new(|cc| Ok(Box::new(app::NtxApp::new(cc, rt.clone())))),
    )
}
