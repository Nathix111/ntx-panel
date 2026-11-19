use crate::state::{ConnectionStatus, Device};
use crate::ui::theme::{self, Palette};
use egui::{RichText, Ui};

pub enum OverviewAction {
    SelectDevice(String),
    ConnectDevice(String),
    DisconnectDevice(String),
    OpenTerminal(String),
    ConfigureDevice(String),
    ConnectAll,
    DisconnectAll,
    OpenAddModal,
    RefreshDiscovery,
}

pub fn render(ui: &mut Ui, devices: &[&Device]) -> Option<OverviewAction> {
    let p = Palette::for_ui(ui);
    let mut action: Option<OverviewAction> = None;

    // 1. Header bar
    let any_connected = devices.iter().any(|d| d.status.is_connected());
    let any_disconnected = devices
        .iter()
        .any(|d| !d.status.is_connected() && !matches!(d.status, ConnectionStatus::Connecting));
    let avail_w = ui.available_width();

    let render_title = |ui: &mut Ui| {
        ui.vertical(|ui| {
            ui.label(
                RichText::new("Vue d'ensemble du parc")
                    .size(20.0)
                    .strong()
                    .color(p.text_primary),
            );
            ui.label(
                RichText::new(
                    "Supervision centralisée, latence TLS et accès rapide à vos serveurs locaux.",
                )
                .size(12.5)
                .color(p.text_secondary),
            );
        });
    };

    let render_header_buttons = |ui: &mut Ui, rtl: bool, action: &mut Option<OverviewAction>| {
        let draw_add = |ui: &mut Ui, action: &mut Option<OverviewAction>| {
            if theme::primary_button(ui, &p, "➕ Ajouter un serveur").clicked() {
                *action = Some(OverviewAction::OpenAddModal);
            }
        };
        let draw_scan = |ui: &mut Ui, action: &mut Option<OverviewAction>| {
            if ui
                .button("🔄 Scanner mDNS")
                .on_hover_text("Relancer la recherche automatique sur le réseau local")
                .clicked()
            {
                *action = Some(OverviewAction::RefreshDiscovery);
            }
        };
        let draw_disconn_all = |ui: &mut Ui, action: &mut Option<OverviewAction>| {
            if any_connected
                && ui
                    .button("🔌 Tout déconnecter")
                    .on_hover_text("Fermer toutes les sessions actives")
                    .clicked()
            {
                *action = Some(OverviewAction::DisconnectAll);
            }
        };
        let draw_conn_all = |ui: &mut Ui, action: &mut Option<OverviewAction>| {
            if any_disconnected
                && ui
                    .button("⚡ Tout connecter")
                    .on_hover_text("Connecter tous les serveurs ayant une clé PSK configurée")
                    .clicked()
            {
                *action = Some(OverviewAction::ConnectAll);
            }
        };

        if rtl {
            draw_add(ui, action);
            draw_scan(ui, action);
            draw_disconn_all(ui, action);
            draw_conn_all(ui, action);
        } else {
            draw_conn_all(ui, action);
            draw_disconn_all(ui, action);
            draw_scan(ui, action);
            draw_add(ui, action);
        }
    };

    if avail_w >= 920.0 {
        ui.horizontal(|ui| {
            render_title(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                render_header_buttons(ui, true, &mut action);
            });
        });
    } else {
        render_title(ui);
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            render_header_buttons(ui, false, &mut action);
        });
    }

    ui.add_space(12.0);

    // 2. Global Fleet KPIs
    let total = devices.len();
    let online_devices: Vec<&Device> = devices
        .iter()
        .copied()
        .filter(|d| d.status.is_connected())
        .collect();
    let online_count = online_devices.len();

    let (avg_cpu, avg_ram, total_rx, total_tx) = if online_devices.is_empty() {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        let mut cpu_sum = 0.0;
        let mut ram_sum = 0.0;
        let mut ram_n = 0usize;
        let mut rx_sum = 0.0;
        let mut tx_sum = 0.0;

        for d in &online_devices {
            if let Some(m) = &d.latest_metrics {
                cpu_sum += m.cpu_usage_percent;
                if m.total_memory_bytes > 0 {
                    ram_sum += (m.used_memory_bytes as f64 / m.total_memory_bytes as f64) * 100.0;
                    ram_n += 1;
                }
                rx_sum += m.network_metrics.iter().map(|n| n.rx_bytes_per_sec as f64).sum::<f64>();
                tx_sum += m.network_metrics.iter().map(|n| n.tx_bytes_per_sec as f64).sum::<f64>();
            } else {
                cpu_sum += d.cpu_history.back().copied().unwrap_or(0.0);
                if let Some(&r) = d.ram_history.back() {
                    ram_sum += r;
                    ram_n += 1;
                }
            }
        }

        let avg_c = cpu_sum / online_devices.len() as f64;
        let avg_r = if ram_n > 0 { ram_sum / ram_n as f64 } else { 0.0 };
        (avg_c, avg_r, rx_sum, tx_sum)
    };

    ui.columns(4, |cols| {
        theme::card_frame(&p).show(&mut cols[0], |ui| {
            ui.set_min_height(66.0);
            ui.label(
                RichText::new("SERVEURS EN LIGNE")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{online_count} / {total}"))
                        .size(21.0)
                        .strong()
                        .color(if online_count > 0 {
                            p.success
                        } else {
                            p.text_secondary
                        }),
                );
            });
            ui.add_space(4.0);
            let ratio = if total > 0 {
                online_count as f32 / total as f32
            } else {
                0.0
            };
            theme::mini_progress_bar(ui, &p, ratio, p.success, ui.available_width().min(160.0), 5.0);
        });

        theme::card_frame(&p).show(&mut cols[1], |ui| {
            ui.set_min_height(66.0);
            ui.label(
                RichText::new("CPU MOYEN")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            ui.add_space(4.0);
            let cpu_col = if online_devices.is_empty() {
                p.text_muted
            } else {
                theme::usage_color(&p, avg_cpu)
            };
            ui.label(
                RichText::new(if online_devices.is_empty() {
                    "—".to_string()
                } else {
                    format!("{avg_cpu:.1}%")
                })
                .size(21.0)
                .strong()
                .color(cpu_col),
            );
            ui.add_space(4.0);
            theme::mini_progress_bar(
                ui,
                &p,
                if online_devices.is_empty() {
                    0.0
                } else {
                    (avg_cpu / 100.0) as f32
                },
                p.accent,
                ui.available_width().min(160.0),
                5.0,
            );
        });

        theme::card_frame(&p).show(&mut cols[2], |ui| {
            ui.set_min_height(66.0);
            ui.label(
                RichText::new("RAM MOYENNE")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            ui.add_space(4.0);
            let ram_col = if online_devices.is_empty() {
                p.text_muted
            } else {
                theme::usage_color(&p, avg_ram)
            };
            ui.label(
                RichText::new(if online_devices.is_empty() {
                    "—".to_string()
                } else {
                    format!("{avg_ram:.1}%")
                })
                .size(21.0)
                .strong()
                .color(ram_col),
            );
            ui.add_space(4.0);
            theme::mini_progress_bar(
                ui,
                &p,
                if online_devices.is_empty() {
                    0.0
                } else {
                    (avg_ram / 100.0) as f32
                },
                p.purple,
                ui.available_width().min(160.0),
                5.0,
            );
        });

        theme::card_frame(&p).show(&mut cols[3], |ui| {
            ui.set_min_height(66.0);
            ui.label(
                RichText::new("TRAFIC CUMULÉ")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            ui.add_space(4.0);
            if online_devices.is_empty() {
                ui.label(RichText::new("—").size(21.0).strong().color(p.text_muted));
                ui.label(
                    RichText::new("Aucun serveur connecté")
                        .size(11.0)
                        .color(p.text_muted),
                );
            } else {
                ui.label(
                    RichText::new(format!("↓ {}", theme::format_bytes_rate(total_rx)))
                        .size(14.5)
                        .strong()
                        .color(p.success),
                );
                ui.label(
                    RichText::new(format!("↑ {}", theme::format_bytes_rate(total_tx)))
                        .size(13.5)
                        .strong()
                        .color(p.orange),
                );
            }
        });
    });

    ui.add_space(14.0);

    // 3. Server Cards Grid (or clean Empty State)
    if devices.is_empty() {
        ui.add_space(24.0);
        theme::card_frame(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(16.0);
                ui.label(RichText::new("🖥").size(32.0).color(p.accent));
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Aucun serveur enregistré pour le moment")
                        .size(16.0)
                        .strong()
                        .color(p.text_primary),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Lancez ntx-server sur votre Raspberry Pi / serveur Linux ou ajoutez son adresse manuellement.",
                    )
                    .size(12.5)
                    .color(p.text_secondary),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let total_w = 340.0;
                    ui.add_space(((ui.available_width() - total_w) * 0.5).max(0.0));
                    if theme::primary_button(ui, &p, "➕ Ajouter manuellement").clicked() {
                        action = Some(OverviewAction::OpenAddModal);
                    }
                    if ui.button("🔄 Scanner le réseau (mDNS)").clicked() {
                        action = Some(OverviewAction::RefreshDiscovery);
                    }
                });
                ui.add_space(14.0);
            });
        });
        return action;
    }

    let num_cols = if ui.available_width() > 1100.0 {
        3
    } else if ui.available_width() > 640.0 {
        2
    } else {
        1
    };

    egui::ScrollArea::vertical().show(ui, |ui| {
        for chunk in devices.chunks(num_cols) {
            ui.columns(num_cols, |cols| {
                for (i, dev) in chunk.iter().enumerate() {
                    if let Some(card_act) = render_server_card(&mut cols[i], &p, dev) {
                        action = Some(card_act);
                    }
                }
            });
            ui.add_space(10.0);
        }
    });

    action
}

