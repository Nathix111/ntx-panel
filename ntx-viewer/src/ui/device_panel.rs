use crate::state::{ConnectionStatus, Device};
use crate::tls::format_fingerprint;
use crate::ui::docker_panel::{self, DockerPanelAction, DockerPanelState};
use crate::ui::metrics_panel::{self, MetricsPanelAction, MetricsPanelState};
use crate::ui::process_panel::{self, ProcessPanelAction};
use crate::ui::services_panel::{self, ServicesPanelAction, ServicesPanelState};
use crate::ui::terminal::{self, TerminalAction};
use crate::ui::theme::{self, Palette};
use egui::{RichText, Stroke, Ui};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceTab {
    Metrics,
    Services,
    Docker,
    System,
    Terminal,
    Config,
}

pub struct DevicePanelState {
    pub active_tab: DeviceTab,
    pub show_psk: bool,
    pub psk_input: String,
    pub psk_status_message: Option<String>,
    pub show_delete_confirm: bool,
    pub metrics_state: MetricsPanelState,
    pub services_state: ServicesPanelState,
    pub docker_state: DockerPanelState,
    pub edit_device_id: Option<String>,
    pub edit_hostname: String,
    pub edit_address: String,
    pub edit_port: String,
    pub edit_auto_connect: bool,
    pub config_status_message: Option<(String, bool)>,
    pub copied_feedback_until: Option<Instant>,
}

impl Default for DevicePanelState {
    fn default() -> Self {
        Self {
            active_tab: DeviceTab::Metrics,
            show_psk: false,
            psk_input: String::new(),
            psk_status_message: None,
            show_delete_confirm: false,
            metrics_state: MetricsPanelState::default(),
            services_state: ServicesPanelState::default(),
            docker_state: DockerPanelState::default(),
            edit_device_id: None,
            edit_hostname: String::new(),
            edit_address: String::new(),
            edit_port: "9850".to_string(),
            edit_auto_connect: false,
            config_status_message: None,
            copied_feedback_until: None,
        }
    }
}

impl DevicePanelState {
    pub fn sync_from_device(&mut self, device: &Device) {
        self.edit_device_id = Some(device.id.clone());
        self.psk_input = device.psk.clone().unwrap_or_default();
        self.edit_hostname = device.hostname.clone();
        self.edit_address = device.primary_address();
        self.edit_port = device.port.to_string();
        self.edit_auto_connect = device.auto_connect;
        self.psk_status_message = None;
        self.config_status_message = None;
        self.show_delete_confirm = false;
        self.metrics_state.reset_for_device();
        self.services_state.reset_for_device();
        self.docker_state.reset_for_device();
    }
}

