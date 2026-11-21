use crate::state::{ConsoleStreamState, Device, DockerConsoleMode};
use crate::ui::console_widget::{
    self, ConsoleWidgetAction, ConsoleWidgetConfig, ConsoleWidgetState,
};
use crate::ui::theme::{self, Palette};
use egui::{Color32, CornerRadius, Margin, RichText, Stroke, Ui};
use ntx_proto::DockerContainerItem;
use std::time::{Duration, Instant};

/// Actions emitted by the Docker container management view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockerPanelAction {
    Refresh {
        include_all: bool,
    },
    ToggleIncludeAll(bool),
    SelectContainer(String),
    SetConsoleMode {
        container_id: String,
        mode: DockerConsoleMode,
    },
    ControlContainer {
        container_id: String,
        action: String,
        force: bool,
    },
    SendConsoleInput {
        container_id: String,
        input: String,
    },
    ExecCommand {
        container_id: String,
        command: String,
    },
    ReconnectConsole {
        container_id: String,
        mode: DockerConsoleMode,
    },
    ClearConsole,
    ClearStatus,
}

/// Filter pill for container lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DockerStatusFilter {
    #[default]
    All,
    Running,
    Paused,
    Stopped,
}

/// Pending confirmation dialog payload for destructive container operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingDockerConfirm {
    Kill {
        id: String,
        name: String,
    },
    Remove {
        id: String,
        name: String,
        force: bool,
    },
}

/// Local UI state owned by the Docker tab.
#[derive(Debug, Clone, Default)]
pub struct DockerPanelState {
    pub search_query: String,
    pub status_filter: DockerStatusFilter,
    pub console_state: ConsoleWidgetState,
    pub pending_confirm: Option<PendingDockerConfirm>,
    pub copied_id: Option<(String, Instant)>,
}

impl DockerPanelState {
    pub fn reset_for_device(&mut self) {
        self.search_query.clear();
        self.status_filter = DockerStatusFilter::All;
        self.console_state = ConsoleWidgetState::default();
        self.pending_confirm = None;
        self.copied_id = None;
    }
}

/// Semantic visual styling for a Docker container state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerStatePresentation {
    pub label: &'static str,
    pub dot: &'static str,
    pub bg: Color32,
    pub fg: Color32,
    pub border: Color32,
    pub accent_strip: Color32,
}

pub fn container_state_presentation(p: &Palette, state: &str) -> ContainerStatePresentation {
    match state.trim().to_ascii_lowercase().as_str() {
        "running" => ContainerStatePresentation {
            label: "● Actif",
            dot: "●",
            bg: p.success_bg,
            fg: p.success,
            border: p.success_border,
            accent_strip: p.success,
        },
        "paused" => ContainerStatePresentation {
            label: "⏸ En pause",
            dot: "●",
            bg: p.warning_bg,
            fg: p.warning,
            border: p.warning_border,
            accent_strip: p.warning,
        },
        "restarting" => ContainerStatePresentation {
            label: "🔄 Redémarrage",
            dot: "●",
            bg: p.accent_bg,
            fg: p.accent,
            border: p.accent_border,
            accent_strip: p.accent,
        },
        "dead" => ContainerStatePresentation {
            label: "✖ Mort",
            dot: "●",
            bg: p.danger_bg,
            fg: p.danger,
            border: p.danger_border,
            accent_strip: p.danger,
        },
        "created" => ContainerStatePresentation {
            label: "○ Créé",
            dot: "○",
            bg: p.bg_subtle,
            fg: p.text_secondary,
            border: p.border,
            accent_strip: p.border_strong,
        },
        "exited" | "stopped" => ContainerStatePresentation {
            label: "○ Arrêté",
            dot: "○",
            bg: p.bg_subtle,
            fg: p.text_muted,
            border: p.border,
            accent_strip: p.border_strong,
        },
        _ => ContainerStatePresentation {
            label: "○ Inconnu",
            dot: "○",
            bg: p.bg_subtle,
            fg: p.text_muted,
            border: p.border,
            accent_strip: p.border_strong,
        },
    }
}

pub fn filter_containers<'a>(
    containers: &'a [DockerContainerItem],
    status_filter: DockerStatusFilter,
    search_query: &str,
) -> Vec<&'a DockerContainerItem> {
    let q = search_query.trim().to_lowercase();

    containers
        .iter()
        .filter(|c| {
            let st = c.state.trim().to_ascii_lowercase();
            let status_ok = match status_filter {
                DockerStatusFilter::All => true,
                DockerStatusFilter::Running => st == "running",
                DockerStatusFilter::Paused => st == "paused",
                DockerStatusFilter::Stopped => {
                    matches!(st.as_str(), "exited" | "created" | "dead" | "stopped")
                }
            };
            if !status_ok {
                return false;
            }
            if q.is_empty() {
                return true;
            }
            c.name.to_lowercase().contains(&q)
                || c.id.to_lowercase().contains(&q)
                || c.image.to_lowercase().contains(&q)
                || c.compose_project.to_lowercase().contains(&q)
                || c.ports.to_lowercase().contains(&q)
                || c.status.to_lowercase().contains(&q)
        })
        .collect()
}

pub fn quick_commands_for_mode(mode: DockerConsoleMode) -> Vec<String> {
    match mode {
        DockerConsoleMode::Logs => vec![
            "ps aux".to_string(),
            "ls -la".to_string(),
            "df -h".to_string(),
            "env".to_string(),
        ],
        DockerConsoleMode::Attach => vec![
            "list".to_string(),
            "save-all".to_string(),
            "help".to_string(),
            "stop".to_string(),
        ],
        DockerConsoleMode::Shell => vec![
            "ls -la".to_string(),
            "pwd".to_string(),
            "ps aux".to_string(),
            "uname -a".to_string(),
        ],
    }
}

fn console_placeholder_for_mode(mode: DockerConsoleMode) -> &'static str {
    match mode {
        DockerConsoleMode::Logs => "Exécuter dans le conteneur (docker exec sh -c ...)...",
        DockerConsoleMode::Attach => {
            "Envoyer sur stdin du conteneur (ex: list, say Bonjour, stop)..."
        }
        DockerConsoleMode::Shell => "Commande shell dans le conteneur...",
    }
}

fn console_mode_tooltip(mode: DockerConsoleMode) -> &'static str {
    match mode {
        DockerConsoleMode::Logs => {
            "Flux docker logs -f + exécution de commandes via docker exec"
        }
        DockerConsoleMode::Attach => {
            "Console interactive stdin/stdout du processus principal (ex: serveur Minecraft Docker)"
        }
        DockerConsoleMode::Shell => "Shell interactif dans le conteneur (docker exec -it)",
    }
}

fn console_mode_short_hint(mode: DockerConsoleMode) -> &'static str {
    match mode {
        DockerConsoleMode::Logs => "docker logs -f  •  docker exec",
        DockerConsoleMode::Attach => "stdin/stdout direct (Minecraft, jeux, bots)",
        DockerConsoleMode::Shell => "PTY interactif (docker exec -it)",
    }
}

