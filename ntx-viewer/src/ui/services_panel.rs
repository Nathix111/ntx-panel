use crate::state::{ConsoleStreamState, Device, FavoriteApp};
use crate::ui::console_widget::{
    self, ConsoleWidgetAction, ConsoleWidgetConfig, ConsoleWidgetState,
};
use crate::ui::theme::{self, Palette};
use egui::{Color32, CornerRadius, Margin, RichText, Stroke, Ui};

/// Actions emitted by the Managed Services ("Mes Services") panel.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ServicesPanelAction {
    Refresh,
    SelectService(String),
    SaveService(ntx_proto::ManagedServiceConfig),
    DeleteService(String),
    ControlService { id: String, action: String },
    SendInput { id: String, input: String },
    ReconnectConsole(String),
    ClearConsole,
    AttachExternalTerminal { command: String },
    ImportLegacyFavorites,
    ClearStatus,
}

/// Filter category for the managed services list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServiceListFilter {
    #[default]
    All,
    Running,
    Stopped,
    Crashed,
}

/// Destructive action confirmation payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingServiceConfirm {
    KillService { id: String, name: String },
    DeleteService { id: String, name: String },
}

/// Local UI state for the Managed Services tab.
#[derive(Debug, Clone)]
pub struct ServicesPanelState {
    pub search_query: String,
    pub status_filter: ServiceListFilter,
    pub console_state: ConsoleWidgetState,
    pub show_modal: bool,
    pub edit_id: Option<String>,
    pub name_input: String,
    pub icon_input: String,
    pub description_input: String,
    pub command_input: String,
    pub working_dir_input: String,
    pub stop_command_input: String,
    pub env_vars_text: String,
    pub quick_commands_text: String,
    pub systemd_unit_input: String,
    pub attach_command_input: String,
    pub auto_start_input: bool,
    pub auto_restart_input: bool,
    pub form_error: Option<String>,
    pub pending_confirm: Option<PendingServiceConfirm>,
}

impl Default for ServicesPanelState {
    fn default() -> Self {
        Self {
            search_query: String::new(),
            status_filter: ServiceListFilter::All,
            console_state: ConsoleWidgetState::default(),
            show_modal: false,
            edit_id: None,
            name_input: String::new(),
            icon_input: "⭐".to_string(),
            description_input: String::new(),
            command_input: String::new(),
            working_dir_input: String::new(),
            stop_command_input: String::new(),
            env_vars_text: String::new(),
            quick_commands_text: String::new(),
            systemd_unit_input: String::new(),
            attach_command_input: String::new(),
            auto_start_input: false,
            auto_restart_input: false,
            form_error: None,
            pending_confirm: None,
        }
    }
}

impl ServicesPanelState {
    pub fn reset_form(&mut self) {
        self.edit_id = None;
        self.name_input.clear();
        self.icon_input = "⭐".to_string();
        self.description_input.clear();
        self.command_input.clear();
        self.working_dir_input.clear();
        self.stop_command_input.clear();
        self.env_vars_text.clear();
        self.quick_commands_text.clear();
        self.systemd_unit_input.clear();
        self.attach_command_input.clear();
        self.auto_start_input = false;
        self.auto_restart_input = false;
        self.form_error = None;
    }

    pub fn open_new(&mut self) {
        self.reset_form();
        self.show_modal = true;
    }

    pub fn open_edit(&mut self, cfg: &ntx_proto::ManagedServiceConfig) {
        self.edit_id = Some(cfg.id.clone());
        self.name_input = cfg.name.clone();
        self.icon_input = if cfg.icon.trim().is_empty() {
            "⭐".to_string()
        } else {
            cfg.icon.clone()
        };
        self.description_input = cfg.description.clone();
        self.command_input = cfg.command.clone();
        self.working_dir_input = cfg.working_dir.clone();
        self.stop_command_input = cfg.stop_command.clone();
        self.env_vars_text = cfg.env_vars.join("\n");
        self.quick_commands_text = cfg.quick_commands.join("\n");
        self.systemd_unit_input = cfg.systemd_unit.clone();
        self.attach_command_input = cfg.attach_command.clone();
        self.auto_start_input = cfg.auto_start;
        self.auto_restart_input = cfg.auto_restart;
        self.form_error = None;
        self.show_modal = true;
    }

    /// Open the create/edit modal pre-populated from a partial config (for example
    /// when promoting a systemd unit or process from the Système tab).
    pub fn open_prefilled(&mut self, cfg: ntx_proto::ManagedServiceConfig) {
        let existing_id = cfg.id.trim().to_string();
        self.open_edit(&cfg);
        self.edit_id = (!existing_id.is_empty()).then_some(existing_id);
    }

    pub fn reset_for_device(&mut self) {
        self.search_query.clear();
        self.status_filter = ServiceListFilter::All;
        self.console_state = ConsoleWidgetState::default();
        self.show_modal = false;
        self.form_error = None;
        self.pending_confirm = None;
    }

    pub fn apply_minecraft_preset(&mut self) {
        self.name_input = "Serveur Minecraft".to_string();
        self.icon_input = "⛏".to_string();
        self.description_input = "Serveur Minecraft supervisé avec console live".to_string();
        self.command_input = "java -Xmx4G -Xms2G -jar server.jar nogui".to_string();
        self.working_dir_input.clear();
        self.stop_command_input = "stop".to_string();
        self.env_vars_text.clear();
        self.quick_commands_text =
            "list\nsave-all\nsay Sauvegarde du serveur\nstop".to_string();
        self.systemd_unit_input.clear();
        self.attach_command_input.clear();
        self.auto_start_input = false;
        self.auto_restart_input = true;
        self.form_error = None;
    }

    pub fn apply_bot_script_preset(&mut self) {
        self.name_input = "Bot / Script".to_string();
        self.icon_input = "⚡".to_string();
        self.description_input = "Bot, API Node.js ou script Python supervisé".to_string();
        self.command_input = "node index.js".to_string();
        self.working_dir_input.clear();
        self.stop_command_input.clear();
        self.env_vars_text.clear();
        self.quick_commands_text = "status\nhelp".to_string();
        self.systemd_unit_input.clear();
        self.attach_command_input.clear();
        self.auto_start_input = false;
        self.auto_restart_input = true;
        self.form_error = None;
    }

    pub fn apply_systemd_preset(&mut self) {
        self.name_input = "Service systemd".to_string();
        self.icon_input = "⚙".to_string();
        self.description_input = "Service système supervisé par systemd".to_string();
        self.command_input.clear();
        self.working_dir_input.clear();
        self.stop_command_input.clear();
        self.env_vars_text.clear();
        self.quick_commands_text.clear();
        self.systemd_unit_input = "mon-service.service".to_string();
        self.attach_command_input.clear();
        self.auto_start_input = false;
        self.auto_restart_input = false;
        self.form_error = None;
    }

    pub fn apply_tmux_preset(&mut self) {
        self.name_input = "Session tmux".to_string();
        self.icon_input = "💻".to_string();
        self.description_input = "Session terminal existante (tmux ou screen)".to_string();
        self.command_input.clear();
        self.working_dir_input.clear();
        self.stop_command_input.clear();
        self.env_vars_text.clear();
        self.quick_commands_text.clear();
        self.systemd_unit_input.clear();
        self.attach_command_input = "tmux attach -t minecraft".to_string();
        self.auto_start_input = false;
        self.auto_restart_input = false;
        self.form_error = None;
    }
}

/// Parse newline-separated `KEY=VALUE` environment variable lines.
pub fn parse_env_vars_text(raw: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, val)) = trimmed.split_once('=') else {
            return Err(format!(
                "Variable d'environnement invalide (format CLE=VALEUR attendu) : « {trimmed} »"
            ));
        };
        let key = key.trim();
        let valid_key = !key.is_empty()
            && key
                .as_bytes()
                .first()
                .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !valid_key {
            return Err(format!(
                "Nom de variable d'environnement invalide : « {key} »"
            ));
        }
        out.push(format!("{key}={val}"));
    }
    Ok(out)
}