#[allow(clippy::large_enum_variant)]
pub enum DevicePanelAction {
    Connect {
        id: String,
        psk: Option<String>,
    },
    Disconnect(String),
    Reconnect(String),
    Remove(String),
    SavePsk {
        id: String,
        psk: String,
    },
    TrustNewFingerprint {
        id: String,
        fingerprint: String,
    },
    ResetFingerprint(String),
    SwitchTab(DeviceTab),
    TerminalActions(Vec<TerminalAction>),
    ProcessAction(ProcessPanelAction),
    ServicesAction(ServicesPanelAction),
    DockerAction(DockerPanelAction),
    Ping(String),
    UpdateDeviceConfig {
        id: String,
        hostname: String,
        address: Option<String>,
        port: Option<u16>,
        auto_connect: bool,
    },
    ToggleHideVirtualDisks,
    ToggleHideVirtualInterfaces,
    ToggleMetricsPause(String),
    ClearMetricsHistory(String),
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    ui: &mut Ui,
    state: &mut DevicePanelState,
    device: &mut Device,
    refresh_rate_ms: u32,
    history_capacity: usize,
    hide_virtual_disks: bool,
    hide_virtual_interfaces: bool,
    terminal_font_size: f32,
) -> Option<DevicePanelAction> {
    if state.edit_device_id.as_deref() != Some(&device.id) {
        state.sync_from_device(device);
    }

    let p = Palette::for_ui(ui);
    let mut action = None;

    if let Some(until) = state.copied_feedback_until {
        if Instant::now() < until {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        } else {
            state.copied_feedback_until = None;
        }
    }

    let icon = theme::os_icon(device.system_info.as_ref().map(|i| i.os_name.as_str()));
    let hero_accent = compute_hero_accent(&p, device, hide_virtual_disks);

    // 1. Hero Header Card (harmonized with left status accent strip)
    let mut hero_prepared = egui::Frame::new()
        .fill(p.bg_card)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .begin(ui);
    {
        let ui = &mut hero_prepared.content_ui;
        ui.set_width(ui.available_width());
        let avail_w = ui.available_width();

        let render_identity_and_status = |ui: &mut Ui| {
            ui.label(
                RichText::new(format!("{icon} {}", device.hostname))
                    .size(19.0)
                    .strong()
                    .color(p.text_primary),
            );

            let (status_text, status_bg, status_fg, status_border) = match &device.status {
                ConnectionStatus::Connected => {
                    ("● Connecté", p.success_bg, p.success, Some(p.success_border))
                }
                ConnectionStatus::Connecting => {
                    ("● Connexion...", p.accent_bg, p.accent, Some(p.accent_border))
                }
                ConnectionStatus::AuthFailed(_) => {
                    ("✖ Échec Auth", p.danger_bg, p.danger, Some(p.danger_border))
                }
                ConnectionStatus::TlsMismatch { .. } => (
                    "⚠ Certificat modifié",
                    p.danger_bg,
                    p.danger,
                    Some(p.danger_border),
                ),
                ConnectionStatus::Error(_) => {
                    ("⚠ Erreur", p.warning_bg, p.warning, Some(p.warning_border))
                }
                ConnectionStatus::Disconnected => {
                    ("○ Déconnecté", p.bg_subtle, p.text_muted, Some(p.border))
                }
                ConnectionStatus::Discovered => {
                    ("○ Découvert", p.warning_bg, p.warning, Some(p.warning_border))
                }
            };
            theme::pill_badge(ui, status_text, status_bg, status_fg, status_border);
        };

        let render_hero_actions = |ui: &mut Ui, rtl: bool, action: &mut Option<DevicePanelAction>, state: &mut DevicePanelState| {
            let draw_delete = |ui: &mut Ui, state: &mut DevicePanelState| {
                if theme::danger_button(ui, &p, "🗑")
                    .on_hover_text("Supprimer ce serveur de la liste")
                    .clicked()
                {
                    state.show_delete_confirm = true;
                }
            };
            let draw_disconnect = |ui: &mut Ui, action: &mut Option<DevicePanelAction>| {
                if ui
                    .button("🔌 Déconnecter")
                    .on_hover_text("Fermer la connexion gRPC TLS")
                    .clicked()
                {
                    *action = Some(DevicePanelAction::Disconnect(device.id.clone()));
                }
            };
            let draw_reconnect = |ui: &mut Ui, action: &mut Option<DevicePanelAction>| {
                if ui
                    .button("🔄 Reconnecter")
                    .on_hover_text("Réinitialiser la session gRPC TLS")
                    .clicked()
                {
                    *action = Some(DevicePanelAction::Reconnect(device.id.clone()));
                }
            };
            let draw_ping = |ui: &mut Ui, action: &mut Option<DevicePanelAction>| {
                if ui
                    .button("📡 Ping")
                    .on_hover_text("Mesurer la latence et l'uptime du serveur")
                    .clicked()
                {
                    *action = Some(DevicePanelAction::Ping(device.id.clone()));
                }
            };
            let draw_connect =
                |ui: &mut Ui, state: &DevicePanelState, action: &mut Option<DevicePanelAction>| {
                    if theme::primary_button(ui, &p, "⚡ Se connecter")
                        .on_hover_text("Établir la connexion TLS sécurisée")
                        .clicked()
                    {
                        let entered = state.psk_input.trim();
                        let psk = if !entered.is_empty() {
                            Some(entered.to_string())
                        } else {
                            device.psk.clone()
                        };
                        *action = Some(DevicePanelAction::Connect {
                            id: device.id.clone(),
                            psk,
                        });
                    }
                };

            if rtl {
                draw_delete(ui, state);
                if device.status.is_connected() {
                    draw_disconnect(ui, action);
                    draw_reconnect(ui, action);
                    draw_ping(ui, action);
                } else if !matches!(device.status, ConnectionStatus::Connecting) {
                    draw_connect(ui, state, action);
                }
            } else {
                if device.status.is_connected() {
                    draw_ping(ui, action);
                    draw_reconnect(ui, action);
                    draw_disconnect(ui, action);
                } else if !matches!(device.status, ConnectionStatus::Connecting) {
                    draw_connect(ui, state, action);
                }
                draw_delete(ui, state);
            }
        };

        // Row 1: Identity + Status + Primary Actions
        if avail_w >= 640.0 {
            ui.horizontal(|ui| {
                render_identity_and_status(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    render_hero_actions(ui, true, &mut action, state);
                });
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                render_identity_and_status(ui);
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                render_hero_actions(ui, false, &mut action, state);
            });
        }

        // Row 2: Telemetry Metadata + Copyable Address
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let addr_text = format!("{}:{}", device.primary_address(), device.port);
            let show_copied = state
                .copied_feedback_until
                .is_some_and(|until| Instant::now() < until);
            let copy_lbl = if show_copied {
                format!("✔ {addr_text} copié")
            } else {
                format!("📋 {addr_text}")
            };
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(copy_lbl)
                            .monospace()
                            .size(11.5)
                            .color(if show_copied { p.success } else { p.text_secondary }),
                    )
                    .fill(p.bg_subtle)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .corner_radius(5),
                )
                .on_hover_text("Cliquer pour copier l'adresse réseau")
                .clicked()
            {
                ui.ctx().copy_text(addr_text);
                state.copied_feedback_until = Some(Instant::now() + Duration::from_secs(2));
            }

            if let Some(info) = &device.system_info {
                let mut hw_parts: Vec<String> = Vec::new();
                if !info.os_name.is_empty() {
                    hw_parts.push(info.os_name.clone());
                }
                if !info.arch.is_empty() {
                    hw_parts.push(info.arch.clone());
                }
                if info.cpu_count > 0 {
                    hw_parts.push(format!("{} cœurs", info.cpu_count));
                }
                if info.total_memory_bytes > 0 {
                    hw_parts.push(theme::format_storage(info.total_memory_bytes));
                }
                if !hw_parts.is_empty() {
                    theme::pill_badge(
                        ui,
                        &hw_parts.join(" • "),
                        p.bg_subtle,
                        p.text_secondary,
                        None,
                    );
                }
            }

            if device.auto_connect {
                theme::pill_badge(ui, "Auto", p.accent_bg, p.accent, Some(p.accent_border));
            }

            if let Some(ms) = device.last_ping_ms {
                theme::pill_badge(
                    ui,
                    &format!("📡 {ms} ms"),
                    p.bg_subtle,
                    theme::latency_color(&p, ms),
                    None,
                );
            }

            if let Some(uptime) = device.server_uptime_secs {
                theme::pill_badge(
                    ui,
                    &format!("⏱ {}", theme::format_uptime(uptime)),
                    p.bg_subtle,
                    p.text_secondary,
                    None,
                );
            }

            if !device.version.is_empty() {
                theme::pill_badge(
                    ui,
                    &format!("v{}", device.version.trim_start_matches('v')),
                    p.bg_subtle,
                    p.text_muted,
                    None,
                );
            }
        });
    }
    let hero_resp = hero_prepared.allocate_space(ui);
    hero_prepared.paint(ui);
    let hero_rect = hero_resp.rect;
    if ui.is_rect_visible(hero_rect) {
        let strip_rect = egui::Rect::from_min_max(
            egui::pos2(hero_rect.left() + 1.0, hero_rect.top() + 8.0),
            egui::pos2(hero_rect.left() + 4.0, hero_rect.bottom() - 8.0),
        );
        ui.painter().rect_filled(strip_rect, 1.5, hero_accent);
    }

    // Confirmation modal for deleting device
    if state.show_delete_confirm {
        egui::Window::new("⚠ Confirmer la suppression")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.label(format!(
                    "Voulez-vous vraiment supprimer le serveur « {} » ({}:{}) de NTX Panel ?",
                    device.hostname,
                    device.primary_address(),
                    device.port
                ));
                ui.label(
                    RichText::new("Les données locales et l'empreinte TLS associées seront supprimées.")
                        .color(p.text_secondary)
                        .size(12.0),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if theme::danger_button(ui, &p, "🗑 Supprimer définitivement").clicked() {
                        state.show_delete_confirm = false;
                        action = Some(DevicePanelAction::Remove(device.id.clone()));
                    }
                    if ui.button("Annuler").clicked() {
                        state.show_delete_confirm = false;
                    }
                });
            });
    }

    // Security alert banner for TLS TOFU mismatch or connection error
    match &device.status {
        ConnectionStatus::TlsMismatch { expected, actual } => {
            ui.add_space(8.0);
            egui::Frame::new()
                .fill(p.danger_bg)
                .stroke(Stroke::new(1.5_f32, p.danger_border))
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(
                            "⚠ ALERTE DE SÉCURITÉ : L'empreinte TLS de ce serveur a changé !",
                        )
                        .color(p.danger)
                        .strong(),
                    );
                    ui.label(
                        RichText::new(
                            "Cela arrive si le serveur a été réinstallé ou en cas d'interception réseau (MITM).",
                        )
                        .size(12.0)
                        .color(p.text_secondary),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!("Attendue : {}", format_fingerprint(expected)))
                            .monospace()
                            .size(11.5)
                            .color(p.text_primary),
                    );
                    ui.label(
                        RichText::new(format!("Reçue    : {}", format_fingerprint(actual)))
                            .monospace()
                            .size(11.5)
                            .color(p.warning),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if theme::primary_button(
                            ui,
                            &p,
                            "✅ Accepter la nouvelle empreinte et reconnecter",
                        )
                        .clicked()
                        {
                            action = Some(DevicePanelAction::TrustNewFingerprint {
                                id: device.id.clone(),
                                fingerprint: actual.clone(),
                            });
                        }
                        if ui.button("Annuler").clicked() {
                            action = Some(DevicePanelAction::Disconnect(device.id.clone()));
                        }
                    });
                });
        }
        ConnectionStatus::AuthFailed(reason) | ConnectionStatus::Error(reason) => {
            let is_auth = matches!(device.status, ConnectionStatus::AuthFailed(_));
            let (bg, border, fg) = if is_auth {
                (p.danger_bg, p.danger_border, p.danger)
            } else {
                (p.warning_bg, p.warning_border, p.warning)
            };
            ui.add_space(8.0);
            egui::Frame::new()
                .fill(bg)
                .stroke(Stroke::new(1.0_f32, border))
                .corner_radius(7)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(format!("⚠ {reason}")).color(fg).size(12.5));
                        if is_auth
                            && state.active_tab != DeviceTab::Config
                            && ui.small_button("🔑 Configurer la PSK").clicked()
                        {
                            state.active_tab = DeviceTab::Config;
                            state.psk_input = device.psk.clone().unwrap_or_default();
                        }
                    });
                });
        }
        _ => {}
    }

    ui.add_space(10.0);

    // 2. Pill Navigation Tabs
    theme::subtle_bar_frame(&p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            if theme::tab_button(
                ui,
                &p,
                state.active_tab == DeviceTab::Metrics,
                "📈 Vue & Métriques",
            )
            .clicked()
            {
                state.active_tab = DeviceTab::Metrics;
                action = Some(DevicePanelAction::SwitchTab(DeviceTab::Metrics));
            }

            let svc_count = if device.managed_services.unsupported_by_server {
                device.favorites.len()
            } else {
                device.managed_services.services.len()
            };
            let svc_label = format!("⭐ Mes Services ({svc_count})");
            if theme::tab_button(ui, &p, state.active_tab == DeviceTab::Services, &svc_label)
                .clicked()
            {
                state.active_tab = DeviceTab::Services;
                action = Some(DevicePanelAction::SwitchTab(DeviceTab::Services));
            }

            let docker_label = format!("🐳 Docker ({})", device.docker_state.containers.len());
            if theme::tab_button(ui, &p, state.active_tab == DeviceTab::Docker, &docker_label)
                .clicked()
            {
                state.active_tab = DeviceTab::Docker;
                action = Some(DevicePanelAction::SwitchTab(DeviceTab::Docker));
            }

            if theme::tab_button(ui, &p, state.active_tab == DeviceTab::System, "⚡ Système")
                .clicked()
            {
                state.active_tab = DeviceTab::System;
                action = Some(DevicePanelAction::SwitchTab(DeviceTab::System));
            }

            if theme::tab_button(
                ui,
                &p,
                state.active_tab == DeviceTab::Terminal,
                "💻 Terminal",
            )
            .clicked()
            {
                state.active_tab = DeviceTab::Terminal;
                action = Some(DevicePanelAction::SwitchTab(DeviceTab::Terminal));
            }

            if theme::tab_button(
                ui,
                &p,
                state.active_tab == DeviceTab::Config,
                "⚙ Configuration",
            )
            .clicked()
            {
                state.active_tab = DeviceTab::Config;
                state.psk_input = device.psk.clone().unwrap_or_default();
                action = Some(DevicePanelAction::SwitchTab(DeviceTab::Config));
            }
        });
    });

    ui.add_space(10.0);

    // 3. Tab Body
    match state.active_tab {
        DeviceTab::Metrics => {
            if let Some(m_act) = metrics_panel::render(
                ui,
                &mut state.metrics_state,
                device,
                refresh_rate_ms,
                history_capacity,
                hide_virtual_disks,
                hide_virtual_interfaces,
            ) {
                match m_act {
                    MetricsPanelAction::Connect => {
                        let entered = state.psk_input.trim();
                        let psk = if !entered.is_empty() {
                            Some(entered.to_string())
                        } else {
                            device.psk.clone()
                        };
                        action = Some(DevicePanelAction::Connect {
                            id: device.id.clone(),
                            psk,
                        });
                    }
                    MetricsPanelAction::TogglePause => {
                        action = Some(DevicePanelAction::ToggleMetricsPause(device.id.clone()));
                    }
                    MetricsPanelAction::ClearHistory => {
                        action = Some(DevicePanelAction::ClearMetricsHistory(device.id.clone()));
                    }
                    MetricsPanelAction::ToggleHideVirtualDisks => {
                        action = Some(DevicePanelAction::ToggleHideVirtualDisks);
                    }
                    MetricsPanelAction::ToggleHideVirtualInterfaces => {
                        action = Some(DevicePanelAction::ToggleHideVirtualInterfaces);
                    }
                    MetricsPanelAction::Ping => {
                        action = Some(DevicePanelAction::Ping(device.id.clone()));
                    }
                    MetricsPanelAction::OpenProcesses => {
                        state.active_tab = DeviceTab::System;
                        action = Some(DevicePanelAction::SwitchTab(DeviceTab::System));
                    }
                    MetricsPanelAction::OpenTerminal => {
                        state.active_tab = DeviceTab::Terminal;
                        action = Some(DevicePanelAction::SwitchTab(DeviceTab::Terminal));
                    }
                }
            }
        }
        DeviceTab::Services => {
            if let Some(svc_act) =
                services_panel::render(ui, &mut state.services_state, device)
            {
                action = Some(DevicePanelAction::ServicesAction(svc_act));
            }
        }
        DeviceTab::Docker => {
            if let Some(dk_act) = docker_panel::render(ui, &mut state.docker_state, device) {
                action = Some(DevicePanelAction::DockerAction(dk_act));
            }
        }
        DeviceTab::System => {
            let cpu_count = device
                .system_info
                .as_ref()
                .map(|s| s.cpu_count)
                .unwrap_or(1);
            if let Some(proc_act) = process_panel::render(
                ui,
                &mut device.process_state,
                device.status.is_connected(),
                cpu_count,
            ) {
                action = Some(DevicePanelAction::ProcessAction(proc_act));
            }
        }
        DeviceTab::Terminal => {
            render_terminal_tab(
                ui,
                &p,
                device,
                state.psk_input.trim(),
                terminal_font_size,
                &mut action,
            );
        }
        DeviceTab::Config => {
            render_config_tab(ui, &p, state, device, &mut action);
        }
    }

    action
}

