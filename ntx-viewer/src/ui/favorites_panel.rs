use crate::state::{Device, FavoriteApp, FavoriteAppType};
use crate::ui::theme::{self, Palette};
use egui::{Color32, RichText, Stroke, Ui};

/// Actions that can be triggered from the Favorites management view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FavoritesPanelAction {
    Refresh,
    ClearStatus,
    AttachTerminal { command: String },
    KillProcess { pid: u32, signal: i32 },
    ControlService { service_name: String, action: String },
    SaveFavorite(FavoriteApp),
    RemoveFavorite(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingFavConfirm {
    KillProcess { name: String, pid: u32, signal: i32 },
    ControlService { service_name: String, action: String },
    RemoveFavorite { id: String, name: String },
}

/// Local UI state for the Favorites tab.
#[derive(Debug, Clone)]
pub struct FavoritesPanelState {
    pub show_add_modal: bool,
    pub edit_id: Option<String>,
    pub name_input: String,
    pub icon_input: String,
    pub app_type_input: FavoriteAppType,
    pub process_filter_input: String,
    pub service_name_input: String,
    pub terminal_command_input: String,
    pub description_input: String,
    pub auto_start_command_input: String,
    pub selected_from_proc_pid: Option<u32>,
    pub search_query: String,
    pub pending_confirm: Option<PendingFavConfirm>,
}

impl Default for FavoritesPanelState {
    fn default() -> Self {
        Self {
            show_add_modal: false,
            edit_id: None,
            name_input: String::new(),
            icon_input: "⭐".to_string(),
            app_type_input: FavoriteAppType::Process,
            process_filter_input: String::new(),
            service_name_input: String::new(),
            terminal_command_input: String::new(),
            description_input: String::new(),
            auto_start_command_input: String::new(),
            selected_from_proc_pid: None,
            search_query: String::new(),
            pending_confirm: None,
        }
    }
}

impl FavoritesPanelState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset_form(&mut self) {
        self.edit_id = None;
        self.name_input.clear();
        self.icon_input = "⭐".to_string();
        self.app_type_input = FavoriteAppType::Process;
        self.process_filter_input.clear();
        self.service_name_input.clear();
        self.terminal_command_input.clear();
        self.description_input.clear();
        self.auto_start_command_input.clear();
        self.selected_from_proc_pid = None;
    }

    pub fn open_add(&mut self) {
        self.reset_form();
        self.show_add_modal = true;
    }

    pub fn open_edit(&mut self, app: &FavoriteApp) {
        self.edit_id = Some(app.id.clone());
        self.name_input = app.name.clone();
        self.icon_input = app.icon.clone();
        self.app_type_input = app.app_type;
        self.process_filter_input = app.process_filter.clone();
        self.service_name_input = app.service_name.clone().unwrap_or_default();
        self.terminal_command_input = app.terminal_command.clone();
        self.description_input = app.description.clone();
        self.auto_start_command_input = app.auto_start_command.clone().unwrap_or_default();
        self.selected_from_proc_pid = None;
        self.show_add_modal = true;
    }

    pub fn apply_minecraft_template(&mut self) {
        self.name_input = "Serveur Minecraft".to_string();
        self.icon_input = "⛏".to_string();
        self.app_type_input = FavoriteAppType::Process;
        self.process_filter_input = "minecraft_server".to_string();
        self.service_name_input.clear();
        self.terminal_command_input = "screen -r minecraft || tmux attach -t minecraft".to_string();
        self.description_input = "Serveur dédié Minecraft Java/Bedrock".to_string();
        self.auto_start_command_input = "java -Xmx2G -Xms1G -jar server.jar nogui".to_string();
    }

    pub fn apply_docker_template(&mut self) {
        self.name_input = "Conteneur Docker".to_string();
        self.icon_input = "🐳".to_string();
        self.app_type_input = FavoriteAppType::Docker;
        self.process_filter_input = "docker".to_string();
        self.service_name_input.clear();
        self.terminal_command_input = "docker ps".to_string();
        self.description_input = "Conteneur ou stack Docker Compose".to_string();
        self.auto_start_command_input = "docker compose up -d".to_string();
    }

    pub fn apply_web_template(&mut self) {
        self.name_input = "Serveur Web (Nginx)".to_string();
        self.icon_input = "🌐".to_string();
        self.app_type_input = FavoriteAppType::Service;
        self.process_filter_input = "nginx".to_string();
        self.service_name_input = "nginx.service".to_string();
        self.terminal_command_input = "journalctl -u nginx -f".to_string();
        self.description_input = "Serveur Web & Reverse Proxy".to_string();
        self.auto_start_command_input = "systemctl start nginx".to_string();
    }

    pub fn apply_db_template(&mut self) {
        self.name_input = "Base de données (PostgreSQL)".to_string();
        self.icon_input = "🗄".to_string();
        self.app_type_input = FavoriteAppType::Service;
        self.process_filter_input = "postgres".to_string();
        self.service_name_input = "postgresql.service".to_string();
        self.terminal_command_input = "journalctl -u postgresql -f".to_string();
        self.description_input = "Serveur de base de données relationnelle".to_string();
        self.auto_start_command_input = "systemctl start postgresql".to_string();
    }

    pub fn apply_process_template(&mut self, proc: &ntx_proto::ProcessItem) {
        self.name_input = proc.name.clone();
        self.icon_input = "⚡".to_string();
        self.app_type_input = FavoriteAppType::Process;
        self.process_filter_input = proc.name.clone();
        self.service_name_input.clear();
        self.terminal_command_input.clear();
        self.description_input = format!("Processus {}", proc.name);
        self.auto_start_command_input.clear();
    }
}

