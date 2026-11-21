use crate::state::{
    PendingProcessConfirm, ProcessFilterCategory, ProcessSortColumn, ProcessState, ProcessSubTab,
    ProcessViewMode, ServiceFilterCategory, ServiceSortColumn, SortDirection,
};
use crate::ui::theme::{self, Palette};
use egui::{RichText, Stroke, Ui};
use egui_extras::{Column, TableBuilder};

/// Actions emitted by the Process & Services management view.
#[derive(Debug, Clone)]
pub enum ProcessPanelAction {
    RefreshProcesses,
    RefreshServices,
    KillProcess { pid: u32, signal: i32 },
    ControlService { service_name: String, action: String },
    AddFavoriteFromProcess(ntx_proto::ProcessItem),
    ViewServiceLogs(String),
    AddFavoriteFromService(ntx_proto::ServiceItem),
}

#[derive(Debug, Clone, PartialEq)]
struct ProcessKpiSummary {
    total: usize,
    running: usize,
    sleeping: usize,
    zombies: usize,
    active_cpu_count: usize,
    total_cpu_pct: f64,
    total_rss_bytes: u64,
    top_cpu: Option<(String, f32)>,
    top_mem: Option<(String, u64)>,
}

fn kpi_column_count(avail_w: f32) -> usize {
    if avail_w >= 820.0 {
        4
    } else if avail_w >= 460.0 {
        2
    } else {
        1
    }
}

fn compute_process_kpi_summary(state: &ProcessState) -> ProcessKpiSummary {
    let (total, running, sleeping, zombies) = state.process_counts();
    let mut active_cpu_count = 0usize;
    let mut total_cpu_pct = 0.0_f64;
    let mut total_rss_bytes = 0_u64;

    for proc in &state.processes {
        if proc.cpu_usage > 0.05 {
            active_cpu_count += 1;
        }
        if proc.cpu_usage.is_finite() && proc.cpu_usage > 0.0 {
            total_cpu_pct += proc.cpu_usage as f64;
        }
        total_rss_bytes = total_rss_bytes.saturating_add(proc.memory_bytes);
    }

    ProcessKpiSummary {
        total,
        running,
        sleeping,
        zombies,
        active_cpu_count,
        total_cpu_pct,
        total_rss_bytes,
        top_cpu: state.top_cpu_process(),
        top_mem: state.top_mem_process(),
    }
}

/// Render the Process & Services management panel.
pub fn render(
    ui: &mut Ui,
    state: &mut ProcessState,
    is_connected: bool,
    cpu_count: u32,
) -> Option<ProcessPanelAction> {
    let p = Palette::for_ui(ui);

    if !is_connected {
        ui.add_space(24.0);
        theme::card_frame(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Serveur actuellement déconnecté")
                        .size(16.0)
                        .strong()
                        .color(p.text_primary),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Connectez-vous pour inspecter et contrôler les processus et services systemd en direct.",
                    )
                    .color(p.text_secondary),
                );
                ui.add_space(12.0);
            });
        });
        return None;
    }

    let mut action: Option<ProcessPanelAction> = None;

    // 1. Top Sub-tab Bar & Global Controls
    let avail_w = ui.available_width();
    let render_subtabs =
        |ui: &mut Ui, state: &mut ProcessState, action: &mut Option<ProcessPanelAction>| {
            let proc_label = format!("⚡ Processus ({})", state.processes.len());
            if theme::tab_button(
                ui,
                &p,
                state.active_subtab == ProcessSubTab::Processes,
                &proc_label,
            )
            .clicked()
            {
                state.active_subtab = ProcessSubTab::Processes;
                if state.processes.is_empty() && !state.is_loading_processes {
                    *action = Some(ProcessPanelAction::RefreshProcesses);
                }
            }

            let srv_label = format!("⚙ Services systemd ({})", state.services.len());
            if theme::tab_button(
                ui,
                &p,
                state.active_subtab == ProcessSubTab::Services,
                &srv_label,
            )
            .clicked()
            {
                state.active_subtab = ProcessSubTab::Services;
                if state.services.is_empty() && !state.is_loading_services {
                    *action = Some(ProcessPanelAction::RefreshServices);
                }
            }
        };

    let render_controls = |ui: &mut Ui,
                           rtl: bool,
                           state: &mut ProcessState,
                           action: &mut Option<ProcessPanelAction>| {
        let is_loading = (state.active_subtab == ProcessSubTab::Processes
            && state.is_loading_processes)
            || (state.active_subtab == ProcessSubTab::Services && state.is_loading_services);

        let draw_refresh = |ui: &mut Ui,
                            state: &ProcessState,
                            action: &mut Option<ProcessPanelAction>| {
            if ui
                .button("🔄 Actualiser")
                .on_hover_text("Actualiser les données depuis le serveur")
                .clicked()
            {
                match state.active_subtab {
                    ProcessSubTab::Processes => {
                        *action = Some(ProcessPanelAction::RefreshProcesses)
                    }
                    ProcessSubTab::Services => {
                        *action = Some(ProcessPanelAction::RefreshServices)
                    }
                }
            }
        };
        let draw_pause = |ui: &mut Ui, state: &mut ProcessState| {
            if state.active_subtab == ProcessSubTab::Processes && state.auto_refresh {
                let pause_label = if state.is_paused {
                    "▶ Reprendre"
                } else {
                    "⏸ Pause"
                };
                if ui
                    .button(pause_label)
                    .on_hover_text("Geler/reprendre l'actualisation automatique pour inspecter")
                    .clicked()
                {
                    state.is_paused = !state.is_paused;
                }
            }
        };
        let draw_auto = |ui: &mut Ui, state: &mut ProcessState| {
            if state.active_subtab == ProcessSubTab::Processes {
                ui.checkbox(&mut state.auto_refresh, "⚡ Auto (3s)")
                    .on_hover_text(
                        "Actualise automatiquement la liste des processus toutes les 3 secondes",
                    );
            }
        };

        if rtl {
            draw_refresh(ui, state, action);
            draw_pause(ui, state);
            draw_auto(ui, state);
            if is_loading {
                ui.spinner();
            }
        } else {
            if is_loading {
                ui.spinner();
            }
            draw_auto(ui, state);
            draw_pause(ui, state);
            draw_refresh(ui, state, action);
        }
    };

    if avail_w >= 680.0 {
        ui.horizontal(|ui| {
            render_subtabs(ui, state, &mut action);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                render_controls(ui, true, state, &mut action);
            });
        });
    } else {
        ui.horizontal_wrapped(|ui| {
            render_subtabs(ui, state, &mut action);
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            render_controls(ui, false, state, &mut action);
        });
    }

    ui.add_space(8.0);

    // 2. Status Notification Banner (if any)
    if let Some((msg, is_error)) = &state.status_message.clone() {
        let (bg, border, fg) = if *is_error {
            (p.danger_bg, p.danger_border, p.danger)
        } else {
            (p.success_bg, p.success_border, p.success)
        };
        egui::Frame::new()
            .fill(bg)
            .stroke(Stroke::new(1.0_f32, border))
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(12, 7))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if ui.available_width() >= 480.0 {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(if *is_error { "⚠" } else { "✔" })
                                .color(fg)
                                .strong(),
                        );
                        ui.add(egui::Label::new(RichText::new(msg).color(fg)).truncate());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button("✖")
                                .on_hover_text("Fermer la notification")
                                .clicked()
                            {
                                state.clear_status();
                            }
                        });
                    });
                } else {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(if *is_error { "⚠" } else { "✔" })
                                .color(fg)
                                .strong(),
                        );
                        ui.label(RichText::new(msg).color(fg));
                        if ui
                            .small_button("✖")
                            .on_hover_text("Fermer la notification")
                            .clicked()
                        {
                            state.clear_status();
                        }
                    });
                }
            });
        ui.add_space(8.0);
    }

    // 3. Responsive KPI Statistics Strip
    match state.active_subtab {
        ProcessSubTab::Processes => render_process_kpi_banner(ui, &p, state, cpu_count),
        ProcessSubTab::Services => render_service_kpi_banner(ui, &p, state),
    }

    ui.add_space(8.0);

    // 4. Filter Toolbar & Table
    match state.active_subtab {
        ProcessSubTab::Processes => {
            let filtered_count = state.filtered_processes().len();
            render_process_filter_toolbar(ui, &p, state, filtered_count);
            ui.add_space(8.0);
            if let Some(act) = render_processes_table(ui, &p, state, cpu_count) {
                action = Some(act);
            }
            if let Some(act) = render_process_inspector(ui, &p, state) {
                action = Some(act);
            }
        }
        ProcessSubTab::Services => {
            let filtered_count = state.filtered_services().len();
            render_service_filter_toolbar(ui, &p, state, filtered_count);
            ui.add_space(8.0);
            if let Some(act) = render_services_table(ui, &p, state) {
                action = Some(act);
            }
        }
    }

    // 5. Confirmation Modal for destructive actions
    if let Some(confirm) = state.pending_confirm.clone() {
        let mut clear_confirm = false;
        egui::Window::new("⚠ Confirmation requise")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                match &confirm {
                    PendingProcessConfirm::KillProcess { pid, name, signal } => {
                        let sig_label = if *signal == 9 {
                            "SIGKILL (9 - arrêt forcé)"
                        } else {
                            "SIGTERM (15 - arrêt propre)"
                        };
                        ui.label(format!(
                            "Envoyer {sig_label} au processus « {name} » (PID {pid}) ?"
                        ));
                        if *pid <= 1 || name.to_lowercase().contains("ntx-server") {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(
                                    "⚠ ATTENTION : Ce processus est critique ou correspond à l'agent NTX lui-même !",
                                )
                                .color(p.danger)
                                .strong(),
                            );
                        }
                    }
                    PendingProcessConfirm::ControlService {
                        service_name,
                        action: svc_act,
                    } => {
                        ui.label(format!(
                            "Exécuter « systemctl {svc_act} {service_name} » ?"
                        ));
                        if service_name.contains("ntx-server") {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(
                                    "⚠ ATTENTION : Cela affectera la session distante NTX active !",
                                )
                                .color(p.danger)
                                .strong(),
                            );
                        }
                    }
                }

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if theme::danger_button(ui, &p, "✔ Confirmer").clicked() {
                        action = Some(match confirm.clone() {
                            PendingProcessConfirm::KillProcess { pid, signal, .. } => {
                                ProcessPanelAction::KillProcess { pid, signal }
                            }
                            PendingProcessConfirm::ControlService {
                                service_name,
                                action: svc_act,
                            } => ProcessPanelAction::ControlService {
                                service_name,
                                action: svc_act,
                            },
                        });
                        clear_confirm = true;
                    }

                    if ui.button("Annuler").clicked() {
                        clear_confirm = true;
                    }
                });
            });

        if clear_confirm {
            state.pending_confirm = None;
        }
    }

    action
}