/// Parse newline-separated quick command lines (trimmed, deduplicated empties, capped at 24).
pub fn parse_quick_commands_text(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .take(24)
        .map(ToString::to_string)
        .collect()
}

/// POSIX single-quote escaping for safe command composition (e.g. `journalctl -u`).
pub fn shell_quote_single(value: &str) -> String {
    let escaped = value.replace('\'', "'\\''");
    format!("'{escaped}'")
}

/// Build a safe `journalctl` follow command for a systemd unit name.
pub fn systemd_journalctl_command(unit: &str) -> String {
    format!("journalctl -u {} -n 100 -f", shell_quote_single(unit.trim()))
}

/// Validate form fields in `state` and build a [`ntx_proto::ManagedServiceConfig`].
pub fn build_config_from_form(
    state: &ServicesPanelState,
) -> Result<ntx_proto::ManagedServiceConfig, String> {
    let name = state.name_input.trim();
    if name.is_empty() {
        return Err("Le nom du service est obligatoire.".to_string());
    }
    if name.chars().count() > 100 {
        return Err("Le nom du service ne doit pas dépasser 100 caractères.".to_string());
    }

    let command = state.command_input.trim().to_string();
    let systemd_unit = state.systemd_unit_input.trim().to_string();
    let attach_command = state.attach_command_input.trim().to_string();

    if command.is_empty() && systemd_unit.is_empty() && attach_command.is_empty() {
        return Err(
            "Renseignez au moins une commande de démarrage, une unité systemd ou une commande d'attachement."
                .to_string(),
        );
    }

    if !systemd_unit.is_empty() {
        let valid_unit = !systemd_unit.starts_with('-')
            && systemd_unit.len() <= 128
            && systemd_unit.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'@' | b'.' | b'_' | b'-' | b':')
            });
        if !valid_unit {
            return Err(format!(
                "Nom d'unité systemd invalide : « {systemd_unit} »"
            ));
        }
    }

    let env_vars = parse_env_vars_text(&state.env_vars_text)?;
    let quick_commands = parse_quick_commands_text(&state.quick_commands_text);

    let icon = {
        let trimmed = state.icon_input.trim();
        if trimmed.is_empty() {
            "⭐".to_string()
        } else {
            trimmed.to_string()
        }
    };

    Ok(ntx_proto::ManagedServiceConfig {
        id: state.edit_id.clone().unwrap_or_default(),
        name: name.to_string(),
        icon,
        description: state.description_input.trim().to_string(),
        command,
        working_dir: state.working_dir_input.trim().to_string(),
        stop_command: state.stop_command_input.trim().to_string(),
        auto_start: state.auto_start_input,
        auto_restart: state.auto_restart_input,
        env_vars,
        quick_commands,
        systemd_unit,
        attach_command,
    })
}

/// Filter managed services by status category and case-insensitive search query.
pub fn filter_services<'a>(
    services: &'a [ntx_proto::ManagedServiceStatus],
    query: &str,
    filter: ServiceListFilter,
) -> Vec<&'a ntx_proto::ManagedServiceStatus> {
    let q = query.trim().to_lowercase();
    services
        .iter()
        .filter(|svc| {
            let st = svc.state.trim().to_ascii_lowercase();
            let status_ok = match filter {
                ServiceListFilter::All => true,
                ServiceListFilter::Running => {
                    matches!(st.as_str(), "running" | "active" | "starting" | "stopping")
                }
                ServiceListFilter::Stopped => {
                    matches!(st.as_str(), "stopped" | "inactive" | "exited")
                }
                ServiceListFilter::Crashed => matches!(st.as_str(), "crashed" | "failed"),
            };
            if !status_ok {
                return false;
            }
            if q.is_empty() {
                return true;
            }
            let Some(cfg) = svc.config.as_ref() else {
                return st.contains(&q);
            };
            cfg.name.to_lowercase().contains(&q)
                || cfg.description.to_lowercase().contains(&q)
                || cfg.command.to_lowercase().contains(&q)
                || cfg.systemd_unit.to_lowercase().contains(&q)
                || cfg.attach_command.to_lowercase().contains(&q)
                || st.contains(&q)
        })
        .collect()
}

/// Render the Managed Services ("⭐ Mes Services") panel.
pub fn render(
    ui: &mut Ui,
    state: &mut ServicesPanelState,
    device: &Device,
) -> Option<ServicesPanelAction> {
    let p = Palette::for_ui(ui);

    // 1. Disconnected server state
    if !device.status.is_connected() {
        ui.add_space(24.0);
        theme::card_frame(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(14.0);
                ui.label(
                    RichText::new("Serveur actuellement déconnecté")
                        .size(16.0)
                        .strong()
                        .color(p.text_primary),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Connectez-vous pour superviser vos services et interagir avec leur console en direct.",
                    )
                    .color(p.text_secondary),
                );
                ui.add_space(14.0);
            });
        });
        return None;
    }

    let mut action: Option<ServicesPanelAction> = None;

    // 2. Backwards compatibility view when connected to an older ntx-server
    if device.managed_services.unsupported_by_server {
        render_unsupported_server_view(ui, &p, device, &mut action);
        return action;
    }

    // 3. Optional legacy favorites migration banner
    if !device.favorites.is_empty() {
        render_legacy_migration_banner(ui, &p, device.favorites.len(), &mut action);
        ui.add_space(6.0);
    }

    // 4. Status notification banner (dismissible)
    if let Some((msg, is_error)) = &device.managed_services.status_message {
        render_status_banner(ui, &p, msg, *is_error, &mut action);
        ui.add_space(6.0);
    }

    // 5. Responsive 4-card KPI strip
    render_kpi_strip(ui, &p, state, device);
    ui.add_space(8.0);

    // 6. Master / detail responsive split
    let filtered = filter_services(
        &device.managed_services.services,
        &state.search_query,
        state.status_filter,
    );
    let avail_w = ui.available_width();

    if avail_w >= 760.0 {
        let gap = 10.0_f32;
        let left_w = ((avail_w - gap) * 0.34).clamp(240.0, 360.0);
        let right_w = (avail_w - gap - left_w).max(340.0);
        let split_h = ui.available_height().max(180.0);

        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(left_w, split_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    render_service_list_pane(ui, &p, state, device, &filtered, true, &mut action);
                },
            );
            let extra_gap = (gap - ui.spacing().item_spacing.x).max(0.0);
            if extra_gap > 0.0 {
                ui.add_space(extra_gap);
            }
            ui.allocate_ui_with_layout(
                egui::vec2(right_w, split_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("svc_detail_scroll", &device.id))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            render_service_detail_pane(ui, &p, state, device, &mut action);
                        });
                },
            );
        });
    } else {
        egui::ScrollArea::vertical()
            .id_salt(("svc_stacked_scroll", &device.id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                render_service_list_pane(ui, &p, state, device, &filtered, false, &mut action);
                ui.add_space(8.0);
                render_service_detail_pane(ui, &p, state, device, &mut action);
            });
    }

    // 7. Create / Edit Modal
    if state.show_modal {
        render_service_modal(ui, &p, state, &mut action);
    }

    // 8. Destructive Action Confirmation Modal
    if state.pending_confirm.is_some() {
        render_confirm_modal(ui, &p, state, &mut action);
    }

    action
}

fn is_service_busy_active(raw_state: &str) -> bool {
    matches!(
        raw_state.trim().to_ascii_lowercase().as_str(),
        "running" | "active" | "starting" | "stopping"
    )
}

