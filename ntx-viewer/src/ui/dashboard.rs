use crate::state::{ConnectionStatus, Device, ManualDevice};
use egui::{Color32, RichText, Stroke, Ui, Vec2};
use std::collections::HashMap;

pub enum DashboardAction {
    Connect { id: String, psk: Option<String> },
    Disconnect(String),
    OpenMonitor(String),
    OpenTerminal(String),
    AddManualDevice(ManualDevice),
    RemoveDevice(String),
    RefreshDiscovery,
}

#[derive(Default)]
pub struct DashboardState {
    pub search_query: String,
    pub show_add_modal: bool,
    pub manual_hostname: String,
    pub manual_address: String,
    pub manual_port: String,
    pub manual_psk: String,
    pub device_psk_inputs: HashMap<String, String>,
    pub show_psk_map: HashMap<String, bool>,
}

impl DashboardState {
    pub fn new() -> Self {
        Self {
            manual_port: "9850".to_string(),
            ..Default::default()
        }
    }
}

pub fn render(
    ui: &mut Ui,
    state: &mut DashboardState,
    devices: &[Device],
) -> Option<DashboardAction> {
    let mut action = None;

    // Header toolbar
    ui.horizontal(|ui| {
        ui.heading("📡 Local Network Devices");

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("➕ Add Device").clicked() {
                state.show_add_modal = !state.show_add_modal;
            }

            if ui.button("🔄 Rescan mDNS").clicked() {
                action = Some(DashboardAction::RefreshDiscovery);
            }

            ui.add(
                egui::TextEdit::singleline(&mut state.search_query)
                    .hint_text("🔍 Search hostname or IP..."),
            );
        });
    });

    ui.separator();

    // Manual add device expandable section
    if state.show_add_modal {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("Add Device Manually");
            ui.horizontal(|ui| {
                ui.label("Hostname:");
                ui.text_edit_singleline(&mut state.manual_hostname);

                ui.label("IP / Host:");
                ui.text_edit_singleline(&mut state.manual_address);

                ui.label("Port:");
                ui.add(egui::TextEdit::singleline(&mut state.manual_port).desired_width(60.0));

                ui.label("PSK:");
                ui.add(
                    egui::TextEdit::singleline(&mut state.manual_psk)
                        .password(true)
                        .desired_width(120.0),
                );

                if ui.button("Add").clicked() {
                    let port = state.manual_port.parse::<u16>().unwrap_or(9850);
                    let address = if state.manual_address.is_empty() {
                        "127.0.0.1".to_string()
                    } else {
                        state.manual_address.clone()
                    };
                    let hostname = if state.manual_hostname.is_empty() {
                        format!("device-{}", port)
                    } else {
                        state.manual_hostname.clone()
                    };
                    let psk = if state.manual_psk.is_empty() {
                        None
                    } else {
                        Some(state.manual_psk.clone())
                    };

                    action = Some(DashboardAction::AddManualDevice(ManualDevice {
                        hostname,
                        address,
                        port,
                        psk,
                    }));
                    state.show_add_modal = false;
                    state.manual_hostname.clear();
                    state.manual_address.clear();
                    state.manual_psk.clear();
                }

                if ui.button("Cancel").clicked() {
                    state.show_add_modal = false;
                }
            });
        });
        ui.separator();
    }

    // Filtered devices
    let query = state.search_query.to_lowercase();
    let filtered_devices: Vec<&Device> = devices
        .iter()
        .filter(|d| {
            if query.is_empty() {
                return true;
            }
            d.hostname.to_lowercase().contains(&query)
                || d.addresses.iter().any(|a| a.contains(&query))
                || d.primary_address().contains(&query)
        })
        .collect();

    // Stats bar
    ui.horizontal(|ui| {
        let total = devices.len();
        let connected = devices
            .iter()
            .filter(|d| matches!(d.status, ConnectionStatus::Connected))
            .count();

        ui.label(
            RichText::new(format!(
                "Showing {} of {} devices ({} connected)",
                filtered_devices.len(),
                total,
                connected
            ))
            .weak(),
        );
    });

    ui.add_space(8.0);

    if filtered_devices.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.label(RichText::new("🔍 No devices found").size(18.0).weak());
            ui.label(
                RichText::new(
                    "Ensure ntx-server is running on your network, or click 'Add Device' above.",
                )
                .weak(),
            );
            ui.add_space(40.0);
        });
        return action;
    }

    // Device cards in a grid/wrap layout
    egui::ScrollArea::vertical().show(ui, |ui| {
        let card_width = 340.0;
        let spacing = 12.0;

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(spacing, spacing);

            for dev in filtered_devices {
                let card_action = render_device_card(ui, dev, card_width, state);
                if card_action.is_some() {
                    action = card_action;
                }
            }
        });
    });

    action
}