fn render_process_kpi_banner(ui: &mut Ui, p: &Palette, state: &ProcessState, cpu_count: u32) {
    if state.processes.is_empty() {
        return;
    }

    let kpi = compute_process_kpi_summary(state);
    let cols_count = kpi_column_count(ui.available_width());

    match cols_count {
        4 => {
            ui.columns(4, |cols| {
                render_proc_kpi_total(&mut cols[0], p, &kpi);
                render_proc_kpi_cpu(&mut cols[1], p, &kpi, cpu_count);
                render_proc_kpi_ram(&mut cols[2], p, &kpi);
                render_proc_kpi_health(&mut cols[3], p, &kpi);
            });
        }
        2 => {
            ui.columns(2, |cols| {
                render_proc_kpi_total(&mut cols[0], p, &kpi);
                render_proc_kpi_cpu(&mut cols[1], p, &kpi, cpu_count);
            });
            ui.add_space(8.0);
            ui.columns(2, |cols| {
                render_proc_kpi_ram(&mut cols[0], p, &kpi);
                render_proc_kpi_health(&mut cols[1], p, &kpi);
            });
        }
        _ => {
            render_proc_kpi_total(ui, p, &kpi);
            ui.add_space(8.0);
            render_proc_kpi_cpu(ui, p, &kpi, cpu_count);
            ui.add_space(8.0);
            render_proc_kpi_ram(ui, p, &kpi);
            ui.add_space(8.0);
            render_proc_kpi_health(ui, p, &kpi);
        }
    }
}

fn render_proc_kpi_total(ui: &mut Ui, p: &Palette, kpi: &ProcessKpiSummary) {
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(90.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("PROCESSUS")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (bg, fg, border) = if kpi.running > 0 {
                    (p.success_bg, p.success, Some(p.success_border))
                } else {
                    (p.bg_subtle, p.text_muted, Some(p.border))
                };
                theme::pill_badge(ui, &format!("● {} actif(s)", kpi.running), bg, fg, border);
            });
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("{}", kpi.total))
                .size(20.0)
                .strong()
                .color(p.text_primary),
        );

        ui.add_space(4.0);
        let ratio = kpi.running as f32 / kpi.total.max(1) as f32;
        theme::mini_progress_bar(ui, p, ratio, p.success, ui.available_width(), 5.0);
        ui.add_space(4.0);

        let caption = format!(
            "{} en veille • {} actif(s) (>0% CPU)",
            kpi.sleeping, kpi.active_cpu_count
        );
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_proc_kpi_cpu(ui: &mut Ui, p: &Palette, kpi: &ProcessKpiSummary, cpu_count: u32) {
    let cores = cpu_count.max(1);
    let max_cpu = 100.0 * cores as f64;
    let norm_pct = (kpi.total_cpu_pct / max_cpu * 100.0).clamp(0.0, 100.0);
    let color = if kpi.total_cpu_pct > 0.05 {
        theme::usage_color(p, norm_pct)
    } else {
        p.text_secondary
    };

    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(90.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("CHARGE CPU")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{cores} cœur(s)"))
                        .size(10.5)
                        .color(p.text_muted),
                );
            });
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("{:.1}%", kpi.total_cpu_pct))
                .size(20.0)
                .strong()
                .color(color),
        );

        ui.add_space(4.0);
        theme::mini_progress_bar(
            ui,
            p,
            (norm_pct / 100.0) as f32,
            color,
            ui.available_width(),
            5.0,
        )
        .on_hover_text(format!(
            "{:.1}% normalisé sur {cores} cœur(s) (plafond {max_cpu:.0}%)",
            norm_pct
        ));
        ui.add_space(4.0);

        let caption = match &kpi.top_cpu {
            Some((name, cpu)) if *cpu > 0.05 => format!("Pic : {name} ({cpu:.1}%)"),
            _ => "Aucune charge CPU significative".to_string(),
        };
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_proc_kpi_ram(ui: &mut Ui, p: &Palette, kpi: &ProcessKpiSummary) {
    let top_bytes = kpi.top_mem.as_ref().map(|(_, m)| *m).unwrap_or(0);
    let top_share = if kpi.total_rss_bytes > 0 {
        (top_bytes as f64 / kpi.total_rss_bytes as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };

    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(90.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("MÉMOIRE RSS")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            if top_bytes > 0 && kpi.total_rss_bytes > 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("Top {:.0}%", top_share * 100.0))
                            .size(10.5)
                            .color(p.text_muted),
                    );
                });
            }
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new(theme::format_ram(kpi.total_rss_bytes))
                .size(20.0)
                .strong()
                .color(p.purple),
        );

        ui.add_space(4.0);
        theme::mini_progress_bar(
            ui,
            p,
            top_share as f32,
            p.purple,
            ui.available_width(),
            5.0,
        )
        .on_hover_text("Part du processus le plus consommateur dans la RAM RSS listée");
        ui.add_space(4.0);

        let caption = match &kpi.top_mem {
            Some((name, mem)) if *mem > 0 => format!("Pic : {name} ({})", theme::format_ram(*mem)),
            _ => "Aucune mémoire RSS mesurée".to_string(),
        };
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_proc_kpi_health(ui: &mut Ui, p: &Palette, kpi: &ProcessKpiSummary) {
    let has_zombies = kpi.zombies > 0;

    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(90.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("ZOMBIES & SANTÉ")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if has_zombies {
                    theme::pill_badge(
                        ui,
                        "⚠ Alerte",
                        p.danger_bg,
                        p.danger,
                        Some(p.danger_border),
                    );
                } else {
                    theme::pill_badge(
                        ui,
                        "✔ Nominal",
                        p.success_bg,
                        p.success,
                        Some(p.success_border),
                    );
                }
            });
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("{}", kpi.zombies))
                .size(20.0)
                .strong()
                .color(if has_zombies { p.danger } else { p.success }),
        );

        ui.add_space(4.0);
        let (frac, bar_col) = if has_zombies {
            (
                (kpi.zombies as f32 / kpi.total.max(1) as f32).max(0.08),
                p.danger,
            )
        } else {
            (1.0, p.success)
        };
        theme::mini_progress_bar(ui, p, frac, bar_col, ui.available_width(), 5.0);
        ui.add_space(4.0);

        let caption = if has_zombies {
            format!("{} processus zombie(s) détecté(s)", kpi.zombies)
        } else {
            "Aucun processus zombie détecté".to_string()
        };
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_service_kpi_banner(ui: &mut Ui, p: &Palette, state: &mut ProcessState) {
    if state.services.is_empty() {
        return;
    }

    let (total, active, inactive, failed) = state.service_counts();
    let cols_count = kpi_column_count(ui.available_width());

    match cols_count {
        4 => {
            ui.columns(4, |cols| {
                render_svc_kpi_total(&mut cols[0], p, state, total, active, failed);
                render_svc_kpi_active(&mut cols[1], p, total, active);
                render_svc_kpi_inactive(&mut cols[2], p, total, inactive);
                render_svc_kpi_failed(&mut cols[3], p, state, total, failed);
            });
        }
        2 => {
            ui.columns(2, |cols| {
                render_svc_kpi_total(&mut cols[0], p, state, total, active, failed);
                render_svc_kpi_active(&mut cols[1], p, total, active);
            });
            ui.add_space(8.0);
            ui.columns(2, |cols| {
                render_svc_kpi_inactive(&mut cols[0], p, total, inactive);
                render_svc_kpi_failed(&mut cols[1], p, state, total, failed);
            });
        }
        _ => {
            render_svc_kpi_total(ui, p, state, total, active, failed);
            ui.add_space(8.0);
            render_svc_kpi_active(ui, p, total, active);
            ui.add_space(8.0);
            render_svc_kpi_inactive(ui, p, total, inactive);
            ui.add_space(8.0);
            render_svc_kpi_failed(ui, p, state, total, failed);
        }
    }
}