fn render_server_card(ui: &mut Ui, p: &Palette, dev: &Device) -> Option<OverviewAction> {
    let mut action = None;

    let (status_label, status_bg, status_fg, status_border) = match &dev.status {
        ConnectionStatus::Connected => ("● Connecté", p.success_bg, p.success, Some(p.success_border)),
        ConnectionStatus::Connecting => ("● Connexion...", p.accent_bg, p.accent, Some(p.accent_border)),
        ConnectionStatus::AuthFailed(_) => ("✖ Auth refusée", p.danger_bg, p.danger, Some(p.danger_border)),
        ConnectionStatus::TlsMismatch { .. } => {
            ("⚠ Certificat modifié", p.danger_bg, p.danger, Some(p.danger_border))
        }
        ConnectionStatus::Error(_) => ("⚠ Erreur", p.warning_bg, p.warning, Some(p.warning_border)),
        ConnectionStatus::Disconnected => ("○ Déconnecté", p.bg_subtle, p.text_muted, Some(p.border)),
        ConnectionStatus::Discovered => ("○ Découvert", p.warning_bg, p.warning, Some(p.warning_border)),
    };

    let os_name = dev.system_info.as_ref().map(|i| i.os_name.as_str());
    let icon = theme::os_icon(os_name);

    let frame = egui::Frame::new()
        .fill(p.bg_card)
        .stroke(egui::Stroke::new(
            1.0_f32,
            if dev.status.is_connected() {
                p.border_strong
            } else {
                p.border
            },
        ))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(14, 12));

    frame.show(ui, |ui| {
        ui.set_min_height(132.0);

        // Card header row
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Label::new(
                        RichText::new(format!("{icon} {}", dev.hostname))
                            .size(15.5)
                            .strong()
                            .color(p.text_primary),
                    )
                    .sense(egui::Sense::click()),
                )
                .on_hover_text("Ouvrir la vue détaillée du serveur")
                .clicked()
            {
                action = Some(OverviewAction::SelectDevice(dev.id.clone()));
            }

            if dev.auto_connect {
                theme::pill_badge(ui, "Auto", p.accent_bg, p.accent, Some(p.accent_border));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::pill_badge(ui, status_label, status_bg, status_fg, status_border);
            });
        });

        ui.add_space(4.0);

        // Address & telemetry badges row
        ui.horizontal_wrapped(|ui| {
            let endpoint_short = format!("{}:{}", dev.primary_address(), dev.port);
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(format!("📋 {endpoint_short}"))
                            .monospace()
                            .size(11.5)
                            .color(p.text_secondary),
                    )
                    .fill(p.bg_subtle)
                    .stroke(egui::Stroke::new(1.0_f32, p.border))
                    .corner_radius(5),
                )
                .on_hover_text("Copier l'adresse dans le presse-papier")
                .clicked()
            {
                ui.ctx().copy_text(endpoint_short);
            }

            if let Some(info) = &dev.system_info {
                if !info.arch.is_empty() {
                    theme::pill_badge(
                        ui,
                        &format!("{} • {}", info.os_name, info.arch),
                        p.bg_subtle,
                        p.text_secondary,
                        None,
                    );
                }
            }

            if let Some(ms) = dev.last_ping_ms {
                theme::pill_badge(ui, &format!("📡 {ms} ms"), p.bg_subtle, p.accent, None);
            }
            if let Some(up) = dev.server_uptime_secs {
                theme::pill_badge(
                    ui,
                    &format!("⏱ {}", theme::format_uptime(up)),
                    p.bg_subtle,
                    p.text_secondary,
                    None,
                );
            }
        });

        ui.add_space(8.0);

        if dev.status.is_connected() {
            let cpu = dev.cpu_history.back().copied().unwrap_or(0.0);
            let ram = dev.ram_history.back().copied().unwrap_or(0.0);
            let rx = dev.rx_history.back().copied().unwrap_or(0.0);
            let tx = dev.tx_history.back().copied().unwrap_or(0.0);

            ui.horizontal(|ui| {
                ui.label(RichText::new("CPU").size(11.0).color(p.text_muted));
                theme::mini_progress_bar(ui, p, (cpu / 100.0) as f32, theme::usage_color(p, cpu), 60.0, 6.0);
                ui.label(
                    RichText::new(format!("{cpu:.0}%"))
                        .size(11.5)
                        .strong()
                        .color(theme::usage_color(p, cpu)),
                );

                ui.add_space(8.0);
                ui.label(RichText::new("RAM").size(11.0).color(p.text_muted));
                theme::mini_progress_bar(ui, p, (ram / 100.0) as f32, p.purple, 60.0, 6.0);
                ui.label(
                    RichText::new(format!("{ram:.0}%"))
                        .size(11.5)
                        .strong()
                        .color(p.purple),
                );
            });

            ui.label(
                RichText::new(format!(
                    "Réseau : ↓ {}  •  ↑ {}",
                    theme::format_bytes_rate(rx),
                    theme::format_bytes_rate(tx)
                ))
                .size(11.0)
                .color(p.text_secondary),
            );
        } else {
            let msg = match &dev.status {
                ConnectionStatus::Connecting => "Négociation TLS & challenge HMAC en cours...".to_string(),
                ConnectionStatus::AuthFailed(e) | ConnectionStatus::Error(e) => e.clone(),
                ConnectionStatus::TlsMismatch { .. } => {
                    "Empreinte TLS modifiée — vérification manuelle requise.".to_string()
                }
                ConnectionStatus::Discovered => {
                    "Découvert sur le réseau local. Prêt pour la connexion sécurisée.".to_string()
                }
                ConnectionStatus::Disconnected => {
                    if dev.psk.as_deref().is_some_and(|s| !s.trim().is_empty()) {
                        "Prêt à se connecter (clé PSK mémorisée).".to_string()
                    } else {
                        "Utilise la clé PSK par défaut ou une clé spécifique.".to_string()
                    }
                }
                ConnectionStatus::Connected => String::new(),
            };
            ui.add(
                egui::Label::new(RichText::new(msg).size(11.5).color(p.text_secondary)).truncate(),
            );
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        // Card footer actions
        ui.horizontal(|ui| {
            if dev.status.is_connected() {
                if theme::primary_button(ui, p, "📊 Ouvrir").clicked() {
                    action = Some(OverviewAction::SelectDevice(dev.id.clone()));
                }
                if ui.button("💻 Terminal").clicked() {
                    action = Some(OverviewAction::OpenTerminal(dev.id.clone()));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("🔌 Déconnecter")
                        .on_hover_text("Fermer la connexion gRPC TLS")
                        .clicked()
                    {
                        action = Some(OverviewAction::DisconnectDevice(dev.id.clone()));
                    }
                });
            } else {
                if !matches!(dev.status, ConnectionStatus::Connecting)
                    && theme::primary_button(ui, p, "⚡ Connecter").clicked()
                {
                    action = Some(OverviewAction::ConnectDevice(dev.id.clone()));
                }
                if ui.button("⚙ Configurer").clicked() {
                    action = Some(OverviewAction::ConfigureDevice(dev.id.clone()));
                }
                if ui.button("Ouvrir").clicked() {
                    action = Some(OverviewAction::SelectDevice(dev.id.clone()));
                }
            }
        });
    });

    action
}