/// Render the Favorites panel tab.
pub fn render(
    ui: &mut Ui,
    state: &mut FavoritesPanelState,
    device: &Device,
) -> Option<FavoritesPanelAction> {
    let p = Palette::for_ui(ui);
    let mut action: Option<FavoritesPanelAction> = None;

    // 1. Top Toolbar
    let avail_w = ui.available_width();
    let render_top_primary = |ui: &mut Ui, state: &mut FavoritesPanelState, action: &mut Option<FavoritesPanelAction>| {
        if theme::primary_button(ui, &p, "➕ Ajouter un favori").clicked() {
            state.open_add();
        }

        if device.status.is_connected()
            && ui
                .button("🔄 Actualiser")
                .on_hover_text("Actualiser les processus et services depuis le serveur")
                .clicked()
        {
            *action = Some(FavoritesPanelAction::Refresh);
        }
    };
    let render_search = |ui: &mut Ui, state: &mut FavoritesPanelState| {
        ui.add(
            egui::TextEdit::singleline(&mut state.search_query)
                .hint_text("🔍 Filtrer les favoris...")
                .desired_width(180.0),
        );
    };

    if avail_w >= 540.0 {
        ui.horizontal(|ui| {
            render_top_primary(ui, state, &mut action);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                render_search(ui, state);
            });
        });
    } else {
        ui.horizontal_wrapped(|ui| {
            render_top_primary(ui, state, &mut action);
            render_search(ui, state);
        });
    }

    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new("Modèles :")
                .color(p.text_secondary)
                .size(12.0),
        );

        if ui
            .button("⛏ Minecraft")
            .on_hover_text("Préconfigurer un serveur Minecraft")
            .clicked()
        {
            state.open_add();
            state.apply_minecraft_template();
        }

        if ui
            .button("🐳 Docker")
            .on_hover_text("Préconfigurer un conteneur ou compose Docker")
            .clicked()
        {
            state.open_add();
            state.apply_docker_template();
        }

        if ui
            .button("🌐 Web")
            .on_hover_text("Préconfigurer un serveur web Nginx")
            .clicked()
        {
            state.open_add();
            state.apply_web_template();
        }

        if ui
            .button("🗄 Base de données")
            .on_hover_text("Préconfigurer une base PostgreSQL")
            .clicked()
        {
            state.open_add();
            state.apply_db_template();
        }

        if !device.process_state.processes.is_empty()
            && ui
                .button("⚡ Depuis processus")
                .on_hover_text("Importer un processus actif de la machine")
                .clicked()
        {
            state.open_add();
            if let Some(first_proc) = device.process_state.processes.first() {
                state.selected_from_proc_pid = Some(first_proc.pid);
                state.apply_process_template(first_proc);
            }
        }
    });

    ui.add_space(6.0);

    // Feedback banner (reuses status_message from process/service actions)
    if let Some((msg, is_error)) = &device.process_state.status_message {
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
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(if *is_error { "⚠" } else { "✔" })
                            .color(fg)
                            .strong(),
                    );
                    ui.label(RichText::new(msg).color(fg));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✖").clicked() {
                            action = Some(FavoritesPanelAction::ClearStatus);
                        }
                    });
                });
            });
        ui.add_space(6.0);
    }

    // 2. Compute KPI Metrics
    let total_favs = device.favorites.len();
    let mut active_count = 0;
    let mut stopped_count = 0;

    for app in &device.favorites {
        let is_proc_matched = device
            .process_state
            .processes
            .iter()
            .any(|pr| app.matches_process(pr));

        let is_service_matched = if app.app_type == FavoriteAppType::Service
            || app
                .service_name
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
        {
            device.process_state.services.iter().any(|s| {
                if app.matches_service(s) {
                    let st = s.active_state.to_lowercase();
                    let sub = s.sub_state.to_lowercase();
                    st == "active" || sub == "running"
                } else {
                    false
                }
            })
        } else {
            false
        };

        if is_proc_matched || is_service_matched {
            active_count += 1;
        } else {
            stopped_count += 1;
        }
    }

    // KPI Banner
    theme::subtle_bar_frame(&p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let banner_w = ui.available_width();
        let render_kpis = |ui: &mut Ui| {
            ui.label(
                RichText::new("⭐ Applications & Favoris")
                    .strong()
                    .size(13.0)
                    .color(p.text_primary),
            );
            ui.separator();
            ui.label(
                RichText::new(format!("Total : {total_favs}"))
                    .strong()
                    .size(12.0),
            );
            ui.separator();
            ui.label(
                RichText::new(format!("● En cours : {active_count}"))
                    .color(p.success)
                    .strong()
                    .size(12.0),
            );
            ui.separator();
            ui.label(
                RichText::new(format!("○ Arrêtés : {stopped_count}"))
                    .color(p.text_secondary)
                    .size(12.0),
            );
        };
        let render_offline_warn = |ui: &mut Ui| {
            ui.label(
                RichText::new("⚠ Serveur déconnecté — statut en direct indisponible")
                    .color(p.warning)
                    .size(11.5),
            );
        };

        if banner_w >= 720.0 {
            ui.horizontal(|ui| {
                render_kpis(ui);
                if !device.status.is_connected() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        render_offline_warn(ui);
                    });
                }
            });
        } else {
            ui.horizontal_wrapped(render_kpis);
            if !device.status.is_connected() {
                ui.add_space(4.0);
                ui.horizontal_wrapped(render_offline_warn);
            }
        }
    });

    ui.add_space(8.0);

    // 3. Favorites Cards List
    let search = state.search_query.trim().to_lowercase();
    let filtered_favorites: Vec<&FavoriteApp> = device
        .favorites
        .iter()
        .filter(|app| {
            if search.is_empty() {
                true
            } else {
                app.name.to_lowercase().contains(&search)
                    || app.description.to_lowercase().contains(&search)
                    || app.process_filter.to_lowercase().contains(&search)
                    || app
                        .service_name
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&search)
            }
        })
        .collect();

    if device.favorites.is_empty() {
        ui.add_space(20.0);
        theme::card_frame(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(14.0);
                ui.label(
                    RichText::new("⭐ Aucune application en favori pour ce serveur")
                        .size(16.0)
                        .strong()
                        .color(p.text_primary),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Épinglez vos serveurs de jeux, conteneurs Docker, bases de données ou services systemd\npour surveiller leur CPU/RAM en direct, consulter leurs logs ou les piloter en 1 clic.",
                    )
                    .color(p.text_secondary),
                );
                ui.add_space(16.0);
                if theme::primary_button(ui, &p, "➕ Ajouter un premier favori").clicked() {
                    state.open_add();
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let total_w = 480.0;
                    ui.add_space(((ui.available_width() - total_w) * 0.5).max(0.0));
                    if ui.button("⛏ Modèle Minecraft").clicked() {
                        state.open_add();
                        state.apply_minecraft_template();
                    }
                    if ui.button("🐳 Modèle Docker").clicked() {
                        state.open_add();
                        state.apply_docker_template();
                    }
                    if ui.button("🌐 Modèle Web").clicked() {
                        state.open_add();
                        state.apply_web_template();
                    }
                    if ui.button("🗄 Modèle Base de données").clicked() {
                        state.open_add();
                        state.apply_db_template();
                    }
                });
                ui.add_space(14.0);
            });
        });
    } else if filtered_favorites.is_empty() {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("Aucun favori ne correspond à votre recherche.")
                    .color(p.text_secondary),
            );
        });
    } else {
        egui::ScrollArea::vertical().show(ui, |ui| {
            for app in filtered_favorites {
                if let Some(card_action) = render_favorite_card(ui, &p, state, device, app) {
                    action = Some(card_action);
                }
                ui.add_space(8.0);
            }
        });
    }

    // 4. Add / Edit Modal Window
    if state.show_add_modal {
        if let Some(modal_action) = render_add_edit_modal(ui, &p, state, device) {
            action = Some(modal_action);
        }
    }

    // 5. Confirmation Modal
    if let Some(confirm) = state.pending_confirm.clone() {
        let mut clear = false;
        egui::Window::new("⚠ Confirmation requise")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                match &confirm {
                    PendingFavConfirm::KillProcess { name, pid, signal } => {
                        let sig_label = if *signal == 9 {
                            "SIGKILL (9 - arrêt forcé)"
                        } else {
                            "SIGTERM (15 - arrêt propre)"
                        };
                        ui.label(format!(
                            "Envoyer {sig_label} au processus « {name} » (PID {pid}) ?"
                        ));
                    }
                    PendingFavConfirm::ControlService {
                        service_name,
                        action: svc_act,
                    } => {
                        ui.label(format!(
                            "Exécuter « systemctl {svc_act} {service_name} » ?"
                        ));
                    }
                    PendingFavConfirm::RemoveFavorite { name, .. } => {
                        ui.label(format!("Supprimer le favori « {name} » de ce serveur ?"));
                    }
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if theme::danger_button(ui, &p, "✅ Confirmer").clicked() {
                        action = Some(match confirm.clone() {
                            PendingFavConfirm::KillProcess { pid, signal, .. } => {
                                FavoritesPanelAction::KillProcess { pid, signal }
                            }
                            PendingFavConfirm::ControlService {
                                service_name,
                                action: svc_act,
                            } => FavoritesPanelAction::ControlService {
                                service_name,
                                action: svc_act,
                            },
                            PendingFavConfirm::RemoveFavorite { id, .. } => {
                                FavoritesPanelAction::RemoveFavorite(id)
                            }
                        });
                        clear = true;
                    }
                    if ui.button("Annuler").clicked() {
                        clear = true;
                    }
                });
            });
        if clear {
            state.pending_confirm = None;
        }
    }

    action
}