fn render_svc_kpi_total(
    ui: &mut Ui,
    p: &Palette,
    state: &mut ProcessState,
    total: usize,
    active: usize,
    failed: usize,
) {
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(90.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("SERVICES SYSTEMD")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if failed > 0 {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("⚠ {failed}"))
                                    .size(10.5)
                                    .strong()
                                    .color(p.danger),
                            )
                            .fill(p.danger_bg)
                            .stroke(Stroke::new(1.0_f32, p.danger_border))
                            .corner_radius(10),
                        )
                        .on_hover_text("Filtrer sur les services en échec")
                        .clicked()
                    {
                        state.service_category = ServiceFilterCategory::FailedOnly;
                    }
                } else {
                    theme::pill_badge(
                        ui,
                        "✔ Nominal",
                        p.success_bg,
                        p.success,
                        Some(p.success_border),
                    );
                }
            });
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("{total}"))
                .size(20.0)
                .strong()
                .color(p.text_primary),
        );

        ui.add_space(4.0);
        let ratio = active as f32 / total.max(1) as f32;
        theme::mini_progress_bar(ui, p, ratio, p.success, ui.available_width(), 5.0);
        ui.add_space(4.0);

        let pct = (active as f64 / total.max(1) as f64) * 100.0;
        let caption = format!("{pct:.0}% des unités sont actives");
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_svc_kpi_active(ui: &mut Ui, p: &Palette, total: usize, active: usize) {
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(90.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("ACTIFS")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::pill_badge(
                    ui,
                    "● running",
                    p.success_bg,
                    p.success,
                    Some(p.success_border),
                );
            });
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("{active}"))
                .size(20.0)
                .strong()
                .color(if active > 0 {
                    p.success
                } else {
                    p.text_secondary
                }),
        );

        ui.add_space(4.0);
        let ratio = active as f32 / total.max(1) as f32;
        theme::mini_progress_bar(ui, p, ratio, p.success, ui.available_width(), 5.0);
        ui.add_space(4.0);

        let caption = format!("{active} / {total} unités en exécution");
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_svc_kpi_inactive(ui: &mut Ui, p: &Palette, total: usize, inactive: usize) {
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(90.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("INACTIFS")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::pill_badge(ui, "○ arrêtés", p.bg_subtle, p.text_muted, Some(p.border));
            });
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("{inactive}"))
                .size(20.0)
                .strong()
                .color(p.text_secondary),
        );

        ui.add_space(4.0);
        let ratio = inactive as f32 / total.max(1) as f32;
        theme::mini_progress_bar(ui, p, ratio, p.text_muted, ui.available_width(), 5.0);
        ui.add_space(4.0);

        let caption = format!("{inactive} unité(s) arrêtée(s) ou en veille");
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_svc_kpi_failed(
    ui: &mut Ui,
    p: &Palette,
    state: &mut ProcessState,
    total: usize,
    failed: usize,
) {
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(90.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("EN ÉCHEC")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if failed > 0 {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("✖ Critique")
                                    .size(10.5)
                                    .strong()
                                    .color(p.danger),
                            )
                            .fill(p.danger_bg)
                            .stroke(Stroke::new(1.0_f32, p.danger_border))
                            .corner_radius(10),
                        )
                        .on_hover_text("Filtrer sur les services en échec")
                        .clicked()
                    {
                        state.service_category = ServiceFilterCategory::FailedOnly;
                    }
                } else {
                    theme::pill_badge(
                        ui,
                        "✔ Aucun",
                        p.success_bg,
                        p.success,
                        Some(p.success_border),
                    );
                }
            });
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("{failed}"))
                .size(20.0)
                .strong()
                .color(if failed > 0 { p.danger } else { p.success }),
        );

        ui.add_space(4.0);
        let (frac, bar_col) = if failed > 0 {
            ((failed as f32 / total.max(1) as f32).max(0.08), p.danger)
        } else {
            (0.0, p.success)
        };
        theme::mini_progress_bar(ui, p, frac, bar_col, ui.available_width(), 5.0);
        ui.add_space(4.0);

        let caption = if failed > 0 {
            format!("{failed} service(s) en erreur à inspecter")
        } else {
            "Aucune unité systemd en échec".to_string()
        };
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_process_filter_toolbar(
    ui: &mut Ui,
    p: &Palette,
    state: &mut ProcessState,
    filtered_count: usize,
) {
    let total_count = state.processes.len();
    let count_text = format!("{filtered_count}/{total_count} affichés");
    let avail_w = ui.available_width();

    let draw_search_and_categories = |ui: &mut Ui, state: &mut ProcessState| {
        ui.label(RichText::new("🔍").color(p.text_muted));
        ui.add(
            egui::TextEdit::singleline(&mut state.process_filter)
                .hint_text("Filtrer (nom, PID, utilisateur, commande...)")
                .desired_width(215.0),
        );
        if !state.process_filter.is_empty()
            && ui
                .small_button("✖")
                .on_hover_text("Effacer la recherche")
                .clicked()
        {
            state.process_filter.clear();
        }

        ui.separator();

        if theme::tab_button(
            ui,
            p,
            state.process_category == ProcessFilterCategory::All,
            "Tous",
        )
        .clicked()
        {
            state.process_category = ProcessFilterCategory::All;
        }
        if theme::tab_button(
            ui,
            p,
            state.process_category == ProcessFilterCategory::ActiveOnly,
            "⚡ Actifs (>0%)",
        )
        .on_hover_text("Afficher uniquement les processus consommant du CPU")
        .clicked()
        {
            state.process_category = ProcessFilterCategory::ActiveOnly;
        }
        if theme::tab_button(
            ui,
            p,
            state.process_category == ProcessFilterCategory::UserOnly,
            "👤 Utilisateur",
        )
        .on_hover_text("Afficher les processus non-système (hors root / daemons)")
        .clicked()
        {
            state.process_category = ProcessFilterCategory::UserOnly;
        }
        if theme::tab_button(
            ui,
            p,
            state.process_category == ProcessFilterCategory::SystemOnly,
            "🛡 Système",
        )
        .on_hover_text("Afficher les processus système / root")
        .clicked()
        {
            state.process_category = ProcessFilterCategory::SystemOnly;
        }
    };

    let draw_view_controls = |ui: &mut Ui, state: &mut ProcessState| {
        let is_tree = state.view_mode == ProcessViewMode::Tree;
        if theme::tab_button(ui, p, is_tree, "🌲 Arborescence")
            .on_hover_text("Regrouper les processus enfants sous leur parent PID")
            .clicked()
        {
            state.view_mode = ProcessViewMode::Tree;
        }
        if theme::tab_button(ui, p, !is_tree, "☰ Liste plate")
            .on_hover_text("Affichage tabulaire plat traditionnel")
            .clicked()
        {
            state.view_mode = ProcessViewMode::Flat;
        }

        if is_tree {
            if ui
                .button("➕ Déplier tout")
                .on_hover_text("Déplier toutes les branches de l'arbre")
                .clicked()
            {
                state.expand_all();
            }
            if ui
                .button("➖ Replier tout")
                .on_hover_text("Replier tous les processus ayant des sous-processus")
                .clicked()
            {
                state.collapse_all();
            }
        }
    };

    theme::subtle_bar_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if avail_w >= 1120.0 {
            ui.horizontal(|ui| {
                draw_search_and_categories(ui, state);
                ui.separator();
                draw_view_controls(ui, state);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::pill_badge(ui, &count_text, p.bg_card, p.text_secondary, Some(p.border));
                });
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                draw_search_and_categories(ui, state);
            });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                draw_view_controls(ui, state);
                ui.separator();
                theme::pill_badge(ui, &count_text, p.bg_card, p.text_secondary, Some(p.border));
            });
        }
    });
}