fn compute_hero_accent(p: &Palette, device: &Device, hide_virtual_disks: bool) -> egui::Color32 {
    match &device.status {
        ConnectionStatus::Connected => {
            let cur_cpu = device
                .latest_metrics
                .as_ref()
                .map(|m| m.cpu_usage_percent)
                .or_else(|| device.cpu_history.back().copied())
                .unwrap_or(0.0);
            let cur_ram = device
                .latest_metrics
                .as_ref()
                .filter(|m| m.total_memory_bytes > 0)
                .map(|m| (m.used_memory_bytes as f64 / m.total_memory_bytes as f64) * 100.0)
                .or_else(|| device.ram_history.back().copied())
                .unwrap_or(0.0);
            let (visible_disks, _) = metrics_panel::collect_disk_rows(device, hide_virtual_disks);
            let alerts =
                metrics_panel::collect_server_alerts(device, cur_cpu, cur_ram, &visible_disks);
            if alerts.iter().any(|a| a.critical) {
                p.danger
            } else if !alerts.is_empty() {
                p.warning
            } else {
                p.success
            }
        }
        ConnectionStatus::TlsMismatch { .. } | ConnectionStatus::AuthFailed(_) => p.danger,
        ConnectionStatus::Error(_) | ConnectionStatus::Discovered => p.warning,
        ConnectionStatus::Connecting => p.accent,
        ConnectionStatus::Disconnected => p.border_strong,
    }
}

