use crate::state::{ConnectionStatus, Device};
use crate::tls::format_fingerprint;
use crate::ui::favorites_panel::{self, FavoritesPanelAction};
use crate::ui::process_panel::{self, ProcessPanelAction};
use crate::ui::terminal::{self, TerminalAction};
use egui::{Color32, ProgressBar, RichText, Ui};
use egui_plot::{Corner, Legend, Line, Plot, PlotPoints};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeviceTab {
    #[default]
    Metrics,
    Favorites,
    Processes,
    Terminal,
    Config,
}

pub struct DevicePanelState {
    pub active_tab: DeviceTab,
    pub psk_input: String,
    pub show_psk: bool,
    pub psk_status_message: Option<String>,
    pub show_delete_confirm: bool,
    pub favorites_state: favorites_panel::FavoritesPanelState,
}

impl Default for DevicePanelState {
    fn default() -> Self {
        Self {
            active_tab: DeviceTab::Metrics,
            psk_input: String::new(),
            show_psk: false,
            psk_status_message: None,
            show_delete_confirm: false,
            favorites_state: favorites_panel::FavoritesPanelState::default(),
        }
    }
}

pub enum DevicePanelAction {
    Connect { id: String, psk: Option<String> },
    Disconnect(String),
    Reconnect(String),
    Remove(String),
    SavePsk { id: String, psk: String },
    TrustNewFingerprint { id: String, fingerprint: String },
    ResetFingerprint(String),
    SwitchTab(DeviceTab),
    TerminalActions(Vec<TerminalAction>),
    ProcessAction(ProcessPanelAction),
    FavoritesAction(FavoritesPanelAction),
}

