use crate::state::{
    PendingProcessConfirm, ProcessFilterCategory, ProcessSortColumn, ProcessState, ProcessSubTab,
    ProcessViewMode, ServiceFilterCategory, ServiceSortColumn, SortDirection,
};
use egui::{Color32, RichText, Ui};
use egui_extras::{Column, TableBuilder};

/// Actions emitted by the Process & Services management view.
#[derive(Debug, Clone)]
pub enum ProcessPanelAction {
    RefreshProcesses,
    RefreshServices,
    KillProcess { pid: u32, signal: i32 },
    ControlService { service_name: String, action: String },
    AddFavoriteFromProcess(ntx_proto::ProcessItem),
}

/// Render the Process & Services management panel.
pub fn render(
    ui: &mut Ui,
    state: &mut ProcessState,
    is_connected: bool,
    cpu_count: u32,
) -> Option<ProcessPanelAction> {
    if !is_connected {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("Ce serveur est actuellement déconnecté.")
                    .size(16.0)
                    .strong(),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Connectez-vous pour inspecter et contrôler les processus et services système en direct.",
                )
                .weak(),
            );
        });
        return None;
    }

    let mut action: Option<ProcessPanelAction> = None;

    // 1. Top Sub-tab Bar & Global Controls
    ui.horizontal(|ui| {
        let proc_label = format!("⚡ Processus ({})", state.processes.len());
        if ui
            .selectable_label(state.active_subtab == ProcessSubTab::Processes, proc_label)
            .clicked()
        {
            state.active_subtab = ProcessSubTab::Processes;
            if state.processes.is_empty() && !state.is_loading_processes {
                action = Some(ProcessPanelAction::RefreshProcesses);
            }
        }

        let srv_label = format!("⚙ Services système ({})", state.services.len());
        if ui
            .selectable_label(state.active_subtab == ProcessSubTab::Services, srv_label)
            .clicked()
        {
            state.active_subtab = ProcessSubTab::Services;
            if state.services.is_empty() && !state.is_loading_services {
                action = Some(ProcessPanelAction::RefreshServices);
            }
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("🔄 Actualiser")
                .on_hover_text("Actualiser les données depuis le serveur")
                .clicked()
            {
                match state.active_subtab {
                    ProcessSubTab::Processes => action = Some(ProcessPanelAction::RefreshProcesses),
                    ProcessSubTab::Services => action = Some(ProcessPanelAction::RefreshServices),
                }
            }

            if state.active_subtab == ProcessSubTab::Processes {
                if state.auto_refresh {
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

                ui.checkbox(&mut state.auto_refresh, "⚡ Auto (3s)").on_hover_text(
                    "Actualise automatiquement la liste des processus toutes les 3 secondes",
                );
            }

            if (state.active_subtab == ProcessSubTab::Processes && state.is_loading_processes)
                || (state.active_subtab == ProcessSubTab::Services && state.is_loading_services)
            {
                ui.spinner();
            }
        });
    });

    ui.add_space(4.0);

    // 2. Status Notification Banner (if any)
    if let Some((msg, is_error)) = &state.status_message.clone() {
        let (bg, fg) = if *is_error {
            (Color32::from_rgb(70, 25, 25), Color32::from_rgb(250, 100, 100))
        } else {
            (Color32::from_rgb(25, 60, 35), Color32::from_rgb(100, 240, 140))
        };
        egui::Frame::new()
            .fill(bg)
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(if *is_error { "⚠" } else { "✔" })
                            .color(fg)
                            .strong(),
                    );
                    ui.label(RichText::new(msg).color(fg));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✖").clicked() {
                            state.clear_status();
                        }
                    });
                });
            });
        ui.add_space(4.0);
    }

    // 3. KPI Statistics Banner
    match state.active_subtab {
        ProcessSubTab::Processes => render_process_kpi_banner(ui, state),
        ProcessSubTab::Services => render_service_kpi_banner(ui, state),
    }

    ui.add_space(4.0);

    // 4. Filter, compute filtered slice once, and render table
    match state.active_subtab {
        ProcessSubTab::Processes => {
            let filtered_count = state.filtered_processes().len();
            render_process_filter_toolbar(ui, state, filtered_count);
            ui.add_space(4.0);
            if let Some(act) = render_processes_table(ui, state, cpu_count) {
                action = Some(act);
            }
            if let Some(act) = render_process_inspector(ui, state) {
                action = Some(act);
            }
        }
        ProcessSubTab::Services => {
            let filtered_count = state.filtered_services().len();
            render_service_filter_toolbar(ui, state, filtered_count);
            ui.add_space(4.0);
            if let Some(act) = render_services_table(ui, state) {
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
                                .color(Color32::from_rgb(255, 110, 110))
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
                    }
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(
                            RichText::new("✅ Confirmer")
                                .color(Color32::from_rgb(255, 120, 120))
                                .strong(),
                        )
                        .clicked()
                    {
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

fn render_process_kpi_banner(ui: &mut Ui, state: &ProcessState) {
    if state.processes.is_empty() {
        return;
    }

    let (total, running, sleeping, zombies) = state.process_counts();
    let top_cpu = state.top_cpu_process();
    let top_mem = state.top_mem_process();

    egui::Frame::new()
        .fill(Color32::from_rgb(22, 25, 32))
        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(45, 50, 62)))
        .corner_radius(5.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("📊 Total: {total}")).strong().size(12.0));
                ui.separator();

                ui.label(
                    RichText::new(format!("⚡ En cours: {running}"))
                        .color(Color32::from_rgb(46, 204, 113))
                        .strong()
                        .size(12.0),
                );
                ui.separator();

                ui.label(
                    RichText::new(format!("💤 En veille: {sleeping}"))
                        .color(Color32::from_rgb(140, 155, 170))
                        .size(12.0),
                );
                ui.separator();

                let zombie_color = if zombies > 0 {
                    Color32::from_rgb(231, 76, 60)
                } else {
                    Color32::from_rgb(120, 130, 140)
                };
                ui.label(
                    RichText::new(format!("💀 Zombies: {zombies}"))
                        .color(zombie_color)
                        .size(12.0),
                );

                if let Some((name, cpu)) = top_cpu {
                    if cpu > 0.05 {
                        ui.separator();
                        ui.label(
                            RichText::new(format!("🔥 Top CPU: {name} ({cpu:.1}%)"))
                                .color(Color32::from_rgb(240, 160, 50))
                                .size(12.0),
                        );
                    }
                }

                if let Some((name, mem)) = top_mem {
                    if mem > 0 {
                        ui.separator();
                        ui.label(
                            RichText::new(format!("💾 Top RAM: {name} ({})", format_ram(mem)))
                                .color(Color32::from_rgb(100, 180, 240))
                                .size(12.0),
                        );
                    }
                }
            });
        });
}