fn service_state_presentation(p: &Palette, raw_state: &str) -> (&'static str, Color32, Color32, Color32) {
    match raw_state.trim().to_ascii_lowercase().as_str() {
        "running" | "active" => ("● Actif", p.success_bg, p.success, p.success_border),
        "starting" => ("● Démarrage...", p.accent_bg, p.accent, p.accent_border),
        "stopping" => ("● Arrêt...", p.warning_bg, p.warning, p.warning_border),
        "crashed" | "failed" => ("✖ Crash", p.danger_bg, p.danger, p.danger_border),
        _ => ("○ Arrêté", p.bg_subtle, p.text_muted, p.border),
    }
}

fn render_unsupported_server_view(
    ui: &mut Ui,
    p: &Palette,
    device: &Device,
    action: &mut Option<ServicesPanelAction>,
) {
    egui::Frame::new()
        .fill(p.warning_bg)
        .stroke(Stroke::new(1.0_f32, p.warning_border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let avail_w = ui.available_width();
            let draw_msg = |ui: &mut Ui| {
                ui.label(
                    RichText::new(
                        "⚠ Ce serveur exécute une version antérieure de ntx-server qui ne supporte pas encore la supervision native « Mes Services » (console PTY live). Mettez à jour ntx-server pour activer cette fonctionnalité.",
                    )
                    .color(p.warning),
                );
            };
            let draw_retry = |ui: &mut Ui, action: &mut Option<ServicesPanelAction>| {
                if ui.button("🔄 Réessayer").clicked() {
                    *action = Some(ServicesPanelAction::Refresh);
                }
            };

            if avail_w >= 680.0 {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_max_width((avail_w - 130.0).max(200.0));
                        draw_msg(ui);
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        draw_retry(ui, action);
                    });
                });
            } else {
                draw_msg(ui);
                ui.add_space(6.0);
                draw_retry(ui, action);
            }
        });

    ui.add_space(10.0);

    if device.favorites.is_empty() {
        theme::card_frame(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(10.0);
                ui.label(
                    RichText::new("Aucun ancien favori enregistré sur ce serveur.")
                        .color(p.text_secondary),
                );
                ui.add_space(10.0);
            });
        });
        return;
    }

    ui.label(
        RichText::new(format!(
            "⭐ Anciens favoris ({}) — mode compatibilité",
            device.favorites.len()
        ))
        .strong()
        .size(14.0)
        .color(p.text_primary),
    );
    ui.add_space(6.0);

    let cols = if ui.available_width() >= 760.0 { 2 } else { 1 };
    egui::ScrollArea::vertical()
        .id_salt(("legacy_favorites_compat", &device.id))
        .show(ui, |ui| {
            for chunk in device.favorites.chunks(cols) {
                ui.columns(cols, |columns| {
                    for (i, fav) in chunk.iter().enumerate() {
                        render_legacy_favorite_card(&mut columns[i], p, fav, action);
                    }
                });
                ui.add_space(8.0);
            }
        });
}

fn render_legacy_favorite_card(
    ui: &mut Ui,
    p: &Palette,
    fav: &FavoriteApp,
    action: &mut Option<ServicesPanelAction>,
) {
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            let icon = if fav.icon.trim().is_empty() {
                "⭐"
            } else {
                fav.icon.as_str()
            };
            ui.label(
                RichText::new(format!("{icon} {}", fav.name))
                    .strong()
                    .size(14.0)
                    .color(p.text_primary),
            );
            theme::pill_badge(
                ui,
                fav.app_type.display_name(),
                p.bg_subtle,
                p.text_secondary,
                Some(p.border),
            );
        });
        if !fav.description.trim().is_empty() {
            ui.add_space(3.0);
            ui.label(RichText::new(&fav.description).size(12.0).color(p.text_secondary));
        }

        let cmd = if !fav.terminal_command.trim().is_empty() {
            Some(fav.terminal_command.trim().to_string())
        } else if let Some(unit) = fav.service_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(systemd_journalctl_command(unit))
        } else {
            fav.auto_start_command
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
        };

        if let Some(cmd) = cmd {
            ui.add_space(6.0);
            if theme::primary_button(ui, p, "💻 Terminal")
                .on_hover_text(format!("Exécuter dans le terminal : {cmd}"))
                .clicked()
            {
                *action = Some(ServicesPanelAction::AttachExternalTerminal { command: cmd });
            }
        }
    });
}

fn render_legacy_migration_banner(
    ui: &mut Ui,
    p: &Palette,
    legacy_count: usize,
    action: &mut Option<ServicesPanelAction>,
) {
    egui::Frame::new()
        .fill(p.accent_bg)
        .stroke(Stroke::new(1.0_f32, p.accent_border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let avail_w = ui.available_width();
            let label = format!("ℹ {legacy_count} ancien(s) favori(s) disponible(s)");

            if avail_w >= 620.0 {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&label).color(p.accent).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::primary_button(ui, p, "📥 Importer vers Mes Services")
                            .on_hover_text(
                                "Convertir vos anciens favoris en services supervisés par le serveur",
                            )
                            .clicked()
                        {
                            *action = Some(ServicesPanelAction::ImportLegacyFavorites);
                        }
                    });
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&label).color(p.accent).strong());
                    if theme::primary_button(ui, p, "📥 Importer vers Mes Services").clicked() {
                        *action = Some(ServicesPanelAction::ImportLegacyFavorites);
                    }
                });
            }
        });
}

fn render_status_banner(
    ui: &mut Ui,
    p: &Palette,
    msg: &str,
    is_error: bool,
    action: &mut Option<ServicesPanelAction>,
) {
    let (bg, border, fg, icon) = if is_error {
        (p.danger_bg, p.danger_border, p.danger, "⚠")
    } else {
        (p.success_bg, p.success_border, p.success, "✔")
    };
    egui::Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0_f32, border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(12, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon).color(fg).strong());
                ui.label(RichText::new(msg).color(fg));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("✖")
                        .on_hover_text("Fermer le message")
                        .clicked()
                    {
                        *action = Some(ServicesPanelAction::ClearStatus);
                    }
                });
            });
        });
}

fn render_kpi_strip(
    ui: &mut Ui,
    p: &Palette,
    state: &mut ServicesPanelState,
    device: &Device,
) {
    let (total, running, stopped, crashed) = device.managed_services.service_counts();
    let avail_w = ui.available_width();

    let denom = total.max(1) as f32;
    let draw_total = |ui: &mut Ui, state: &mut ServicesPanelState| {
        render_kpi_card(
            ui,
            p,
            "TOTAL SERVICES",
            total,
            p.accent,
            if total > 0 { running as f32 / denom } else { 0.0 },
            if total == 0 {
                "Aucun service créé".to_string()
            } else {
                format!("{running} actif(s) sur {total}")
            },
            state.status_filter == ServiceListFilter::All,
        )
        .then(|| state.status_filter = ServiceListFilter::All);
    };
    let draw_running = |ui: &mut Ui, state: &mut ServicesPanelState| {
        render_kpi_card(
            ui,
            p,
            "EN COURS",
            running,
            p.success,
            running as f32 / denom,
            "Console PTY disponible".to_string(),
            state.status_filter == ServiceListFilter::Running,
        )
        .then(|| state.status_filter = ServiceListFilter::Running);
    };
    let draw_stopped = |ui: &mut Ui, state: &mut ServicesPanelState| {
        render_kpi_card(
            ui,
            p,
            "ARRÊTÉS",
            stopped,
            p.text_muted,
            stopped as f32 / denom,
            "Prêts à démarrer".to_string(),
            state.status_filter == ServiceListFilter::Stopped,
        )
        .then(|| state.status_filter = ServiceListFilter::Stopped);
    };
    let draw_crashed = |ui: &mut Ui, state: &mut ServicesPanelState| {
        let col = if crashed > 0 { p.danger } else { p.text_muted };
        render_kpi_card(
            ui,
            p,
            "EN ERREUR / CRASH",
            crashed,
            col,
            crashed as f32 / denom,
            if crashed > 0 {
                "Inspectez la console".to_string()
            } else {
                "Aucun incident".to_string()
            },
            state.status_filter == ServiceListFilter::Crashed,
        )
        .then(|| state.status_filter = ServiceListFilter::Crashed);
    };

    if avail_w >= 820.0 {
        ui.columns(4, |cols| {
            draw_total(&mut cols[0], state);
            draw_running(&mut cols[1], state);
            draw_stopped(&mut cols[2], state);
            draw_crashed(&mut cols[3], state);
        });
    } else if avail_w >= 460.0 {
        ui.columns(2, |cols| {
            draw_total(&mut cols[0], state);
            draw_running(&mut cols[1], state);
        });
        ui.add_space(6.0);
        ui.columns(2, |cols| {
            draw_stopped(&mut cols[0], state);
            draw_crashed(&mut cols[1], state);
        });
    } else {
        draw_total(ui, state);
        ui.add_space(6.0);
        draw_running(ui, state);
        ui.add_space(6.0);
        draw_stopped(ui, state);
        ui.add_space(6.0);
        draw_crashed(ui, state);
    }
}