fn render_service_filter_toolbar(
    ui: &mut Ui,
    p: &Palette,
    state: &mut ProcessState,
    filtered_count: usize,
) {
    let (total, active, inactive, failed) = state.service_counts();
    let count_text = format!("{filtered_count}/{total} affichés");
    let avail_w = ui.available_width();

    let draw_search_and_filters = |ui: &mut Ui, state: &mut ProcessState| {
        ui.label(RichText::new("🔍").color(p.text_muted));
        ui.add(
            egui::TextEdit::singleline(&mut state.service_filter)
                .hint_text("Filtrer par nom de service, description...")
                .desired_width(220.0),
        );
        if !state.service_filter.is_empty()
            && ui
                .small_button("✖")
                .on_hover_text("Effacer la recherche")
                .clicked()
        {
            state.service_filter.clear();
        }

        ui.separator();

        if theme::tab_button(
            ui,
            p,
            state.service_category == ServiceFilterCategory::All,
            &format!("Tous ({total})"),
        )
        .clicked()
        {
            state.service_category = ServiceFilterCategory::All;
        }
        if theme::tab_button(
            ui,
            p,
            state.service_category == ServiceFilterCategory::ActiveOnly,
            &format!("● Actifs ({active})"),
        )
        .clicked()
        {
            state.service_category = ServiceFilterCategory::ActiveOnly;
        }
        if theme::tab_button(
            ui,
            p,
            state.service_category == ServiceFilterCategory::InactiveOnly,
            &format!("○ Inactifs ({inactive})"),
        )
        .clicked()
        {
            state.service_category = ServiceFilterCategory::InactiveOnly;
        }
        if theme::tab_button(
            ui,
            p,
            state.service_category == ServiceFilterCategory::FailedOnly,
            &format!("✖ En échec ({failed})"),
        )
        .clicked()
        {
            state.service_category = ServiceFilterCategory::FailedOnly;
        }
    };

    theme::subtle_bar_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if avail_w >= 860.0 {
            ui.horizontal(|ui| {
                draw_search_and_filters(ui, state);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::pill_badge(ui, &count_text, p.bg_card, p.text_secondary, Some(p.border));
                });
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                draw_search_and_filters(ui, state);
                ui.separator();
                theme::pill_badge(ui, &count_text, p.bg_card, p.text_secondary, Some(p.border));
            });
        }
    });
}

fn sort_indicator(direction: SortDirection) -> &'static str {
    match direction {
        SortDirection::Ascending => " ▲",
        SortDirection::Descending => " ▼",
    }
}