pub fn render(
    ui: &mut Ui,
    state: &mut DevicePanelState,
    device: &mut Device,
    refresh_rate_ms: u32,
    history_capacity: usize,
) -> Option<DevicePanelAction> {
    let mut action = None;

    // 1. Device Header Bar
    let os_icon = if let Some(info) = &device.system_info {
        let os = info.os_name.to_lowercase();
        if os.contains("win") {
            "🪟"
        } else if os.contains("raspbian")
            || os.contains("debian")
            || os.contains("ubuntu")
            || os.contains("arch")
            || os.contains("linux")
        {
            "🐧"
        } else if os.contains("mac") || os.contains("darwin") {
            "🍎"
        } else {
            "🖥️"
        }
    } else {
        "🖥️"
    };

    ui.horizontal(|ui| {
        ui.heading(format!("{} {}", os_icon, device.hostname));

        let (status_text, status_bg, status_fg) = match &device.status {
            ConnectionStatus::Connected => (
                "● Connecté",
                Color32::from_rgb(26, 60, 40),
                Color32::from_rgb(46, 204, 113),
            ),
            ConnectionStatus::Connecting => (
                "◌ Connexion...",
                Color32::from_rgb(30, 48, 70),
                Color32::from_rgb(52, 152, 219),
            ),
            ConnectionStatus::AuthFailed(_) => (
                "✕ Échec Auth",
                Color32::from_rgb(60, 25, 25),
                Color32::from_rgb(231, 76, 60),
            ),
            ConnectionStatus::TlsMismatch { .. } => (
                "⚠ Certificat modifié",
                Color32::from_rgb(75, 25, 25),
                Color32::from_rgb(255, 110, 110),
            ),
            ConnectionStatus::Error(_) => (
                "⚠ Erreur",
                Color32::from_rgb(60, 40, 20),
                Color32::from_rgb(230, 126, 34),
            ),
            ConnectionStatus::Disconnected => (
                "○ Déconnecté",
                Color32::from_rgb(35, 38, 42),
                Color32::from_rgb(140, 145, 155),
            ),
            ConnectionStatus::Discovered => (
                "○ Découvert",
                Color32::from_rgb(50, 45, 20),
                Color32::from_rgb(241, 196, 15),
            ),
        };

        egui::Frame::new()
            .fill(status_bg)
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.label(RichText::new(status_text).color(status_fg).strong().size(12.0));
            });

        ui.label(
            RichText::new(format!("{}:{}", device.primary_address(), device.port))
                .weak()
                .size(12.0),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("🗑 Supprimer")
                .on_hover_text("Supprimer ce serveur de la liste")
                .clicked()
            {
                state.show_delete_confirm = true;
            }

            if device.status.is_connected() {
                if ui
                    .button("🔌 Déconnecter")
                    .on_hover_text("Fermer la connexion gRPC TLS avec ce serveur")
                    .clicked()
                {
                    action = Some(DevicePanelAction::Disconnect(device.id.clone()));
                }
            } else if !matches!(device.status, ConnectionStatus::Connecting)
                && ui
                    .button("⚡ Se connecter")
                    .on_hover_text("Établir la connexion TLS sécurisée")
                    .clicked()
            {
                action = Some(DevicePanelAction::Connect {
                    id: device.id.clone(),
                    psk: device.psk.clone(),
                });
            }
        });
    });

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
                    RichText::new("Les favoris et l'empreinte TLS associés seront supprimés.")
                        .weak()
                        .size(12.0),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(
                            RichText::new("🗑 Supprimer définitivement")
                                .color(Color32::from_rgb(255, 110, 110))
                                .strong(),
                        )
                        .clicked()
                    {
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
            ui.add_space(4.0);
            egui::Frame::new()
                .fill(Color32::from_rgb(65, 22, 22))
                .stroke(egui::Stroke::new(1.5_f32, Color32::from_rgb(231, 76, 60)))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(
                            "⚠ ALERTE DE SÉCURITÉ : L'empreinte TLS de ce serveur a changé !",
                        )
                        .color(Color32::from_rgb(255, 130, 130))
                        .strong(),
                    );
                    ui.label(
                        RichText::new(
                            "Cela arrive si le serveur a été réinstallé ou en cas d'interception réseau (MITM).",
                        )
                        .size(12.0),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!("Attendue : {}", format_fingerprint(expected)))
                            .monospace()
                            .size(11.0),
                    );
                    ui.label(
                        RichText::new(format!("Reçue    : {}", format_fingerprint(actual)))
                            .monospace()
                            .size(11.0)
                            .color(Color32::from_rgb(255, 210, 120)),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button("✅ Accepter la nouvelle empreinte et reconnecter")
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
            ui.add_space(4.0);
            egui::Frame::new()
                .fill(Color32::from_rgb(55, 30, 22))
                .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(230, 126, 34)))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!("⚠ {reason}"))
                                .color(Color32::from_rgb(255, 180, 120))
                                .size(12.0),
                        );
                    });
                });
        }
        _ => {}
    }

    ui.add_space(4.0);

    // 2. Tab Navigation Bar
    ui.horizontal(|ui| {
        if ui
            .selectable_label(
                state.active_tab == DeviceTab::Metrics,
                "📊 Métriques & Graphiques",
            )
            .clicked()
        {
            state.active_tab = DeviceTab::Metrics;
            action = Some(DevicePanelAction::SwitchTab(DeviceTab::Metrics));
        }

        let fav_label = format!("⭐ Applications & Favoris ({})", device.favorites.len());
        if ui
            .selectable_label(state.active_tab == DeviceTab::Favorites, fav_label)
            .clicked()
        {
            state.active_tab = DeviceTab::Favorites;
            action = Some(DevicePanelAction::SwitchTab(DeviceTab::Favorites));
        }

        if ui
            .selectable_label(
                state.active_tab == DeviceTab::Processes,
                "⚡ Processus & Services",
            )
            .clicked()
        {
            state.active_tab = DeviceTab::Processes;
            action = Some(DevicePanelAction::SwitchTab(DeviceTab::Processes));
        }

        if ui
            .selectable_label(
                state.active_tab == DeviceTab::Terminal,
                "💻 Terminal distant",
            )
            .clicked()
        {
            state.active_tab = DeviceTab::Terminal;
            action = Some(DevicePanelAction::SwitchTab(DeviceTab::Terminal));
        }

        if ui
            .selectable_label(
                state.active_tab == DeviceTab::Config,
                "⚙ Système & Paramètres",
            )
            .clicked()
        {
            state.active_tab = DeviceTab::Config;
            state.psk_input = device.psk.clone().unwrap_or_default();
            action = Some(DevicePanelAction::SwitchTab(DeviceTab::Config));
        }
    });

    ui.separator();

    let cpu_count = device
        .system_info
        .as_ref()
        .map(|s| s.cpu_count.max(1))
        .unwrap_or(1);

    // 3. Tab Body
    match state.active_tab {
        DeviceTab::Metrics => {
            render_metrics_tab(ui, device, refresh_rate_ms, history_capacity, &mut action);
        }
        DeviceTab::Favorites => {
            if let Some(act) = favorites_panel::render(ui, &mut state.favorites_state, device) {
                action = Some(DevicePanelAction::FavoritesAction(act));
            }
        }
        DeviceTab::Processes => {
            if let Some(act) = process_panel::render(
                ui,
                &mut device.process_state,
                device.status.is_connected(),
                cpu_count,
            ) {
                action = Some(DevicePanelAction::ProcessAction(act));
            }
        }
        DeviceTab::Terminal => {
            render_terminal_tab(ui, device, &mut action);
        }
        DeviceTab::Config => {
            render_config_tab(ui, state, device, &mut action);
        }
    }

    action
}