#[allow(clippy::too_many_arguments)]
fn render_kpi_card(
    ui: &mut Ui,
    p: &Palette,
    title: &str,
    value: usize,
    accent: Color32,
    ratio: f32,
    caption: String,
    active_filter: bool,
) -> bool {
    let border = if active_filter {
        p.accent_border
    } else {
        p.border
    };
    let resp = ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt(("svc_kpi", title))
            .sense(egui::Sense::click()),
        |ui| {
            ui.style_mut().interaction.selectable_labels = false;
            egui::Frame::new()
                .fill(p.bg_card)
                .stroke(Stroke::new(1.0_f32, border))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(title).size(10.5).strong().color(p.text_muted));
                    ui.add_space(2.0);
                    ui.label(RichText::new(value.to_string()).size(20.0).strong().color(accent));
                    ui.add_space(4.0);
                    theme::mini_progress_bar(ui, p, ratio, accent, ui.available_width(), 4.5);
                    ui.add_space(3.0);
                    ui.add(
                        egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted))
                            .truncate(),
                    );
                });
        },
    );
    resp.response.clicked()
}

fn render_service_list_pane(
    ui: &mut Ui,
    p: &Palette,
    state: &mut ServicesPanelState,
    device: &Device,
    filtered: &[&ntx_proto::ManagedServiceStatus],
    side_by_side: bool,
    action: &mut Option<ServicesPanelAction>,
) {
    // Controls & Search bar
    theme::subtle_bar_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            if theme::primary_button(ui, p, "➕ Nouveau service")
                .on_hover_text("Créer un service supervisé (Minecraft, bot, systemd, tmux...)")
                .clicked()
            {
                state.open_new();
            }
            if ui
                .button("🔄")
                .on_hover_text("Actualiser la liste des services")
                .clicked()
            {
                *action = Some(ServicesPanelAction::Refresh);
            }
            if device.managed_services.is_loading {
                ui.spinner();
            }
        });

        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.label(RichText::new("🔍").size(12.5).color(p.text_muted));
            let clear_w = if state.search_query.is_empty() { 0.0 } else { 26.0 };
            let search_w = (ui.available_width() - 24.0 - clear_w).max(90.0);
            ui.add(
                egui::TextEdit::singleline(&mut state.search_query)
                    .id_salt(("svc_list_search", &device.id))
                    .hint_text("Rechercher un service...")
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

        let (_, _, _, crashed) = device.managed_services.service_counts();
        ui.horizontal_wrapped(|ui| {
            if theme::tab_button(ui, p, state.status_filter == ServiceListFilter::All, "Tous")
                .clicked()
            {
                state.status_filter = ServiceListFilter::All;
            }
            if theme::tab_button(
                ui,
                p,
                state.status_filter == ServiceListFilter::Running,
                "Actifs",
            )
            .clicked()
            {
                state.status_filter = ServiceListFilter::Running;
            }
            if theme::tab_button(
                ui,
                p,
                state.status_filter == ServiceListFilter::Stopped,
                "Arrêtés",
            )
            .clicked()
            {
                state.status_filter = ServiceListFilter::Stopped;
            }
            if (crashed > 0 || state.status_filter == ServiceListFilter::Crashed)
                && theme::tab_button(
                    ui,
                    p,
                    state.status_filter == ServiceListFilter::Crashed,
                    "Crash",
                )
                .clicked()
            {
                state.status_filter = ServiceListFilter::Crashed;
            }
        });
    });

    ui.add_space(6.0);

    // Scrollable service cards
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("svc_cards_scroll", &device.id))
        .auto_shrink([false, !side_by_side]);
    if !side_by_side {
        scroll = scroll.max_height(200.0);
    }

    scroll.show(ui, |ui| {
        if device.managed_services.services.is_empty() {
            theme::card_frame(p).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Aucun service configuré")
                            .strong()
                            .color(p.text_primary),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Créez votre premier serveur Minecraft, bot ou service.")
                            .size(12.0)
                            .color(p.text_secondary),
                    );
                    ui.add_space(8.0);
                    if ui.button("⛏ Modèle Minecraft").clicked() {
                        state.open_new();
                        state.apply_minecraft_preset();
                    }
                    ui.add_space(6.0);
                });
            });
            return;
        }

        if filtered.is_empty() {
            theme::card_frame(p).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Aucun service ne correspond aux filtres.")
                            .size(12.5)
                            .color(p.text_secondary),
                    );
                    ui.add_space(4.0);
                    if ui.small_button("Réinitialiser les filtres").clicked() {
                        state.search_query.clear();
                        state.status_filter = ServiceListFilter::All;
                    }
                    ui.add_space(6.0);
                });
            });
            return;
        }

        let selected_id = device.managed_services.selected_service_id.as_deref();
        for svc in filtered {
            if let Some(card_act) = render_service_list_card(ui, p, svc, selected_id) {
                *action = Some(card_act);
            }
            ui.add_space(6.0);
        }
    });
}