fn console_status_label(
    console_state: &ConsoleStreamState,
    mode: DockerConsoleMode,
    is_running: bool,
    matches_container: bool,
) -> &'static str {
    if !matches_container {
        return if is_running {
            "○ Inactif"
        } else {
            "○ Conteneur arrêté"
        };
    }

    match console_state {
        ConsoleStreamState::Streaming => match mode {
            DockerConsoleMode::Logs => "● Logs en direct",
            DockerConsoleMode::Attach => "● Attach (stdin)",
            DockerConsoleMode::Shell => "● Shell actif",
        },
        ConsoleStreamState::Connecting => "● Connexion...",
        ConsoleStreamState::Error(_) => "✖ Erreur de flux",
        ConsoleStreamState::Closed => "○ Flux fermé",
        ConsoleStreamState::Idle => {
            if is_running {
                "○ Inactif"
            } else {
                "○ Conteneur arrêté"
            }
        }
    }
}

fn console_empty_hint(
    console_state: &ConsoleStreamState,
    mode: DockerConsoleMode,
    is_running: bool,
    matches_container: bool,
) -> String {
    if matches_container {
        if let ConsoleStreamState::Error(err) = console_state {
            return format!("Erreur du flux console : {err}");
        }
        if matches!(console_state, ConsoleStreamState::Connecting) {
            return "Connexion au flux console Docker en cours...".to_string();
        }
    }

    if !is_running && mode != DockerConsoleMode::Logs {
        return "Démarrez le conteneur pour ouvrir cette console interactive.".to_string();
    }

    match mode {
        DockerConsoleMode::Logs => {
            "En attente des journaux du conteneur (docker logs -f)...".to_string()
        }
        DockerConsoleMode::Attach => {
            "En attente de la sortie du processus principal (stdin prêt dès connexion)..."
                .to_string()
        }
        DockerConsoleMode::Shell => {
            "En attente du shell interactif dans le conteneur...".to_string()
        }
    }
}

fn display_container_name(c: &DockerContainerItem) -> &str {
    let trimmed = c.name.trim().trim_start_matches('/');
    if trimmed.is_empty() {
        c.id.as_str()
    } else {
        trimmed
    }
}

fn short_id_display(id: &str) -> &str {
    let trimmed = id.trim();
    if trimmed.len() > 12 {
        &trimmed[..12]
    } else {
        trimmed
    }
}

fn aggregate_running_usage(containers: &[DockerContainerItem]) -> (f64, u64) {
    let mut cpu_sum = 0.0_f64;
    let mut mem_sum = 0_u64;
    for c in containers {
        if c.state.trim().eq_ignore_ascii_case("running") {
            if c.cpu_percent.is_finite() && c.cpu_percent > 0.0 {
                cpu_sum += c.cpu_percent;
            }
            mem_sum = mem_sum.saturating_add(c.memory_used_bytes);
        }
    }
    (cpu_sum, mem_sum)
}

/// Render the Docker container supervision and live console panel.
pub fn render(
    ui: &mut Ui,
    state: &mut DockerPanelState,
    device: &Device,
) -> Option<DockerPanelAction> {
    let p = Palette::for_ui(ui);
    let mut action: Option<DockerPanelAction> = None;

    if let Some((_, until)) = &state.copied_id {
        let now = Instant::now();
        if now < *until {
            ui.ctx().request_repaint_after(
                until
                    .saturating_duration_since(now)
                    .min(Duration::from_millis(250)),
            );
        } else {
            state.copied_id = None;
        }
    }

    // 1. Disconnected server state
    if !device.status.is_connected() {
        render_disconnected_card(ui, &p);
        return None;
    }

    let docker = &device.docker_state;

    // 2. Legacy ntx-server upgrade banner
    if docker.unsupported_by_server {
        render_upgrade_banner(ui, &p, docker.include_all, &mut action);
        return action;
    }

    // 3. Docker unavailable state
    if !docker.docker_available {
        render_docker_unavailable_card(
            ui,
            &p,
            docker.docker_error.as_deref(),
            docker.include_all,
            docker.is_loading,
            &mut action,
        );
        return action;
    }

    // 4-6. Responsive layout: side-by-side at >= 760px (with scrollable right pane),
    // or full-panel vertical ScrollArea below 760px / on ultra-short viewports so the
    // live console and bottom command bar remain reachable down to 720x520.
    let avail_w = ui.available_width();
    let avail_h = ui.available_height();
    let use_side_by_side = avail_w >= 760.0 && avail_h >= 340.0;

    if use_side_by_side {
        if let Some((msg, is_error)) = &docker.status_message {
            render_status_banner(ui, &p, msg, *is_error, &mut action);
            ui.add_space(8.0);
        }

        render_kpi_strip(ui, &p, device);
        ui.add_space(10.0);

        let gap = 12.0;
        let left_w = (avail_w * 0.35).clamp(240.0, 360.0);
        let right_w = (avail_w - left_w - gap).max(320.0);
        let split_h = ui.available_height().max(240.0);

        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(left_w, split_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    render_left_pane(ui, &p, state, device, None, &mut action);
                },
            );
            ui.add_space(gap);
            ui.allocate_ui_with_layout(
                egui::vec2(right_w, split_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("docker_right_pane_scroll", &device.id))
                        .auto_shrink([false, false])
                        .max_height(split_h)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            render_right_pane(ui, &p, state, device, &mut action);
                        });
                },
            );
        });
    } else {
        egui::ScrollArea::vertical()
            .id_salt(("docker_panel_stacked_scroll", &device.id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                if let Some((msg, is_error)) = &docker.status_message {
                    render_status_banner(ui, &p, msg, *is_error, &mut action);
                    ui.add_space(8.0);
                }

                render_kpi_strip(ui, &p, device);
                ui.add_space(10.0);

                render_left_pane(ui, &p, state, device, Some(220.0), &mut action);
                ui.add_space(10.0);
                render_right_pane(ui, &p, state, device, &mut action);
                ui.add_space(6.0);
            });
    }

    // 7. Destructive action confirmation modal
    render_confirm_modal(ui, &p, state, &mut action);

    action
}

fn render_disconnected_card(ui: &mut Ui, p: &Palette) {
    ui.add_space(24.0);
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            ui.add_space(12.0);
            ui.label(
                RichText::new("🐳 Serveur actuellement déconnecté")
                    .size(16.0)
                    .strong()
                    .color(p.text_primary),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Connectez-vous pour superviser les conteneurs Docker, suivre leurs journaux et interagir en direct.",
                )
                .color(p.text_secondary),
            );
            ui.add_space(12.0);
        });
    });
}

fn render_upgrade_banner(
    ui: &mut Ui,
    p: &Palette,
    include_all: bool,
    action: &mut Option<DockerPanelAction>,
) {
    ui.add_space(20.0);
    egui::Frame::new()
        .fill(p.warning_bg)
        .stroke(Stroke::new(1.0_f32, p.warning_border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(18, 16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(4.0);
                ui.label(
                    RichText::new("⚠ Mise à jour de ntx-server requise")
                        .size(16.0)
                        .strong()
                        .color(p.warning),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Cet hôte exécute une ancienne version de ntx-server qui ne propose pas encore les RPC natifs de gestion Docker.",
                    )
                    .color(p.text_primary),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Mettez à jour ntx-server sur ce serveur distant pour débloquer la supervision Docker, les logs en direct, Attach (stdin) et Shell.",
                    )
                    .size(12.0)
                    .color(p.text_secondary),
                );
                ui.add_space(12.0);
                if ui
                    .button("🔄 Vérifier à nouveau")
                    .on_hover_text("Interroger à nouveau le serveur distant")
                    .clicked()
                {
                    *action = Some(DockerPanelAction::Refresh { include_all });
                }
                ui.add_space(4.0);
            });
        });
}