fn render_terminal_tab(
    ui: &mut Ui,
    p: &Palette,
    device: &mut Device,
    entered_psk: &str,
    terminal_font_size: f32,
    action: &mut Option<DevicePanelAction>,
) {
    if !device.status.is_connected() {
        ui.add_space(24.0);
        theme::card_frame(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(14.0);
                ui.label(
                    RichText::new("Le terminal distant nécessite une connexion active")
                        .size(16.0)
                        .strong()
                        .color(p.text_primary),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Connectez-vous au serveur pour ouvrir une session interactive VT100.",
                    )
                    .color(p.text_secondary),
                );
                ui.add_space(14.0);
                if !matches!(device.status, ConnectionStatus::Connecting)
                    && theme::primary_button(ui, p, "⚡ Se connecter pour ouvrir le terminal")
                        .clicked()
                {
                    let psk = if !entered_psk.is_empty() {
                        Some(entered_psk.to_string())
                    } else {
                        device.psk.clone()
                    };
                    *action = Some(DevicePanelAction::Connect {
                        id: device.id.clone(),
                        psk,
                    });
                }
                ui.add_space(14.0);
            });
        });
        return;
    }

    let term_actions = terminal::render(ui, device, terminal_font_size);
    if !term_actions.is_empty() {
        *action = Some(DevicePanelAction::TerminalActions(term_actions));
    }
}

