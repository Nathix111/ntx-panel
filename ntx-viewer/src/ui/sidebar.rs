use crate::state::{ConnectionStatus, Device, ManualDevice};
use egui::{Color32, RichText, Stroke, Ui, Vec2};

pub enum SidebarAction {
    SelectDevice(String),
    ConnectDevice(String),
    DisconnectDevice(String),
    RemoveDevice(String),
    AddManualDevice(ManualDevice),
    RefreshDiscovery,
    OpenGlobalSettings,
}

pub struct SidebarState {
    pub search_query: String,
    pub show_add_modal: bool,
    pub manual_hostname: String,
    pub manual_address: String,
    pub manual_port: String,
    pub manual_psk: String,
    pub add_error: Option<String>,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            search_query: String::new(),
            show_add_modal: false,
            manual_hostname: String::new(),
            manual_address: String::new(),
            manual_port: "9850".to_string(),
            manual_psk: String::new(),
            add_error: None,
        }
    }
}

impl SidebarState {
    pub fn new() -> Self {
        Self::default()
    }
}

pub fn render(
    ui: &mut Ui,
    state: &mut SidebarState,
    devices: &[Device],
    selected_id: Option<&str>,
) -> Option<SidebarAction> {
    let mut action = None;

    // Header Toolbar
    ui.horizontal(|ui| {
        ui.heading("⚡ NTX Panel");

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("🔄")
                .on_hover_text("Scanner le réseau local (mDNS)")
                .clicked()
            {
                action = Some(SidebarAction::RefreshDiscovery);
            }

            if ui
                .button("➕")
                .on_hover_text("Ajouter un serveur manuellement")
                .clicked()
            {
                state.show_add_modal = true;
                state.add_error = None;
            }
        });
    });

    ui.add_space(4.0);

    // Search filter bar
    ui.add(
        egui::TextEdit::singleline(&mut state.search_query)
            .hint_text("🔍 Filtrer les serveurs...")
            .desired_width(ui.available_width()),
    );

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);

    // Filtered devices list
    let query = state.search_query.trim().to_lowercase();
    let filtered_devices: Vec<&Device> = devices
        .iter()
        .filter(|d| {
            if query.is_empty() {
                true
            } else {
                d.hostname.to_lowercase().contains(&query)
                    || d.primary_address().contains(&query)
                    || d.version.to_lowercase().contains(&query)
            }
        })
        .collect();

    let online_count = devices
        .iter()
        .filter(|d| matches!(d.status, ConnectionStatus::Connected))
        .count();

    // Device List Scroll Area
    let avail_height = ui.available_height() - 48.0;
    egui::ScrollArea::vertical()
        .max_height(avail_height.max(100.0))
        .show(ui, |ui| {
            if filtered_devices.is_empty() {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    if devices.is_empty() {
                        ui.label(RichText::new("Aucun serveur enregistré").weak());
                        ui.add_space(6.0);
                        if ui.button("➕ Ajouter un serveur").clicked() {
                            state.show_add_modal = true;
                        }
                    } else {
                        ui.label(RichText::new("Aucun résultat pour cette recherche").weak());
                    }
                });
            } else {
                ui.label(
                    RichText::new(format!("SERVEURS ({})", filtered_devices.len()))
                        .size(11.0)
                        .strong()
                        .color(Color32::from_rgb(140, 140, 150)),
                );
                ui.add_space(4.0);

                for dev in filtered_devices {
                    let is_selected = selected_id == Some(&dev.id);

                    // Colors & status
                    let (dot_color, _status_tip) = match &dev.status {
                        ConnectionStatus::Connected => (Color32::from_rgb(46, 204, 113), "Connecté"),
                        ConnectionStatus::Connecting => {
                            (Color32::from_rgb(52, 152, 219), "Connexion en cours...")
                        }
                        ConnectionStatus::AuthFailed(_) => {
                            (Color32::from_rgb(231, 76, 60), "Erreur d'authentification")
                        }
                        ConnectionStatus::Error(_) => {
                            (Color32::from_rgb(230, 126, 34), "Erreur de connexion")
                        }
                        ConnectionStatus::Disconnected => {
                            (Color32::from_rgb(120, 120, 130), "Déconnecté")
                        }
                        ConnectionStatus::Discovered => {
                            (Color32::from_rgb(241, 196, 15), "Découvert")
                        }
                    };

                    let os_icon = if let Some(info) = &dev.system_info {
                        let os = info.os_name.to_lowercase();
                        if os.contains("win") {
                            "🪟"
                        } else if os.contains("raspbian")
                            || os.contains("debian")
                            || os.contains("ubuntu")
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

                    let bg_color = if is_selected {
                        Color32::from_rgb(32, 44, 62)
                    } else {
                        Color32::from_rgb(22, 24, 28)
                    };

                    let border_color = if is_selected {
                        Color32::from_rgb(52, 152, 219)
                    } else {
                        Color32::from_rgb(40, 42, 48)
                    };

                    let frame = egui::Frame::new()
                        .fill(bg_color)
                        .stroke(Stroke::new(if is_selected { 1.5_f32 } else { 1.0_f32 }, border_color))
                        .corner_radius(6.0)
                        .inner_margin(egui::Margin::symmetric(10, 7));

                    let content_resp = frame.show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        ui.horizontal(|ui| {
                            // Status Dot
                            let (dot_rect, _) =
                                ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
                            ui.painter().circle_filled(dot_rect.center(), 4.5, dot_color);

                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!("{} {}", os_icon, dev.hostname))
                                            .strong()
                                            .size(13.5),
                                    );
                                });

                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!("{}:{}", dev.primary_address(), dev.port))
                                            .size(11.0)
                                            .color(Color32::from_rgb(150, 155, 165)),
                                    );
                                });

                                // Live CPU / RAM stats snippet if connected
                                if matches!(dev.status, ConnectionStatus::Connected) {
                                    if let Some(metrics) = &dev.latest_metrics {
                                        let cpu = metrics.cpu_usage_percent;
                                        let ram_pct = if metrics.total_memory_bytes > 0 {
                                            (metrics.used_memory_bytes as f64
                                                / metrics.total_memory_bytes as f64)
                                                * 100.0
                                        } else {
                                            0.0
                                        };
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(format!(
                                                    "CPU {:.0}% • RAM {:.0}%",
                                                    cpu, ram_pct
                                                ))
                                                .size(10.5)
                                                .color(Color32::from_rgb(46, 204, 113)),
                                            );
                                        });
                                    }
                                }
                            });
                        });
                    });

                    // Click detection on device card
                    let interact = ui.interact(
                        content_resp.response.rect,
                        ui.make_persistent_id(&dev.id),
                        egui::Sense::click(),
                    );

                    if interact.clicked() {
                        action = Some(SidebarAction::SelectDevice(dev.id.clone()));
                    }

                    // Context Menu (Right Click)
                    let dev_id = dev.id.clone();
                    let is_conn = dev.status.is_connected();
                    interact.context_menu(|ui| {
                        if is_conn {
                            if ui.button("🔌 Déconnecter").clicked() {
                                action = Some(SidebarAction::DisconnectDevice(dev_id.clone()));
                                ui.close_menu();
                            }
                        } else {
                            if ui.button("⚡ Connecter").clicked() {
                                action = Some(SidebarAction::ConnectDevice(dev_id.clone()));
                                ui.close_menu();
                            }
                        }

                        ui.separator();
                        if ui.button("🗑 Supprimer de la liste").clicked() {
                            action = Some(SidebarAction::RemoveDevice(dev_id));
                            ui.close_menu();
                        }
                    });

                    ui.add_space(4.0);
                }
            }
        });

    // Footer of Sidebar
    ui.separator();
    ui.add_space(2.0);

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("🟢 {}/{} en ligne", online_count, devices.len()))
                .size(11.0)
                .color(Color32::from_rgb(160, 165, 175)),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("⚙ Paramètres")
                .on_hover_text("Paramètres généraux de l'application")
                .clicked()
            {
                action = Some(SidebarAction::OpenGlobalSettings);
            }
        });
    });

    // Add Device Modal
    if state.show_add_modal {
        egui::Window::new("➕ Ajouter un serveur")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(340.0)
            .show(ui.ctx(), |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;

                ui.label(
                    RichText::new("Entrez les informations de connexion du serveur distant :")
                        .size(12.0)
                        .weak(),
                );

                ui.horizontal(|ui| {
                    ui.label("Nom d'hôte :");
                    ui.text_edit_singleline(&mut state.manual_hostname);
                });

                ui.horizontal(|ui| {
                    ui.label("Adresse IP / Hôte :");
                    ui.text_edit_singleline(&mut state.manual_address);
                });

                ui.horizontal(|ui| {
                    ui.label("Port gRPC :");
                    ui.add(egui::TextEdit::singleline(&mut state.manual_port).desired_width(70.0));
                });

                ui.horizontal(|ui| {
                    ui.label("Clé PSK :");
                    ui.add(
                        egui::TextEdit::singleline(&mut state.manual_psk)
                            .password(true)
                            .hint_text("Optionnel si déjà configuré"),
                    );
                });

                if let Some(err) = &state.add_error {
                    ui.label(RichText::new(err).color(Color32::from_rgb(231, 76, 60)).small());
                }

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Annuler").clicked() {
                        state.show_add_modal = false;
                        state.add_error = None;
                    }

                    if ui.button("✅ Enregistrer et Ajouter").clicked() {
                        let address = state.manual_address.trim().to_string();
                        if address.is_empty() {
                            state.add_error = Some("L'adresse IP ne peut pas être vide.".to_string());
                        } else {
                            let port = state.manual_port.trim().parse::<u16>().unwrap_or(9850);
                            let hostname = if state.manual_hostname.trim().is_empty() {
                                format!("server-{}", port)
                            } else {
                                state.manual_hostname.trim().to_string()
                            };
                            let psk = if state.manual_psk.trim().is_empty() {
                                None
                            } else {
                                Some(state.manual_psk.trim().to_string())
                            };

                            let manual = ManualDevice {
                                hostname,
                                address,
                                port,
                                psk,
                            };

                            action = Some(SidebarAction::AddManualDevice(manual));
                            state.show_add_modal = false;
                            state.manual_hostname.clear();
                            state.manual_address.clear();
                            state.manual_psk.clear();
                            state.manual_port = "9850".to_string();
                            state.add_error = None;
                        }
                    }
                });
            });
    }

    action
}