fn render_processes_table(
    ui: &mut Ui,
    p: &Palette,
    state: &mut ProcessState,
    cpu_count: u32,
) -> Option<ProcessPanelAction> {
    if state.processes.is_empty() && !state.is_loading_processes {
        ui.add_space(24.0);
        let mut load_clicked = false;
        theme::card_frame(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(10.0);
                ui.label(
                    RichText::new("Aucun processus chargé pour l'instant.")
                        .color(p.text_secondary),
                );
                ui.add_space(8.0);
                if theme::primary_button(ui, p, "⚡ Charger la liste des processus").clicked() {
                    load_clicked = true;
                }
                ui.add_space(10.0);
            });
        });
        if load_clicked {
            return Some(ProcessPanelAction::RefreshProcesses);
        }
        return None;
    }

    let mut action: Option<ProcessPanelAction> = None;
    let mut sort_to_toggle: Option<ProcessSortColumn> = None;
    let mut collapse_to_toggle: Option<u32> = None;
    let mut pid_to_select: Option<Option<u32>> = None;
    let mut confirm_to_set: Option<PendingProcessConfirm> = None;

    let active_col = state.process_sort_column;
    let active_dir = state.process_sort_direction;
    let is_tree_mode = state.view_mode == ProcessViewMode::Tree;
    let tree_rows = state.build_tree_rows();

    if tree_rows.is_empty() {
        let mut reset_filters = false;
        theme::card_frame(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Aucun processus ne correspond aux filtres actuels.")
                        .size(14.0)
                        .strong()
                        .color(p.text_primary),
                );
                ui.add_space(6.0);
                if theme::primary_button(ui, p, "Réinitialiser les filtres").clicked() {
                    reset_filters = true;
                }
                ui.add_space(12.0);
            });
        });
        if reset_filters {
            state.process_filter.clear();
            state.process_category = ProcessFilterCategory::All;
        }
        return None;
    }

    let reserved_h = if state.selected_pid.is_some() {
        220.0
    } else {
        12.0
    };

    theme::card_frame(p).show(ui, |ui| {
        let outer_avail_w = ui.available_width();
        let avail_h = (ui.available_height() - reserved_h).max(200.0);
        let table_w = outer_avail_w.max(960.0);

        egui::ScrollArea::horizontal()
            .id_salt("process_table_hscroll")
            .show(ui, |ui| {
                let max_rect =
                    egui::Rect::from_min_size(ui.cursor().min, egui::vec2(table_w, avail_h));
                ui.scope_builder(egui::UiBuilder::new().max_rect(max_rect), |ui| {
                    TableBuilder::new(ui)
                        .id_salt("process_table_grid")
                        .striped(true)
                        .resizable(true)
                        .auto_shrink([false, true])
                        .min_scrolled_height(0.0)
                        .max_scroll_height((avail_h - 32.0).max(150.0))
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::initial(70.0).at_least(50.0))
                        .column(Column::initial(220.0).at_least(140.0))
                        .column(Column::initial(85.0).at_least(60.0))
                        .column(Column::initial(115.0).at_least(90.0))
                        .column(Column::initial(95.0).at_least(70.0))
                        .column(Column::initial(85.0).at_least(65.0))
                        .column(Column::remainder().at_least(140.0))
                        .column(Column::exact(180.0))
                        .header(26.0, |mut header| {
                            header.col(|ui| {
                                let text = format!(
                                    "PID{}",
                                    if active_col == ProcessSortColumn::Pid {
                                        sort_indicator(active_dir)
                                    } else {
                                        ""
                                    }
                                );
                                if ui
                                    .selectable_label(active_col == ProcessSortColumn::Pid, text)
                                    .clicked()
                                {
                                    sort_to_toggle = Some(ProcessSortColumn::Pid);
                                }
                            });
                            header.col(|ui| {
                                let text = format!(
                                    "Nom{}",
                                    if active_col == ProcessSortColumn::Name {
                                        sort_indicator(active_dir)
                                    } else {
                                        ""
                                    }
                                );
                                if ui
                                    .selectable_label(active_col == ProcessSortColumn::Name, text)
                                    .clicked()
                                {
                                    sort_to_toggle = Some(ProcessSortColumn::Name);
                                }
                            });
                            header.col(|ui| {
                                let text = format!(
                                    "Utilisateur{}",
                                    if active_col == ProcessSortColumn::User {
                                        sort_indicator(active_dir)
                                    } else {
                                        ""
                                    }
                                );
                                if ui
                                    .selectable_label(active_col == ProcessSortColumn::User, text)
                                    .clicked()
                                {
                                    sort_to_toggle = Some(ProcessSortColumn::User);
                                }
                            });
                            header.col(|ui| {
                                let text = format!(
                                    "CPU %{}",
                                    if active_col == ProcessSortColumn::Cpu {
                                        sort_indicator(active_dir)
                                    } else {
                                        ""
                                    }
                                );
                                if ui
                                    .selectable_label(active_col == ProcessSortColumn::Cpu, text)
                                    .clicked()
                                {
                                    sort_to_toggle = Some(ProcessSortColumn::Cpu);
                                }
                            });
                            header.col(|ui| {
                                let text = format!(
                                    "RAM{}",
                                    if active_col == ProcessSortColumn::Memory {
                                        sort_indicator(active_dir)
                                    } else {
                                        ""
                                    }
                                );
                                if ui
                                    .selectable_label(active_col == ProcessSortColumn::Memory, text)
                                    .clicked()
                                {
                                    sort_to_toggle = Some(ProcessSortColumn::Memory);
                                }
                            });
                            header.col(|ui| {
                                let text = format!(
                                    "Statut{}",
                                    if active_col == ProcessSortColumn::Status {
                                        sort_indicator(active_dir)
                                    } else {
                                        ""
                                    }
                                );
                                if ui
                                    .selectable_label(active_col == ProcessSortColumn::Status, text)
                                    .clicked()
                                {
                                    sort_to_toggle = Some(ProcessSortColumn::Status);
                                }
                            });
                            header.col(|ui| {
                                let text = format!(
                                    "Ligne de commande{}",
                                    if active_col == ProcessSortColumn::CommandLine {
                                        sort_indicator(active_dir)
                                    } else {
                                        ""
                                    }
                                );
                                if ui
                                    .selectable_label(
                                        active_col == ProcessSortColumn::CommandLine,
                                        text,
                                    )
                                    .clicked()
                                {
                                    sort_to_toggle = Some(ProcessSortColumn::CommandLine);
                                }
                            });
                            header.col(|ui| {
                                ui.label(
                                    RichText::new("Actions").strong().color(p.text_secondary),
                                );
                            });
                        })
                        .body(|body| {
                            body.rows(28.0, tree_rows.len(), |mut row| {
                                let tree_row = &tree_rows[row.index()];
                                let proc = tree_row.item;
                                let is_selected = state.selected_pid == Some(proc.pid);

                                row.col(|ui| {
                                    let pid_text = if is_selected {
                                        RichText::new(proc.pid.to_string())
                                            .color(p.accent)
                                            .strong()
                                    } else {
                                        RichText::new(proc.pid.to_string()).color(p.text_muted)
                                    };
                                    let resp = ui.selectable_label(is_selected, pid_text);
                                    if resp.clicked() {
                                        pid_to_select =
                                            Some(if is_selected { None } else { Some(proc.pid) });
                                    }
                                    if proc.parent_pid > 0 {
                                        resp.on_hover_text(format!(
                                            "PID: {}\nPPID (parent): {}",
                                            proc.pid, proc.parent_pid
                                        ));
                                    }
                                });

                                row.col(|ui| {
                                    ui.push_id(proc.pid, |ui| {
                                        ui.horizontal(|ui| {
                                            if is_tree_mode {
                                                if tree_row.depth > 0 {
                                                    ui.add_space(tree_row.depth as f32 * 14.0);
                                                }

                                                if tree_row.has_children {
                                                    let icon = if tree_row.is_collapsed {
                                                        "▶"
                                                    } else {
                                                        "▼"
                                                    };
                                                    let badge_label =
                                                        format!("{icon} {}", tree_row.children_count);
                                                    let btn = egui::Button::new(
                                                        RichText::new(badge_label)
                                                            .size(11.0)
                                                            .strong()
                                                            .color(if tree_row.is_collapsed {
                                                                p.accent
                                                            } else {
                                                                p.text_secondary
                                                            }),
                                                    )
                                                    .fill(if tree_row.is_collapsed {
                                                        p.accent_bg
                                                    } else {
                                                        p.bg_subtle
                                                    })
                                                    .stroke(Stroke::new(1.0_f32, p.border))
                                                    .corner_radius(4);

                                                    if ui
                                                        .add(btn)
                                                        .on_hover_text(
                                                            "Déplier / replier ce groupe (ou double-clic sur le nom)",
                                                        )
                                                        .clicked()
                                                    {
                                                        collapse_to_toggle = Some(proc.pid);
                                                    }
                                                } else if tree_row.depth > 0 {
                                                    ui.label(
                                                        RichText::new("└─")
                                                            .color(p.text_muted)
                                                            .size(11.0),
                                                    );
                                                }
                                            }

                                            let name_label = if is_selected {
                                                RichText::new(&proc.name).color(p.accent).strong()
                                            } else {
                                                RichText::new(&proc.name)
                                                    .strong()
                                                    .color(p.text_primary)
                                            };

                                            let resp = ui.selectable_label(is_selected, name_label);
                                            if resp.clicked() {
                                                pid_to_select = Some(if is_selected {
                                                    None
                                                } else {
                                                    Some(proc.pid)
                                                });
                                            }
                                            if resp.double_clicked() && tree_row.has_children {
                                                collapse_to_toggle = Some(proc.pid);
                                            }
                                            resp.on_hover_text(format!(
                                                "{} (PID: {})\nCliquer pour inspecter\nDouble-cliquer pour déplier/replier",
                                                proc.name, proc.pid
                                            ));
                                        });
                                    });
                                });

                                row.col(|ui| {
                                    let user_display = if proc.user.is_empty() {
                                        "—"
                                    } else {
                                        &proc.user
                                    };
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(user_display)
                                                .color(p.text_secondary)
                                                .size(12.0),
                                        )
                                        .truncate(),
                                    );
                                });

                                row.col(|ui| {
                                    render_cpu_bar(ui, p, proc.cpu_usage, cpu_count);
                                    if tree_row.is_collapsed && tree_row.has_children {
                                        ui.label(
                                            RichText::new(format!("Σ{:.1}%", tree_row.total_cpu))
                                                .size(10.0)
                                                .color(p.text_muted),
                                        )
                                        .on_hover_text(format!(
                                            "Total avec {} enfants: {:.1}%",
                                            tree_row.children_count, tree_row.total_cpu
                                        ));
                                    }
                                });

                                row.col(|ui| {
                                    let mem_str = theme::format_ram(proc.memory_bytes);
                                    ui.label(RichText::new(mem_str).size(12.0));
                                    if tree_row.is_collapsed && tree_row.has_children {
                                        ui.label(
                                            RichText::new(format!(
                                                "Σ{}",
                                                theme::format_ram(tree_row.total_memory)
                                            ))
                                            .size(10.0)
                                            .color(p.text_muted),
                                        )
                                        .on_hover_text(format!(
                                            "Total avec {} enfants: {}",
                                            tree_row.children_count,
                                            theme::format_ram(tree_row.total_memory)
                                        ));
                                    }
                                });

                                row.col(|ui| {
                                    let st = proc.status.to_lowercase();
                                    let (badge_text, color) = if st.contains("run") {
                                        ("● Actif", p.success)
                                    } else if st.contains("sleep") || st.contains("idle") {
                                        ("○ Veille", p.text_muted)
                                    } else if st.contains("stop") {
                                        ("■ Stop", p.warning)
                                    } else if st.contains("zomb") || st.contains("dead") {
                                        ("✖ Zombie", p.danger)
                                    } else {
                                        ("• Autre", p.text_secondary)
                                    };
                                    ui.label(
                                        RichText::new(badge_text).color(color).size(11.0).strong(),
                                    )
                                    .on_hover_text(&proc.status);
                                });

                                row.col(|ui| {
                                    let cmd = if proc.command_line.is_empty() {
                                        &proc.name
                                    } else {
                                        &proc.command_line
                                    };
                                    let lbl = ui.add(
                                        egui::Label::new(
                                            RichText::new(cmd)
                                                .monospace()
                                                .size(11.0)
                                                .color(p.text_secondary),
                                        )
                                        .truncate(),
                                    );
                                    lbl.on_hover_text(cmd);
                                });

                                row.col(|ui| {
                                    ui.horizontal(|ui| {
                                        if ui
                                            .button("⭐")
                                            .on_hover_text("⭐ Ajouter à Mes Services")
                                            .clicked()
                                        {
                                            action = Some(
                                                ProcessPanelAction::AddFavoriteFromProcess(
                                                    proc.clone(),
                                                ),
                                            );
                                        }

                                        if ui
                                            .button("Arrêter")
                                            .on_hover_text("Envoyer SIGTERM (15) avec confirmation")
                                            .clicked()
                                        {
                                            confirm_to_set =
                                                Some(PendingProcessConfirm::KillProcess {
                                                    pid: proc.pid,
                                                    name: proc.name.clone(),
                                                    signal: 15,
                                                });
                                        }

                                        if theme::danger_button(ui, p, "Tuer")
                                            .on_hover_text("Envoyer SIGKILL (9) avec confirmation")
                                            .clicked()
                                        {
                                            confirm_to_set =
                                                Some(PendingProcessConfirm::KillProcess {
                                                    pid: proc.pid,
                                                    name: proc.name.clone(),
                                                    signal: 9,
                                                });
                                        }
                                    });
                                });
                            });
                        });
                });
            });
    });

    if let Some(col) = sort_to_toggle {
        state.toggle_process_sort(col);
    }

    if let Some(pid) = collapse_to_toggle {
        state.toggle_collapse(pid);
    }

    if let Some(pid_opt) = pid_to_select {
        state.selected_pid = pid_opt;
    }

    if let Some(c) = confirm_to_set {
        state.pending_confirm = Some(c);
    }

    action
}