fn render_service_list_card(
    ui: &mut Ui,
    p: &Palette,
    svc: &ntx_proto::ManagedServiceStatus,
    selected_id: Option<&str>,
) -> Option<ServicesPanelAction> {
    let cfg = svc.config.as_ref()?;
    let is_selected = selected_id == Some(cfg.id.as_str());
    let (st_label, st_bg, st_fg, st_border) = service_state_presentation(p, &svc.state);
    let is_busy_active = is_service_busy_active(&svc.state);
    let has_controllable = !cfg.command.trim().is_empty() || !cfg.systemd_unit.trim().is_empty();

    let mut card_action: Option<ServicesPanelAction> = None;

    let base_fill = if is_selected {
        p.bg_card_selected
    } else {
        p.bg_card
    };
    let base_border = if is_selected {
        p.accent_border
    } else {
        p.border
    };

    let card_resp = ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt(("svc_card", &cfg.id))
            .sense(egui::Sense::click()),
        |ui| {
            ui.style_mut().interaction.selectable_labels = false;
            let mut prepared = egui::Frame::new()
                .fill(base_fill)
                .stroke(Stroke::new(1.0_f32, base_border))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin {
                    left: 14,
                    right: 10,
                    top: 9,
                    bottom: 9,
                })
                .begin(ui);

            {
                let cui = &mut prepared.content_ui;
                cui.set_width(cui.available_width());

                // Header row: status dot + icon + name + status pill
                cui.horizontal(|ui| {
                    let icon = if cfg.icon.trim().is_empty() {
                        "⭐"
                    } else {
                        cfg.icon.as_str()
                    };
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{icon} {}", cfg.name))
                                .strong()
                                .size(13.5)
                                .color(p.text_primary),
                        )
                        .truncate(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        theme::pill_badge(ui, st_label, st_bg, st_fg, Some(st_border));
                    });
                });

                // Live metrics or hybrid badges
                cui.add_space(4.0);
                cui.horizontal_wrapped(|ui| {
                    if is_busy_active && !cfg.command.trim().is_empty() {
                        let cpu_col = theme::usage_color(p, f64::from(svc.cpu_usage));
                        ui.label(
                            RichText::new(format!("CPU {:.1}%", svc.cpu_usage))
                                .monospace()
                                .size(11.0)
                                .color(cpu_col),
                        );
                        ui.label(RichText::new("•").size(10.5).color(p.text_muted));
                        ui.label(
                            RichText::new(theme::format_ram(svc.memory_bytes))
                                .monospace()
                                .size(11.0)
                                .color(p.text_secondary),
                        );
                        if svc.uptime_seconds > 0 {
                            ui.label(RichText::new("•").size(10.5).color(p.text_muted));
                            ui.label(
                                RichText::new(format!("⏱ {}", theme::format_uptime(svc.uptime_seconds)))
                                    .monospace()
                                    .size(11.0)
                                    .color(p.text_muted),
                            );
                        }
                    } else if !cfg.description.trim().is_empty() {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&cfg.description)
                                    .size(11.5)
                                    .color(p.text_secondary),
                            )
                            .truncate(),
                        );
                    }

                    if !cfg.systemd_unit.trim().is_empty() {
                        theme::pill_badge(
                            ui,
                            "⚙ systemd",
                            p.bg_subtle,
                            p.text_secondary,
                            Some(p.border),
                        );
                    }
                    if !cfg.attach_command.trim().is_empty() {
                        theme::pill_badge(
                            ui,
                            "💻 tmux",
                            p.bg_subtle,
                            p.text_secondary,
                            Some(p.border),
                        );
                    }
                });

                // Quick lifecycle action buttons
                if has_controllable {
                    cui.add_space(5.0);
                    cui.horizontal_wrapped(|ui| {
                        if !is_busy_active {
                            if ui
                                .small_button("▶")
                                .on_hover_text("Démarrer le service")
                                .clicked()
                            {
                                card_action = Some(ServicesPanelAction::ControlService {
                                    id: cfg.id.clone(),
                                    action: "start".to_string(),
                                });
                            }
                        } else {
                            if ui
                                .small_button("⏹")
                                .on_hover_text("Arrêter le service")
                                .clicked()
                            {
                                card_action = Some(ServicesPanelAction::ControlService {
                                    id: cfg.id.clone(),
                                    action: "stop".to_string(),
                                });
                            }
                            if ui
                                .small_button("🔄")
                                .on_hover_text("Redémarrer le service")
                                .clicked()
                            {
                                card_action = Some(ServicesPanelAction::ControlService {
                                    id: cfg.id.clone(),
                                    action: "restart".to_string(),
                                });
                            }
                        }
                    });
                }
            }

            let outer_resp = prepared.allocate_space(ui);
            let is_hovered = ui.response().hovered() || outer_resp.hovered();
            if is_hovered && !is_selected {
                prepared.frame.fill = p.bg_card_hover;
                prepared.frame.stroke = Stroke::new(1.0_f32, p.border_strong);
            }
            prepared.paint(ui);

            // 3px left status accent strip
            let card_rect = outer_resp.rect;
            if ui.is_rect_visible(card_rect) {
                let strip_rect = egui::Rect::from_min_max(
                    egui::pos2(card_rect.left() + 1.0, card_rect.top() + 7.0),
                    egui::pos2(card_rect.left() + 4.0, card_rect.bottom() - 7.0),
                );
                ui.painter().rect_filled(strip_rect, 1.5, st_fg);
            }
        },
    );

    if card_action.is_none() && card_resp.response.clicked() {
        card_action = Some(ServicesPanelAction::SelectService(cfg.id.clone()));
    }

    card_action
}