fn render_docker_unavailable_card(
    ui: &mut Ui,
    p: &Palette,
    docker_error: Option<&str>,
    include_all: bool,
    is_loading: bool,
    action: &mut Option<DockerPanelAction>,
) {
    ui.add_space(20.0);
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            ui.add_space(10.0);
            ui.label(
                RichText::new("🐳 Docker indisponible sur cet hôte")
                    .size(16.0)
                    .strong()
                    .color(p.text_primary),
            );
            ui.add_space(8.0);
            let err_msg = docker_error
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(
                    "Le démon Docker n'est pas installé, démarré ou accessible depuis ntx-server.",
                );
            egui::Frame::new()
                .fill(p.danger_bg)
                .stroke(Stroke::new(1.0_f32, p.danger_border))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.label(RichText::new(format!("⚠ {err_msg}")).color(p.danger));
                });
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "Vérifiez que Docker est installé, que le service dockerd est actif et que l'utilisateur de ntx-server dispose des droits d'accès au socket Docker.",
                )
                .size(12.0)
                .color(p.text_secondary),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if is_loading {
                    ui.spinner();
                }
                if theme::primary_button(ui, p, "🔄 Réessayer").clicked() {
                    *action = Some(DockerPanelAction::Refresh { include_all });
                }
            });
            ui.add_space(10.0);
        });
    });
}

fn render_status_banner(
    ui: &mut Ui,
    p: &Palette,
    msg: &str,
    is_error: bool,
    action: &mut Option<DockerPanelAction>,
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
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("✖")
                    .on_hover_text("Fermer la notification")
                    .clicked()
                {
                    *action = Some(DockerPanelAction::ClearStatus);
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(RichText::new(icon).color(fg).strong());
                    ui.add(egui::Label::new(RichText::new(msg).color(fg).size(12.5)).wrap());
                });
            });
        });
}

fn render_kpi_strip(ui: &mut Ui, p: &Palette, device: &Device) {
    let docker = &device.docker_state;
    let (total, running, paused, stopped) = docker.container_counts();
    let (total_cpu, total_mem) = aggregate_running_usage(&docker.containers);
    let restarting = docker
        .containers
        .iter()
        .filter(|c| c.state.trim().eq_ignore_ascii_case("restarting"))
        .count();

    let avail_w = ui.available_width();
    if avail_w >= 820.0 {
        ui.columns(4, |cols| {
            render_kpi_total(&mut cols[0], p, total, running, &docker.docker_version, docker.include_all);
            render_kpi_running(&mut cols[1], p, total, running, total_cpu, total_mem);
            render_kpi_paused(&mut cols[2], p, total, paused, restarting);
            render_kpi_stopped(&mut cols[3], p, total, stopped, docker.include_all);
        });
    } else if avail_w >= 460.0 {
        ui.columns(2, |cols| {
            render_kpi_total(&mut cols[0], p, total, running, &docker.docker_version, docker.include_all);
            render_kpi_running(&mut cols[1], p, total, running, total_cpu, total_mem);
        });
        ui.add_space(8.0);
        ui.columns(2, |cols| {
            render_kpi_paused(&mut cols[0], p, total, paused, restarting);
            render_kpi_stopped(&mut cols[1], p, total, stopped, docker.include_all);
        });
    } else {
        render_kpi_total(ui, p, total, running, &docker.docker_version, docker.include_all);
        ui.add_space(8.0);
        render_kpi_running(ui, p, total, running, total_cpu, total_mem);
        ui.add_space(8.0);
        render_kpi_paused(ui, p, total, paused, restarting);
        ui.add_space(8.0);
        render_kpi_stopped(ui, p, total, stopped, docker.include_all);
    }
}