fn render_metrics_tab(
    ui: &mut Ui,
    device: &Device,
    refresh_rate_ms: u32,
    history_capacity: usize,
    action: &mut Option<DevicePanelAction>,
) {
    if !device.status.is_connected() {
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
                    "Connectez-vous pour visualiser le monitoring CPU, mémoire vive, réseau et stockage en direct.",
                )
                .weak(),
            );
            ui.add_space(12.0);
            if !matches!(device.status, ConnectionStatus::Connecting)
                && ui.button("⚡ Se connecter maintenant").clicked()
            {
                *action = Some(DevicePanelAction::Connect {
                    id: device.id.clone(),
                    psk: device.psk.clone(),
                });
            }
        });
        return;
    }

    let step_secs = (refresh_rate_ms.max(250) as f64) / 1000.0;
    let window_secs = (history_capacity.max(10) as f64) * step_secs;

    egui::ScrollArea::vertical().show(ui, |ui| {
        let cur_cpu = device.cpu_history.back().copied().unwrap_or(0.0);
        let cur_ram = device.ram_history.back().copied().unwrap_or(0.0);
        let cur_rx = device.rx_history.back().copied().unwrap_or(0.0);
        let cur_tx = device.tx_history.back().copied().unwrap_or(0.0);

        let (used_ram_gb, total_ram_gb) = if let Some(m) = &device.latest_metrics {
            (
                m.used_memory_bytes as f64 / 1_073_741_824.0,
                m.total_memory_bytes as f64 / 1_073_741_824.0,
            )
        } else {
            (0.0, 0.0)
        };

        ui.columns(4, |cols| {
            cols[0].group(|ui| {
                ui.label(RichText::new("PROCESSEUR").size(11.0).weak());
                ui.label(
                    RichText::new(format!("{:.1}%", cur_cpu))
                        .size(20.0)
                        .strong()
                        .color(Color32::from_rgb(0, 200, 255)),
                );
                ui.label(RichText::new("Utilisation totale").size(10.5).weak());
            });

            cols[1].group(|ui| {
                ui.label(RichText::new("MÉMOIRE VIVE").size(11.0).weak());
                ui.label(
                    RichText::new(format!("{:.1}%", cur_ram))
                        .size(20.0)
                        .strong()
                        .color(Color32::from_rgb(180, 100, 240)),
                );
                ui.label(
                    RichText::new(format!("{:.1} / {:.1} Go", used_ram_gb, total_ram_gb))
                        .size(10.5)
                        .weak(),
                );
            });

            cols[2].group(|ui| {
                ui.label(RichText::new("DÉBIT ENTRANT (↓)").size(11.0).weak());
                ui.label(
                    RichText::new(format_bytes_rate(cur_rx))
                        .size(20.0)
                        .strong()
                        .color(Color32::from_rgb(46, 204, 113)),
                );
                ui.label(RichText::new("Réception").size(10.5).weak());
            });

            cols[3].group(|ui| {
                ui.label(RichText::new("DÉBIT SORTANT (↑)").size(11.0).weak());
                ui.label(
                    RichText::new(format_bytes_rate(cur_tx))
                        .size(20.0)
                        .strong()
                        .color(Color32::from_rgb(230, 126, 34)),
                );
                ui.label(RichText::new("Émission").size(10.5).weak());
            });
        });

        ui.add_space(10.0);

        ui.columns(2, |columns| {
            columns[0].vertical(|ui| {
                ui.label(RichText::new("📈 Utilisation du Processeur (%)").strong().size(13.0));

                let n = device.cpu_history.len();
                let points: Vec<[f64; 2]> = device
                    .cpu_history
                    .iter()
                    .enumerate()
                    .map(|(i, &val)| {
                        let sec_ago = if n > 0 {
                            -((n - 1 - i) as f64 * step_secs)
                        } else {
                            0.0
                        };
                        [sec_ago, val]
                    })
                    .collect();

                let line = Line::new(PlotPoints::new(points))
                    .name("CPU %")
                    .color(Color32::from_rgb(0, 200, 255))
                    .fill(0.0_f32)
                    .width(2.0_f32);

                Plot::new("cpu_graph")
                    .height(170.0)
                    .include_x(-window_secs)
                    .include_x(0.0)
                    .include_y(0.0)
                    .include_y(100.0)
                    .allow_zoom(false)
                    .allow_drag(false)
                    .allow_scroll(false)
                    .allow_boxed_zoom(false)
                    .legend(Legend::default().position(Corner::RightTop))
                    .show(ui, |plot_ui| {
                        plot_ui.line(line);
                    });
            });

            columns[1].vertical(|ui| {
                ui.label(
                    RichText::new("📈 Utilisation de la Mémoire RAM (%)")
                        .strong()
                        .size(13.0),
                );

                let n = device.ram_history.len();
                let points: Vec<[f64; 2]> = device
                    .ram_history
                    .iter()
                    .enumerate()
                    .map(|(i, &val)| {
                        let sec_ago = if n > 0 {
                            -((n - 1 - i) as f64 * step_secs)
                        } else {
                            0.0
                        };
                        [sec_ago, val]
                    })
                    .collect();

                let line = Line::new(PlotPoints::new(points))
                    .name("RAM %")
                    .color(Color32::from_rgb(180, 100, 240))
                    .fill(0.0_f32)
                    .width(2.0_f32);

                Plot::new("ram_graph")
                    .height(170.0)
                    .include_x(-window_secs)
                    .include_x(0.0)
                    .include_y(0.0)
                    .include_y(100.0)
                    .allow_zoom(false)
                    .allow_drag(false)
                    .allow_scroll(false)
                    .allow_boxed_zoom(false)
                    .legend(Legend::default().position(Corner::RightTop))
                    .show(ui, |plot_ui| {
                        plot_ui.line(line);
                    });
            });
        });

        ui.add_space(8.0);

        ui.label(
            RichText::new(format!(
                "🌐 Débit Réseau — Entrant: {}  |  Sortant: {}",
                format_bytes_rate(cur_rx),
                format_bytes_rate(cur_tx)
            ))
            .strong()
            .size(13.0),
        );

        let n = device.rx_history.len();
        let rx_points: Vec<[f64; 2]> = device
            .rx_history
            .iter()
            .enumerate()
            .map(|(i, &val)| {
                let sec_ago = if n > 0 {
                    -((n - 1 - i) as f64 * step_secs)
                } else {
                    0.0
                };
                [sec_ago, val / 1024.0]
            })
            .collect();

        let tx_points: Vec<[f64; 2]> = device
            .tx_history
            .iter()
            .enumerate()
            .map(|(i, &val)| {
                let sec_ago = if n > 0 {
                    -((n - 1 - i) as f64 * step_secs)
                } else {
                    0.0
                };
                [sec_ago, val / 1024.0]
            })
            .collect();

        let rx_line = Line::new(PlotPoints::new(rx_points))
            .name("Entrant (Ko/s)")
            .color(Color32::from_rgb(46, 204, 113))
            .fill(0.0_f32)
            .width(2.0_f32);

        let tx_line = Line::new(PlotPoints::new(tx_points))
            .name("Sortant (Ko/s)")
            .color(Color32::from_rgb(230, 126, 34))
            .width(2.0_f32);

        Plot::new("net_graph")
            .height(130.0)
            .include_x(-window_secs)
            .include_x(0.0)
            .include_y(0.0)
            .include_y(10.0)
            .allow_zoom(false)
            .allow_drag(false)
            .allow_scroll(false)
            .allow_boxed_zoom(false)
            .legend(Legend::default().position(Corner::RightTop))
            .show(ui, |plot_ui| {
                plot_ui.line(rx_line);
                plot_ui.line(tx_line);
            });

        ui.add_space(12.0);

        // Disks / Storage Section (prefer live disk_metrics, fallback to static system_info.disks)
        let live_disks = device
            .latest_metrics
            .as_ref()
            .map(|m| m.disk_metrics.as_slice())
            .unwrap_or(&[]);

        if !live_disks.is_empty() {
            ui.heading("💾 Stockage & Disques (en direct)");
            egui::Grid::new("disks_grid")
                .striped(true)
                .min_col_width(80.0)
                .spacing([18.0, 8.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Point de montage").strong());
                    ui.label(RichText::new("Système").strong());
                    ui.label(RichText::new("Total").strong());
                    ui.label(RichText::new("Utilisé").strong());
                    ui.label(RichText::new("Disponible").strong());
                    ui.label(RichText::new("I/O (L / É)").strong());
                    ui.label(RichText::new("Occupation").strong());
                    ui.end_row();

                    for dm in live_disks {
                        let fs_name = device
                            .system_info
                            .as_ref()
                            .and_then(|info| {
                                info.disks
                                    .iter()
                                    .find(|d| d.mount_point == dm.mount_point)
                                    .map(|d| d.filesystem.as_str())
                            })
                            .unwrap_or("—");

                        let total_gb = dm.total_bytes as f64 / 1_073_741_824.0;
                        let used_gb = dm.used_bytes as f64 / 1_073_741_824.0;
                        let avail_gb = dm.total_bytes.saturating_sub(dm.used_bytes) as f64
                            / 1_073_741_824.0;
                        let pct = if dm.total_bytes > 0 {
                            (dm.used_bytes as f64 / dm.total_bytes as f64) as f32
                        } else {
                            0.0
                        };

                        ui.label(&dm.mount_point);
                        ui.label(fs_name);
                        ui.label(format!("{:.1} Go", total_gb));
                        ui.label(format!("{:.1} Go", used_gb));
                        ui.label(format!("{:.1} Go", avail_gb));
                        ui.label(format!(
                            "L {} / É {}",
                            format_bytes_rate(dm.read_bytes_per_sec as f64),
                            format_bytes_rate(dm.write_bytes_per_sec as f64)
                        ));

                        let bar = ProgressBar::new(pct.clamp(0.0, 1.0))
                            .text(format!("{:.0}%", pct * 100.0))
                            .desired_width(110.0);
                        ui.add(bar);
                        ui.end_row();
                    }
                });

            ui.add_space(12.0);
        } else if let Some(info) = &device.system_info {
            if !info.disks.is_empty() {
                ui.heading("💾 Stockage & Disques");
                egui::Grid::new("disks_grid_static")
                    .striped(true)
                    .min_col_width(80.0)
                    .spacing([20.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Point de montage").strong());
                        ui.label(RichText::new("Système de fichiers").strong());
                        ui.label(RichText::new("Total").strong());
                        ui.label(RichText::new("Utilisé").strong());
                        ui.label(RichText::new("Disponible").strong());
                        ui.label(RichText::new("Taux").strong());
                        ui.end_row();

                        for disk in &info.disks {
                            let total_gb = disk.total_bytes as f64 / 1_073_741_824.0;
                            let avail_gb = disk.available_bytes as f64 / 1_073_741_824.0;
                            let used_gb = total_gb - avail_gb;
                            let pct = if total_gb > 0.0 {
                                (used_gb / total_gb) as f32
                            } else {
                                0.0
                            };

                            ui.label(&disk.mount_point);
                            ui.label(&disk.filesystem);
                            ui.label(format!("{:.1} Go", total_gb));
                            ui.label(format!("{:.1} Go", used_gb));
                            ui.label(format!("{:.1} Go", avail_gb));

                            let bar = ProgressBar::new(pct.clamp(0.0, 1.0))
                                .text(format!("{:.0}%", pct * 100.0))
                                .desired_width(120.0);
                            ui.add(bar);
                            ui.end_row();
                        }
                    });

                ui.add_space(12.0);
            }
        }

        // Network Interfaces Section
        if let Some(info) = &device.system_info {
            if !info.networks.is_empty() {
                ui.heading("🖧 Interfaces Réseau");
                egui::Grid::new("networks_grid")
                    .striped(true)
                    .min_col_width(100.0)
                    .spacing([20.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Interface").strong());
                        ui.label(RichText::new("Adresse MAC").strong());
                        ui.label(RichText::new("Adresses IP").strong());
                        ui.label(RichText::new("Débit Actuel").strong());
                        ui.end_row();

                        for net in &info.networks {
                            ui.label(&net.name);
                            ui.label(if net.mac_address.is_empty() {
                                "—".to_string()
                            } else {
                                net.mac_address.clone()
                            });
                            ui.label(net.ip_addresses.join(", "));

                            let throughput = if let Some(m) = &device.latest_metrics {
                                m.network_metrics
                                    .iter()
                                    .find(|nm| nm.interface_name == net.name)
                                    .map(|nm| {
                                        format!(
                                            "↓ {} / ↑ {}",
                                            format_bytes_rate(nm.rx_bytes_per_sec as f64),
                                            format_bytes_rate(nm.tx_bytes_per_sec as f64)
                                        )
                                    })
                                    .unwrap_or_else(|| "—".to_string())
                            } else {
                                "—".to_string()
                            };

                            ui.label(throughput);
                            ui.end_row();
                        }
                    });
            }
        }
    });
}