fn render_service_detail_pane(
    ui: &mut Ui,
    p: &Palette,
    state: &mut ServicesPanelState,
    device: &Device,
    action: &mut Option<ServicesPanelAction>,
) {
    let Some(selected) = device.managed_services.selected_service() else {
        render_empty_selection_card(ui, p, state, device, action);
        return;
    };
    let Some(cfg) = selected.config.as_ref() else {
        render_empty_selection_card(ui, p, state, device, action);
        return;
    };

    let (st_label, st_bg, st_fg, st_border) = service_state_presentation(p, &selected.state);
    let lower_state = selected.state.trim().to_ascii_lowercase();
    let is_running = matches!(lower_state.as_str(), "running" | "active");
    let is_busy_active = is_service_busy_active(&selected.state);
    let has_command = !cfg.command.trim().is_empty();
    let has_systemd = !cfg.systemd_unit.trim().is_empty();
    let has_attach = !cfg.attach_command.trim().is_empty();

    // 1. Selected Service Header Card
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let icon = if cfg.icon.trim().is_empty() {
            "⭐"
        } else {
            cfg.icon.as_str()
        };

        // Title + status pill + auto flags
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("{icon} {}", cfg.name))
                    .size(17.0)
                    .strong()
                    .color(p.text_primary),
            );
            theme::pill_badge(ui, st_label, st_bg, st_fg, Some(st_border));
            if cfg.auto_start {
                theme::pill_badge(
                    ui,
                    "⚡ Auto-start",
                    p.accent_bg,
                    p.accent,
                    Some(p.accent_border),
                );
            }
            if cfg.auto_restart {
                theme::pill_badge(
                    ui,
                    "🔄 Auto-restart",
                    p.bg_subtle,
                    p.text_secondary,
                    Some(p.border),
                );
            }
        });

        if !cfg.description.trim().is_empty() {
            ui.add_space(3.0);
            ui.label(
                RichText::new(&cfg.description)
                    .size(12.5)
                    .color(p.text_secondary),
            );
        }

        // Telemetry & runtime diagnostics badges
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if selected.pid > 0 {
                theme::pill_badge(
                    ui,
                    &format!("PID {}", selected.pid),
                    p.bg_subtle,
                    p.text_primary,
                    Some(p.border),
                );
            }
            if is_busy_active && has_command {
                let cpu_col = theme::usage_color(p, f64::from(selected.cpu_usage));
                theme::pill_badge(
                    ui,
                    &format!("CPU {:.1}%", selected.cpu_usage),
                    p.bg_subtle,
                    cpu_col,
                    Some(p.border),
                );
                theme::pill_badge(
                    ui,
                    &format!("RAM {}", theme::format_ram(selected.memory_bytes)),
                    p.bg_subtle,
                    p.purple,
                    Some(p.border),
                );
                theme::pill_badge(
                    ui,
                    &format!("⏱ {}", theme::format_uptime(selected.uptime_seconds)),
                    p.bg_subtle,
                    p.text_secondary,
                    Some(p.border),
                );
            }
            if selected.restart_count > 0 {
                theme::pill_badge(
                    ui,
                    &format!("🔄 {} redémarrage(s)", selected.restart_count),
                    p.warning_bg,
                    p.warning,
                    Some(p.warning_border),
                );
            }
            if selected.has_exit_code {
                let (bg, fg, bd) = if selected.last_exit_code == 0 {
                    (p.bg_subtle, p.text_secondary, p.border)
                } else {
                    (p.danger_bg, p.danger, p.danger_border)
                };
                theme::pill_badge(
                    ui,
                    &format!("Code sortie : {}", selected.last_exit_code),
                    bg,
                    fg,
                    Some(bd),
                );
            }
        });

        // Last error banner if present
        if !selected.last_error.trim().is_empty() {
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(p.danger_bg)
                .stroke(Stroke::new(1.0_f32, p.danger_border))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new(format!("⚠ {}", selected.last_error))
                            .size(12.0)
                            .color(p.danger),
                    );
                });
        }

        // Command & working_dir monospace summary
        if has_command || !cfg.working_dir.trim().is_empty() || has_systemd || has_attach {
            ui.add_space(6.0);
            theme::subtle_bar_frame(p).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if has_command {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("$ {}", cfg.command))
                                .monospace()
                                .size(11.5)
                                .color(p.text_primary),
                        )
                        .truncate(),
                    );
                }
                if !cfg.working_dir.trim().is_empty() {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("Dossier : {}", cfg.working_dir))
                                .monospace()
                                .size(11.0)
                                .color(p.text_muted),
                        )
                        .truncate(),
                    );
                }
                if has_systemd {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("⚙ Unité systemd : {}", cfg.systemd_unit))
                                .monospace()
                                .size(11.0)
                                .color(p.text_muted),
                        )
                        .truncate(),
                    );
                }
                if has_attach {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("💻 Attachement : {}", cfg.attach_command))
                                .monospace()
                                .size(11.0)
                                .color(p.text_muted),
                        )
                        .truncate(),
                    );
                }
            });
        }

        // Action Bar
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if has_command || has_systemd {
                if !is_busy_active {
                    if theme::primary_button(ui, p, "▶ Démarrer")
                        .on_hover_text("Lancer le service sur le serveur")
                        .clicked()
                    {
                        *action = Some(ServicesPanelAction::ControlService {
                            id: cfg.id.clone(),
                            action: "start".to_string(),
                        });
                    }
                } else {
                    if ui
                        .button("⏹ Arrêter")
                        .on_hover_text("Arrêt propre du service (stop_command puis SIGTERM)")
                        .clicked()
                    {
                        *action = Some(ServicesPanelAction::ControlService {
                            id: cfg.id.clone(),
                            action: "stop".to_string(),
                        });
                    }
                }

                if ui
                    .button("🔄 Redémarrer")
                    .on_hover_text("Redémarrer le service")
                    .clicked()
                {
                    *action = Some(ServicesPanelAction::ControlService {
                        id: cfg.id.clone(),
                        action: "restart".to_string(),
                    });
                }

                if is_busy_active
                    && has_command
                    && theme::danger_button(ui, p, "⛔ Kill")
                        .on_hover_text("Arrêt forcé immédiat (SIGKILL)")
                        .clicked()
                {
                    state.pending_confirm = Some(PendingServiceConfirm::KillService {
                        id: cfg.id.clone(),
                        name: cfg.name.clone(),
                    });
                }
            }

            if has_systemd
                && ui
                    .button("📜 Logs systemd")
                    .on_hover_text("Ouvrir journalctl en direct dans le terminal")
                    .clicked()
            {
                *action = Some(ServicesPanelAction::AttachExternalTerminal {
                    command: systemd_journalctl_command(&cfg.systemd_unit),
                });
            }

            if has_attach
                && ui
                    .button("💻 Attacher")
                    .on_hover_text("Ouvrir la commande d'attachement dans le terminal")
                    .clicked()
            {
                *action = Some(ServicesPanelAction::AttachExternalTerminal {
                    command: cfg.attach_command.clone(),
                });
            }

            if ui
                .button("⚙ Modifier")
                .on_hover_text("Modifier la configuration de ce service")
                .clicked()
            {
                state.open_edit(cfg);
            }

            if ui
                .button("🗑 Supprimer")
                .on_hover_text("Supprimer définitivement ce service")
                .clicked()
            {
                state.pending_confirm = Some(PendingServiceConfirm::DeleteService {
                    id: cfg.id.clone(),
                    name: cfg.name.clone(),
                });
            }
        });
    });

    ui.add_space(8.0);

    // 2. Embedded Live Interactive Console Widget
    let console_matches = device
        .managed_services
        .console_service_id
        .as_deref()
        == Some(cfg.id.as_str());
    let (connected_label, is_streaming) = if console_matches {
        match &device.managed_services.console_state {
            ConsoleStreamState::Streaming => ("● Flux live", true),
            ConsoleStreamState::Connecting => ("● Connexion...", false),
            ConsoleStreamState::Error(_) => ("✖ Flux interrompu", false),
            ConsoleStreamState::Closed => ("○ Flux fermé", false),
            ConsoleStreamState::Idle => ("○ Inactif", false),
        }
    } else {
        ("○ Non connecté", false)
    };

    let can_send_input = is_running && has_command;
    let empty_hint = if !has_command && has_systemd {
        "Service systemd : cliquez sur « 📜 Logs systemd » pour suivre journalctl en direct."
    } else if !has_command && has_attach {
        "Session externe : cliquez sur « 💻 Attacher » pour ouvrir le terminal dédié."
    } else if !is_busy_active {
        "Service arrêté. Cliquez sur « ▶ Démarrer » pour lancer la console en direct."
    } else {
        "En attente de sortie du service..."
    };

    let console_cfg = ConsoleWidgetConfig {
        id_salt: &cfg.id,
        connected_label,
        is_streaming,
        can_send_input,
        input_placeholder: "Envoyer une commande en direct (ex: say Bonjour, list, stop)...",
        quick_commands: &cfg.quick_commands,
        empty_hint,
    };

    if let Some(console_act) = console_widget::render(
        ui,
        p,
        &device.managed_services.console_buffer,
        &mut state.console_state,
        &console_cfg,
    ) {
        match console_act {
            ConsoleWidgetAction::SendCommand(cmd) => {
                *action = Some(ServicesPanelAction::SendInput {
                    id: cfg.id.clone(),
                    input: cmd,
                });
            }
            ConsoleWidgetAction::Reconnect => {
                *action = Some(ServicesPanelAction::ReconnectConsole(cfg.id.clone()));
            }
            ConsoleWidgetAction::ClearBuffer => {
                *action = Some(ServicesPanelAction::ClearConsole);
            }
        }
    }
}

fn render_empty_selection_card(
    ui: &mut Ui,
    p: &Palette,
    state: &mut ServicesPanelState,
    device: &Device,
    action: &mut Option<ServicesPanelAction>,
) {
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            ui.add_space(28.0);
            if device.managed_services.services.is_empty() {
                ui.label(
                    RichText::new("⭐ Supervisez vos services avec console en direct")
                        .size(16.0)
                        .strong()
                        .color(p.text_primary),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "Créez un serveur Minecraft, un bot Discord, une API ou reliez un service systemd/tmux pour consulter ses logs colorés et envoyer des commandes en temps réel.",
                    )
                    .color(p.text_secondary),
                );
                ui.add_space(16.0);
                ui.horizontal_wrapped(|ui| {
                    if theme::primary_button(ui, p, "⛏ Créer un serveur Minecraft").clicked() {
                        state.open_new();
                        state.apply_minecraft_preset();
                    }
                    if ui.button("➕ Créer un service").clicked() {
                        state.open_new();
                    }
                });
            } else {
                ui.label(
                    RichText::new("Sélectionnez un service dans la liste")
                        .size(15.0)
                        .strong()
                        .color(p.text_primary),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Cliquez sur une carte à gauche pour contrôler le service et afficher sa console interactive.",
                    )
                    .color(p.text_secondary),
                );
                if let Some(first_id) = device
                    .managed_services
                    .services
                    .first()
                    .and_then(|s| s.config.as_ref())
                    .map(|c| c.id.clone())
                {
                    ui.add_space(10.0);
                    if theme::primary_button(ui, p, "▶ Ouvrir le premier service").clicked() {
                        *action = Some(ServicesPanelAction::SelectService(first_id));
                    }
                }
            }
            ui.add_space(28.0);
        });
    });
}

