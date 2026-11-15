use crate::state::{Device, FavoriteApp, FavoriteAppType};
use egui::{Color32, RichText, Ui};

/// Actions that can be triggered from the Favorites management view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FavoritesPanelAction {
    AttachTerminal { command: String },
    KillProcess { pid: u32, signal: i32 },
    ControlService { service_name: String, action: String },
    SaveFavorite(FavoriteApp),
    RemoveFavorite(String),
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
        self.icon_input = "⛏️".to_string();
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
        self.icon_input = "🗄️".to_string();
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
        self.terminal_command_input = if !proc.command_line.is_empty() {
            format!("top -p {}", proc.pid)
        } else {
            String::new()
        };
        self.description_input = format!("Processus {} (PID: {})", proc.name, proc.pid);
        self.auto_start_command_input.clear();
    }
}

/// Render the Favorites panel tab.
pub fn render(
    ui: &mut Ui,
    state: &mut FavoritesPanelState,
    device: &Device,
) -> Option<FavoritesPanelAction> {
    let mut action: Option<FavoritesPanelAction> = None;

    // 1. Top Toolbar (Add button, quick templates, and search filter)
    ui.horizontal(|ui| {
        let add_btn = ui.button(
            RichText::new("➕ Ajouter une application / favori")
                .strong()
                .color(Color32::from_rgb(255, 255, 255)),
        );
        if add_btn.clicked() {
            state.open_add();
        }

        ui.separator();
        ui.label(RichText::new("Modèles rapides:").weak().size(12.0));

        if ui.button("⛏️ Minecraft").on_hover_text("Préconfigurer un serveur Minecraft").clicked() {
            state.open_add();
            state.apply_minecraft_template();
        }

        if ui.button("🐳 Docker").on_hover_text("Préconfigurer un conteneur ou compose Docker").clicked() {
            state.open_add();
            state.apply_docker_template();
        }

        if ui.button("🌐 Web").on_hover_text("Préconfigurer un serveur web Nginx").clicked() {
            state.open_add();
            state.apply_web_template();
        }

        if ui.button("🗄️ Database").on_hover_text("Préconfigurer une base PostgreSQL").clicked() {
            state.open_add();
            state.apply_db_template();
        }

        if !device.process_state.processes.is_empty()
            && ui.button("⚡ Depuis processus en cours").on_hover_text("Importer un processus actif de la machine").clicked()
        {
            state.open_add();
            if let Some(first_proc) = device.process_state.processes.first() {
                state.selected_from_proc_pid = Some(first_proc.pid);
                state.apply_process_template(first_proc);
            }
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.search_query)
                    .hint_text("🔍 Filtrer les favoris...")
                    .desired_width(180.0),
            );
        });
    });

    ui.add_space(6.0);

    // 2. Compute KPI Metrics
    let total_favs = device.favorites.len();
    let mut active_count = 0;
    let mut stopped_count = 0;

    for app in &device.favorites {
        let is_proc_matched = device
            .process_state
            .processes
            .iter()
            .any(|p| app.matches_process(p));

        let is_service_matched = if app.app_type == FavoriteAppType::Service || app.service_name.is_some() {
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

    // Render KPI Banner
    egui::Frame::new()
        .fill(Color32::from_rgb(22, 25, 32))
        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(45, 50, 62)))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("⭐ Applications & Favoris").strong().size(13.0));
                ui.separator();
                ui.label(RichText::new(format!("Total: {total_favs}")).strong().size(12.0));
                ui.separator();
                ui.label(
                    RichText::new(format!("● En cours: {active_count}"))
                        .color(Color32::from_rgb(46, 204, 113))
                        .strong()
                        .size(12.0),
                );
                ui.separator();
                ui.label(
                    RichText::new(format!("○ Arrêtés: {stopped_count}"))
                        .color(Color32::from_rgb(140, 150, 165))
                        .size(12.0),
                );

                if !device.status.is_connected() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new("⚠️ Serveur déconnecté - statut en direct indisponible")
                                .color(Color32::from_rgb(240, 160, 40))
                                .size(11.0),
                        );
                    });
                }
            });
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
                    || app.service_name.as_deref().unwrap_or("").to_lowercase().contains(&search)
            }
        })
        .collect();

    if device.favorites.is_empty() {
        // Empty state when no favorites exist
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("⭐ Aucune application en favori pour ce serveur.").size(16.0).strong());
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Épinglez vos serveurs de jeux (Minecraft), conteneurs Docker, bases de données ou services web\npour surveiller leur CPU, RAM et statut en temps réel, ou interagir directement via terminal dédié.",
                )
                .weak(),
            );
            ui.add_space(16.0);
            if ui.button(RichText::new("➕ Ajouter un premier favori").size(14.0).strong()).clicked() {
                state.open_add();
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let total_w = 420.0;
                ui.add_space((ui.available_width() - total_w).max(0.0) / 2.0);
                if ui.button("⛏️ Modèle Minecraft").clicked() {
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
                if ui.button("🗄️ Modèle Database").clicked() {
                    state.open_add();
                    state.apply_db_template();
                }
            });
        });
    } else if filtered_favorites.is_empty() {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Aucun favori ne correspond à votre recherche.").weak());
        });
    } else {
        egui::ScrollArea::vertical().show(ui, |ui| {
            for app in filtered_favorites {
                if let Some(card_action) = render_favorite_card(ui, state, device, app) {
                    action = Some(card_action);
                }
                ui.add_space(6.0);
            }
        });
    }

    // 4. Add / Edit Modal Window
    if state.show_add_modal {
        if let Some(modal_action) = render_add_edit_modal(ui, state, device) {
            action = Some(modal_action);
        }
    }

    action
}