fn render_terminal_tab(
    ui: &mut Ui,
    device: &mut Device,
    action: &mut Option<DevicePanelAction>,
) {
    if !device.status.is_connected() {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("Le terminal distant nécessite une connexion active.")
                    .size(16.0)
                    .strong(),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Connectez-vous au serveur pour ouvrir une session interactive VT100.",
                )
                .weak(),
            );
            ui.add_space(12.0);
            if !matches!(device.status, ConnectionStatus::Connecting)
                && ui.button("⚡ Se connecter pour ouvrir le terminal").clicked()
            {
                *action = Some(DevicePanelAction::Connect {
                    id: device.id.clone(),
                    psk: device.psk.clone(),
                });
            }
        });
        return;
    }

    let term_actions = terminal::render(ui, device);
    if !term_actions.is_empty() {
        *action = Some(DevicePanelAction::TerminalActions(term_actions));
    }
}

fn render_config_tab(
    ui: &mut Ui,
    state: &mut DevicePanelState,
    device: &Device,
    action: &mut Option<DevicePanelAction>,
) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(6.0);

        // System specs card
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("ℹ️ Informations Système & Spécifications");
            ui.add_space(6.0);

            egui::Grid::new("sys_specs_grid")
                .striped(true)
                .min_col_width(140.0)
                .spacing([20.0, 8.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Nom d'hôte :").strong());
                    ui.label(&device.hostname);
                    ui.end_row();

                    ui.label(RichText::new("Adresse cible :").strong());
                    ui.label(device.grpc_endpoint());
                    ui.end_row();

                    if let Some(info) = &device.system_info {
                        ui.label(RichText::new("Système d'exploitation :").strong());
                        ui.label(format!("{} {}", info.os_name, info.os_version));
                        ui.end_row();

                        ui.label(RichText::new("Architecture :").strong());
                        ui.label(&info.arch);
                        ui.end_row();

                        ui.label(RichText::new("Cœurs Processeur :").strong());
                        ui.label(format!("{} cœurs", info.cpu_count));
                        ui.end_row();

                        ui.label(RichText::new("Mémoire Totale :").strong());
                        let total_gb = info.total_memory_bytes as f64 / 1_073_741_824.0;
                        ui.label(format!("{:.2} Go", total_gb));
                        ui.end_row();
                    }

                    if !device.version.is_empty() {
                        ui.label(RichText::new("Version de l'agent NTX :").strong());
                        ui.label(&device.version);
                        ui.end_row();
                    }
                });
        });

        ui.add_space(14.0);

        // TLS Fingerprint Card
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("🛡️ Sécurité TLS (Épinglage TOFU)");
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "L'empreinte SHA-256 du certificat serveur est vérifiée à chaque connexion.",
                )
                .weak()
                .size(12.0),
            );
            ui.add_space(6.0);

            if let Some(ref fp) = device.cert_fingerprint {
                ui.label(
                    RichText::new(format!("Empreinte épinglée : {}", format_fingerprint(fp)))
                        .monospace()
                        .size(11.5),
                );
                ui.add_space(4.0);
                if ui.button("🔄 Réinitialiser l'empreinte TLS").clicked() {
                    *action = Some(DevicePanelAction::ResetFingerprint(device.id.clone()));
                }
            } else if let Some(ref mdns_fp) = device.mdns_fingerprint {
                ui.label(
                    RichText::new(format!(
                        "Empreinte annoncée (mDNS) : {}",
                        format_fingerprint(mdns_fp)
                    ))
                    .monospace()
                    .size(11.5)
                    .weak(),
                );
            } else {
                ui.label(
                    RichText::new(
                        "Aucune empreinte enregistrée (sera épinglée à la première connexion).",
                    )
                    .weak()
                    .size(12.0),
                );
            }
        });

        ui.add_space(14.0);

        // PSK & Authentication
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("🔑 Authentification & Clé Pré-Partagée (PSK)");
            ui.add_space(4.0);
            ui.label(
                RichText::new("Cette clé est utilisée pour le challenge-response HMAC avec ce serveur. Laissez vide pour utiliser la clé par défaut.")
                    .weak()
                    .size(12.0),
            );

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Clé PSK :");
                if state.show_psk {
                    ui.add(egui::TextEdit::singleline(&mut state.psk_input).desired_width(260.0));
                } else {
                    ui.add(
                        egui::TextEdit::singleline(&mut state.psk_input)
                            .password(true)
                            .desired_width(260.0)
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

                if ui.button("💾 Enregistrer la clé").clicked() {
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
            });

            if let Some(msg) = &state.psk_status_message {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(msg)
                        .color(Color32::from_rgb(46, 204, 113))
                        .size(12.0),
                );
            }
        });

        ui.add_space(14.0);

        // Danger Zone
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(
                RichText::new("⚠️ Zone de Danger")
                    .strong()
                    .color(Color32::from_rgb(231, 76, 60)),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("Supprimer ce serveur de l'application NTX Panel.")
                    .weak()
                    .size(12.0),
            );

            ui.add_space(6.0);
            if ui.button("🗑 Supprimer définitivement cette connexion").clicked() {
                state.show_delete_confirm = true;
            }
        });
    });
}

fn format_bytes_rate(bytes_per_sec: f64) -> String {
    if bytes_per_sec < 1024.0 {
        format!("{:.0} o/s", bytes_per_sec)
    } else if bytes_per_sec < 1024.0 * 1024.0 {
        format!("{:.1} Ko/s", bytes_per_sec / 1024.0)
    } else if bytes_per_sec < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} Mo/s", bytes_per_sec / (1024.0 * 1024.0))
    } else {
        format!("{:.2} Go/s", bytes_per_sec / (1024.0 * 1024.0 * 1024.0))
    }
}