fn render_favorite_card(
    ui: &mut Ui,
    p: &Palette,
    state: &mut FavoritesPanelState,
    device: &Device,
    app: &FavoriteApp,
) -> Option<FavoritesPanelAction> {
    let mut action = None;

    let matching_procs: Vec<&ntx_proto::ProcessItem> = device
        .process_state
        .processes
        .iter()
        .filter(|pr| app.matches_process(pr))
        .collect();

    let has_service_ref = app.app_type == FavoriteAppType::Service
        || app
            .service_name
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);

    let matching_services: Vec<&ntx_proto::ServiceItem> = if has_service_ref {
        device
            .process_state
            .services
            .iter()
            .filter(|s| app.matches_service(s))
            .collect()
    } else {
        Vec::new()
    };

    let active_service = matching_services.iter().copied().find(|s| {
        let st = s.active_state.to_lowercase();
        let sub = s.sub_state.to_lowercase();
        st == "active" || sub == "running"
    });

    let is_active = !matching_procs.is_empty() || active_service.is_some();

    let primary_proc = matching_procs
        .iter()
        .max_by(|a, b| {
            a.cpu_usage
                .partial_cmp(&b.cpu_usage)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied();

    let total_cpu: f32 = matching_procs.iter().map(|pr| pr.cpu_usage).sum();
    let total_mem: u64 = matching_procs.iter().map(|pr| pr.memory_bytes).sum();

    let (card_bg, card_border) = if is_active {
        (p.bg_card, p.success_border)
    } else {
        (p.bg_card, p.border)
    };

    egui::Frame::new()
        .fill(card_bg)
        .stroke(Stroke::new(1.0_f32, card_border))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            let card_w = ui.available_width();
            let render_card_header_left = |ui: &mut Ui| {
                ui.label(RichText::new(&app.icon).size(18.0));
                ui.label(
                    RichText::new(&app.name)
                        .size(15.5)
                        .strong()
                        .color(p.text_primary),
                );

                let (badge_text, badge_bg, badge_fg) = get_type_badge(p, app);
                theme::pill_badge(ui, badge_text, badge_bg, badge_fg, None);

                let (status_text, status_bg, status_fg) = if is_active {
                    ("● En cours", p.success_bg, p.success)
                } else {
                    ("○ Arrêté", p.bg_subtle, p.text_muted)
                };
                theme::pill_badge(ui, status_text, status_bg, status_fg, None);
            };
            let render_card_header_right = |ui: &mut Ui, rtl: bool, state: &mut FavoritesPanelState| {
                let draw_delete = |ui: &mut Ui, state: &mut FavoritesPanelState| {
                    if ui
                        .button("🗑")
                        .on_hover_text("Supprimer ce favori de la liste")
                        .clicked()
                    {
                        state.pending_confirm = Some(PendingFavConfirm::RemoveFavorite {
                            id: app.id.clone(),
                            name: app.name.clone(),
                        });
                    }
                };
                let draw_edit = |ui: &mut Ui, state: &mut FavoritesPanelState| {
                    if ui
                        .button("✏ Modifier")
                        .on_hover_text("Modifier les paramètres de ce favori")
                        .clicked()
                    {
                        state.open_edit(app);
                    }
                };
                if rtl {
                    draw_delete(ui, state);
                    draw_edit(ui, state);
                } else {
                    draw_edit(ui, state);
                    draw_delete(ui, state);
                }
            };

            if card_w >= 520.0 {
                ui.horizontal(|ui| {
                    render_card_header_left(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        render_card_header_right(ui, true, state);
                    });
                });
            } else {
                ui.horizontal_wrapped(render_card_header_left);
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    render_card_header_right(ui, false, state);
                });
            }

            if !app.description.is_empty() {
                ui.add_space(2.0);
                ui.label(
                    RichText::new(&app.description)
                        .size(12.0)
                        .color(p.text_secondary),
                );
            }

            ui.add_space(4.0);

            ui.horizontal_wrapped(|ui| {
                if !app.process_filter.is_empty() {
                    ui.label(
                        RichText::new(format!("Filtre : {}", app.process_filter))
                            .size(11.0)
                            .color(p.text_muted),
                    );
                    ui.separator();
                }
                if let Some(srv) = &app.service_name {
                    if !srv.trim().is_empty() {
                        ui.label(
                            RichText::new(format!("Service : {srv}"))
                                .size(11.0)
                                .color(p.text_muted),
                        );
                        ui.separator();
                    }
                }
                if let Some(auto_cmd) = &app.auto_start_command {
                    if !auto_cmd.trim().is_empty() {
                        ui.label(
                            RichText::new(format!("Démarrage : {auto_cmd}"))
                                .size(11.0)
                                .color(p.text_muted),
                        );
                    }
                }
            });

            ui.add_space(8.0);

            if is_active {
                let render_active_metrics = |ui: &mut Ui| {
                    if let Some(s) = active_service {
                        ui.label(
                            RichText::new(format!("Service actif : {}", s.name))
                                .strong()
                                .size(12.0)
                                .color(p.accent),
                        );
                        ui.separator();
                    }

                    if let Some(pr) = primary_proc {
                        ui.label(
                            RichText::new(format!("PID : {}", pr.pid))
                                .strong()
                                .size(12.0)
                                .color(p.accent),
                        );

                        if matching_procs.len() > 1 {
                            ui.label(
                                RichText::new(format!("(+{} instances)", matching_procs.len() - 1))
                                    .size(11.0)
                                    .color(p.text_muted),
                            );
                        }

                        ui.separator();
                    }

                    ui.label(
                        RichText::new(format!("CPU : {:.1}%", total_cpu))
                            .strong()
                            .size(12.0)
                            .color(if total_cpu > 20.0 {
                                p.orange
                            } else {
                                p.success
                            }),
                    );
                    ui.separator();

                    ui.label(
                        RichText::new(format!("RAM : {}", theme::format_ram(total_mem)))
                            .strong()
                            .size(12.0)
                            .color(p.purple),
                    );
                };

                let render_active_actions = |ui: &mut Ui,
                                             rtl: bool,
                                             state: &mut FavoritesPanelState,
                                             action: &mut Option<FavoritesPanelAction>| {
                    let draw_kill = |ui: &mut Ui, state: &mut FavoritesPanelState| {
                        if let Some(pr) = primary_proc {
                            if theme::danger_button(ui, p, "⛔ Tuer")
                                .on_hover_text(format!("Envoyer SIGKILL (9) au PID {}", pr.pid))
                                .clicked()
                            {
                                state.pending_confirm = Some(PendingFavConfirm::KillProcess {
                                    name: pr.name.clone(),
                                    pid: pr.pid,
                                    signal: 9,
                                });
                            }
                        }
                    };
                    let draw_service_or_proc_stop = |ui: &mut Ui,
                                                     rtl: bool,
                                                     state: &mut FavoritesPanelState,
                                                     action: &mut Option<FavoritesPanelAction>| {
                        if let Some(s) = active_service {
                            let draw_stop = |ui: &mut Ui, state: &mut FavoritesPanelState| {
                                if ui
                                    .button("⏹ Arrêter")
                                    .on_hover_text(format!(
                                        "Arrêter le service {} (systemctl stop)",
                                        s.name
                                    ))
                                    .clicked()
                                {
                                    state.pending_confirm =
                                        Some(PendingFavConfirm::ControlService {
                                            service_name: s.name.clone(),
                                            action: "stop".to_string(),
                                        });
                                }
                            };
                            let draw_restart = |ui: &mut Ui, state: &mut FavoritesPanelState| {
                                if ui
                                    .button("🔄 Redémarrer")
                                    .on_hover_text(format!(
                                        "Redémarrer le service {} (avec confirmation)",
                                        s.name
                                    ))
                                    .clicked()
                                {
                                    state.pending_confirm =
                                        Some(PendingFavConfirm::ControlService {
                                            service_name: s.name.clone(),
                                            action: "restart".to_string(),
                                        });
                                }
                            };
                            let draw_logs = |ui: &mut Ui, action: &mut Option<FavoritesPanelAction>| {
                                if ui
                                    .button("📜 Logs")
                                    .on_hover_text(format!(
                                        "Voir les journaux de {} dans le terminal",
                                        s.name
                                    ))
                                    .clicked()
                                {
                                    let safe_unit = s.name.trim().replace('\'', "'\\''");
                                    *action = Some(FavoritesPanelAction::AttachTerminal {
                                        command: format!("journalctl -u '{safe_unit}' -n 100 -f"),
                                    });
                                }
                            };
                            if rtl {
                                draw_stop(ui, state);
                                draw_restart(ui, state);
                                draw_logs(ui, action);
                            } else {
                                draw_logs(ui, action);
                                draw_restart(ui, state);
                                draw_stop(ui, state);
                            }
                        } else if let Some(pr) = primary_proc {
                            if ui
                                .button("⏹ Arrêter")
                                .on_hover_text(format!("Envoyer SIGTERM (15) au PID {}", pr.pid))
                                .clicked()
                            {
                                state.pending_confirm = Some(PendingFavConfirm::KillProcess {
                                    name: pr.name.clone(),
                                    pid: pr.pid,
                                    signal: 15,
                                });
                            }
                        }
                    };
                    let draw_term = |ui: &mut Ui, action: &mut Option<FavoritesPanelAction>| {
                        if !app.terminal_command.trim().is_empty()
                            && theme::primary_button(ui, p, "💻 Terminal dédié")
                                .on_hover_text(format!(
                                    "Ouvrir le terminal et exécuter : {}",
                                    app.terminal_command
                                ))
                                .clicked()
                        {
                            *action = Some(FavoritesPanelAction::AttachTerminal {
                                command: app.terminal_command.clone(),
                            });
                        }
                    };

                    if rtl {
                        draw_kill(ui, state);
                        draw_service_or_proc_stop(ui, true, state, action);
                        draw_term(ui, action);
                    } else {
                        draw_term(ui, action);
                        draw_service_or_proc_stop(ui, false, state, action);
                        draw_kill(ui, state);
                    }
                };

                if card_w >= 860.0 {
                    ui.horizontal(|ui| {
                        render_active_metrics(ui);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            render_active_actions(ui, true, state, &mut action);
                        });
                    });
                } else {
                    ui.horizontal_wrapped(render_active_metrics);
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        render_active_actions(ui, false, state, &mut action);
                    });
                }
            } else {
                let render_stopped_label = |ui: &mut Ui| {
                    ui.label(
                        RichText::new("Application ou service actuellement arrêté.")
                            .size(12.0)
                            .color(p.text_muted),
                    );
                };
                let render_stopped_actions =
                    |ui: &mut Ui, rtl: bool, action: &mut Option<FavoritesPanelAction>| {
                        let auto_cmd = app
                            .auto_start_command
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty());
                        let svc_name = app
                            .service_name
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty());

                        let draw_start = |ui: &mut Ui,
                                          rtl: bool,
                                          action: &mut Option<FavoritesPanelAction>| {
                            if app.app_type == FavoriteAppType::Service {
                                let target_svc = svc_name.or_else(|| {
                                    auto_cmd
                                        .and_then(|c| c.strip_prefix("systemctl start "))
                                        .map(str::trim)
                                        .filter(|s| !s.is_empty())
                                });

                                if let Some(sname) = target_svc {
                                    let draw_start_srv =
                                        |ui: &mut Ui, action: &mut Option<FavoritesPanelAction>| {
                                            if theme::primary_button(
                                                ui,
                                                p,
                                                "▶ Démarrer le service",
                                            )
                                            .on_hover_text(format!(
                                                "Démarrer le service {sname} (systemctl start)"
                                            ))
                                            .clicked()
                                            {
                                                *action =
                                                    Some(FavoritesPanelAction::ControlService {
                                                        service_name: sname.to_string(),
                                                        action: "start".to_string(),
                                                    });
                                            }
                                        };
                                    let draw_logs =
                                        |ui: &mut Ui, action: &mut Option<FavoritesPanelAction>| {
                                            if ui
                                                .button("📜 Logs")
                                                .on_hover_text(format!(
                                                    "Voir les journaux de {sname}"
                                                ))
                                                .clicked()
                                            {
                                                let safe_unit =
                                                    sname.trim().replace('\'', "'\\''");
                                                *action =
                                                    Some(FavoritesPanelAction::AttachTerminal {
                                                        command: format!(
                                                        "journalctl -u '{safe_unit}' -n 100 -f"
                                                    ),
                                                    });
                                            }
                                        };
                                    if rtl {
                                        draw_start_srv(ui, action);
                                        draw_logs(ui, action);
                                    } else {
                                        draw_logs(ui, action);
                                        draw_start_srv(ui, action);
                                    }
                                } else if let Some(cmd) = auto_cmd {
                                    if theme::primary_button(ui, p, "▶ Démarrer")
                                        .on_hover_text(format!(
                                            "Démarrer l'application avec : {cmd}"
                                        ))
                                        .clicked()
                                    {
                                        *action = Some(FavoritesPanelAction::AttachTerminal {
                                            command: cmd.to_string(),
                                        });
                                    }
                                }
                            } else if let Some(cmd) = auto_cmd {
                                if theme::primary_button(ui, p, "▶ Démarrer")
                                    .on_hover_text(format!("Démarrer l'application avec : {cmd}"))
                                    .clicked()
                                {
                                    *action = Some(FavoritesPanelAction::AttachTerminal {
                                        command: cmd.to_string(),
                                    });
                                }
                            } else if let Some(sname) = svc_name {
                                if theme::primary_button(ui, p, "▶ Démarrer le service")
                                    .on_hover_text(format!(
                                        "Démarrer le service {sname} (systemctl start)"
                                    ))
                                    .clicked()
                                {
                                    *action = Some(FavoritesPanelAction::ControlService {
                                        service_name: sname.to_string(),
                                        action: "start".to_string(),
                                    });
                                }
                            }
                        };

                        let draw_term = |ui: &mut Ui, action: &mut Option<FavoritesPanelAction>| {
                            if !app.terminal_command.trim().is_empty()
                                && ui
                                    .button("💻 Terminal")
                                    .on_hover_text(format!(
                                        "Ouvrir le terminal distant avec la commande : {}",
                                        app.terminal_command
                                    ))
                                    .clicked()
                            {
                                *action = Some(FavoritesPanelAction::AttachTerminal {
                                    command: app.terminal_command.clone(),
                                });
                            }
                        };

                        if rtl {
                            draw_start(ui, true, action);
                            draw_term(ui, action);
                        } else {
                            draw_term(ui, action);
                            draw_start(ui, false, action);
                        }
                    };

                if card_w >= 560.0 {
                    ui.horizontal(|ui| {
                        render_stopped_label(ui);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            render_stopped_actions(ui, true, &mut action);
                        });
                    });
                } else {
                    ui.horizontal_wrapped(render_stopped_label);
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        render_stopped_actions(ui, false, &mut action);
                    });
                }
            }
        });

    action
}