fn render_process_inspector(
    ui: &mut Ui,
    p: &Palette,
    state: &mut ProcessState,
) -> Option<ProcessPanelAction> {
    let pid = state.selected_pid?;
    let proc = state.processes.iter().find(|pr| pr.pid == pid)?.clone();

    let mut action: Option<ProcessPanelAction> = None;
    let mut close_inspector = false;
    let mut select_parent_pid: Option<u32> = None;

    ui.add_space(8.0);

    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let avail_w = ui.available_width();

        let draw_title_and_parent = |ui: &mut Ui, select_parent_pid: &mut Option<u32>| {
            ui.label(
                RichText::new(format!("🔍 Inspecteur : {} (PID : {})", proc.name, proc.pid))
                    .strong()
                    .size(14.5)
                    .color(p.accent),
            );

            if proc.parent_pid > 0 {
                theme::pill_badge(
                    ui,
                    &format!("PPID : {}", proc.parent_pid),
                    p.bg_subtle,
                    p.text_secondary,
                    Some(p.border),
                );
                if ui
                    .small_button("⬆ Parent")
                    .on_hover_text("Basculer vers le processus parent")
                    .clicked()
                {
                    *select_parent_pid = Some(proc.parent_pid);
                }
            }
        };

        if avail_w >= 540.0 {
            ui.horizontal(|ui| {
                draw_title_and_parent(ui, &mut select_parent_pid);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✖ Fermer").clicked() {
                        close_inspector = true;
                    }
                });
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                draw_title_and_parent(ui, &mut select_parent_pid);
                if ui.small_button("✖ Fermer").clicked() {
                    close_inspector = true;
                }
            });
        }

        ui.add_space(6.0);

        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!(
                    "Utilisateur : {}",
                    if proc.user.is_empty() {
                        "—"
                    } else {
                        &proc.user
                    }
                ))
                .color(p.text_secondary)
                .size(12.0),
            );
            ui.separator();
            ui.label(
                RichText::new(format!("Statut : {}", proc.status))
                    .color(p.text_secondary)
                    .size(12.0),
            );
            ui.separator();
            ui.label(
                RichText::new(format!("CPU : {:.1}%", proc.cpu_usage))
                    .strong()
                    .size(12.0)
                    .color(p.orange),
            );
            ui.separator();
            ui.label(
                RichText::new(format!("Mémoire : {}", theme::format_ram(proc.memory_bytes)))
                    .strong()
                    .size(12.0)
                    .color(p.purple),
            );
        });

        ui.add_space(6.0);

        let cmd_text = if proc.command_line.is_empty() {
            proc.name.clone()
        } else {
            proc.command_line.clone()
        };

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Ligne de commande :")
                    .color(p.text_muted)
                    .size(11.5),
            );
            if ui
                .small_button("📋 Copier")
                .on_hover_text("Copier la ligne de commande complète dans le presse-papier")
                .clicked()
            {
                ui.ctx().copy_text(cmd_text.clone());
                state.set_status("Ligne de commande copiée dans le presse-papier.", false);
            }
        });

        theme::subtle_bar_frame(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(&cmd_text)
                    .monospace()
                    .size(11.5)
                    .color(p.text_primary),
            );
        });

        ui.add_space(6.0);

        ui.horizontal_wrapped(|ui| {
            if ui
                .button("⭐ Ajouter à Mes Services")
                .on_hover_text("Ajouter ce processus dans l'onglet Mes Services")
                .clicked()
            {
                action = Some(ProcessPanelAction::AddFavoriteFromProcess(proc.clone()));
            }

            if ui
                .button("⏹ Arrêter (SIGTERM 15)")
                .on_hover_text("Demande de terminaison propre (avec confirmation)")
                .clicked()
            {
                state.pending_confirm = Some(PendingProcessConfirm::KillProcess {
                    pid: proc.pid,
                    name: proc.name.clone(),
                    signal: 15,
                });
            }

            if theme::danger_button(ui, p, "⛔ Forcer l'arrêt (SIGKILL 9)")
                .on_hover_text("Arrêt forcé et immédiat par le noyau (avec confirmation)")
                .clicked()
            {
                state.pending_confirm = Some(PendingProcessConfirm::KillProcess {
                    pid: proc.pid,
                    name: proc.name.clone(),
                    signal: 9,
                });
            }
        });
    });

    if close_inspector {
        state.selected_pid = None;
    }

    if let Some(ppid) = select_parent_pid {
        state.selected_pid = Some(ppid);
    }

    action
}