fn render_device_card(
    ui: &mut Ui,
    dev: &Device,
    width: f32,
    state: &mut DashboardState,
) -> Option<DashboardAction> {
    let mut action = None;

    let (border_color, status_text, status_color) = match &dev.status {
        ConnectionStatus::Connected => (
            Color32::from_rgb(46, 204, 113),
            "● Connected",
            Color32::from_rgb(46, 204, 113),
        ),
        ConnectionStatus::Connecting => (
            Color32::from_rgb(52, 152, 219),
            "◌ Connecting...",
            Color32::from_rgb(52, 152, 219),
        ),
        ConnectionStatus::Discovered => (
            Color32::from_rgb(149, 165, 166),
            "○ Discovered",
            Color32::from_rgb(189, 195, 199),
        ),
        ConnectionStatus::AuthFailed(_) => (
            Color32::from_rgb(231, 76, 60),
            "✕ Auth Failed",
            Color32::from_rgb(231, 76, 60),
        ),
        ConnectionStatus::Error(_) => (
            Color32::from_rgb(230, 126, 34),
            "⚠ Error",
            Color32::from_rgb(230, 126, 34),
        ),
        ConnectionStatus::Disconnected => (
            Color32::from_rgb(127, 140, 141),
            "○ Disconnected",
            Color32::from_rgb(127, 140, 141),
        ),
    };

    let frame = egui::Frame::group(ui.style())
        .stroke(Stroke::new(1.5_f32, border_color))
        .inner_margin(12.0);

    frame.show(ui, |ui| {
        ui.set_width(width);

        // Header: Hostname + Status Badge
        ui.horizontal(|ui| {
            let icon = if let Some(info) = &dev.system_info {
                let os = info.os_name.to_lowercase();
                if os.contains("win") {
                    "🪟"
                } else if os.contains("raspbian") || os.contains("debian") || os.contains("ubuntu")
                {
                    "🐧"
                } else if os.contains("mac") || os.contains("darwin") {
                    "🍎"
                } else {
                    "🖥️"
                }
            } else {
                "🖥️"
            };

            ui.label(
                RichText::new(format!("{} {}", icon, dev.hostname))
                    .strong()
                    .size(16.0),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(status_text).color(status_color).small());
            });
        });

        // Subtitle: IP & Port & OS
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{}:{}", dev.primary_address(), dev.port))
                    .weak()
                    .small(),
            );

            if let Some(info) = &dev.system_info {
                ui.label(
                    RichText::new(format!("• {} {}", info.os_name, info.arch))
                        .weak()
                        .small(),
                );
            } else if !dev.version.is_empty() {
                ui.label(RichText::new(format!("• v{}", dev.version)).weak().small());
            }
        });

        ui.add_space(6.0);

        // Metrics snippet if connected
        if let Some(metrics) = &dev.latest_metrics {
            // CPU bar
            ui.horizontal(|ui| {
                ui.label(RichText::new("CPU:").small().strong());
                let cpu_val = (metrics.cpu_usage_percent / 100.0) as f32;
                ui.add(
                    egui::ProgressBar::new(cpu_val.clamp(0.0, 1.0))
                        .text(format!("{:.1}%", metrics.cpu_usage_percent))
                        .desired_height(14.0),
                );
            });

            // RAM bar
            ui.horizontal(|ui| {
                ui.label(RichText::new("RAM:").small().strong());
                let ram_pct = if metrics.total_memory_bytes > 0 {
                    (metrics.used_memory_bytes as f64 / metrics.total_memory_bytes as f64) as f32
                } else {
                    0.0
                };
                let used_gb = metrics.used_memory_bytes as f64 / 1_073_741_824.0;
                let total_gb = metrics.total_memory_bytes as f64 / 1_073_741_824.0;
                ui.add(
                    egui::ProgressBar::new(ram_pct.clamp(0.0, 1.0))
                        .text(format!("{:.1}/{:.1} GB", used_gb, total_gb))
                        .desired_height(14.0),
                );
            });
        } else {
            ui.add_space(28.0);
        }

        ui.add_space(6.0);

        // If not connected, show PSK input field and any auth error message
        if !dev.status.is_connected() {
            if let ConnectionStatus::AuthFailed(reason) = &dev.status {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("⚠ {}", reason))
                            .color(Color32::from_rgb(231, 76, 60))
                            .small(),
                    );
                });
                ui.add_space(2.0);
            }

            ui.horizontal(|ui| {
                ui.label(RichText::new("🔑 PSK:").small().strong());
                let show_psk = state.show_psk_map.entry(dev.id.clone()).or_insert(false);
                let psk_input = state
                    .device_psk_inputs
                    .entry(dev.id.clone())
                    .or_insert_with(|| dev.psk.clone().unwrap_or_default());
                ui.add(
                    egui::TextEdit::singleline(psk_input)
                        .password(!*show_psk)
                        .hint_text("Paste PSK key here...")
                        .desired_width(170.0),
                );
                if ui
                    .small_button(if *show_psk { "👁" } else { "🔒" })
                    .on_hover_text(if *show_psk { "Hide key" } else { "Show key" })
                    .clicked()
                {
                    *show_psk = !*show_psk;
                }
            });
            ui.add_space(4.0);
        }

        ui.separator();

        // Action Buttons
        ui.horizontal(|ui| {
            match &dev.status {
                ConnectionStatus::Connected => {
                    if ui.button("📊 Monitor").clicked() {
                        action = Some(DashboardAction::OpenMonitor(dev.id.clone()));
                    }

                    if ui.button("💻 Terminal").clicked() {
                        action = Some(DashboardAction::OpenTerminal(dev.id.clone()));
                    }

                    if ui.button("Disconnect").clicked() {
                        action = Some(DashboardAction::Disconnect(dev.id.clone()));
                    }
                }
                ConnectionStatus::Connecting => {
                    ui.spinner();
                    ui.label("Connecting...");
                }
                _ => {
                    let connect_text = if matches!(dev.status, ConnectionStatus::AuthFailed(_)) {
                        "🔄 Retry Connect"
                    } else {
                        "🔗 Connect"
                    };
                    if ui.button(connect_text).clicked() {
                        let entered_psk = state.device_psk_inputs.get(&dev.id).cloned();
                        action = Some(DashboardAction::Connect {
                            id: dev.id.clone(),
                            psk: entered_psk,
                        });
                    }

                    // For offline/discovered devices, remove option if manual
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("🗑")
                            .on_hover_text("Remove device")
                            .clicked()
                        {
                            action = Some(DashboardAction::RemoveDevice(dev.id.clone()));
                        }
                    });
                }
            }
        });
    });

    action
}