fn render_add_edit_modal(
    ui: &mut Ui,
    p: &Palette,
    state: &mut FavoritesPanelState,
    device: &Device,
) -> Option<FavoritesPanelAction> {
    let mut action = None;
    let mut close_modal = false;

    let is_edit = state.edit_id.is_some();
    let title = if is_edit {
        "✏ Modifier l'application favorite"
    } else {
        "➕ Ajouter une application / favori"
    };

    egui::Window::new(title)
        .collapsible(false)
        .resizable(true)
        .default_width(540.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ui.ctx(), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Modèles rapides :").strong().size(12.0));
                if ui.small_button("⛏ Minecraft").clicked() {
                    state.apply_minecraft_template();
                }
                if ui.small_button("🐳 Docker").clicked() {
                    state.apply_docker_template();
                }
                if ui.small_button("🌐 Web").clicked() {
                    state.apply_web_template();
                }
                if ui.small_button("🗄 Base de données").clicked() {
                    state.apply_db_template();
                }
            });

            if !device.process_state.processes.is_empty() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("⚡ Importer depuis processus actif :")
                            .size(12.0)
                            .color(p.text_secondary),
                    );
                    let sel_label = state
                        .selected_from_proc_pid
                        .and_then(|pid| {
                            device.process_state.processes.iter().find(|pr| pr.pid == pid)
                        })
                        .map(|pr| format!("{} (PID: {})", pr.name, pr.pid))
                        .unwrap_or_else(|| "Choisir dans la liste...".to_string());

                    egui::ComboBox::from_id_salt("modal_proc_selector")
                        .selected_text(sel_label)
                        .width(230.0)
                        .show_ui(ui, |ui| {
                            for pr in &device.process_state.processes {
                                let label =
                                    format!("{} (PID: {}, CPU: {:.1}%)", pr.name, pr.pid, pr.cpu_usage);
                                let is_sel = state.selected_from_proc_pid == Some(pr.pid);
                                if ui.selectable_label(is_sel, label).clicked() {
                                    state.selected_from_proc_pid = Some(pr.pid);
                                    state.apply_process_template(pr);
                                }
                            }
                        });
                });
            }

            ui.separator();
            ui.add_space(4.0);

            egui::Grid::new("fav_modal_form_grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Nom du favori *").strong());
                    ui.add(
                        egui::TextEdit::singleline(&mut state.name_input).desired_width(300.0),
                    );
                    ui.end_row();

                    ui.label("Icône / Emoji");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut state.icon_input).desired_width(40.0),
                        );
                        for emoji in &["⭐", "⛏", "🐳", "🌐", "🗄", "⚡", "🚀", "🎮", "📦", "🔥"]
                        {
                            if ui.small_button(*emoji).clicked() {
                                state.icon_input = (*emoji).to_string();
                            }
                        }
                    });
                    ui.end_row();

                    ui.label("Type d'application");
                    egui::ComboBox::from_id_salt("modal_app_type_combo")
                        .selected_text(match state.app_type_input {
                            FavoriteAppType::Process => "⚡ Processus standard",
                            FavoriteAppType::Docker => "🐳 Conteneur Docker",
                            FavoriteAppType::Service => "⚙ Service systemd",
                            FavoriteAppType::Custom => "✨ Personnalisé",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut state.app_type_input,
                                FavoriteAppType::Process,
                                "⚡ Processus standard",
                            );
                            ui.selectable_value(
                                &mut state.app_type_input,
                                FavoriteAppType::Docker,
                                "🐳 Conteneur Docker",
                            );
                            ui.selectable_value(
                                &mut state.app_type_input,
                                FavoriteAppType::Service,
                                "⚙ Service systemd",
                            );
                            ui.selectable_value(
                                &mut state.app_type_input,
                                FavoriteAppType::Custom,
                                "✨ Personnalisé",
                            );
                        });
                    ui.end_row();

                    ui.label("Filtre processus");
                    ui.vertical(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut state.process_filter_input)
                                .desired_width(300.0),
                        );
                        ui.label(
                            RichText::new(
                                "Nom de l'exécutable, argument ou PID numérique (ex: java, nginx, 1234)",
                            )
                            .color(p.text_muted)
                            .size(10.5),
                        );
                    });
                    ui.end_row();

                    ui.label("Nom service systemd");
                    ui.vertical(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut state.service_name_input)
                                .desired_width(300.0),
                        );
                        ui.label(
                            RichText::new(
                                "Optionnel si service (ex: nginx.service, postgresql.service)",
                            )
                            .color(p.text_muted)
                            .size(10.5),
                        );
                    });
                    ui.end_row();

                    ui.label("Commande terminal");
                    ui.vertical(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut state.terminal_command_input)
                                .desired_width(300.0),
                        );
                        ui.label(
                            RichText::new(
                                "Exécutée dans le terminal dédié (ex: screen -r mc, docker logs -f myapp)",
                            )
                            .color(p.text_muted)
                            .size(10.5),
                        );
                    });
                    ui.end_row();

                    ui.label("Commande démarrage");
                    ui.vertical(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut state.auto_start_command_input)
                                .desired_width(300.0),
                        );
                        ui.label(
                            RichText::new(
                                "Exécutée pour démarrer si arrêté (ex: systemctl start nginx, docker compose up -d)",
                            )
                            .color(p.text_muted)
                            .size(10.5),
                        );
                    });
                    ui.end_row();

                    ui.label("Description");
                    ui.add(
                        egui::TextEdit::multiline(&mut state.description_input)
                            .desired_width(300.0),
                    );
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                let can_save = !state.name_input.trim().is_empty();
                let save_label = if is_edit {
                    "💾 Mettre à jour"
                } else {
                    "➕ Ajouter"
                };
                let save_btn = ui.add_enabled(
                    can_save,
                    egui::Button::new(RichText::new(save_label).strong().color(p.primary_btn_fg))
                        .fill(p.primary_btn_bg)
                        .stroke(Stroke::new(1.0_f32, p.accent_border))
                        .corner_radius(6),
                );

                if save_btn.clicked() {
                    let id = state
                        .edit_id
                        .clone()
                        .unwrap_or_else(|| format!("fav_{}", uuid::Uuid::new_v4().simple()));

                    let srv_name = if state.service_name_input.trim().is_empty() {
                        None
                    } else {
                        Some(state.service_name_input.trim().to_string())
                    };

                    let auto_cmd = if state.auto_start_command_input.trim().is_empty() {
                        None
                    } else {
                        Some(state.auto_start_command_input.trim().to_string())
                    };

                    let fav = FavoriteApp::new(
                        id,
                        state.name_input.trim(),
                        if state.icon_input.trim().is_empty() {
                            "⭐"
                        } else {
                            state.icon_input.trim()
                        },
                        state.app_type_input,
                        state.process_filter_input.trim(),
                        srv_name,
                        state.terminal_command_input.trim(),
                        state.description_input.trim(),
                        auto_cmd,
                    );

                    action = Some(FavoritesPanelAction::SaveFavorite(fav));
                    close_modal = true;
                }

                if ui.button("Annuler").clicked() {
                    close_modal = true;
                }
            });
        });

    if close_modal {
        state.show_add_modal = false;
        state.reset_form();
    }

    action
}