/// Render a single Favorite Card.
fn render_favorite_card(
    ui: &mut Ui,
    state: &mut FavoritesPanelState,
    device: &Device,
    app: &FavoriteApp,
) -> Option<FavoritesPanelAction> {
    let mut action = None;

    // Match against live processes
    let matching_procs: Vec<&ntx_proto::ProcessItem> = device
        .process_state
        .processes
        .iter()
        .filter(|p| app.matches_process(p))
        .collect();

    // Match against live services
    let matching_services: Vec<&ntx_proto::ServiceItem> = if app.app_type == FavoriteAppType::Service || app.service_name.is_some() {
        device
            .process_state
            .services
            .iter()
            .filter(|s| app.matches_service(s))
            .collect()
    } else {
        Vec::new()
    };

    let is_service_active = matching_services.iter().any(|s| {
        let st = s.active_state.to_lowercase();
        let sub = s.sub_state.to_lowercase();
        st == "active" || sub == "running"
    });

    let is_active = !matching_procs.is_empty() || is_service_active;

    // Aggregate metrics
    let primary_proc = matching_procs
        .iter()
        .max_by(|a, b| a.cpu_usage.partial_cmp(&b.cpu_usage).unwrap_or(std::cmp::Ordering::Equal))
        .copied();

    let total_cpu: f32 = matching_procs.iter().map(|p| p.cpu_usage).sum();
    let total_mem: u64 = matching_procs.iter().map(|p| p.memory_bytes).sum();

    // Border and background colors based on active state
    let (card_bg, card_border) = if is_active {
        (
            Color32::from_rgb(18, 26, 24),
            Color32::from_rgb(36, 74, 52),
        )
    } else {
        (
            Color32::from_rgb(20, 23, 28),
            Color32::from_rgb(45, 50, 62),
        )
    };

    egui::Frame::new()
        .fill(card_bg)
        .stroke(egui::Stroke::new(1.0_f32, card_border))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            // Header Row: Icon, Name, Type Badge, Status Badge, Edit & Delete buttons
            ui.horizontal(|ui| {
                ui.label(RichText::new(&app.icon).size(18.0));
                ui.label(RichText::new(&app.name).size(15.0).strong().color(Color32::from_rgb(230, 240, 255)));

                // Type Badge
                let (badge_text, badge_bg, badge_fg) = get_type_badge(app);
                egui::Frame::new()
                    .fill(badge_bg)
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(6, 2))
                    .show(ui, |ui| {
                        ui.label(RichText::new(badge_text).color(badge_fg).size(11.0).strong());
                    });

                // Status Badge
                let (status_text, status_bg, status_fg) = if is_active {
                    (
                        "● En cours",
                        Color32::from_rgb(26, 60, 40),
                        Color32::from_rgb(46, 204, 113),
                    )
                } else {
                    (
                        "○ Arrêté",
                        Color32::from_rgb(38, 42, 50),
                        Color32::from_rgb(140, 150, 160),
                    )
                };

                egui::Frame::new()
                    .fill(status_bg)
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(7, 2))
                    .show(ui, |ui| {
                        ui.label(RichText::new(status_text).color(status_fg).size(11.0).strong());
                    });

                // Right-aligned edit & remove buttons
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("🗑 Supprimer").on_hover_text("Supprimer ce favori de la liste").clicked() {
                        action = Some(FavoritesPanelAction::RemoveFavorite(app.id.clone()));
                    }

                    if ui.button("✏ Modifier").on_hover_text("Modifier les paramètres de ce favori").clicked() {
                        state.open_edit(app);
                    }
                });
            });

            // Description / Target Row
            if !app.description.is_empty() {
                ui.add_space(2.0);
                ui.label(RichText::new(&app.description).size(12.0).weak());
            }

            ui.add_space(4.0);

            // Technical details row
            ui.horizontal_wrapped(|ui| {
                if !app.process_filter.is_empty() {
                    ui.label(RichText::new(format!("Filtre: {}", app.process_filter)).size(11.0).weak());
                    ui.separator();
                }
                if let Some(srv) = &app.service_name {
                    if !srv.is_empty() {
                        ui.label(RichText::new(format!("Service: {srv}")).size(11.0).weak());
                        ui.separator();
                    }
                }
                if let Some(auto_cmd) = &app.auto_start_command {
                    if !auto_cmd.is_empty() {
                        ui.label(RichText::new(format!("Auto-start: {auto_cmd}")).size(11.0).weak());
                    }
                }
            });

            ui.add_space(6.0);

            // Live metrics & controls
            if is_active {
                // Active Section
                ui.horizontal(|ui| {
                    if let Some(p) = primary_proc {
                        ui.label(
                            RichText::new(format!("PID: {}", p.pid))
                                .strong()
                                .size(12.0)
                                .color(Color32::from_rgb(100, 200, 255)),
                        );

                        if matching_procs.len() > 1 {
                            ui.label(
                                RichText::new(format!("(+{} instances)", matching_procs.len() - 1))
                                    .size(11.0)
                                    .weak(),
                            );
                        }

                        ui.separator();
                    } else if is_service_active {
                        if let Some(s) = matching_services.first() {
                            ui.label(
                                RichText::new(format!("Service actif: {}", s.name))
                                    .strong()
                                    .size(12.0)
                                    .color(Color32::from_rgb(100, 200, 255)),
                            );
                            ui.separator();
                        }
                    }

                    // CPU Metric
                    ui.label(
                        RichText::new(format!("CPU: {:.1}%", total_cpu))
                            .strong()
                            .size(12.0)
                            .color(if total_cpu > 20.0 {
                                Color32::from_rgb(240, 160, 40)
                            } else {
                                Color32::from_rgb(46, 204, 113)
                            }),
                    );
                    ui.separator();

                    // RAM Metric
                    ui.label(
                        RichText::new(format!("RAM: {}", format_ram(total_mem)))
                            .strong()
                            .size(12.0)
                            .color(Color32::from_rgb(100, 180, 240)),
                    );

                    // Actions on right
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Tuer (SIGKILL 9)
                        if let Some(p) = primary_proc {
                            let kill_btn = ui.button(
                                RichText::new("🛑 Tuer")
                                    .color(Color32::from_rgb(255, 100, 100))
                                    .strong(),
                            );
                            if kill_btn.on_hover_text(format!("Envoyer SIGKILL (9) au PID {}", p.pid)).clicked() {
                                action = Some(FavoritesPanelAction::KillProcess {
                                    pid: p.pid,
                                    signal: 9,
                                });
                            }
                        }

                        // Arrêter (SIGTERM 15 ou service stop)
                        let stop_btn = ui.button("⏹ Arrêter");
                        if let Some(p) = primary_proc {
                            if stop_btn.on_hover_text(format!("Envoyer SIGTERM (15) au PID {}", p.pid)).clicked() {
                                action = Some(FavoritesPanelAction::KillProcess {
                                    pid: p.pid,
                                    signal: 15,
                                });
                            }
                        } else if let Some(s) = matching_services.first() {
                            if stop_btn.on_hover_text(format!("Arrêter le service {} (systemctl stop)", s.name)).clicked() {
                                action = Some(FavoritesPanelAction::ControlService {
                                    service_name: s.name.clone(),
                                    action: "stop".to_string(),
                                });
                            }
                        }

                        // Terminal dédié
                        if !app.terminal_command.trim().is_empty() {
                            let term_btn = ui.button(
                                RichText::new("💻 Terminal dédié")
                                    .color(Color32::from_rgb(120, 200, 255))
                                    .strong(),
                            );
                            if term_btn.on_hover_text(format!("Ouvrir le terminal et exécuter: {}", app.terminal_command)).clicked() {
                                action = Some(FavoritesPanelAction::AttachTerminal {
                                    command: app.terminal_command.clone(),
                                });
                            }
                        }
                    });
                });
            } else {
                // Stopped Section
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Application ou processus actuellement arrêté.")
                            .size(12.0)
                            .weak(),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Start Button (if auto_start_command or service_name is present)
                        if let Some(cmd) = &app.auto_start_command {
                            if !cmd.trim().is_empty() {
                                let start_btn = ui.button(
                                    RichText::new("▶ Démarrer")
                                        .color(Color32::from_rgb(46, 204, 113))
                                        .strong(),
                                );
                                if start_btn.on_hover_text(format!("Démarrer l'application avec: {cmd}")).clicked() {
                                    if app.app_type == FavoriteAppType::Service && cmd.starts_with("systemctl start ") {
                                        let s_name = cmd.trim_start_matches("systemctl start ").trim().to_string();
                                        action = Some(FavoritesPanelAction::ControlService {
                                            service_name: s_name,
                                            action: "start".to_string(),
                                        });
                                    } else {
                                        action = Some(FavoritesPanelAction::AttachTerminal {
                                            command: cmd.clone(),
                                        });
                                    }
                                }
                            }
                        } else if app.app_type == FavoriteAppType::Service || app.service_name.is_some() {
                            let sname = app.service_name.as_deref().unwrap_or(&app.name);
                            let start_btn = ui.button(
                                RichText::new("▶ Démarrer le service")
                                    .color(Color32::from_rgb(46, 204, 113))
                                    .strong(),
                            );
                            if start_btn.on_hover_text(format!("Démarrer le service {sname} (systemctl start)")).clicked() {
                                action = Some(FavoritesPanelAction::ControlService {
                                    service_name: sname.to_string(),
                                    action: "start".to_string(),
                                });
                            }
                        }

                        // Terminal Shortcut
                        if !app.terminal_command.trim().is_empty()
                            && ui.button("💻 Terminal").on_hover_text(format!("Ouvrir le terminal distant avec la commande: {}", app.terminal_command)).clicked()
                        {
                            action = Some(FavoritesPanelAction::AttachTerminal {
                                command: app.terminal_command.clone(),
                            });
                        }
                    });
                });
            }
        });

    action
}