fn render_service_kpi_banner(ui: &mut Ui, state: &ProcessState) {
    if state.services.is_empty() {
        return;
    }

    let (total, active, inactive, failed) = state.service_counts();

    egui::Frame::new()
        .fill(Color32::from_rgb(22, 25, 32))
        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(45, 50, 62)))
        .corner_radius(5.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("📊 Total: {total}")).strong().size(12.0));
                ui.separator();

                ui.label(
                    RichText::new(format!("🟢 Actifs: {active}"))
                        .color(Color32::from_rgb(46, 204, 113))
                        .strong()
                        .size(12.0),
                );
                ui.separator();

                ui.label(
                    RichText::new(format!("⚪ Inactifs: {inactive}"))
                        .color(Color32::from_rgb(140, 155, 170))
                        .size(12.0),
                );
                ui.separator();

                let failed_color = if failed > 0 {
                    Color32::from_rgb(231, 76, 60)
                } else {
                    Color32::from_rgb(120, 130, 140)
                };
                ui.label(
                    RichText::new(format!("🔴 En échec: {failed}"))
                        .color(failed_color)
                        .strong()
                        .size(12.0),
                );
            });
        });
}

fn render_process_filter_toolbar(ui: &mut Ui, state: &mut ProcessState, filtered_count: usize) {
    ui.horizontal_wrapped(|ui| {
        ui.label("🔍");
        ui.add(
            egui::TextEdit::singleline(&mut state.process_filter)
                .hint_text("Filtrer (nom, PID, utilisateur, commande...)")
                .desired_width(220.0),
        );
        if !state.process_filter.is_empty() && ui.small_button("✖").clicked() {
            state.process_filter.clear();
        }

        ui.separator();

        if ui
            .selectable_label(state.process_category == ProcessFilterCategory::All, "Tous")
            .clicked()
        {
            state.process_category = ProcessFilterCategory::All;
        }
        if ui
            .selectable_label(
                state.process_category == ProcessFilterCategory::ActiveOnly,
                "⚡ Actifs (>0%)",
            )
            .on_hover_text("Afficher uniquement les processus consommant du CPU")
            .clicked()
        {
            state.process_category = ProcessFilterCategory::ActiveOnly;
        }
        if ui
            .selectable_label(
                state.process_category == ProcessFilterCategory::UserOnly,
                "👤 Utilisateur",
            )
            .on_hover_text("Afficher les processus non-système (hors root / daemons)")
            .clicked()
        {
            state.process_category = ProcessFilterCategory::UserOnly;
        }
        if ui
            .selectable_label(
                state.process_category == ProcessFilterCategory::SystemOnly,
                "🛡 Système",
            )
            .on_hover_text("Afficher les processus système / root")
            .clicked()
        {
            state.process_category = ProcessFilterCategory::SystemOnly;
        }

        ui.separator();

        let is_tree = state.view_mode == ProcessViewMode::Tree;
        if ui
            .selectable_label(is_tree, "🌲 Arborescence")
            .on_hover_text("Regrouper les processus enfants sous leur parent PID")
            .clicked()
        {
            state.view_mode = ProcessViewMode::Tree;
        }
        if ui
            .selectable_label(!is_tree, "📋 Liste plate")
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

        ui.separator();

        ui.label(
            RichText::new(format!("{filtered_count}/{} affichés", state.processes.len()))
                .weak()
                .size(11.0),
        );
    });
}