fn render_kpi_total(
    ui: &mut Ui,
    p: &Palette,
    total: usize,
    running: usize,
    docker_version: &str,
    include_all: bool,
) {
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(84.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("CONTENEURS")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let version_trimmed = docker_version.trim();
                if !version_trimmed.is_empty() {
                    let badge = format!("🐳 v{}", version_trimmed.trim_start_matches('v'));
                    theme::pill_badge(ui, &badge, p.accent_bg, p.accent, Some(p.accent_border));
                } else {
                    theme::pill_badge(ui, "🐳 Docker", p.bg_subtle, p.text_secondary, Some(p.border));
                }
            });
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new(total.to_string())
                .size(21.0)
                .strong()
                .color(p.text_primary),
        );

        ui.add_space(4.0);
        let ratio = if total > 0 {
            running as f32 / total as f32
        } else {
            0.0
        };
        theme::mini_progress_bar(ui, p, ratio, p.accent, ui.available_width(), 5.0);
        ui.add_space(4.0);

        let caption = if include_all {
            "Tous les conteneurs affichés"
        } else {
            "Conteneurs actifs uniquement"
        };
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_kpi_running(
    ui: &mut Ui,
    p: &Palette,
    total: usize,
    running: usize,
    total_cpu: f64,
    total_mem: u64,
) {
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(84.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("ACTIFS")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let pct = if total > 0 {
                    (running as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                theme::pill_badge(
                    ui,
                    &format!("{pct:.0}%"),
                    p.success_bg,
                    p.success,
                    Some(p.success_border),
                );
            });
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new(running.to_string())
                .size(21.0)
                .strong()
                .color(if running > 0 {
                    p.success
                } else {
                    p.text_secondary
                }),
        );

        ui.add_space(4.0);
        let ratio = if total > 0 {
            running as f32 / total as f32
        } else {
            0.0
        };
        theme::mini_progress_bar(ui, p, ratio, p.success, ui.available_width(), 5.0);
        ui.add_space(4.0);

        let caption = if running > 0 {
            format!(
                "⚡ {total_cpu:.1}% CPU  •  💾 {}",
                theme::format_storage(total_mem)
            )
        } else {
            "Aucun conteneur actif".to_string()
        };
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_kpi_paused(ui: &mut Ui, p: &Palette, total: usize, paused: usize, restarting: usize) {
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(84.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("EN PAUSE")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            if restarting > 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::pill_badge(
                        ui,
                        &format!("🔄 {restarting}"),
                        p.accent_bg,
                        p.accent,
                        Some(p.accent_border),
                    );
                });
            }
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new(paused.to_string())
                .size(21.0)
                .strong()
                .color(if paused > 0 {
                    p.warning
                } else {
                    p.text_secondary
                }),
        );

        ui.add_space(4.0);
        let ratio = if total > 0 {
            paused as f32 / total as f32
        } else {
            0.0
        };
        theme::mini_progress_bar(ui, p, ratio, p.warning, ui.available_width(), 5.0);
        ui.add_space(4.0);

        let caption = if restarting > 0 {
            format!("{restarting} en cours de redémarrage")
        } else if paused > 0 {
            "Processus gelés (docker pause)".to_string()
        } else {
            "Aucun conteneur en pause".to_string()
        };
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_kpi_stopped(ui: &mut Ui, p: &Palette, total: usize, stopped: usize, include_all: bool) {
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(84.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("ARRÊTÉS")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            if !include_all {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::pill_badge(ui, "Masqués", p.bg_subtle, p.text_muted, Some(p.border));
                });
            }
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new(stopped.to_string())
                .size(21.0)
                .strong()
                .color(p.text_muted),
        );

        ui.add_space(4.0);
        let ratio = if total > 0 {
            stopped as f32 / total as f32
        } else {
            0.0
        };
        theme::mini_progress_bar(ui, p, ratio, p.border_strong, ui.available_width(), 5.0);
        ui.add_space(4.0);

        let caption = if !include_all {
            "Cochez « Tous » pour les afficher"
        } else if stopped > 0 {
            "Conteneurs exited, créés ou morts"
        } else {
            "Aucun conteneur arrêté"
        };
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_left_pane(
    ui: &mut Ui,
    p: &Palette,
    state: &mut DockerPanelState,
    device: &Device,
    stacked_list_max_height: Option<f32>,
    action: &mut Option<DockerPanelAction>,
) {
    let docker = &device.docker_state;
    let (total, running, paused, stopped) = docker.container_counts();

    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());

        // Top controls: Refresh + Include-All checkbox
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("🔄 Actualiser")
                .on_hover_text("Actualiser la liste et les métriques Docker")
                .clicked()
            {
                *action = Some(DockerPanelAction::Refresh {
                    include_all: docker.include_all,
                });
            }
            if docker.is_loading {
                ui.spinner();
            }

            let mut include_all = docker.include_all;
            if ui
                .checkbox(&mut include_all, "Tous (arrêtés inclus)")
                .on_hover_text("Inclure les conteneurs arrêtés (docker ps -a)")
                .changed()
            {
                *action = Some(DockerPanelAction::ToggleIncludeAll(include_all));
            }
        });

        ui.add_space(6.0);

        // Search box
        theme::subtle_bar_frame(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("🔍").size(12.5).color(p.text_muted));
                let clear_reserve = if state.search_query.is_empty() {
                    0.0
                } else {
                    26.0
                };
                let input_w = (ui.available_width() - clear_reserve).max(90.0);
                ui.add(
                    egui::TextEdit::singleline(&mut state.search_query)
                        .id_salt(("docker_search", &device.id))
                        .hint_text("Nom, image, projet Compose, port...")
                        .desired_width(input_w),
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
        });

        ui.add_space(6.0);

        // Filter pills
        ui.horizontal_wrapped(|ui| {
            let filters = [
                (DockerStatusFilter::All, format!("Tous ({total})")),
                (DockerStatusFilter::Running, format!("Actifs ({running})")),
                (DockerStatusFilter::Paused, format!("Pause ({paused})")),
                (DockerStatusFilter::Stopped, format!("Arrêtés ({stopped})")),
            ];
            for (kind, label) in filters {
                if theme::tab_button(ui, p, state.status_filter == kind, &label).clicked() {
                    state.status_filter = kind;
                }
            }
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);

        let filtered = filter_containers(&docker.containers, state.status_filter, &state.search_query);
        let list_scroll_h = stacked_list_max_height
            .unwrap_or_else(|| (ui.available_height() - 6.0).max(110.0));

        if docker.containers.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                if docker.is_loading {
                    ui.spinner();
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Chargement des conteneurs Docker...")
                            .color(p.text_secondary),
                    );
                } else {
                    ui.label(
                        RichText::new("🐳 Aucun conteneur Docker")
                            .size(14.0)
                            .strong()
                            .color(p.text_primary),
                    );
                    ui.add_space(4.0);
                    let hint = if docker.include_all {
                        "Aucun conteneur n'est présent sur ce serveur."
                    } else {
                        "Aucun conteneur actif. Cochez « Tous (arrêtés inclus) » pour voir les conteneurs arrêtés."
                    };
                    ui.label(RichText::new(hint).size(12.0).color(p.text_secondary));
                    ui.add_space(10.0);
                    if ui.button("🔄 Actualiser").clicked() {
                        *action = Some(DockerPanelAction::Refresh {
                            include_all: docker.include_all,
                        });
                    }
                }
                ui.add_space(20.0);
            });
        } else if filtered.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                ui.label(
                    RichText::new("Aucun conteneur ne correspond aux filtres")
                        .strong()
                        .color(p.text_primary),
                );
                ui.add_space(6.0);
                if ui.button("♻ Réinitialiser les filtres").clicked() {
                    state.search_query.clear();
                    state.status_filter = DockerStatusFilter::All;
                }
                ui.add_space(20.0);
            });
        } else {
            egui::ScrollArea::vertical()
                .id_salt(("docker_container_list_scroll", &device.id))
                .auto_shrink([false, false])
                .max_height(list_scroll_h)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for container in filtered {
                        let is_selected =
                            docker.selected_container_id.as_deref() == Some(container.id.as_str());
                        if let Some(card_act) =
                            render_container_list_card(ui, p, container, is_selected)
                        {
                            *action = Some(card_act);
                        }
                        ui.add_space(6.0);
                    }
                });
        }
    });
}