fn render_config_tab(
    ui: &mut Ui,
    p: &Palette,
    state: &mut DevicePanelState,
    device: &Device,
    action: &mut Option<DevicePanelAction>,
) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(4.0);

        // Card 1: Identity & Connection Settings
        theme::card_frame(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("🖥 Identité & Connexion")
                        .size(15.0)
                        .strong()
                        .color(p.text_primary),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::pill_badge(
                        ui,
                        if device.is_manual { "Manuel" } else { "mDNS" },
                        p.bg_subtle,
                        p.text_secondary,
                        Some(p.border),
                    );
                });
            });
            ui.add_space(8.0);

            egui::Grid::new("device_identity_grid")
                .num_columns(2)
                .spacing([18.0, 8.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Nom affiché (Alias) :").strong());
                    ui.add(egui::TextEdit::singleline(&mut state.edit_hostname).desired_width(260.0));
                    ui.end_row();

                    if device.is_manual {
                        ui.label(RichText::new("Adresse IP / Hôte :").strong());
                        ui.add(
                            egui::TextEdit::singleline(&mut state.edit_address)
                                .desired_width(260.0),
                        );
                        ui.end_row();

                        ui.label(RichText::new("Port gRPC :").strong());
                        ui.add(
                            egui::TextEdit::singleline(&mut state.edit_port).desired_width(90.0),
                        );
                        ui.end_row();
                    } else {
                        ui.label(RichText::new("Adresse cible :").strong());
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{}:{}", device.primary_address(), device.port))
                                    .monospace(),
                            );
                            if ui
                                .small_button("📋 Copier")
                                .on_hover_text("Copier l'adresse")
                                .clicked()
                            {
                                ui.ctx().copy_text(format!(
                                    "{}:{}",
                                    device.primary_address(),
                                    device.port
                                ));
                            }
                        });
                        ui.end_row();
                    }

                    ui.label(RichText::new("Démarrage :").strong());
                    ui.checkbox(
                        &mut state.edit_auto_connect,
                        "Connexion automatique au lancement de NTX Panel",
                    );
                    ui.end_row();
                });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if theme::primary_button(ui, p, "💾 Enregistrer les modifications").clicked() {
                    let clean_name = state.edit_hostname.trim().to_string();
                    if clean_name.is_empty() {
                        state.config_status_message =
                            Some(("Le nom du serveur ne peut pas être vide.".to_string(), true));
                    } else if device.is_manual {
                        let clean_addr =
                            crate::discovery::clean_address_str(&state.edit_address).to_string();
                        let parsed_port = state.edit_port.trim().parse::<u16>().ok().filter(|&n| n > 0);
                        if clean_addr.is_empty()
                            || !crate::discovery::is_usable_address(&clean_addr)
                        {
                            state.config_status_message = Some((
                                "Adresse invalide ou non supportée (évitez fe80::).".to_string(),
                                true,
                            ));
                        } else if let Some(port) = parsed_port {
                            state.config_status_message = Some((
                                "Configuration du serveur enregistrée !".to_string(),
                                false,
                            ));
                            *action = Some(DevicePanelAction::UpdateDeviceConfig {
                                id: device.id.clone(),
                                hostname: clean_name,
                                address: Some(clean_addr),
                                port: Some(port),
                                auto_connect: state.edit_auto_connect,
                            });
                        } else {
                            state.config_status_message = Some((
                                "Le port doit être un entier entre 1 et 65535.".to_string(),
                                true,
                            ));
                        }
                    } else {
                        state.config_status_message = Some((
                            "Configuration du serveur enregistrée !".to_string(),
                            false,
                        ));
                        *action = Some(DevicePanelAction::UpdateDeviceConfig {
                            id: device.id.clone(),
                            hostname: clean_name,
                            address: None,
                            port: None,
                            auto_connect: state.edit_auto_connect,
                        });
                    }
                }

                if let Some((msg, is_err)) = &state.config_status_message {
                    ui.label(
                        RichText::new(msg)
                            .color(if *is_err { p.danger } else { p.success })
                            .size(12.0),
                    );
                }
            });
        });

        ui.add_space(12.0);

        // Card 2: PSK & Authentication
        theme::card_frame(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("🔑 Authentification & Clé Pré-Partagée (PSK)")
                    .size(15.0)
                    .strong()
                    .color(p.text_primary),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Utilisée pour le challenge-response HMAC-SHA256 avec cet agent. Laissez vide pour utiliser la clé globale des Paramètres.",
                )
                .color(p.text_secondary)
                .size(12.0),
            );

            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("Clé PSK :");
                if state.show_psk {
                    ui.add(egui::TextEdit::singleline(&mut state.psk_input).desired_width(280.0));
                } else {
                    ui.add(
                        egui::TextEdit::singleline(&mut state.psk_input)
                            .password(true)
                            .desired_width(280.0)
                            .hint_text("Entrez la clé PSK..."),
                    );
                }

                if ui
                    .button(if state.show_psk {
                        "🙈 Masquer"
                    } else {
                        "👁 Afficher"
                    })
                    .clicked()
                {
                    state.show_psk = !state.show_psk;
                }

                if ui.button("💾 Enregistrer").clicked() {
                    let psk = state.psk_input.trim().to_string();
                    state.psk_status_message = Some(if psk.is_empty() {
                        "Clé spécifique effacée (la clé par défaut sera utilisée).".to_string()
                    } else {
                        "Clé PSK enregistrée !".to_string()
                    });
                    *action = Some(DevicePanelAction::SavePsk {
                        id: device.id.clone(),
                        psk,
                    });
                }

                if !device.status.is_connected()
                    && !matches!(device.status, ConnectionStatus::Connecting)
                    && theme::primary_button(ui, p, "⚡ Enregistrer & Connecter").clicked()
                {
                    let psk = state.psk_input.trim().to_string();
                    *action = Some(DevicePanelAction::Connect {
                        id: device.id.clone(),
                        psk: if psk.is_empty() {
                            device.psk.clone()
                        } else {
                            Some(psk)
                        },
                    });
                }
            });

            if let Some(msg) = &state.psk_status_message {
                ui.add_space(4.0);
                ui.label(RichText::new(msg).color(p.success).size(12.0));
            }
        });

        ui.add_space(12.0);

        // Card 3: TLS Fingerprint Card
        theme::card_frame(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("🛡 Sécurité TLS (Épinglage TOFU)")
                    .size(15.0)
                    .strong()
                    .color(p.text_primary),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "L'empreinte SHA-256 du certificat ECDSA serveur est vérifiée à chaque connexion.",
                )
                .color(p.text_secondary)
                .size(12.0),
            );
            ui.add_space(6.0);

            if let Some(ref fp) = device.cert_fingerprint {
                let formatted = format_fingerprint(fp);
                theme::subtle_bar_frame(p).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!("Empreinte épinglée : {formatted}"))
                                .monospace()
                                .size(11.5)
                                .color(p.text_primary),
                        );
                    });
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("📋 Copier l'empreinte").clicked() {
                        ui.ctx().copy_text(formatted);
                    }
                    if ui.button("🔄 Réinitialiser l'empreinte TLS").clicked() {
                        *action = Some(DevicePanelAction::ResetFingerprint(device.id.clone()));
                    }
                });
            } else if let Some(ref mdns_fp) = device.mdns_fingerprint {
                ui.label(
                    RichText::new(format!(
                        "Empreinte annoncée (mDNS) : {}",
                        format_fingerprint(mdns_fp)
                    ))
                    .monospace()
                    .size(11.5)
                    .color(p.text_secondary),
                );
            } else {
                ui.label(
                    RichText::new(
                        "Aucune empreinte enregistrée (sera épinglée automatiquement lors de la première connexion).",
                    )
                    .color(p.text_muted)
                    .size(12.0),
                );
            }
        });

        ui.add_space(12.0);

        // Card 4: System Specs & Ping Diagnostics
        theme::card_frame(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("ℹ Informations Système & Diagnostic")
                        .size(15.0)
                        .strong()
                        .color(p.text_primary),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if device.status.is_connected() && ui.button("📡 Tester le Ping").clicked() {
                        *action = Some(DevicePanelAction::Ping(device.id.clone()));
                    }
                });
            });
            ui.add_space(8.0);

            egui::Grid::new("sys_specs_grid")
                .striped(true)
                .min_col_width(150.0)
                .spacing([20.0, 8.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Adresses connues :").strong());
                    ui.label(RichText::new(device.addresses.join(", ")).monospace());
                    ui.end_row();

                    if let Some(ms) = device.last_ping_ms {
                        ui.label(RichText::new("Latence gRPC (Ping) :").strong());
                        ui.label(RichText::new(format!("{ms} ms")).color(p.accent));
                        ui.end_row();
                    }

                    if let Some(up) = device.server_uptime_secs {
                        ui.label(RichText::new("Temps d'activité agent :").strong());
                        ui.label(theme::format_uptime(up));
                        ui.end_row();
                    }

                    if let Some(info) = &device.system_info {
                        ui.label(RichText::new("Système d'exploitation :").strong());
                        ui.label(format!("{} {}", info.os_name, info.os_version));
                        ui.end_row();

                        ui.label(RichText::new("Architecture CPU :").strong());
                        ui.label(&info.arch);
                        ui.end_row();

                        ui.label(RichText::new("Cœurs logiques :").strong());
                        ui.label(format!("{} cœur(s)", info.cpu_count));
                        ui.end_row();

                        let ram_gb = info.total_memory_bytes as f64 / 1_073_741_824.0;
                        ui.label(RichText::new("Mémoire physique :").strong());
                        ui.label(format!("{ram_gb:.2} Go"));
                        ui.end_row();
                    }

                    if !device.version.is_empty() {
                        ui.label(RichText::new("Version de l'agent NTX :").strong());
                        ui.label(&device.version);
                        ui.end_row();
                    }
                });
        });

        ui.add_space(12.0);

        // Card 5: Danger Zone
        egui::Frame::new()
            .fill(p.danger_bg)
            .stroke(Stroke::new(1.0_f32, p.danger_border))
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new("⚠ Zone de Danger").strong().color(p.danger));
                        ui.label(
                            RichText::new(
                                "Supprimer ce serveur ainsi que ses favoris et son empreinte TLS épinglée.",
                            )
                            .color(p.text_secondary)
                            .size(12.0),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::danger_button(ui, p, "🗑 Supprimer définitivement").clicked() {
                            state.show_delete_confirm = true;
                        }
                    });
                });
            });
    });
}
