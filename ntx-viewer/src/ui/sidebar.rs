use crate::state::{ConnectionStatus, Device, ManualDevice};
use crate::ui::theme::{self, Palette};
use egui::{RichText, Stroke, Ui, Vec2};

pub enum SidebarAction {
    OpenOverview,
    SelectDevice(String),
    ConnectDevice(String),
    DisconnectDevice(String),
    RemoveDevice(String),
    AddManualDevice(ManualDevice),
    RefreshDiscovery,
    OpenGlobalSettings,
    OpenTerminal(String),
    ConfigureDevice(String),
    ToggleTheme,
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
    devices: &[&Device],
    selected_id: Option<&str>,
    is_overview_active: bool,
    is_settings_active: bool,
    dark_mode: bool,
) -> Option<SidebarAction> {
    let p = Palette::for_ui(ui);
    let mut action = None;

    // 1. Header Toolbar
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("⚡ NTX Panel")
                .size(16.5)
                .strong()
                .color(p.text_primary),
        );
        theme::pill_badge(
            ui,
            concat!("v", env!("CARGO_PKG_VERSION")),
            p.bg_subtle,
            p.text_muted,
            Some(p.border),
        );

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

    ui.add_space(6.0);

    // 2. Fleet Overview Navigation Button
    let overview_btn = egui::Button::new(
        RichText::new("⊞  Vue d'ensemble")
            .size(13.0)
            .strong()
            .color(if is_overview_active {
                p.text_primary
            } else {
                p.text_secondary
            }),
    )
    .fill(if is_overview_active {
        p.accent_bg
    } else {
        p.bg_card
    })
    .stroke(Stroke::new(
        1.0_f32,
        if is_overview_active {
            p.accent_border
        } else {
            p.border
        },
    ))
    .corner_radius(6)
    .min_size(Vec2::new(ui.available_width(), 30.0));

    if ui.add(overview_btn).clicked() {
        action = Some(SidebarAction::OpenOverview);
    }

    ui.add_space(6.0);

    // 3. Search filter bar
    ui.horizontal(|ui| {
        let search_w = if state.search_query.is_empty() {
            ui.available_width()
        } else {
            (ui.available_width() - 28.0).max(80.0)
        };
        ui.add(
            egui::TextEdit::singleline(&mut state.search_query)
                .hint_text("🔍 Filtrer les serveurs...")
                .desired_width(search_w),
        );
        if !state.search_query.is_empty()
            && ui
                .small_button("✖")
                .on_hover_text("Effacer la recherche")
                .clicked()
        {
            state.search_query.clear();
        }
    });

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);

    // 4. Filtered devices list
    let query = state.search_query.trim().to_lowercase();
    let filtered_devices: Vec<&Device> = devices
        .iter()
        .copied()
        .filter(|d| {
            if query.is_empty() {
                true
            } else {
                d.hostname.to_lowercase().contains(&query)
                    || d.primary_address().to_lowercase().contains(&query)
                    || d.addresses
                        .iter()
                        .any(|a| a.to_lowercase().contains(&query))
                    || d.version.to_lowercase().contains(&query)
            }
        })
        .collect();

    let online_count = devices
        .iter()
        .filter(|d| matches!(d.status, ConnectionStatus::Connected))
        .count();

    let avail_height = (ui.available_height() - 46.0).max(100.0);
    egui::ScrollArea::vertical()
        .max_height(avail_height)
        .show(ui, |ui| {
            if filtered_devices.is_empty() {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    if devices.is_empty() {
                        ui.label(RichText::new("Aucun serveur enregistré").color(p.text_secondary));
                        ui.add_space(6.0);
                        if theme::primary_button(ui, &p, "➕ Ajouter un serveur").clicked() {
                            state.show_add_modal = true;
                        }
                    } else {
                        ui.label(
                            RichText::new("Aucun résultat pour cette recherche")
                                .color(p.text_secondary),
                        );
                    }
                });
            } else {
                ui.label(
                    RichText::new(format!("SERVEURS ({})", filtered_devices.len()))
                        .size(10.5)
                        .strong()
                        .color(p.text_muted),
                );
                ui.add_space(4.0);

                for dev in filtered_devices {
                    let is_selected =
                        !is_overview_active && !is_settings_active && selected_id == Some(&dev.id);

                    let dot_color = match &dev.status {
                        ConnectionStatus::Connected => p.success,
                        ConnectionStatus::Connecting => p.accent,
                        ConnectionStatus::AuthFailed(_) => p.danger,
                        ConnectionStatus::TlsMismatch { .. } => p.danger,
                        ConnectionStatus::Error(_) => p.orange,
                        ConnectionStatus::Disconnected => p.text_muted,
                        ConnectionStatus::Discovered => p.warning,
                    };

                    let icon = theme::os_icon(dev.system_info.as_ref().map(|i| i.os_name.as_str()));

                    let bg_color = if is_selected {
                        p.bg_card_selected
                    } else {
                        p.bg_card
                    };
                    let border_color = if is_selected { p.accent_border } else { p.border };

                    let frame = egui::Frame::new()
                        .fill(bg_color)
                        .stroke(Stroke::new(
                            if is_selected { 1.5_f32 } else { 1.0_f32 },
                            border_color,
                        ))
                        .corner_radius(7)
                        .inner_margin(egui::Margin::symmetric(10, 8));

                    let content_resp = frame.show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        ui.horizontal(|ui| {
                            let (dot_rect, _) =
                                ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
                            ui.painter().circle_filled(dot_rect.center(), 4.0, dot_color);

                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;

                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(format!("{icon} {}", dev.hostname))
                                                .strong()
                                                .size(13.0)
                                                .color(p.text_primary),
                                        )
                                        .truncate(),
                                    );
                                    if dev.auto_connect {
                                        theme::pill_badge(
                                            ui,
                                            "Auto",
                                            p.accent_bg,
                                            p.accent,
                                            None,
                                        );
                                    }
                                });

                                ui.label(
                                    RichText::new(format!("{}:{}", dev.primary_address(), dev.port))
                                        .monospace()
                                        .size(10.5)
                                        .color(p.text_muted),
                                );

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
                                        ui.add_space(2.0);
                                        ui.horizontal(|ui| {
                                            theme::mini_progress_bar(
                                                ui,
                                                &p,
                                                (cpu / 100.0) as f32,
                                                theme::usage_color(&p, cpu),
                                                34.0,
                                                4.0,
                                            );
                                            ui.label(
                                                RichText::new(format!("{cpu:.0}%"))
                                                    .size(10.0)
                                                    .color(p.text_secondary),
                                            );
                                            theme::mini_progress_bar(
                                                ui,
                                                &p,
                                                (ram_pct / 100.0) as f32,
                                                p.purple,
                                                34.0,
                                                4.0,
                                            );
                                            ui.label(
                                                RichText::new(format!("{ram_pct:.0}%"))
                                                    .size(10.0)
                                                    .color(p.text_secondary),
                                            );
                                        });
                                    }
                                } else if matches!(dev.status, ConnectionStatus::TlsMismatch { .. })
                                {
                                    ui.label(
                                        RichText::new("⚠ Certificat TLS modifié")
                                            .size(10.5)
                                            .color(p.danger),
                                    );
                                } else if matches!(dev.status, ConnectionStatus::AuthFailed(_)) {
                                    ui.label(
                                        RichText::new("🔑 Clé PSK requise")
                                            .size(10.5)
                                            .color(p.warning),
                                    );
                                }
                            });
                        });
                    });

                    let interact = ui.interact(
                        content_resp.response.rect,
                        ui.make_persistent_id(&dev.id),
                        egui::Sense::click(),
                    );

                    if interact.clicked() {
                        action = Some(SidebarAction::SelectDevice(dev.id.clone()));
                    }

                    let dev_id = dev.id.clone();
                    let addr_copy = format!("{}:{}", dev.primary_address(), dev.port);
                    let is_conn = dev.status.is_connected();
                    interact.context_menu(|ui| {
                        if is_conn {
                            if ui.button("🔌 Déconnecter").clicked() {
                                action = Some(SidebarAction::DisconnectDevice(dev_id.clone()));
                                ui.close_menu();
                            }
                            if ui.button("💻 Terminal").clicked() {
                                action = Some(SidebarAction::OpenTerminal(dev_id.clone()));
                                ui.close_menu();
                            }
                        } else if ui.button("⚡ Connecter").clicked() {
                            action = Some(SidebarAction::ConnectDevice(dev_id.clone()));
                            ui.close_menu();
                        }

                        if ui.button("📋 Copier l'adresse").clicked() {
                            ui.ctx().copy_text(addr_copy);
                            ui.close_menu();
                        }

                        if ui.button("⚙ Configurer").clicked() {
                            action = Some(SidebarAction::ConfigureDevice(dev_id.clone()));
                            ui.close_menu();
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

    // 5. Footer of Sidebar
    ui.separator();
    ui.add_space(3.0);

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("● {online_count}/{} en ligne", devices.len()))
                .size(11.0)
                .strong()
                .color(if online_count > 0 {
                    p.success
                } else {
                    p.text_muted
                }),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let settings_btn = egui::Button::new(
                RichText::new("⚙ Paramètres")
                    .size(12.0)
                    .color(if is_settings_active {
                        p.text_primary
                    } else {
                        p.text_secondary
                    }),
            )
            .fill(if is_settings_active {
                p.accent_bg
            } else {
                p.bg_card
            })
            .stroke(Stroke::new(
                1.0_f32,
                if is_settings_active {
                    p.accent_border
                } else {
                    p.border
                },
            ))
            .corner_radius(6);

            if ui
                .add(settings_btn)
                .on_hover_text("Paramètres généraux de l'application")
                .clicked()
            {
                action = Some(SidebarAction::OpenGlobalSettings);
            }

            let theme_icon = if dark_mode { "☀" } else { "🌙" };
            let theme_tip = if dark_mode {
                "Passer au mode clair"
            } else {
                "Passer au mode sombre"
            };
            if ui.button(theme_icon).on_hover_text(theme_tip).clicked() {
                action = Some(SidebarAction::ToggleTheme);
            }
        });
    });

    // 6. Add Device Modal
    if state.show_add_modal {
        egui::Window::new("➕ Ajouter un serveur")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(380.0)
            .show(ui.ctx(), |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;

                ui.label(
                    RichText::new("Entrez les informations de connexion de l'agent ntx-server :")
                        .size(12.0)
                        .color(p.text_secondary),
                );

                egui::Grid::new("add_server_grid")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Nom d'hôte :");
                        ui.add(
                            egui::TextEdit::singleline(&mut state.manual_hostname)
                                .desired_width(220.0)
                                .hint_text("ex: raspberry-pi"),
                        );
                        ui.end_row();

                        ui.label("Adresse IP / Hôte * :");
                        ui.add(
                            egui::TextEdit::singleline(&mut state.manual_address)
                                .desired_width(220.0)
                                .hint_text("ex: 192.168.1.42"),
                        );
                        ui.end_row();

                        ui.label("Port gRPC :");
                        ui.add(
                            egui::TextEdit::singleline(&mut state.manual_port)
                                .desired_width(80.0),
                        );
                        ui.end_row();

                        ui.label("Clé PSK :");
                        ui.add(
                            egui::TextEdit::singleline(&mut state.manual_psk)
                                .password(true)
                                .desired_width(220.0)
                                .hint_text("Optionnel si clé globale définie"),
                        );
                        ui.end_row();
                    });

                if let Some(err) = &state.add_error {
                    ui.label(RichText::new(format!("⚠ {err}")).color(p.danger).small());
                }

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Annuler").clicked() {
                        state.show_add_modal = false;
                        state.add_error = None;
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::primary_button(ui, &p, "✅ Enregistrer et Ajouter").clicked() {
                            let mut raw = state.manual_address.trim();
                            if let Some(rest) = raw
                                .strip_prefix("https://")
                                .or_else(|| raw.strip_prefix("http://"))
                            {
                                raw = rest;
                            }
                            raw = raw.trim_end_matches('/');

                            let mut port_override: Option<String> = None;
                            if !raw.starts_with('[') && raw.matches(':').count() == 1 {
                                if let Some((h, prt)) = raw.split_once(':') {
                                    if prt.trim().parse::<u16>().is_ok_and(|n| n > 0) {
                                        raw = h;
                                        if state.manual_port.trim().is_empty()
                                            || state.manual_port.trim() == "9850"
                                        {
                                            port_override = Some(prt.trim().to_string());
                                        }
                                    }
                                }
                            }

                            let address = crate::discovery::clean_address_str(raw).to_string();
                            if let Some(p_str) = port_override {
                                state.manual_port = p_str;
                            }

                            if address.is_empty() {
                                state.add_error = Some(
                                    "L'adresse IP ou le nom d'hôte ne peut pas être vide."
                                        .to_string(),
                                );
                            } else if address.to_ascii_lowercase().starts_with("fe80:")
                                || address.contains('%')
                                || address
                                    .parse::<std::net::IpAddr>()
                                    .is_ok_and(|ip| crate::discovery::is_ipv6_link_local(&ip))
                            {
                                state.add_error = Some(
                                    "Les adresses IPv6 link-local (fe80::) ne sont pas supportées ; utilisez l'adresse IPv4 locale."
                                        .to_string(),
                                );
                            } else if !crate::discovery::is_usable_address(&address) {
                                state.add_error = Some(
                                    "Adresse réseau invalide (utilisez une adresse IPv4 ou un nom d'hôte valide)."
                                        .to_string(),
                                );
                            } else {
                                match state.manual_port.trim().parse::<u16>() {
                                    Ok(port) if port > 0 => {
                                        let hostname = if state.manual_hostname.trim().is_empty() {
                                            format!("serveur-{}", port)
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
                                    _ => {
                                        state.add_error = Some(
                                            "Le port doit être un nombre entier entre 1 et 65535."
                                                .to_string(),
                                        );
                                    }
                                }
                            }
                        }
                    });
                });
            });
    }

    action
}