fn render_services_table(
    ui: &mut Ui,
    p: &Palette,
    state: &mut ProcessState,
) -> Option<ProcessPanelAction> {
    if state.services.is_empty() && !state.is_loading_services {
        ui.add_space(24.0);
        let mut load_clicked = false;
        theme::card_frame(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(10.0);
                ui.label(
                    RichText::new("Aucun service système chargé pour l'instant.")
                        .color(p.text_secondary),
                );
                ui.add_space(8.0);
                if theme::primary_button(ui, p, "⚡ Charger la liste des services").clicked() {
                    load_clicked = true;
                }
                ui.add_space(10.0);
            });
        });
        if load_clicked {
            return Some(ProcessPanelAction::RefreshServices);
        }
        return None;
    }

    let mut action: Option<ProcessPanelAction> = None;
    let mut sort_to_toggle: Option<ServiceSortColumn> = None;
    let mut confirm_to_set: Option<PendingProcessConfirm> = None;

    let active_col = state.service_sort_column;
    let active_dir = state.service_sort_direction;
    let services = state.filtered_services();

    if services.is_empty() {
        let mut reset_filters = false;
        theme::card_frame(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Aucun service systemd ne correspond aux filtres actuels.")
                        .size(14.0)
                        .strong()
                        .color(p.text_primary),
                );
                ui.add_space(6.0);
                if theme::primary_button(ui, p, "Réinitialiser les filtres").clicked() {
                    reset_filters = true;
                }
                ui.add_space(12.0);
            });
        });
        if reset_filters {
            state.service_filter.clear();
            state.service_category = ServiceFilterCategory::All;
        }
        return None;
    }

    theme::card_frame(p).show(ui, |ui| {
        let outer_avail_w = ui.available_width();
        let avail_h = (ui.available_height() - 12.0).max(220.0);
        let table_w = outer_avail_w.max(880.0);

        egui::ScrollArea::horizontal()
            .id_salt("service_table_hscroll")
            .show(ui, |ui| {
                let max_rect =
                    egui::Rect::from_min_size(ui.cursor().min, egui::vec2(table_w, avail_h));
                ui.scope_builder(egui::UiBuilder::new().max_rect(max_rect), |ui| {
                    TableBuilder::new(ui)
                        .id_salt("service_table_grid")
                        .striped(true)
                        .resizable(true)
                        .auto_shrink([false, true])
                        .min_scrolled_height(0.0)
                        .max_scroll_height((avail_h - 32.0).max(160.0))
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::initial(195.0).at_least(125.0))
                        .column(Column::remainder().at_least(150.0))
                        .column(Column::initial(95.0).at_least(75.0))
                        .column(Column::initial(90.0).at_least(65.0))
                        .column(Column::initial(90.0).at_least(65.0))
                        .column(Column::exact(250.0))
                        .header(26.0, |mut header| {
                            header.col(|ui| {
                                let text = format!(
                                    "Nom du service{}",
                                    if active_col == ServiceSortColumn::Name {
                                        sort_indicator(active_dir)
                                    } else {
                                        ""
                                    }
                                );
                                if ui
                                    .selectable_label(active_col == ServiceSortColumn::Name, text)
                                    .clicked()
                                {
                                    sort_to_toggle = Some(ServiceSortColumn::Name);
                                }
                            });
                            header.col(|ui| {
                                let text = format!(
                                    "Description{}",
                                    if active_col == ServiceSortColumn::Description {
                                        sort_indicator(active_dir)
                                    } else {
                                        ""
                                    }
                                );
                                if ui
                                    .selectable_label(
                                        active_col == ServiceSortColumn::Description,
                                        text,
                                    )
                                    .clicked()
                                {
                                    sort_to_toggle = Some(ServiceSortColumn::Description);
                                }
                            });
                            header.col(|ui| {
                                let text = format!(
                                    "État{}",
                                    if active_col == ServiceSortColumn::ActiveState {
                                        sort_indicator(active_dir)
                                    } else {
                                        ""
                                    }
                                );
                                if ui
                                    .selectable_label(
                                        active_col == ServiceSortColumn::ActiveState,
                                        text,
                                    )
                                    .clicked()
                                {
                                    sort_to_toggle = Some(ServiceSortColumn::ActiveState);
                                }
                            });
                            header.col(|ui| {
                                let text = format!(
                                    "Sous-état{}",
                                    if active_col == ServiceSortColumn::SubState {
                                        sort_indicator(active_dir)
                                    } else {
                                        ""
                                    }
                                );
                                if ui
                                    .selectable_label(
                                        active_col == ServiceSortColumn::SubState,
                                        text,
                                    )
                                    .clicked()
                                {
                                    sort_to_toggle = Some(ServiceSortColumn::SubState);
                                }
                            });
                            header.col(|ui| {
                                let text = format!(
                                    "Chargement{}",
                                    if active_col == ServiceSortColumn::LoadState {
                                        sort_indicator(active_dir)
                                    } else {
                                        ""
                                    }
                                );
                                if ui
                                    .selectable_label(
                                        active_col == ServiceSortColumn::LoadState,
                                        text,
                                    )
                                    .clicked()
                                {
                                    sort_to_toggle = Some(ServiceSortColumn::LoadState);
                                }
                            });
                            header.col(|ui| {
                                ui.label(
                                    RichText::new("Actions").strong().color(p.text_secondary),
                                );
                            });
                        })
                        .body(|body| {
                            body.rows(28.0, services.len(), |mut row| {
                                let srv = services[row.index()];

                                row.col(|ui| {
                                    let lbl = ui.add(
                                        egui::Label::new(
                                            RichText::new(&srv.name)
                                                .strong()
                                                .size(12.0)
                                                .color(p.text_primary),
                                        )
                                        .truncate(),
                                    );
                                    lbl.on_hover_text(&srv.name);
                                });

                                row.col(|ui| {
                                    let desc = if srv.description.is_empty() {
                                        "—"
                                    } else {
                                        &srv.description
                                    };
                                    let lbl = ui.add(
                                        egui::Label::new(
                                            RichText::new(desc).color(p.text_secondary).size(12.0),
                                        )
                                        .truncate(),
                                    );
                                    lbl.on_hover_text(desc);
                                });

                                row.col(|ui| {
                                    let act = srv.active_state.to_lowercase();
                                    let (badge_text, bg, fg, border) =
                                        if act == "active" || act == "running" {
                                            (
                                                "● actif",
                                                p.success_bg,
                                                p.success,
                                                Some(p.success_border),
                                            )
                                        } else if act == "inactive" || act == "dead" {
                                            (
                                                "○ inactif",
                                                p.bg_subtle,
                                                p.text_muted,
                                                Some(p.border),
                                            )
                                        } else if act == "failed" {
                                            (
                                                "✖ échec",
                                                p.danger_bg,
                                                p.danger,
                                                Some(p.danger_border),
                                            )
                                        } else {
                                            (
                                                "• autre",
                                                p.warning_bg,
                                                p.warning,
                                                Some(p.warning_border),
                                            )
                                        };
                                    theme::pill_badge(ui, badge_text, bg, fg, border);
                                });

                                row.col(|ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&srv.sub_state)
                                                .color(p.text_secondary)
                                                .size(12.0),
                                        )
                                        .truncate(),
                                    );
                                });

                                row.col(|ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&srv.load_state)
                                                .color(p.text_secondary)
                                                .size(12.0),
                                        )
                                        .truncate(),
                                    );
                                });

                                row.col(|ui| {
                                    ui.push_id(&srv.name, |ui| {
                                        ui.horizontal(|ui| {
                                            let is_running = srv.active_state == "active"
                                                || srv.sub_state == "running";
                                            let s_name = srv.name.clone();

                                            if !is_running {
                                                if ui
                                                    .button("▶ Démarrer")
                                                    .on_hover_text(
                                                        "Démarrer ce service (systemctl start)",
                                                    )
                                                    .clicked()
                                                {
                                                    action =
                                                        Some(ProcessPanelAction::ControlService {
                                                            service_name: s_name.clone(),
                                                            action: "start".to_string(),
                                                        });
                                                }
                                            } else if ui
                                                .button("⏹ Arrêter")
                                                .on_hover_text(
                                                    "Arrêter ce service (avec confirmation)",
                                                )
                                                .clicked()
                                            {
                                                confirm_to_set = Some(
                                                    PendingProcessConfirm::ControlService {
                                                        service_name: s_name.clone(),
                                                        action: "stop".to_string(),
                                                    },
                                                );
                                            }

                                            if ui
                                                .button("🔄 Redémarrer")
                                                .on_hover_text(
                                                    "Redémarrer ce service (avec confirmation)",
                                                )
                                                .clicked()
                                            {
                                                confirm_to_set =
                                                    Some(PendingProcessConfirm::ControlService {
                                                        service_name: s_name.clone(),
                                                        action: "restart".to_string(),
                                                    });
                                            }

                                            ui.menu_button("⋯", |ui| {
                                                if ui.button("♻ Recharger (reload)").clicked() {
                                                    action =
                                                        Some(ProcessPanelAction::ControlService {
                                                            service_name: s_name.clone(),
                                                            action: "reload".to_string(),
                                                        });
                                                    ui.close_menu();
                                                }
                                                if ui.button("📜 Logs (journalctl)").clicked() {
                                                    action =
                                                        Some(ProcessPanelAction::ViewServiceLogs(
                                                            s_name.clone(),
                                                        ));
                                                    ui.close_menu();
                                                }
                                                if ui.button("⭐ Ajouter à Mes Services").clicked()
                                                {
                                                    action = Some(
                                                        ProcessPanelAction::AddFavoriteFromService(
                                                            srv.clone(),
                                                        ),
                                                    );
                                                    ui.close_menu();
                                                }
                                                ui.separator();
                                                if ui.button("✔ Activer au boot (enable)").clicked()
                                                {
                                                    confirm_to_set = Some(
                                                        PendingProcessConfirm::ControlService {
                                                            service_name: s_name.clone(),
                                                            action: "enable".to_string(),
                                                        },
                                                    );
                                                    ui.close_menu();
                                                }
                                                if ui
                                                    .button("✖ Désactiver au boot (disable)")
                                                    .clicked()
                                                {
                                                    confirm_to_set = Some(
                                                        PendingProcessConfirm::ControlService {
                                                            service_name: s_name,
                                                            action: "disable".to_string(),
                                                        },
                                                    );
                                                    ui.close_menu();
                                                }
                                            });
                                        });
                                    });
                                });
                            });
                        });
                });
            });
    });

    if let Some(col) = sort_to_toggle {
        state.toggle_service_sort(col);
    }

    if let Some(c) = confirm_to_set {
        state.pending_confirm = Some(c);
    }

    action
}