fn render_service_filter_toolbar(ui: &mut Ui, state: &mut ProcessState, filtered_count: usize) {
    ui.horizontal_wrapped(|ui| {
        ui.label("🔍");
        ui.add(
            egui::TextEdit::singleline(&mut state.service_filter)
                .hint_text("Filtrer par nom de service, description...")
                .desired_width(220.0),
        );
        if !state.service_filter.is_empty() && ui.small_button("✖").clicked() {
            state.service_filter.clear();
        }

        ui.separator();

        if ui
            .selectable_label(state.service_category == ServiceFilterCategory::All, "Tous")
            .clicked()
        {
            state.service_category = ServiceFilterCategory::All;
        }
        if ui
            .selectable_label(
                state.service_category == ServiceFilterCategory::ActiveOnly,
                "🟢 Actifs",
            )
            .clicked()
        {
            state.service_category = ServiceFilterCategory::ActiveOnly;
        }
        if ui
            .selectable_label(
                state.service_category == ServiceFilterCategory::InactiveOnly,
                "⚪ Inactifs",
            )
            .clicked()
        {
            state.service_category = ServiceFilterCategory::InactiveOnly;
        }
        if ui
            .selectable_label(
                state.service_category == ServiceFilterCategory::FailedOnly,
                "🔴 En échec",
            )
            .clicked()
        {
            state.service_category = ServiceFilterCategory::FailedOnly;
        }

        ui.separator();

        ui.label(
            RichText::new(format!("{filtered_count}/{} affichés", state.services.len()))
                .weak()
                .size(11.0),
        );
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
    state: &mut ProcessState,
    cpu_count: u32,
) -> Option<ProcessPanelAction> {
    if state.processes.is_empty() && !state.is_loading_processes {
        ui.add_space(40.0);
        let mut load_clicked = false;
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Aucun processus chargé pour l'instant.").weak());
            ui.add_space(8.0);
            if ui.button("⚡ Charger la liste des processus").clicked() {
                load_clicked = true;
            }
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

    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(70.0).at_least(50.0))
        .column(Column::initial(220.0).at_least(140.0))
        .column(Column::initial(85.0).at_least(60.0))
        .column(Column::initial(110.0).at_least(85.0))
        .column(Column::initial(90.0).at_least(70.0))
        .column(Column::initial(85.0).at_least(60.0))
        .column(Column::remainder().at_least(140.0))
        .column(Column::exact(165.0))
        .header(24.0, |mut header| {
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
                    .selectable_label(active_col == ProcessSortColumn::CommandLine, text)
                    .clicked()
                {
                    sort_to_toggle = Some(ProcessSortColumn::CommandLine);
                }
            });
            header.col(|ui| {
                ui.label(RichText::new("Actions").strong());
            });
        })
        .body(|body| {
            body.rows(26.0, tree_rows.len(), |mut row| {
                let tree_row = &tree_rows[row.index()];
                let proc = tree_row.item;
                let is_selected = state.selected_pid == Some(proc.pid);

                row.col(|ui| {
                    let pid_text = if is_selected {
                        RichText::new(proc.pid.to_string())
                            .color(Color32::from_rgb(100, 200, 255))
                            .strong()
                    } else {
                        RichText::new(proc.pid.to_string()).weak()
                    };
                    let resp = ui.selectable_label(is_selected, pid_text);
                    if resp.clicked() {
                        pid_to_select = Some(if is_selected { None } else { Some(proc.pid) });
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
                                    let icon = if tree_row.is_collapsed { "▶" } else { "▼" };
                                    let badge_label = format!("{icon} {}", tree_row.children_count);
                                    let btn = egui::Button::new(
                                        RichText::new(badge_label)
                                            .size(11.0)
                                            .strong()
                                            .color(if tree_row.is_collapsed {
                                                Color32::from_rgb(100, 200, 255)
                                            } else {
                                                Color32::from_rgb(180, 200, 220)
                                            }),
                                    )
                                    .fill(if tree_row.is_collapsed {
                                        Color32::from_rgb(32, 48, 70)
                                    } else {
                                        Color32::from_rgb(28, 34, 44)
                                    })
                                    .corner_radius(3.0);

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
                                    ui.label(RichText::new("└─").weak().size(11.0));
                                }
                            }

                            let name_label = if is_selected {
                                RichText::new(&proc.name)
                                    .color(Color32::from_rgb(100, 200, 255))
                                    .strong()
                            } else {
                                RichText::new(&proc.name).strong()
                            };

                            let resp = ui.selectable_label(is_selected, name_label);
                            if resp.clicked() {
                                pid_to_select =
                                    Some(if is_selected { None } else { Some(proc.pid) });
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
                    let user_display = if proc.user.is_empty() { "-" } else { &proc.user };
                    ui.label(RichText::new(user_display).weak().size(12.0));
                });

                row.col(|ui| {
                    render_cpu_bar(ui, proc.cpu_usage, cpu_count);
                    if tree_row.is_collapsed && tree_row.has_children {
                        ui.label(
                            RichText::new(format!("Σ{:.1}%", tree_row.total_cpu))
                                .size(10.0)
                                .weak(),
                        )
                        .on_hover_text(format!(
                            "Total avec {} enfants: {:.1}%",
                            tree_row.children_count, tree_row.total_cpu
                        ));
                    }
                });

                row.col(|ui| {
                    let mem_str = format_ram(proc.memory_bytes);
                    ui.label(RichText::new(mem_str).size(12.0));
                    if tree_row.is_collapsed && tree_row.has_children {
                        ui.label(
                            RichText::new(format!("Σ{}", format_ram(tree_row.total_memory)))
                                .size(10.0)
                                .weak(),
                        )
                        .on_hover_text(format!(
                            "Total avec {} enfants: {}",
                            tree_row.children_count,
                            format_ram(tree_row.total_memory)
                        ));
                    }
                });

                row.col(|ui| {
                    let st = proc.status.to_lowercase();
                    let (badge_text, color) = if st.contains("run") {
                        ("● Actif", Color32::from_rgb(46, 204, 113))
                    } else if st.contains("sleep") || st.contains("idle") {
                        ("○ Veille", Color32::from_rgb(140, 150, 160))
                    } else if st.contains("stop") {
                        ("■ Stop", Color32::from_rgb(240, 160, 40))
                    } else if st.contains("zomb") || st.contains("dead") {
                        ("✕ Zombie", Color32::from_rgb(231, 76, 60))
                    } else {
                        ("◌ Autre", Color32::from_rgb(180, 180, 180))
                    };
                    ui.label(RichText::new(badge_text).color(color).size(11.0).strong())
                        .on_hover_text(&proc.status);
                });

                row.col(|ui| {
                    let cmd = if proc.command_line.is_empty() {
                        &proc.name
                    } else {
                        &proc.command_line
                    };
                    let lbl = ui.add(
                        egui::Label::new(RichText::new(cmd).monospace().size(11.0).weak())
                            .truncate(),
                    );
                    lbl.on_hover_text(cmd);
                });

                row.col(|ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .button("⭐")
                            .on_hover_text("Épingler dans l'onglet Applications & Favoris")
                            .clicked()
                        {
                            action = Some(ProcessPanelAction::AddFavoriteFromProcess(proc.clone()));
                        }

                        if ui
                            .button("Arrêter")
                            .on_hover_text("Envoyer SIGTERM (15) avec confirmation")
                            .clicked()
                        {
                            confirm_to_set = Some(PendingProcessConfirm::KillProcess {
                                pid: proc.pid,
                                name: proc.name.clone(),
                                signal: 15,
                            });
                        }

                        if ui
                            .button(RichText::new("Tuer").color(Color32::from_rgb(255, 100, 100)))
                            .on_hover_text("Envoyer SIGKILL (9) avec confirmation")
                            .clicked()
                        {
                            confirm_to_set = Some(PendingProcessConfirm::KillProcess {
                                pid: proc.pid,
                                name: proc.name.clone(),
                                signal: 9,
                            });
                        }
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
    state: &mut ProcessState,
) -> Option<ProcessPanelAction> {
    let pid = state.selected_pid?;
    let proc = state.processes.iter().find(|p| p.pid == pid)?.clone();

    let mut action: Option<ProcessPanelAction> = None;
    let mut close_inspector = false;
    let mut select_parent_pid: Option<u32> = None;

    ui.add_space(6.0);

    egui::Frame::new()
        .fill(Color32::from_rgb(20, 24, 30))
        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(60, 75, 100)))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("🔍 Inspecteur: {} (PID: {})", proc.name, proc.pid))
                        .strong()
                        .size(14.0)
                        .color(Color32::from_rgb(100, 200, 255)),
                );

                if proc.parent_pid > 0 {
                    ui.label(RichText::new(format!("PPID: {}", proc.parent_pid)).weak().size(12.0));
                    if ui
                        .small_button("Inspecter parent")
                        .on_hover_text("Basculer vers le processus parent")
                        .clicked()
                    {
                        select_parent_pid = Some(proc.parent_pid);
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✖ Fermer").clicked() {
                        close_inspector = true;
                    }
                });
            });

            ui.add_space(4.0);

            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!(
                        "Utilisateur: {}",
                        if proc.user.is_empty() { "-" } else { &proc.user }
                    ))
                    .weak()
                    .size(12.0),
                );
                ui.separator();
                ui.label(RichText::new(format!("Statut: {}", proc.status)).weak().size(12.0));
                ui.separator();
                ui.label(
                    RichText::new(format!("CPU: {:.1}%", proc.cpu_usage))
                        .strong()
                        .size(12.0)
                        .color(Color32::from_rgb(240, 160, 50)),
                );
                ui.separator();
                ui.label(
                    RichText::new(format!("Mémoire: {}", format_ram(proc.memory_bytes)))
                        .strong()
                        .size(12.0)
                        .color(Color32::from_rgb(100, 180, 240)),
                );
            });

            ui.add_space(4.0);

            let cmd_text = if proc.command_line.is_empty() {
                proc.name.clone()
            } else {
                proc.command_line.clone()
            };

            ui.horizontal(|ui| {
                ui.label(RichText::new("Ligne de commande:").weak().size(11.0));
                if ui
                    .small_button("📋 Copier")
                    .on_hover_text("Copier la ligne de commande complète dans le presse-papier")
                    .clicked()
                {
                    ui.ctx().copy_text(cmd_text.clone());
                    state.set_status("Ligne de commande copiée dans le presse-papier.", false);
                }
            });

            egui::Frame::new()
                .fill(Color32::from_rgb(12, 14, 18))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(&cmd_text)
                            .monospace()
                            .size(11.0)
                            .color(Color32::from_rgb(200, 210, 225)),
                    );
                });

            ui.add_space(4.0);

            ui.horizontal(|ui| {
                if ui
                    .button("⭐ Mettre en favori")
                    .on_hover_text("Épingler ce processus dans l'onglet Applications & Favoris")
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

                if ui
                    .button(
                        RichText::new("🛑 Forcer l'arrêt (SIGKILL 9)")
                            .color(Color32::from_rgb(255, 100, 100)),
                    )
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

fn render_services_table(ui: &mut Ui, state: &mut ProcessState) -> Option<ProcessPanelAction> {
    if state.services.is_empty() && !state.is_loading_services {
        ui.add_space(40.0);
        let mut load_clicked = false;
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Aucun service système chargé pour l'instant.").weak());
            ui.add_space(8.0);
            if ui.button("⚡ Charger la liste des services").clicked() {
                load_clicked = true;
            }
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

    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(180.0).at_least(120.0))
        .column(Column::remainder().at_least(150.0))
        .column(Column::initial(100.0).at_least(75.0))
        .column(Column::initial(90.0).at_least(65.0))
        .column(Column::initial(90.0).at_least(65.0))
        .column(Column::exact(220.0))
        .header(24.0, |mut header| {
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
                    .selectable_label(active_col == ServiceSortColumn::Description, text)
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
                    .selectable_label(active_col == ServiceSortColumn::ActiveState, text)
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
                    .selectable_label(active_col == ServiceSortColumn::SubState, text)
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
                    .selectable_label(active_col == ServiceSortColumn::LoadState, text)
                    .clicked()
                {
                    sort_to_toggle = Some(ServiceSortColumn::LoadState);
                }
            });
            header.col(|ui| {
                ui.label(RichText::new("Actions").strong());
            });
        })
        .body(|body| {
            body.rows(26.0, services.len(), |mut row| {
                let srv = services[row.index()];

                row.col(|ui| {
                    let lbl = ui.add(
                        egui::Label::new(RichText::new(&srv.name).strong().size(12.0)).truncate(),
                    );
                    lbl.on_hover_text(&srv.name);
                });

                row.col(|ui| {
                    let desc = if srv.description.is_empty() {
                        "-"
                    } else {
                        &srv.description
                    };
                    let lbl =
                        ui.add(egui::Label::new(RichText::new(desc).weak().size(12.0)).truncate());
                    lbl.on_hover_text(desc);
                });

                row.col(|ui| {
                    let act = srv.active_state.to_lowercase();
                    let (badge_text, bg, fg) = if act == "active" || act == "running" {
                        (
                            "● actif",
                            Color32::from_rgb(26, 60, 40),
                            Color32::from_rgb(46, 204, 113),
                        )
                    } else if act == "inactive" || act == "dead" {
                        (
                            "○ inactif",
                            Color32::from_rgb(40, 42, 48),
                            Color32::from_rgb(140, 145, 155),
                        )
                    } else if act == "failed" {
                        (
                            "✕ échec",
                            Color32::from_rgb(60, 25, 25),
                            Color32::from_rgb(231, 76, 60),
                        )
                    } else {
                        (
                            "◌ autre",
                            Color32::from_rgb(60, 45, 20),
                            Color32::from_rgb(241, 196, 15),
                        )
                    };

                    egui::Frame::new()
                        .fill(bg)
                        .corner_radius(3.0)
                        .inner_margin(egui::Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.label(RichText::new(badge_text).color(fg).size(11.0).strong());
                        });
                });

                row.col(|ui| {
                    ui.label(RichText::new(&srv.sub_state).weak().size(12.0));
                });

                row.col(|ui| {
                    ui.label(RichText::new(&srv.load_state).weak().size(12.0));
                });

                row.col(|ui| {
                    ui.horizontal(|ui| {
                        let is_running =
                            srv.active_state == "active" || srv.sub_state == "running";
                        let s_name = srv.name.clone();

                        if !is_running {
                            if ui
                                .button("Démarrer")
                                .on_hover_text("Démarrer ce service (systemctl start)")
                                .clicked()
                            {
                                action = Some(ProcessPanelAction::ControlService {
                                    service_name: s_name.clone(),
                                    action: "start".to_string(),
                                });
                            }
                        } else if ui
                            .button("Arrêter")
                            .on_hover_text("Arrêter ce service (avec confirmation)")
                            .clicked()
                        {
                            confirm_to_set = Some(PendingProcessConfirm::ControlService {
                                service_name: s_name.clone(),
                                action: "stop".to_string(),
                            });
                        }

                        if ui
                            .button("Redémarrer")
                            .on_hover_text("Redémarrer ce service (avec confirmation)")
                            .clicked()
                        {
                            confirm_to_set = Some(PendingProcessConfirm::ControlService {
                                service_name: s_name,
                                action: "restart".to_string(),
                            });
                        }
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

fn render_cpu_bar(ui: &mut Ui, cpu: f32, cpu_count: u32) {
    let max_cpu = 100.0 * cpu_count.max(1) as f32;
    let fraction = (cpu / max_cpu).clamp(0.0, 1.0);
    let color = if cpu >= 50.0 {
        Color32::from_rgb(235, 75, 75)
    } else if cpu >= 15.0 {
        Color32::from_rgb(240, 160, 40)
    } else if cpu > 0.05 {
        Color32::from_rgb(46, 204, 113)
    } else {
        Color32::from_rgb(120, 130, 140)
    };

    ui.horizontal(|ui| {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(28.0, 7.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 2.0, Color32::from_rgb(35, 38, 46));
        if fraction > 0.0 {
            let filled_width = (rect.width() * fraction).max(2.0);
            let filled_rect =
                egui::Rect::from_min_size(rect.min, egui::vec2(filled_width, rect.height()));
            ui.painter().rect_filled(filled_rect, 2.0, color);
        }
        response.on_hover_text(format!(
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

fn format_ram(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 {
        format!("{:.2} Go", mb / 1024.0)
    } else {
        format!("{:.1} Mo", mb)
    }
}