fn render_container_list_card(
    ui: &mut Ui,
    p: &Palette,
    container: &DockerContainerItem,
    is_selected: bool,
) -> Option<DockerPanelAction> {
    let mut card_action: Option<DockerPanelAction> = None;
    let pres = container_state_presentation(p, &container.state);
    let st_lower = container.state.trim().to_ascii_lowercase();
    let is_running = st_lower == "running";
    let is_paused = st_lower == "paused";
    let name = display_container_name(container);

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

    let resp = ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt(("docker_card", &container.id))
            .sense(egui::Sense::click()),
        |ui| {
            ui.style_mut().interaction.selectable_labels = false;
            let mut prepared = egui::Frame::new()
                .fill(base_fill)
                .stroke(Stroke::new(
                    if is_selected { 1.3_f32 } else { 1.0_f32 },
                    base_border,
                ))
                .corner_radius(CornerRadius::same(7))
                .inner_margin(Margin {
                    left: 12,
                    right: 10,
                    top: 8,
                    bottom: 8,
                })
                .begin(ui);

            {
                let cui = &mut prepared.content_ui;
                cui.set_width(cui.available_width());

                // Row 1: Quick controls anchored right, status dot + container name on left
                cui.allocate_ui_with_layout(
                    egui::vec2(cui.available_width(), 22.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if is_running {
                            if ui
                                .small_button("🔄")
                                .on_hover_text("Redémarrer le conteneur")
                                .clicked()
                            {
                                card_action = Some(DockerPanelAction::ControlContainer {
                                    container_id: container.id.clone(),
                                    action: "restart".to_string(),
                                    force: false,
                                });
                            }
                            if ui
                                .small_button("⏹")
                                .on_hover_text("Arrêter le conteneur")
                                .clicked()
                            {
                                card_action = Some(DockerPanelAction::ControlContainer {
                                    container_id: container.id.clone(),
                                    action: "stop".to_string(),
                                    force: false,
                                });
                            }
                        } else if is_paused {
                            if ui
                                .small_button("▶")
                                .on_hover_text("Reprendre le conteneur")
                                .clicked()
                            {
                                card_action = Some(DockerPanelAction::ControlContainer {
                                    container_id: container.id.clone(),
                                    action: "unpause".to_string(),
                                    force: false,
                                });
                            }
                        } else if ui
                            .small_button("▶")
                            .on_hover_text("Démarrer le conteneur")
                            .clicked()
                        {
                            card_action = Some(DockerPanelAction::ControlContainer {
                                container_id: container.id.clone(),
                                action: "start".to_string(),
                                force: false,
                            });
                        }

                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.label(RichText::new(pres.dot).size(11.0).color(pres.fg));
                            ui.add(
                                egui::Label::new(
                                    RichText::new(name)
                                        .size(13.0)
                                        .strong()
                                        .color(p.text_primary),
                                )
                                .truncate(),
                            );
                        });
                    },
                );

                // Row 2: Image + Compose badge
                cui.add_space(3.0);
                cui.horizontal_wrapped(|ui| {
                    if !container.compose_project.trim().is_empty() {
                        theme::pill_badge(
                            ui,
                            &format!("⊞ {}", container.compose_project.trim()),
                            p.accent_bg,
                            p.accent,
                            Some(p.accent_border),
                        );
                    }
                    if !container.image.trim().is_empty() {
                        ui.add(
                            egui::Label::new(
                                RichText::new(container.image.trim())
                                    .monospace()
                                    .size(11.0)
                                    .color(p.text_secondary),
                            )
                            .truncate(),
                        );
                    }
                });

                // Row 3: Status + Ports summary
                let status_txt = container.status.trim();
                let ports_txt = container.ports.trim();
                if !status_txt.is_empty() || !ports_txt.is_empty() {
                    cui.add_space(3.0);
                    let summary = match (status_txt.is_empty(), ports_txt.is_empty()) {
                        (false, false) => format!("⏱ {status_txt}  •  🌐 {ports_txt}"),
                        (false, true) => format!("⏱ {status_txt}"),
                        (true, false) => format!("🌐 {ports_txt}"),
                        (true, true) => String::new(),
                    };
                    cui.add(
                        egui::Label::new(
                            RichText::new(summary).size(10.5).color(p.text_muted),
                        )
                        .truncate(),
                    );
                }

                // Row 4: Live CPU% and RAM mini-bars when running
                if is_running {
                    cui.add_space(5.0);
                    cui.columns(2, |cols| {
                        let cpu = if container.cpu_percent.is_finite() {
                            container.cpu_percent.max(0.0)
                        } else {
                            0.0
                        };
                        let cpu_col = theme::usage_color(p, cpu);
                        cols[0].horizontal(|ui| {
                            ui.label(RichText::new("CPU").size(10.0).color(p.text_muted));
                            ui.label(
                                RichText::new(format!("{cpu:.1}%"))
                                    .monospace()
                                    .size(10.5)
                                    .strong()
                                    .color(cpu_col),
                            );
                        });
                        let col0_w = cols[0].available_width();
                        theme::mini_progress_bar(
                            &mut cols[0],
                            p,
                            (cpu / 100.0) as f32,
                            cpu_col,
                            col0_w,
                            4.0,
                        );

                        let mem_frac = if container.memory_limit_bytes > 0 {
                            (container.memory_used_bytes as f64
                                / container.memory_limit_bytes as f64)
                                .clamp(0.0, 1.0) as f32
                        } else {
                            0.0
                        };
                        let mem_pct = (mem_frac * 100.0) as f64;
                        let mem_col = if container.memory_limit_bytes > 0 {
                            theme::usage_color(p, mem_pct)
                        } else {
                            p.purple
                        };
                        cols[1].horizontal(|ui| {
                            ui.label(RichText::new("RAM").size(10.0).color(p.text_muted));
                            ui.add(
                                egui::Label::new(
                                    RichText::new(theme::format_storage(
                                        container.memory_used_bytes,
                                    ))
                                    .monospace()
                                    .size(10.5)
                                    .strong()
                                    .color(p.text_secondary),
                                )
                                .truncate(),
                            );
                        });
                        let col1_w = cols[1].available_width();
                        theme::mini_progress_bar(
                            &mut cols[1],
                            p,
                            mem_frac,
                            mem_col,
                            col1_w,
                            4.0,
                        );
                    });
                }
            }

            let outer_resp = prepared.allocate_space(ui);
            let is_hovered = ui.response().hovered() || outer_resp.hovered();
            if is_hovered && !is_selected {
                prepared.frame.fill = p.bg_card_hover;
                prepared.frame.stroke = Stroke::new(1.1_f32, p.border_strong);
            }
            prepared.paint(ui);

            // 3px left status accent strip
            let rect = outer_resp.rect;
            if ui.is_rect_visible(rect) {
                let strip = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 1.0, rect.top() + 6.0),
                    egui::pos2(rect.left() + 4.0, rect.bottom() - 6.0),
                );
                ui.painter().rect_filled(strip, 1.5, pres.accent_strip);
            }
        },
    );

    if card_action.is_none() && resp.response.clicked() {
        card_action = Some(DockerPanelAction::SelectContainer(container.id.clone()));
    }

    card_action
}

fn render_right_pane(
    ui: &mut Ui,
    p: &Palette,
    state: &mut DockerPanelState,
    device: &Device,
    action: &mut Option<DockerPanelAction>,
) {
    let docker = &device.docker_state;
    let Some(container) = docker.selected_container() else {
        theme::card_frame(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(
                    RichText::new("🐳 Sélectionnez un conteneur")
                        .size(16.0)
                        .strong()
                        .color(p.text_primary),
                );
                ui.add_space(6.0);
                let hint = if docker.containers.is_empty() {
                    "Aucun conteneur Docker n'est encore chargé sur ce serveur."
                } else {
                    "Choisissez un conteneur dans la liste pour afficher ses métriques, le contrôler ou ouvrir sa console en direct (Logs, Attach stdin ou Shell)."
                };
                ui.label(RichText::new(hint).color(p.text_secondary));
                ui.add_space(40.0);
            });
        });
        return;
    };

    // 1. Selected container header & lifecycle actions card
    render_selected_container_header(ui, p, state, container, action);
    ui.add_space(8.0);

    // 2. Console mode switcher bar
    let active_mode = docker.console_mode;
    render_console_mode_bar(ui, p, container, active_mode, action);
    ui.add_space(8.0);

    // 3. Console stream error notice (if any)
    let matches_container =
        docker.console_container_id.as_deref() == Some(container.id.as_str());
    if matches_container {
        if let ConsoleStreamState::Error(err) = &docker.console_state {
            egui::Frame::new()
                .fill(p.danger_bg)
                .stroke(Stroke::new(1.0_f32, p.danger_border))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("⚠").color(p.danger).strong());
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("Flux console interrompu : {err}"))
                                    .size(12.0)
                                    .color(p.danger),
                            )
                            .wrap(),
                        );
                    });
                });
            ui.add_space(6.0);
        }
    }

    // 4. Embedded reusable live console widget
    let is_running = container.state.trim().eq_ignore_ascii_case("running");
    let is_streaming =
        matches_container && docker.console_state == ConsoleStreamState::Streaming;
    let can_send_input = match active_mode {
        DockerConsoleMode::Logs => is_running,
        DockerConsoleMode::Attach | DockerConsoleMode::Shell => is_running && is_streaming,
    };

    let connected_label =
        console_status_label(&docker.console_state, active_mode, is_running, matches_container);
    let empty_hint =
        console_empty_hint(&docker.console_state, active_mode, is_running, matches_container);
    let quick_commands = quick_commands_for_mode(active_mode);
    let id_salt = format!("docker_console_{}_{}", device.id, container.id);

    let cfg = ConsoleWidgetConfig {
        id_salt: &id_salt,
        connected_label,
        is_streaming,
        can_send_input,
        input_placeholder: console_placeholder_for_mode(active_mode),
        quick_commands: &quick_commands,
        empty_hint: &empty_hint,
    };

    if let Some(widget_act) = console_widget::render(
        ui,
        p,
        &docker.console_buffer,
        &mut state.console_state,
        &cfg,
    ) {
        match widget_act {
            ConsoleWidgetAction::SendCommand(cmd) => {
                if active_mode == DockerConsoleMode::Logs {
                    *action = Some(DockerPanelAction::ExecCommand {
                        container_id: container.id.clone(),
                        command: cmd,
                    });
                } else {
                    *action = Some(DockerPanelAction::SendConsoleInput {
                        container_id: container.id.clone(),
                        input: cmd,
                    });
                }
            }
            ConsoleWidgetAction::Reconnect => {
                *action = Some(DockerPanelAction::ReconnectConsole {
                    container_id: container.id.clone(),
                    mode: active_mode,
                });
            }
            ConsoleWidgetAction::ClearBuffer => {
                *action = Some(DockerPanelAction::ClearConsole);
            }
        }
    }
}

