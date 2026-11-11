use crate::state::AppSettings;
use egui::{RichText, Ui};

pub enum SettingsAction {
    BackToDashboard,
    Saved,
}

#[derive(Default)]
pub struct SettingsState {
    pub show_psk: bool,
    pub status_message: Option<String>,
}

pub fn render(
    ui: &mut Ui,
    settings: &mut AppSettings,
    state: &mut SettingsState,
) -> Option<SettingsAction> {
    let mut action = None;

    // Header
    ui.horizontal(|ui| {
        if ui.button("⬅ Dashboard").clicked() {
            action = Some(SettingsAction::BackToDashboard);
        }

        ui.separator();
        ui.heading("⚙ Application Settings");

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("💾 Save Settings").clicked() {
                if let Err(e) = settings.save() {
                    state.status_message = Some(format!("Error saving settings: {e}"));
                } else {
                    state.status_message = Some("Settings successfully saved!".to_string());
                    action = Some(SettingsAction::Saved);
                }
            }
        });
    });

    if let Some(msg) = &state.status_message {
        ui.add_space(4.0);
        ui.label(RichText::new(msg).color(egui::Color32::from_rgb(46, 204, 113)));
    }

    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(8.0);

        // Security / PSK
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("🔑 Security & Pre-Shared Key (PSK)");
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Set the default PSK key used to authenticate with discovered ntx-servers.",
                )
                .weak(),
            );

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Default PSK:");
                if state.show_psk {
                    ui.text_edit_singleline(&mut settings.default_psk);
                } else {
                    ui.add(
                        egui::TextEdit::singleline(&mut settings.default_psk)
                            .password(true)
                            .hint_text("Enter default network PSK..."),
                    );
                }

                if ui
                    .button(if state.show_psk {
                        "🙈 Hide"
                    } else {
                        "👁 Show"
                    })
                    .clicked()
                {
                    state.show_psk = !state.show_psk;
                }
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Viewer ID:");
                ui.label(RichText::new(&settings.viewer_id).monospace());
                if ui.button("🔄 Regenerate").clicked() {
                    settings.viewer_id = crate::auth::generate_viewer_id();
                }
            });
        });

        ui.add_space(12.0);

        // Monitoring Configuration
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("📊 Monitoring Preferences");
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.label("Metrics stream interval:");
                egui::ComboBox::from_id_salt("refresh_rate_combo")
                    .selected_text(format!("{} ms", settings.refresh_rate_ms))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut settings.refresh_rate_ms, 250, "250 ms (Fast)");
                        ui.selectable_value(&mut settings.refresh_rate_ms, 500, "500 ms");
                        ui.selectable_value(
                            &mut settings.refresh_rate_ms,
                            1000,
                            "1000 ms (Recommended)",
                        );
                        ui.selectable_value(
                            &mut settings.refresh_rate_ms,
                            2000,
                            "2000 ms (Low overhead)",
                        );
                    });
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Plot history retention (data points):");
                ui.add(egui::Slider::new(&mut settings.history_capacity, 30..=300));
            });
        });

        ui.add_space(12.0);

        // Saved Device PSK overrides
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("💻 Device-Specific PSK Overrides");
            ui.add_space(4.0);
            ui.label(
                RichText::new("Customize individual PSKs for specific server hostnames/endpoints.")
                    .weak(),
            );

            let mut to_remove = None;
            if settings.device_psks.is_empty() {
                ui.label(RichText::new("No device-specific overrides configured.").weak());
            } else {
                egui::Grid::new("device_psks_grid")
                    .striped(true)
                    .min_col_width(120.0)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Device ID / Hostname").strong());
                        ui.label(RichText::new("PSK").strong());
                        ui.label(RichText::new("Action").strong());
                        ui.end_row();

                        for (dev_id, psk) in &mut settings.device_psks {
                            ui.label(dev_id.as_str());
                            ui.text_edit_singleline(psk);
                            if ui.button("🗑 Remove").clicked() {
                                to_remove = Some(dev_id.clone());
                            }
                            ui.end_row();
                        }
                    });
            }

            if let Some(key) = to_remove {
                settings.device_psks.remove(&key);
            }
        });

        ui.add_space(12.0);

        // About
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("ℹ About NTX Panel");
            ui.label("NTX Panel — Local Network Device Management & Monitoring");
            ui.label(format!("Viewer Version: {}", env!("CARGO_PKG_VERSION")));
            ui.label("Built with eframe, egui, tonic, mdns-sd and Rust.");
        });
    });

    action
}