fn render_cpu_bar(ui: &mut Ui, p: &Palette, cpu: f32, cpu_count: u32) {
    let max_cpu = 100.0 * cpu_count.max(1) as f32;
    let fraction = (cpu / max_cpu).clamp(0.0, 1.0);
    let color = if cpu >= 50.0 {
        p.danger
    } else if cpu >= 15.0 {
        p.orange
    } else if cpu > 0.05 {
        p.success
    } else {
        p.text_muted
    };

    ui.horizontal(|ui| {
        let resp = theme::mini_progress_bar(ui, p, fraction, color, 28.0, 6.0);
        resp.on_hover_text(format!(
            "{cpu:.1}% (échelle sur {max_cpu:.0}% / {} cœur(s))",
            cpu_count.max(1)
        ));
        ui.label(
            RichText::new(format!("{cpu:.1}%"))
                .color(color)
                .strong()
                .size(11.0),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state() -> ProcessState {
        let mut st = ProcessState::new();
        st.processes = vec![
            ntx_proto::ProcessItem {
                pid: 10,
                name: "java".to_string(),
                cpu_usage: 45.0,
                memory_bytes: 1024 * 1024 * 1024,
                user: "mc".to_string(),
                status: "Running".to_string(),
                command_line: "java -Xmx4G -jar server.jar nogui".to_string(),
                parent_pid: 1,
            },
            ntx_proto::ProcessItem {
                pid: 20,
                name: "systemd-journald".to_string(),
                cpu_usage: 0.0,
                memory_bytes: 64 * 1024 * 1024,
                user: "root".to_string(),
                status: "Sleeping".to_string(),
                command_line: "/lib/systemd/systemd-journald".to_string(),
                parent_pid: 1,
            },
            ntx_proto::ProcessItem {
                pid: 30,
                name: "old-worker".to_string(),
                cpu_usage: 0.0,
                memory_bytes: 0,
                user: "app".to_string(),
                status: "Zombie".to_string(),
                command_line: String::new(),
                parent_pid: 10,
            },
        ];
        st.services = vec![
            ntx_proto::ServiceItem {
                name: "nginx.service".to_string(),
                description: "Nginx HTTP server".to_string(),
                load_state: "loaded".to_string(),
                active_state: "active".to_string(),
                sub_state: "running".to_string(),
            },
            ntx_proto::ServiceItem {
                name: "backup.service".to_string(),
                description: "Nightly backup".to_string(),
                load_state: "loaded".to_string(),
                active_state: "failed".to_string(),
                sub_state: "failed".to_string(),
            },
        ];
        st
    }

    #[test]
    fn test_kpi_column_count_breakpoints() {
        assert_eq!(kpi_column_count(1200.0), 4);
        assert_eq!(kpi_column_count(820.0), 4);
        assert_eq!(kpi_column_count(819.9), 2);
        assert_eq!(kpi_column_count(460.0), 2);
        assert_eq!(kpi_column_count(459.9), 1);
        assert_eq!(kpi_column_count(320.0), 1);
    }

    #[test]
    fn test_compute_process_kpi_summary() {
        let st = sample_state();
        let kpi = compute_process_kpi_summary(&st);
        assert_eq!(kpi.total, 3);
        assert_eq!(kpi.running, 1);
        assert_eq!(kpi.sleeping, 1);
        assert_eq!(kpi.zombies, 1);
        assert_eq!(kpi.active_cpu_count, 1);
        assert!((kpi.total_cpu_pct - 45.0).abs() < 0.01);
        assert_eq!(kpi.total_rss_bytes, (1024 + 64) * 1024 * 1024);
        assert_eq!(kpi.top_cpu.as_ref().map(|(n, _)| n.as_str()), Some("java"));
        assert_eq!(kpi.top_mem.as_ref().map(|(n, _)| n.as_str()), Some("java"));
    }

    #[test]
    fn test_process_panel_render_smoke_responsive() {
        let ctx = egui::Context::default();
        let mut state = sample_state();
        state.selected_pid = Some(10);
        state.set_status("Opération réussie", false);

        for width in [1280.0_f32, 640.0, 380.0] {
            for subtab in [ProcessSubTab::Processes, ProcessSubTab::Services] {
                state.active_subtab = subtab;
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 800.0),
                    )),
                    ..Default::default()
                };
                let _ = ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let _ = render(ui, &mut state, true, 4);
                    });
                });
            }
        }
    }
}