fn render_selected_container_header(
    ui: &mut Ui,
    p: &Palette,
    state: &mut DockerPanelState,
    container: &DockerContainerItem,
    action: &mut Option<DockerPanelAction>,
) {
    let pres = container_state_presentation(p, &container.state);
    let st_lower = container.state.trim().to_ascii_lowercase();
    let is_running = st_lower == "running";
    let is_paused = st_lower == "paused";
    let is_restarting = st_lower == "restarting";
    let name = display_container_name(container);
    let short_id = short_id_display(&container.id);

    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());

        // Header line: icon + name + copyable short ID + status pill
        let avail_w = ui.available_width();
        let draw_copy_id = |ui: &mut Ui, state: &mut DockerPanelState| {
            let is_copied = state
                .copied_id
                .as_ref()
                .is_some_and(|(cid, until)| cid == &container.id && Instant::now() < *until);
            let (id_label, id_color) = if is_copied {
                (format!("✔ {short_id}"), p.success)
            } else {
                (format!("📋 {short_id}"), p.text_secondary)
            };
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(id_label)
                            .monospace()
                            .size(11.5)
                            .color(id_color),
                    )
                    .fill(p.bg_subtle)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .corner_radius(CornerRadius::same(5)),
                )
                .on_hover_text("Cliquer pour copier l'identifiant du conteneur")
                .clicked()
            {
                ui.ctx().copy_text(container.id.clone());
                state.copied_id = Some((
                    container.id.clone(),
                    Instant::now() + Duration::from_secs(2),
                ));
            }
        };

        if avail_w >= 480.0 {
            ui.allocate_ui_with_layout(
                egui::vec2(avail_w, 26.0),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    theme::pill_badge(ui, pres.label, pres.bg, pres.fg, Some(pres.border));
                    draw_copy_id(ui, state);
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.label(RichText::new("🐳").size(17.0));
                        ui.add(
                            egui::Label::new(
                                RichText::new(name)
                                    .size(16.5)
                                    .strong()
                                    .color(p.text_primary),
                            )
                            .truncate(),
                        );
                    });
                },
            );
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("🐳").size(17.0));
                ui.label(
                    RichText::new(name)
                        .size(16.5)
                        .strong()
                        .color(p.text_primary),
                );
                draw_copy_id(ui, state);
                theme::pill_badge(ui, pres.label, pres.bg, pres.fg, Some(pres.border));
            });
        }

        ui.add_space(6.0);

        // Metadata pills: image, compose project, ports, status
        ui.horizontal_wrapped(|ui| {
            if !container.image.trim().is_empty() {
                theme::pill_badge(
                    ui,
                    &format!("🗄 {}", container.image.trim()),
                    p.bg_subtle,
                    p.text_primary,
                    Some(p.border),
                );
            }
            if !container.compose_project.trim().is_empty() {
                theme::pill_badge(
                    ui,
                    &format!("⊞ {}", container.compose_project.trim()),
                    p.accent_bg,
                    p.accent,
                    Some(p.accent_border),
                );
            }
            if !container.ports.trim().is_empty() {
                theme::pill_badge(
                    ui,
                    &format!("🌐 {}", container.ports.trim()),
                    p.bg_subtle,
                    p.text_secondary,
                    Some(p.border),
                );
            }
            if !container.status.trim().is_empty() {
                ui.label(
                    RichText::new(format!("⏱ {}", container.status.trim()))
                        .size(11.5)
                        .color(p.text_muted),
                );
            }
        });

        // Live CPU / RAM bar when running or metrics populated
        if is_running || container.memory_used_bytes > 0 {
            ui.add_space(8.0);
            theme::subtle_bar_frame(p).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let cpu = if container.cpu_percent.is_finite() {
                    container.cpu_percent.max(0.0)
                } else {
                    0.0
                };
                let cpu_col = theme::usage_color(p, cpu);
                let mem_frac = if container.memory_limit_bytes > 0 {
                    (container.memory_used_bytes as f64 / container.memory_limit_bytes as f64)
                        .clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let mem_col = if container.memory_limit_bytes > 0 {
                    theme::usage_color(p, mem_frac * 100.0)
                } else {
                    p.purple
                };
                let mem_text = if container.memory_limit_bytes > 0 {
                    format!(
                        "{} / {} ({:.0}%)",
                        theme::format_storage(container.memory_used_bytes),
                        theme::format_storage(container.memory_limit_bytes),
                        mem_frac * 100.0
                    )
                } else {
                    theme::format_storage(container.memory_used_bytes)
                };

                if ui.available_width() >= 440.0 {
                    ui.columns(2, |cols| {
                        cols[0].horizontal(|ui| {
                            ui.label(RichText::new("⚡ CPU").size(11.5).color(p.text_muted));
                            ui.label(
                                RichText::new(format!("{cpu:.1}%"))
                                    .monospace()
                                    .size(12.0)
                                    .strong()
                                    .color(cpu_col),
                            );
                        });
                        let col0_w = cols[0].available_width();
                        theme::mini_progress_bar(
                            &mut cols[0],
                            p,
                            (cpu / 100.0) as f32,
                            cpu_col,
                            col0_w,
                            5.0,
                        );

                        cols[1].horizontal(|ui| {
                            ui.label(RichText::new("💾 RAM").size(11.5).color(p.text_muted));
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&mem_text)
                                        .monospace()
                                        .size(12.0)
                                        .strong()
                                        .color(p.text_primary),
                                )
                                .truncate(),
                            );
                        });
                        let col1_w = cols[1].available_width();
                        theme::mini_progress_bar(
                            &mut cols[1],
                            p,
                            mem_frac as f32,
                            mem_col,
                            col1_w,
                            5.0,
                        );
                    });
                } else {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!("⚡ CPU : {cpu:.1}%"))
                                .monospace()
                                .size(12.0)
                                .strong()
                                .color(cpu_col),
                        );
                        ui.label(RichText::new("•").color(p.text_muted));
                        ui.label(
                            RichText::new(format!("💾 RAM : {mem_text}"))
                                .monospace()
                                .size(12.0)
                                .strong()
                                .color(p.text_primary),
                        );
                    });
                }
            });
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);

        // Lifecycle action buttons
        ui.horizontal_wrapped(|ui| {
            if !is_running && !is_paused
                && theme::primary_button(ui, p, "▶ Démarrer")
                    .on_hover_text("Démarrer ce conteneur (docker start)")
                    .clicked()
            {
                *action = Some(DockerPanelAction::ControlContainer {
                    container_id: container.id.clone(),
                    action: "start".to_string(),
                    force: false,
                });
            }

            if is_running
                && ui
                    .button("⏹ Arrêter")
                    .on_hover_text("Arrêter proprement ce conteneur (docker stop)")
                    .clicked()
            {
                *action = Some(DockerPanelAction::ControlContainer {
                    container_id: container.id.clone(),
                    action: "stop".to_string(),
                    force: false,
                });
            }

            if (is_running || is_paused || is_restarting)
                && ui
                    .button("🔄 Redémarrer")
                    .on_hover_text("Redémarrer ce conteneur (docker restart)")
                    .clicked()
            {
                *action = Some(DockerPanelAction::ControlContainer {
                    container_id: container.id.clone(),
                    action: "restart".to_string(),
                    force: false,
                });
            }

            if is_running
                && ui
                    .button("⏸ Pause")
                    .on_hover_text("Mettre en pause les processus du conteneur (docker pause)")
                    .clicked()
            {
                *action = Some(DockerPanelAction::ControlContainer {
                    container_id: container.id.clone(),
                    action: "pause".to_string(),
                    force: false,
                });
            }

            if is_paused
                && theme::primary_button(ui, p, "▶ Reprendre")
                    .on_hover_text("Reprendre l'exécution du conteneur (docker unpause)")
                    .clicked()
            {
                *action = Some(DockerPanelAction::ControlContainer {
                    container_id: container.id.clone(),
                    action: "unpause".to_string(),
                    force: false,
                });
            }

            if (is_running || is_paused || is_restarting)
                && theme::danger_button(ui, p, "⛔ Kill")
                    .on_hover_text("Arrêt immédiat par SIGKILL (confirmation requise)")
                    .clicked()
            {
                state.pending_confirm = Some(PendingDockerConfirm::Kill {
                    id: container.id.clone(),
                    name: name.to_string(),
                });
            }

            if theme::danger_button(ui, p, "🗑 Supprimer")
                .on_hover_text("Supprimer ce conteneur (confirmation requise)")
                .clicked()
            {
                state.pending_confirm = Some(PendingDockerConfirm::Remove {
                    id: container.id.clone(),
                    name: name.to_string(),
                    force: is_running || is_paused || is_restarting,
                });
            }
        });
    });
}