fn render_service_modal(
    ui: &mut Ui,
    p: &Palette,
    state: &mut ServicesPanelState,
    action: &mut Option<ServicesPanelAction>,
) {
    let title = if state.edit_id.is_some() {
        "⚙ Modifier le service"
    } else {
        "➕ Nouveau service"
    };

    let mut close_modal = false;

    egui::Window::new(title)
        .collapsible(false)
        .resizable(true)
        .default_width(580.0)
        .max_height(640.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            // Template presets row
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Modèles :").size(12.0).color(p.text_muted));
                if ui.small_button("⛏ Minecraft").clicked() {
                    state.apply_minecraft_preset();
                }
                if ui.small_button("⚡ Bot / Script").clicked() {
                    state.apply_bot_script_preset();
                }
                if ui.small_button("⚙ Systemd").clicked() {
                    state.apply_systemd_preset();
                }
                if ui.small_button("💻 tmux / screen").clicked() {
                    state.apply_tmux_preset();
                }
            });

            ui.separator();

            egui::ScrollArea::vertical()
                .id_salt("svc_modal_scroll")
                .max_height(460.0)
                .show(ui, |ui| {
                    // Identity: icon + name
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Icône :").color(p.text_secondary));
                        for preset_icon in ["⭐", "⛏", "⚡", "⚙", "💻", "🐳", "🌐", "🗄", "📜"] {
                            let selected = state.icon_input.trim() == preset_icon;
                            if theme::tab_button(ui, p, selected, preset_icon).clicked() {
                                state.icon_input = preset_icon.to_string();
                            }
                        }
                    });
                    ui.add_space(6.0);

                    ui.label(RichText::new("Nom du service *").strong().color(p.text_primary));
                    ui.add(
                        egui::TextEdit::singleline(&mut state.name_input)
                            .hint_text("ex: Serveur Minecraft Survie")
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(6.0);

                    ui.label(RichText::new("Description").color(p.text_secondary));
                    ui.add(
                        egui::TextEdit::singleline(&mut state.description_input)
                            .hint_text("Courte description (optionnel)")
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(10.0);

                    // Supervised PTY section
                    ui.label(
                        RichText::new("⚡ Supervision native (PTY & console live)")
                            .strong()
                            .color(p.accent),
                    );
                    ui.add_space(4.0);

                    ui.label(RichText::new("Commande de démarrage").color(p.text_secondary));
                    ui.add(
                        egui::TextEdit::singleline(&mut state.command_input)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("ex: java -Xmx4G -Xms2G -jar server.jar nogui")
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(6.0);

                    ui.label(RichText::new("Dossier de travail (working_dir)").color(p.text_secondary));
                    ui.add(
                        egui::TextEdit::singleline(&mut state.working_dir_input)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("ex: /srv/minecraft (optionnel)")
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(6.0);

                    ui.label(
                        RichText::new("Commande d'arrêt propre (envoyée sur stdin)")
                            .color(p.text_secondary),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut state.stop_command_input)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("ex: stop (optionnel — fallback SIGTERM)")
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(6.0);

                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(
                            &mut state.auto_start_input,
                            "Démarrer automatiquement avec ntx-server",
                        );
                        ui.checkbox(
                            &mut state.auto_restart_input,
                            "Redémarrer automatiquement en cas de crash",
                        );
                    });
                    ui.add_space(6.0);

                    ui.label(
                        RichText::new("Commandes rapides de console (une par ligne, max 24)")
                            .color(p.text_secondary),
                    );
                    ui.add(
                        egui::TextEdit::multiline(&mut state.quick_commands_text)
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(3)
                            .hint_text("list\nsave-all\nsay Bonjour\nstop")
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(6.0);

                    ui.label(
                        RichText::new("Variables d'environnement (CLE=VALEUR, une par ligne)")
                            .color(p.text_secondary),
                    );
                    ui.add(
                        egui::TextEdit::multiline(&mut state.env_vars_text)
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(2)
                            .hint_text("NODE_ENV=production\nPORT=8080")
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(10.0);

                    // Hybrid systemd / tmux links
                    ui.label(
                        RichText::new("⚙ Liens hybrides (systemd / tmux / screen — optionnels)")
                            .strong()
                            .color(p.text_secondary),
                    );
                    ui.add_space(4.0);

                    ui.label(RichText::new("Unité systemd liée").color(p.text_secondary));
                    ui.add(
                        egui::TextEdit::singleline(&mut state.systemd_unit_input)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("ex: nginx.service ou minecraft.service")
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(6.0);

                    ui.label(
                        RichText::new("Commande d'attachement terminal").color(p.text_secondary),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut state.attach_command_input)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("ex: tmux attach -t minecraft")
                            .desired_width(f32::INFINITY),
                    );
                });

            if let Some(err) = &state.form_error {
                ui.add_space(6.0);
                egui::Frame::new()
                    .fill(p.danger_bg)
                    .stroke(Stroke::new(1.0_f32, p.danger_border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(format!("⚠ {err}")).color(p.danger));
                    });
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Annuler").clicked() {
                    close_modal = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::primary_button(ui, p, "✔ Enregistrer").clicked() {
                        match build_config_from_form(state) {
                            Ok(cfg) => {
                                *action = Some(ServicesPanelAction::SaveService(cfg));
                                close_modal = true;
                            }
                            Err(msg) => {
                                state.form_error = Some(msg);
                            }
                        }
                    }
                });
            });
        });

    if close_modal {
        state.show_modal = false;
        state.form_error = None;
    }
}

