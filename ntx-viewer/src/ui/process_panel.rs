use crate::state::{
    ProcessSortColumn, ProcessState, ProcessSubTab, ServiceSortColumn, SortDirection,
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
}

/// Render the Process & Services management panel.
pub fn render(
    ui: &mut Ui,
    state: &mut ProcessState,
    is_connected: bool,
) -> Option<ProcessPanelAction> {
    if !is_connected {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Ce serveur est actuellement déconnecté.").size(16.0).strong());
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
            // Refresh button
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

            // Auto-refresh toggle (for processes)
            if state.active_subtab == ProcessSubTab::Processes {
                ui.checkbox(&mut state.auto_refresh, "⚡ Auto (3s)")
                    .on_hover_text("Actualise automatiquement la liste des processus toutes les 3 secondes");
            }

            // Spinner if active loading
            if (state.active_subtab == ProcessSubTab::Processes && state.is_loading_processes)
                || (state.active_subtab == ProcessSubTab::Services && state.is_loading_services)
            {
                ui.spinner();
            }
        });
    });

    ui.add_space(4.0);

    // 2. Search filter bar
    ui.horizontal(|ui| {
        ui.label("🔍");
        match state.active_subtab {
            ProcessSubTab::Processes => {
                ui.add(
                    egui::TextEdit::singleline(&mut state.process_filter)
                        .hint_text("Filtrer par nom, PID, utilisateur, commande...")
                        .desired_width(280.0),
                );
                if !state.process_filter.is_empty() && ui.small_button("✖").clicked() {
                    state.process_filter.clear();
                }
                let filtered_count = state.filtered_processes().len();
                ui.label(
                    RichText::new(format!(
                        "{filtered_count} / {} processus affichés",
                        state.processes.len()
                    ))
                    .weak()
                    .size(12.0),
                );
            }
            ProcessSubTab::Services => {
                ui.add(
                    egui::TextEdit::singleline(&mut state.service_filter)
                        .hint_text("Filtrer par nom de service, description...")
                        .desired_width(280.0),
                );
                if !state.service_filter.is_empty() && ui.small_button("✖").clicked() {
                    state.service_filter.clear();
                }
                let filtered_count = state.filtered_services().len();
                ui.label(
                    RichText::new(format!(
                        "{filtered_count} / {} services affichés",
                        state.services.len()
                    ))
                    .weak()
                    .size(12.0),
                );
            }
        }
    });

    ui.add_space(4.0);

    // 3. Status Notification Banner (if any)
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

    // 4. Main Body Table
    match state.active_subtab {
        ProcessSubTab::Processes => {
            if let Some(act) = render_processes_table(ui, state) {
                action = Some(act);
            }
        }
        ProcessSubTab::Services => {
            if let Some(act) = render_services_table(ui, state) {
                action = Some(act);
            }
        }
    }

    action
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

    let active_col = state.process_sort_column;
    let active_dir = state.process_sort_direction;

    // Filter and sort items
    let processes = state.filtered_processes();

    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(70.0).at_least(50.0))  // PID
        .column(Column::initial(160.0).at_least(100.0)) // Name
        .column(Column::initial(90.0).at_least(60.0))   // User
        .column(Column::initial(75.0).at_least(60.0))   // CPU %
        .column(Column::initial(90.0).at_least(70.0))   // RAM MB
        .column(Column::initial(80.0).at_least(60.0))   // Status
        .column(Column::remainder().at_least(150.0))    // Command Line
        .column(Column::exact(130.0))                   // Actions
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
                if ui.selectable_label(active_col == ProcessSortColumn::Pid, text).clicked() {
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
                if ui.selectable_label(active_col == ProcessSortColumn::Name, text).clicked() {
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
                if ui.selectable_label(active_col == ProcessSortColumn::User, text).clicked() {
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
                if ui.selectable_label(active_col == ProcessSortColumn::Cpu, text).clicked() {
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
                if ui.selectable_label(active_col == ProcessSortColumn::Memory, text).clicked() {
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
                if ui.selectable_label(active_col == ProcessSortColumn::Status, text).clicked() {
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
            body.rows(26.0, processes.len(), |mut row| {
                let proc = processes[row.index()];

                // Col 0: PID
                row.col(|ui| {
                    ui.label(RichText::new(proc.pid.to_string()).weak());
                });

                // Col 1: Name
                row.col(|ui| {
                    ui.label(RichText::new(&proc.name).strong());
                });

                // Col 2: User
                row.col(|ui| {
                    let user_display = if proc.user.is_empty() { "-" } else { &proc.user };
                    ui.label(RichText::new(user_display).weak().size(12.0));
                });

                // Col 3: CPU %
                row.col(|ui| {
                    let cpu_color = if proc.cpu_usage > 50.0 {
                        Color32::from_rgb(255, 90, 90)
                    } else if proc.cpu_usage > 15.0 {
                        Color32::from_rgb(255, 180, 50)
                    } else {
                        Color32::from_rgb(180, 220, 180)
                    };
                    ui.label(
                        RichText::new(format!("{:.1}%", proc.cpu_usage))
                            .color(cpu_color)
                            .strong(),
                    );
                });

                // Col 4: RAM
                row.col(|ui| {
                    let mb = proc.memory_bytes as f64 / (1024.0 * 1024.0);
                    let mem_str = if mb >= 1024.0 {
                        format!("{:.2} Go", mb / 1024.0)
                    } else {
                        format!("{:.1} Mo", mb)
                    };
                    ui.label(RichText::new(mem_str).size(12.0));
                });

                // Col 5: Status
                row.col(|ui| {
                    let st = proc.status.to_lowercase();
                    let color = if st.contains("run") {
                        Color32::from_rgb(46, 204, 113)
                    } else if st.contains("sleep") || st.contains("idle") {
                        Color32::from_rgb(140, 150, 160)
                    } else if st.contains("stop") || st.contains("zomb") {
                        Color32::from_rgb(231, 76, 60)
                    } else {
                        Color32::from_rgb(200, 200, 200)
                    };
                    ui.label(RichText::new(&proc.status).color(color).size(12.0));
                });

                // Col 6: Command Line
                row.col(|ui| {
                    let cmd = if proc.command_line.is_empty() {
                        &proc.name
                    } else {
                        &proc.command_line
                    };
                    let lbl = ui.add(
                        egui::Label::new(
                            RichText::new(cmd).monospace().size(11.0).weak(),
                        )
                        .truncate(),
                    );
                    lbl.on_hover_text(cmd);
                });

                // Col 7: Actions (SIGTERM / SIGKILL)
                row.col(|ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .button("Arrêter")
                            .on_hover_text("Envoyer SIGTERM (15) pour terminer le processus proprement")
                            .clicked()
                        {
                            action = Some(ProcessPanelAction::KillProcess {
                                pid: proc.pid,
                                signal: 15,
                            });
                        }

                        if ui
                            .button(RichText::new("Tuer").color(Color32::from_rgb(255, 100, 100)))
                            .on_hover_text("Envoyer SIGKILL (9) pour forcer l'arrêt immédiat")
                            .clicked()
                        {
                            action = Some(ProcessPanelAction::KillProcess {
                                pid: proc.pid,
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

    action
}

fn render_services_table(
    ui: &mut Ui,
    state: &mut ProcessState,
) -> Option<ProcessPanelAction> {
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

    let active_col = state.service_sort_column;
    let active_dir = state.service_sort_direction;

    // Filter and sort items
    let services = state.filtered_services();

    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(180.0).at_least(120.0)) // Service Name
        .column(Column::remainder().at_least(150.0))    // Description
        .column(Column::initial(100.0).at_least(75.0))  // Active State (badge)
        .column(Column::initial(90.0).at_least(65.0))   // Sub State
        .column(Column::initial(90.0).at_least(65.0))   // Load State
        .column(Column::exact(220.0))                   // Actions
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
                if ui.selectable_label(active_col == ServiceSortColumn::Name, text).clicked() {
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

                // Col 0: Name
                row.col(|ui| {
                    let lbl = ui.add(
                        egui::Label::new(RichText::new(&srv.name).strong().size(12.0)).truncate(),
                    );
                    lbl.on_hover_text(&srv.name);
                });

                // Col 1: Description
                row.col(|ui| {
                    let desc = if srv.description.is_empty() {
                        "-"
                    } else {
                        &srv.description
                    };
                    let lbl = ui.add(egui::Label::new(RichText::new(desc).weak().size(12.0)).truncate());
                    lbl.on_hover_text(desc);
                });

                // Col 2: Active State Badge
                row.col(|ui| {
                    let act = srv.active_state.to_lowercase();
                    let (badge_text, bg, fg) = if act == "active" || act == "running" {
                        (
                            "● active",
                            Color32::from_rgb(26, 60, 40),
                            Color32::from_rgb(46, 204, 113),
                        )
                    } else if act == "inactive" || act == "dead" {
                        (
                            "○ inactive",
                            Color32::from_rgb(40, 42, 48),
                            Color32::from_rgb(140, 145, 155),
                        )
                    } else if act == "failed" {
                        (
                            "✕ failed",
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

                // Col 3: Sub State
                row.col(|ui| {
                    ui.label(RichText::new(&srv.sub_state).weak().size(12.0));
                });

                // Col 4: Load State
                row.col(|ui| {
                    ui.label(RichText::new(&srv.load_state).weak().size(12.0));
                });

                // Col 5: Action Buttons (Démarrer, Arrêter, Redémarrer)
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
                        } else {
                            if ui
                                .button("Arrêter")
                                .on_hover_text("Arrêter ce service (systemctl stop)")
                                .clicked()
                            {
                                action = Some(ProcessPanelAction::ControlService {
                                    service_name: s_name.clone(),
                                    action: "stop".to_string(),
                                });
                            }
                        }

                        if ui
                            .button("Redémarrer")
                            .on_hover_text("Redémarrer ce service (systemctl restart)")
                            .clicked()
                        {
                            action = Some(ProcessPanelAction::ControlService {
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

    action
}