/// Render Add/Edit Modal Dialog Window.
fn render_add_edit_modal(
    ui: &mut Ui,
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
        .default_width(520.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ui.ctx(), |ui| {
            // Quick template buttons at top of modal
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Modèles rapides:").strong().size(12.0));
                if ui.small_button("⛏️ Minecraft").clicked() {
                    state.apply_minecraft_template();
                }
                if ui.small_button("🐳 Docker").clicked() {
                    state.apply_docker_template();
                }
                if ui.small_button("🌐 Web").clicked() {
                    state.apply_web_template();
                }
                if ui.small_button("🗄️ Database").clicked() {
                    state.apply_db_template();
                }
            });

            // Process importer picker
            if !device.process_state.processes.is_empty() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("⚡ Importer depuis processus actif:").size(12.0).weak());
                    let sel_label = state.selected_from_proc_pid
                        .and_then(|pid| device.process_state.processes.iter().find(|p| p.pid == pid))
                        .map(|p| format!("{} (PID: {})", p.name, p.pid))
                        .unwrap_or_else(|| "Choisir dans la liste...".to_string());

                    egui::ComboBox::from_id_salt("modal_proc_selector")
                        .selected_text(sel_label)
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for p in &device.process_state.processes {
                                let label = format!("{} (PID: {}, CPU: {:.1}%)", p.name, p.pid, p.cpu_usage);
                                let is_sel = state.selected_from_proc_pid == Some(p.pid);
                                if ui.selectable_label(is_sel, label).clicked() {
                                    state.selected_from_proc_pid = Some(p.pid);
                                    state.apply_process_template(p);
                                }
                            }
                        });
                });
            }

            ui.separator();
            ui.add_space(4.0);

            // Form inputs
            egui::Grid::new("fav_modal_form_grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    // Name
                    ui.label(RichText::new("Nom du favori *").strong());
                    ui.text_edit_singleline(&mut state.name_input);
                    ui.end_row();

                    // Icon
                    ui.label("Icône / Emoji");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut state.icon_input).desired_width(40.0));
                        for emoji in &["⭐", "⛏️", "🐳", "🌐", "🗄️", "⚡", "🚀", "🎮", "📦", "🔥"] {
                            if ui.small_button(*emoji).clicked() {
                                state.icon_input = (*emoji).to_string();
                            }
                        }
                    });
                    ui.end_row();

                    // Type
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

                    // Process filter
                    ui.label("Filtre processus");
                    ui.vertical(|ui| {
                        ui.text_edit_singleline(&mut state.process_filter_input);
                        ui.label(
                            RichText::new("Nom de l'exécutable, argument ou PID numérique (ex: java, nginx, 1234)")
                                .weak()
                                .size(10.5),
                        );
                    });
                    ui.end_row();

                    // Service Name
                    ui.label("Nom service systemd");
                    ui.vertical(|ui| {
                        ui.text_edit_singleline(&mut state.service_name_input);
                        ui.label(RichText::new("Optionnel si service (ex: nginx.service, postgresql.service)").weak().size(10.5));
                    });
                    ui.end_row();

                    // Terminal command
                    ui.label("Commande terminal");
                    ui.vertical(|ui| {
                        ui.text_edit_singleline(&mut state.terminal_command_input);
                        ui.label(
                            RichText::new("Exécutée dans le terminal dédié (ex: screen -r mc, docker logs -f myapp)")
                                .weak()
                                .size(10.5),
                        );
                    });
                    ui.end_row();

                    // Auto-start command
                    ui.label("Commande démarrage");
                    ui.vertical(|ui| {
                        ui.text_edit_singleline(&mut state.auto_start_command_input);
                        ui.label(
                            RichText::new("Exécutée pour démarrer l'application si arrêtée (ex: systemctl start nginx, docker compose up -d)")
                                .weak()
                                .size(10.5),
                        );
                    });
                    ui.end_row();

                    // Description
                    ui.label("Description");
                    ui.text_edit_multiline(&mut state.description_input);
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);

            // Modal footer buttons
            ui.horizontal(|ui| {
                let can_save = !state.name_input.trim().is_empty();
                let save_btn = ui.add_enabled(
                    can_save,
                    egui::Button::new(
                        RichText::new(if is_edit { "💾 Mettre à jour" } else { "➕ Ajouter" })
                            .strong()
                            .color(Color32::from_rgb(255, 255, 255)),
                    ),
                );

                if save_btn.clicked() {
                    let id = state.edit_id.clone().unwrap_or_else(|| {
                        let ts = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0);
                        format!("fav_{ts}")
                    });

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
                        if state.icon_input.trim().is_empty() { "⭐" } else { state.icon_input.trim() },
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

/// Helper to get badge label and color for FavoriteAppType.
fn get_type_badge(app: &FavoriteApp) -> (&'static str, Color32, Color32) {
    match app.app_type {
        FavoriteAppType::Docker => (
            "🐳 Docker",
            Color32::from_rgb(25, 45, 70),
            Color32::from_rgb(80, 180, 255),
        ),
        FavoriteAppType::Service => (
            "⚙ Service",
            Color32::from_rgb(50, 40, 25),
            Color32::from_rgb(240, 180, 70),
        ),
        FavoriteAppType::Process => {
            if app.icon == "⛏️" || app.name.to_lowercase().contains("minecraft") {
                (
                    "⛏️ Minecraft",
                    Color32::from_rgb(35, 55, 30),
                    Color32::from_rgb(120, 220, 110),
                )
            } else {
                (
                    "⚡ Processus",
                    Color32::from_rgb(45, 35, 60),
                    Color32::from_rgb(200, 140, 255),
                )
            }
        }
        FavoriteAppType::Custom => (
            "✨ Personnalisé",
            Color32::from_rgb(35, 45, 55),
            Color32::from_rgb(170, 190, 210),
        ),
    }
}

/// Helper to format RAM in human-readable units.
fn format_ram(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 {
        format!("{:.2} Go", mb / 1024.0)
    } else {
        format!("{:.1} Mo", mb)
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
        assert_eq!(state.icon_input, "⛏️");
        assert_eq!(state.app_type_input, FavoriteAppType::Process);
        assert_eq!(state.process_filter_input, "minecraft_server");
        assert!(state.auto_start_command_input.contains("server.jar"));

        state.apply_docker_template();
        assert_eq!(state.icon_input, "🐳");
        assert_eq!(state.app_type_input, FavoriteAppType::Docker);
        assert_eq!(state.process_filter_input, "docker");

        state.apply_web_template();
        assert_eq!(state.icon_input, "🌐");
        assert_eq!(state.app_type_input, FavoriteAppType::Service);
        assert_eq!(state.service_name_input, "nginx.service");

        state.apply_db_template();
        assert_eq!(state.icon_input, "🗄️");
        assert_eq!(state.app_type_input, FavoriteAppType::Service);
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
        assert_eq!(state.icon_input, "⚡");
        assert_eq!(state.process_filter_input, "custom_proc");
        assert_eq!(state.terminal_command_input, "top -p 4321");
    }

    #[test]
    fn test_favorites_panel_open_edit() {
        let app = FavoriteApp::new(
            "fav_1",
            "Mon App",
            "🚀",
            FavoriteAppType::Custom,
            "filter_x",
            Some("srv_x.service".to_string()),
            "sh attach.sh",
            "Ma super app",
            Some("sh run.sh".to_string()),
        );

        let mut state = FavoritesPanelState::new();
        state.open_edit(&app);

        assert!(state.show_add_modal);
        assert_eq!(state.edit_id, Some("fav_1".to_string()));
        assert_eq!(state.name_input, "Mon App");
        assert_eq!(state.icon_input, "🚀");
        assert_eq!(state.app_type_input, FavoriteAppType::Custom);
        assert_eq!(state.process_filter_input, "filter_x");
        assert_eq!(state.service_name_input, "srv_x.service");
        assert_eq!(state.terminal_command_input, "sh attach.sh");
        assert_eq!(state.auto_start_command_input, "sh run.sh");
    }

    #[test]
    fn test_get_type_badge() {
        let app_docker = FavoriteApp {
            app_type: FavoriteAppType::Docker,
            ..Default::default()
        };
        let (label, _, _) = get_type_badge(&app_docker);
        assert_eq!(label, "🐳 Docker");

        let app_service = FavoriteApp {
            app_type: FavoriteAppType::Service,
            ..Default::default()
        };
        let (label, _, _) = get_type_badge(&app_service);
        assert_eq!(label, "⚙ Service");

        let app_minecraft = FavoriteApp {
            app_type: FavoriteAppType::Process,
            icon: "⛏️".to_string(),
            ..Default::default()
        };
        let (label, _, _) = get_type_badge(&app_minecraft);
        assert_eq!(label, "⛏️ Minecraft");

        let app_proc = FavoriteApp {
            app_type: FavoriteAppType::Process,
            icon: "⚡".to_string(),
            name: "htop".to_string(),
            ..Default::default()
        };
        let (label, _, _) = get_type_badge(&app_proc);
        assert_eq!(label, "⚡ Processus");
    }

    #[test]
    fn test_format_ram() {
        assert_eq!(format_ram(500 * 1024 * 1024), "500.0 Mo");
        assert_eq!(format_ram(2 * 1024 * 1024 * 1024), "2.00 Go");
    }
}