fn get_type_badge(p: &Palette, app: &FavoriteApp) -> (&'static str, Color32, Color32) {
    match app.app_type {
        FavoriteAppType::Docker => ("🐳 Docker", p.accent_bg, p.accent),
        FavoriteAppType::Service => ("⚙ Service", p.warning_bg, p.warning),
        FavoriteAppType::Process => {
            if app.icon == "⛏" || app.icon == "⛏️" || app.name.to_lowercase().contains("minecraft") {
                ("⛏ Minecraft", p.success_bg, p.success)
            } else {
                ("⚡ Processus", p.bg_subtle, p.purple)
            }
        }
        FavoriteAppType::Custom => ("✨ Personnalisé", p.bg_subtle, p.text_secondary),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::FavoriteApp;

    #[test]
    fn test_favorites_panel_state_default_and_reset() {
        let mut state = FavoritesPanelState::default();
        assert!(!state.show_add_modal);
        assert_eq!(state.icon_input, "⭐");
        assert_eq!(state.app_type_input, FavoriteAppType::Process);

        state.name_input = "Test".to_string();
        state.edit_id = Some("id_123".to_string());
        state.show_add_modal = true;
        state.reset_form();

        assert!(state.name_input.is_empty());
        assert!(state.edit_id.is_none());
        assert_eq!(state.icon_input, "⭐");
    }

    #[test]
    fn test_favorites_panel_templates() {
        let mut state = FavoritesPanelState::new();

        state.apply_minecraft_template();
        assert_eq!(state.icon_input, "⛏");
        assert_eq!(state.app_type_input, FavoriteAppType::Process);
        assert_eq!(state.process_filter_input, "minecraft_server");

        state.apply_docker_template();
        assert_eq!(state.icon_input, "🐳");
        assert_eq!(state.app_type_input, FavoriteAppType::Docker);

        state.apply_web_template();
        assert_eq!(state.icon_input, "🌐");
        assert_eq!(state.app_type_input, FavoriteAppType::Service);
        assert_eq!(state.service_name_input, "nginx.service");

        state.apply_db_template();
        assert_eq!(state.icon_input, "🗄");
        assert_eq!(state.service_name_input, "postgresql.service");

        let proc = ntx_proto::ProcessItem {
            pid: 4321,
            name: "custom_proc".to_string(),
            cpu_usage: 12.5,
            memory_bytes: 1024 * 1024 * 50,
            user: "root".to_string(),
            status: "running".to_string(),
            command_line: "/usr/bin/custom_proc --daemon".to_string(),
            parent_pid: 1,
        };
        state.apply_process_template(&proc);
        assert_eq!(state.name_input, "custom_proc");
        assert_eq!(state.terminal_command_input, "");
    }

    #[test]
    fn test_get_type_badge_and_format_ram() {
        let p = Palette::dark();
        let app_docker = FavoriteApp {
            app_type: FavoriteAppType::Docker,
            ..Default::default()
        };
        let (label, _, _) = get_type_badge(&p, &app_docker);
        assert_eq!(label, "🐳 Docker");

        assert_eq!(theme::format_ram(500 * 1024 * 1024), "500.0 Mo");
        assert_eq!(theme::format_ram(2 * 1024 * 1024 * 1024), "2.00 Go");
    }
}