fn render_console_mode_bar(
    ui: &mut Ui,
    p: &Palette,
    container: &DockerContainerItem,
    active_mode: DockerConsoleMode,
    action: &mut Option<DockerPanelAction>,
) {
    let modes = [
        DockerConsoleMode::Logs,
        DockerConsoleMode::Attach,
        DockerConsoleMode::Shell,
    ];

    theme::subtle_bar_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let avail_w = ui.available_width();

        let draw_tabs = |ui: &mut Ui, action: &mut Option<DockerPanelAction>| {
            for mode in modes {
                let selected = active_mode == mode;
                if theme::tab_button(ui, p, selected, mode.display_label())
                    .on_hover_text(console_mode_tooltip(mode))
                    .clicked()
                    && !selected
                {
                    *action = Some(DockerPanelAction::SetConsoleMode {
                        container_id: container.id.clone(),
                        mode,
                    });
                }
            }
        };

        if avail_w >= 560.0 {
            ui.horizontal(|ui| {
                draw_tabs(ui, action);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(console_mode_short_hint(active_mode))
                            .size(11.0)
                            .color(p.text_muted),
                    );
                });
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                draw_tabs(ui, action);
            });
        }
    });
}

fn render_confirm_modal(
    ui: &mut Ui,
    p: &Palette,
    state: &mut DockerPanelState,
    action: &mut Option<DockerPanelAction>,
) {
    let Some(confirm) = state.pending_confirm.as_mut() else {
        return;
    };

    let mut close_modal = false;

    match confirm {
        PendingDockerConfirm::Kill { id, name } => {
            let cid = id.clone();
            let cname = name.clone();
            let short = short_id_display(&cid).to_string();

            egui::Window::new("⚠ Confirmer l'arrêt forcé (Kill)")
                .id(egui::Id::new("docker_confirm_kill_modal"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label(format!(
                        "Voulez-vous envoyer SIGKILL au conteneur « {cname} » ({short}) ?"
                    ));
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Le processus principal sera interrompu immédiatement sans sauvegarde ni arrêt gracieux.",
                        )
                        .size(12.0)
                        .color(p.text_secondary),
                    );
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if theme::danger_button(ui, p, "⛔ Confirmer Kill").clicked() {
                            *action = Some(DockerPanelAction::ControlContainer {
                                container_id: cid,
                                action: "kill".to_string(),
                                force: false,
                            });
                            close_modal = true;
                        }
                        if ui.button("Annuler").clicked() {
                            close_modal = true;
                        }
                    });
                });
        }
        PendingDockerConfirm::Remove { id, name, force } => {
            let cid = id.clone();
            let cname = name.clone();
            let short = short_id_display(&cid).to_string();

            egui::Window::new("⚠ Confirmer la suppression du conteneur")
                .id(egui::Id::new("docker_confirm_remove_modal"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label(format!(
                        "Voulez-vous vraiment supprimer le conteneur « {cname} » ({short}) ?"
                    ));
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Cette action est irréversible (les volumes nommés externes sont conservés).",
                        )
                        .size(12.0)
                        .color(p.text_secondary),
                    );
                    ui.add_space(8.0);
                    ui.checkbox(force, "Forcer la suppression (-f)")
                        .on_hover_text(
                            "Forcer l'arrêt et la suppression même si le conteneur est en cours d'exécution (docker rm -f).",
                        );
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        let force_val = *force;
                        if theme::danger_button(ui, p, "🗑 Supprimer le conteneur").clicked() {
                            *action = Some(DockerPanelAction::ControlContainer {
                                container_id: cid,
                                action: "remove".to_string(),
                                force: force_val,
                            });
                            close_modal = true;
                        }
                        if ui.button("Annuler").clicked() {
                            close_modal = true;
                        }
                    });
                });
        }
    }

    if close_modal {
        state.pending_confirm = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ConnectionStatus;

    fn sample_containers() -> Vec<DockerContainerItem> {
        vec![
            DockerContainerItem {
                id: "0123456789ab".to_string(),
                name: "mc-survival".to_string(),
                image: "itzg/minecraft-server:latest".to_string(),
                state: "running".to_string(),
                status: "Up 3 hours (healthy)".to_string(),
                ports: "0.0.0.0:25565->25565/tcp".to_string(),
                created: "2026-01-10 12:00:00".to_string(),
                cpu_percent: 42.5,
                memory_used_bytes: 2 * 1024 * 1024 * 1024,
                memory_limit_bytes: 4 * 1024 * 1024 * 1024,
                compose_project: "gaming-stack".to_string(),
            },
            DockerContainerItem {
                id: "fedcba987654".to_string(),
                name: "postgres-db".to_string(),
                image: "postgres:16-alpine".to_string(),
                state: "paused".to_string(),
                status: "Up 2 days (Paused)".to_string(),
                ports: "5432/tcp".to_string(),
                created: "2026-01-08 09:00:00".to_string(),
                cpu_percent: 0.0,
                memory_used_bytes: 128 * 1024 * 1024,
                memory_limit_bytes: 1024 * 1024 * 1024,
                compose_project: "backend".to_string(),
            },
            DockerContainerItem {
                id: "aabbccddeeff".to_string(),
                name: "nginx-old".to_string(),
                image: "nginx:stable".to_string(),
                state: "exited".to_string(),
                status: "Exited (0) 4 days ago".to_string(),
                ports: String::new(),
                created: "2026-01-05 08:00:00".to_string(),
                cpu_percent: 0.0,
                memory_used_bytes: 0,
                memory_limit_bytes: 0,
                compose_project: String::new(),
            },
        ]
    }

    #[test]
    fn test_filter_containers_by_status_and_search() {
        let items = sample_containers();

        // Status filters
        assert_eq!(filter_containers(&items, DockerStatusFilter::All, "").len(), 3);
        let running = filter_containers(&items, DockerStatusFilter::Running, "");
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].name, "mc-survival");

        let paused = filter_containers(&items, DockerStatusFilter::Paused, "");
        assert_eq!(paused.len(), 1);
        assert_eq!(paused[0].name, "postgres-db");

        let stopped = filter_containers(&items, DockerStatusFilter::Stopped, "");
        assert_eq!(stopped.len(), 1);
        assert_eq!(stopped[0].name, "nginx-old");

        // Search by Compose project (case-insensitive)
        let by_compose = filter_containers(&items, DockerStatusFilter::All, "GAMING");
        assert_eq!(by_compose.len(), 1);
        assert_eq!(by_compose[0].id, "0123456789ab");

        // Search by image
        let by_image = filter_containers(&items, DockerStatusFilter::All, "alpine");
        assert_eq!(by_image.len(), 1);
        assert_eq!(by_image[0].name, "postgres-db");

        // Search by port
        let by_port = filter_containers(&items, DockerStatusFilter::All, "25565");
        assert_eq!(by_port.len(), 1);
        assert_eq!(by_port[0].name, "mc-survival");

        // Search by container ID
        let by_id = filter_containers(&items, DockerStatusFilter::All, "aabbcc");
        assert_eq!(by_id.len(), 1);
        assert_eq!(by_id[0].name, "nginx-old");

        // Combined status + search mismatch
        assert!(filter_containers(&items, DockerStatusFilter::Running, "postgres").is_empty());
    }

    #[test]
    fn test_container_state_presentation() {
        let dark = Palette::dark();
        let light = Palette::light();

        for p in [&dark, &light] {
            let run = container_state_presentation(p, "running");
            assert_eq!(run.fg, p.success);
            assert_eq!(run.accent_strip, p.success);
            assert!(run.label.contains("Actif"));

            let paused = container_state_presentation(p, "Paused");
            assert_eq!(paused.fg, p.warning);
            assert!(paused.label.contains("pause"));

            let restarting = container_state_presentation(p, "restarting");
            assert_eq!(restarting.fg, p.accent);

            let dead = container_state_presentation(p, "dead");
            assert_eq!(dead.fg, p.danger);

            let exited = container_state_presentation(p, "exited");
            assert_eq!(exited.fg, p.text_muted);
            assert!(exited.label.contains("Arrêté"));

            let created = container_state_presentation(p, "created");
            assert!(created.label.contains("Créé"));

            let unknown = container_state_presentation(p, "weird-state");
            assert!(unknown.label.contains("Inconnu"));
        }
    }

    #[test]
    fn test_quick_commands_and_placeholders_per_mode() {
        assert_eq!(
            quick_commands_for_mode(DockerConsoleMode::Logs),
            vec!["ps aux", "ls -la", "df -h", "env"]
        );
        assert_eq!(
            quick_commands_for_mode(DockerConsoleMode::Attach),
            vec!["list", "save-all", "help", "stop"]
        );
        assert_eq!(
            quick_commands_for_mode(DockerConsoleMode::Shell),
            vec!["ls -la", "pwd", "ps aux", "uname -a"]
        );

        assert!(console_placeholder_for_mode(DockerConsoleMode::Logs).contains("docker exec"));
        assert!(console_placeholder_for_mode(DockerConsoleMode::Attach).contains("stdin"));
        assert!(console_placeholder_for_mode(DockerConsoleMode::Shell).contains("shell"));
    }

    #[test]
    fn test_docker_panel_state_reset_for_device() {
        let mut state = DockerPanelState {
            search_query: "minecraft".to_string(),
            status_filter: DockerStatusFilter::Running,
            console_state: ConsoleWidgetState {
                input: "list".to_string(),
                search_query: "error".to_string(),
                ..Default::default()
            },
            pending_confirm: Some(PendingDockerConfirm::Kill {
                id: "abc".to_string(),
                name: "mc".to_string(),
            }),
            copied_id: Some(("abc".to_string(), Instant::now() + Duration::from_secs(2))),
        };

        state.reset_for_device();
        assert!(state.search_query.is_empty());
        assert_eq!(state.status_filter, DockerStatusFilter::All);
        assert!(state.console_state.input.is_empty());
        assert!(state.console_state.search_query.is_empty());
        assert!(state.pending_confirm.is_none());
        assert!(state.copied_id.is_none());
    }

    #[test]
    fn test_render_smoke_wide_narrow_and_guard_states() {
        let ctx = egui::Context::default();
        let mut device = Device::new(
            "srv-1".to_string(),
            "rpi-home".to_string(),
            vec!["192.168.1.50".to_string()],
            9850,
            "0.3.0".to_string(),
        );
        let mut panel_state = DockerPanelState::default();

        // 1. Disconnected
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(render(ui, &mut panel_state, &device).is_none());
            });
        });

        // 2. Connected + unsupported server
        device.status = ConnectionStatus::Connected;
        device.docker_state.unsupported_by_server = true;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = render(ui, &mut panel_state, &device);
            });
        });

        // 3. Connected + Docker unavailable
        device.docker_state.unsupported_by_server = false;
        device.docker_state.docker_available = false;
        device.docker_state.docker_error = Some("Démon Docker arrêté".to_string());
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = render(ui, &mut panel_state, &device);
            });
        });

        // 4. Connected + populated containers (wide 1280px & narrow 600px + modal)
        device.docker_state.docker_available = true;
        device.docker_state.docker_version = "27.3.1".to_string();
        device.docker_state.containers = sample_containers();
        device.docker_state.selected_container_id = Some("0123456789ab".to_string());
        device.docker_state.console_container_id = Some("0123456789ab".to_string());
        device.docker_state.console_state = ConsoleStreamState::Streaming;
        device.docker_state.console_mode = DockerConsoleMode::Attach;
        device
            .docker_state
            .console_buffer
            .push_bytes(b"[Server thread/INFO]: Done!\r\n");
        device
            .docker_state
            .set_status("Conteneur démarré avec succès", false);
        panel_state.pending_confirm = Some(PendingDockerConfirm::Remove {
            id: "0123456789ab".to_string(),
            name: "mc-survival".to_string(),
            force: true,
        });

        for (width, height) in [
            (1280.0_f32, 820.0_f32),
            (853.0_f32, 580.0_f32),
            (760.0_f32, 520.0_f32),
            (600.0_f32, 520.0_f32),
            (440.0_f32, 380.0_f32),
        ] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, height),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = render(ui, &mut panel_state, &device);
                });
            });
        }

        // 5. Unbound console_container_id must not attribute streaming/error to selected container
        device.docker_state.console_container_id = None;
        device.docker_state.console_state = ConsoleStreamState::Error("stale error".to_string());
        panel_state.pending_confirm = None;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(853.0, 580.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = render(ui, &mut panel_state, &device);
                });
            },
        );
    }

    #[test]
    fn test_console_status_requires_bound_container() {
        let streaming = ConsoleStreamState::Streaming;
        let err = ConsoleStreamState::Error("boom".to_string());

        assert_eq!(
            console_status_label(&streaming, DockerConsoleMode::Attach, true, false),
            "○ Inactif"
        );
        assert_eq!(
            console_status_label(&err, DockerConsoleMode::Attach, false, false),
            "○ Conteneur arrêté"
        );
        assert_eq!(
            console_status_label(&streaming, DockerConsoleMode::Attach, true, true),
            "● Attach (stdin)"
        );
        assert!(
            !console_empty_hint(&err, DockerConsoleMode::Attach, true, false).contains("boom")
        );
        assert!(console_empty_hint(&err, DockerConsoleMode::Attach, true, true).contains("boom"));
    }
}