fn render_confirm_modal(
    ui: &mut Ui,
    p: &Palette,
    state: &mut ServicesPanelState,
    action: &mut Option<ServicesPanelAction>,
) {
    let Some(confirm) = state.pending_confirm.clone() else {
        return;
    };
    let mut clear_confirm = false;

    egui::Window::new("⚠ Confirmation requise")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            match &confirm {
                PendingServiceConfirm::KillService { id, name } => {
                    ui.label(format!(
                        "Forcer l'arrêt immédiat (SIGKILL) du service « {name} » ?"
                    ));
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Les données non sauvegardées en mémoire (ex: monde Minecraft) risquent d'être perdues.",
                        )
                        .size(12.0)
                        .color(p.warning),
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Annuler").clicked() {
                            clear_confirm = true;
                        }
                        if theme::danger_button(ui, p, "⛔ Forcer l'arrêt (Kill)").clicked() {
                            *action = Some(ServicesPanelAction::ControlService {
                                id: id.clone(),
                                action: "kill".to_string(),
                            });
                            clear_confirm = true;
                        }
                    });
                }
                PendingServiceConfirm::DeleteService { id, name } => {
                    ui.label(format!("Supprimer définitivement le service « {name} » ?"));
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "S'il est en cours d'exécution, il sera arrêté puis retiré de la configuration du serveur.",
                        )
                        .size(12.0)
                        .color(p.text_secondary),
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Annuler").clicked() {
                            clear_confirm = true;
                        }
                        if theme::danger_button(ui, p, "🗑 Supprimer").clicked() {
                            *action = Some(ServicesPanelAction::DeleteService(id.clone()));
                            clear_confirm = true;
                        }
                    });
                }
            }
        });

    if clear_confirm {
        state.pending_confirm = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_presets_populate_valid_configs() {
        let mut st = ServicesPanelState::default();

        st.apply_minecraft_preset();
        let mc = build_config_from_form(&st).expect("minecraft preset must be valid");
        assert_eq!(mc.icon, "⛏");
        assert!(mc.command.contains("server.jar"));
        assert_eq!(mc.stop_command, "stop");
        assert!(mc.auto_restart);
        assert_eq!(
            mc.quick_commands,
            vec!["list", "save-all", "say Sauvegarde du serveur", "stop"]
        );

        st.apply_bot_script_preset();
        let bot = build_config_from_form(&st).expect("bot preset must be valid");
        assert_eq!(bot.icon, "⚡");
        assert_eq!(bot.command, "node index.js");
        assert_eq!(bot.quick_commands, vec!["status", "help"]);

        st.apply_systemd_preset();
        let sysd = build_config_from_form(&st).expect("systemd preset must be valid");
        assert_eq!(sysd.icon, "⚙");
        assert_eq!(sysd.systemd_unit, "mon-service.service");

        st.apply_tmux_preset();
        let tmux = build_config_from_form(&st).expect("tmux preset must be valid");
        assert_eq!(tmux.icon, "💻");
        assert_eq!(tmux.attach_command, "tmux attach -t minecraft");
    }

    #[test]
    fn test_parse_env_vars_and_quick_commands_validation() {
        let ok = parse_env_vars_text("# comment\n\nPORT=8080\nMC_OPTS=-Xmx4G -XX:+UseG1GC\n")
            .expect("valid env vars");
        assert_eq!(ok, vec!["PORT=8080", "MC_OPTS=-Xmx4G -XX:+UseG1GC"]);

        assert!(parse_env_vars_text("MISSING_EQUALS").is_err());
        assert!(parse_env_vars_text("123BAD=val").is_err());
        assert!(parse_env_vars_text("BAD-KEY=val").is_err());

        let many = (0..30).map(|i| format!("cmd {i}")).collect::<Vec<_>>().join("\n");
        let parsed = parse_quick_commands_text(&many);
        assert_eq!(parsed.len(), 24);
        assert_eq!(parsed[0], "cmd 0");
        assert_eq!(parsed[23], "cmd 23");
    }

    #[test]
    fn test_form_validation_rejects_empty_and_invalid_systemd_unit() {
        let mut st = ServicesPanelState::default();
        assert!(build_config_from_form(&st).is_err());

        st.name_input = "Mon app".to_string();
        assert!(build_config_from_form(&st).is_err());

        st.systemd_unit_input = "nginx; rm -rf /".to_string();
        assert!(build_config_from_form(&st).is_err());

        st.systemd_unit_input = "nginx.service".to_string();
        let cfg = build_config_from_form(&st).expect("valid systemd service");
        assert_eq!(cfg.name, "Mon app");
        assert_eq!(cfg.systemd_unit, "nginx.service");
    }

    #[test]
    fn test_filter_services_by_status_and_search() {
        let make = |id: &str, name: &str, cmd: &str, state: &str| ntx_proto::ManagedServiceStatus {
            config: Some(ntx_proto::ManagedServiceConfig {
                id: id.to_string(),
                name: name.to_string(),
                command: cmd.to_string(),
                ..Default::default()
            }),
            state: state.to_string(),
            ..Default::default()
        };

        let services = vec![
            make("1", "Minecraft Survie", "java -jar paper.jar", "running"),
            make("2", "Bot Discord", "node bot.js", "stopped"),
            make("3", "Worker API", "python3 worker.py", "crashed"),
        ];

        assert_eq!(filter_services(&services, "", ServiceListFilter::All).len(), 3);
        assert_eq!(filter_services(&services, "", ServiceListFilter::Running).len(), 1);
        assert_eq!(filter_services(&services, "", ServiceListFilter::Stopped).len(), 1);
        assert_eq!(filter_services(&services, "", ServiceListFilter::Crashed).len(), 1);

        let by_cmd = filter_services(&services, "paper.jar", ServiceListFilter::All);
        assert_eq!(by_cmd.len(), 1);
        assert_eq!(by_cmd[0].config.as_ref().unwrap().id, "1");
    }

    #[test]
    fn test_systemd_journalctl_quote_escaping() {
        assert_eq!(
            systemd_journalctl_command("nginx.service"),
            "journalctl -u 'nginx.service' -n 100 -f"
        );
        assert_eq!(
            systemd_journalctl_command("weird'unit"),
            "journalctl -u 'weird'\\''unit' -n 100 -f"
        );
    }

    #[test]
    fn test_open_prefilled_preserves_fields_and_optional_id() {
        let mut st = ServicesPanelState::default();
        let prefill = ntx_proto::ManagedServiceConfig {
            id: String::new(),
            name: "Postgres".to_string(),
            icon: "🗄".to_string(),
            systemd_unit: "postgresql.service".to_string(),
            ..Default::default()
        };
        st.open_prefilled(prefill);
        assert!(st.show_modal);
        assert!(st.edit_id.is_none());
        assert_eq!(st.name_input, "Postgres");
        assert_eq!(st.icon_input, "🗄");
        assert_eq!(st.systemd_unit_input, "postgresql.service");
    }

    #[test]
    fn test_services_panel_render_smoke_modes() {
        use crate::state::{ConnectionStatus, FavoriteAppType};

        let ctx = egui::Context::default();
        theme::apply_theme(&ctx, true);

        let mut dev = Device::new(
            "srv-1".to_string(),
            "rpi-home".to_string(),
            vec!["192.168.1.50".to_string()],
            9850,
            "0.3.0".to_string(),
        );
        let mut st = ServicesPanelState::default();

        // 1. Disconnected state
        let raw_narrow = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(520.0, 620.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(raw_narrow.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(render(ui, &mut st, &dev).is_none());
            });
        });

        // 2. Connected + legacy unsupported server fallback with favorites
        dev.status = ConnectionStatus::Connected;
        dev.managed_services.unsupported_by_server = true;
        dev.favorites.push(FavoriteApp::new(
            "fav-1",
            "Minecraft Legacy",
            "⛏",
            FavoriteAppType::Process,
            "minecraft",
            None,
            "tmux attach -t mc",
            "Ancien serveur",
            None,
        ));
        let _ = ctx.run(raw_narrow, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(render(ui, &mut st, &dev).is_none());
            });
        });

        // 3. Connected + supported native services + live console + modal (wide viewport, light theme)
        theme::apply_theme(&ctx, false);
        dev.managed_services.unsupported_by_server = false;
        dev.managed_services.services.push(ntx_proto::ManagedServiceStatus {
            config: Some(ntx_proto::ManagedServiceConfig {
                id: "svc_mc".to_string(),
                name: "Minecraft Survie".to_string(),
                icon: "⛏".to_string(),
                description: "PaperMC 1.21".to_string(),
                command: "java -Xmx4G -jar paper.jar nogui".to_string(),
                working_dir: "/srv/mc".to_string(),
                stop_command: "stop".to_string(),
                auto_start: true,
                auto_restart: true,
                quick_commands: vec!["list".to_string(), "save-all".to_string()],
                ..Default::default()
            }),
            state: "running".to_string(),
            pid: 4242,
            cpu_usage: 18.5,
            memory_bytes: 1500 * 1024 * 1024,
            uptime_seconds: 3720,
            ..Default::default()
        });
        dev.managed_services.selected_service_id = Some("svc_mc".to_string());
        dev.managed_services.console_service_id = Some("svc_mc".to_string());
        dev.managed_services.console_state = ConsoleStreamState::Streaming;
        dev.managed_services
            .console_buffer
            .push_bytes(b"\x1b[32m[INFO]\x1b[0m Done (3.4s)!\n");
        st.open_new();
        st.apply_minecraft_preset();

        let raw_wide = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1180.0, 740.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(raw_wide, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(render(ui, &mut st, &dev).is_none());
            });
        });

        // 4. Default window inner pane (850x600 -> side-by-side >= 760) & min window pane (420x420 -> stacked scroll)
        // Also exercise the "stopping" state on a service card.
        st.show_modal = false;
        dev.managed_services.services[0].state = "stopping".to_string();
        assert!(is_service_busy_active("stopping"));
        assert!(!is_service_busy_active("stopped"));

        for (w, h) in [(850.0_f32, 600.0_f32), (420.0_f32, 420.0_f32)] {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(w, h),
                )),
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    assert!(render(ui, &mut st, &dev).is_none());
                });
            });
        }
    }
}
