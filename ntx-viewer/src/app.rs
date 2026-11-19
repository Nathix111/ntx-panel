use crate::client::{ClientConnectError, NtxClient};
use crate::discovery::{start_discovery, DiscoveredDevice, DiscoveryEvent, DiscoveryHandle};
use crate::state::{
    AppSettings, ConnectionStatus, Device, KnownDevice, ManualDevice, TerminalSessionState,
};
use crate::ui::device_panel::{self, DevicePanelAction, DevicePanelState, DeviceTab};
use crate::ui::favorites_panel::FavoritesPanelAction;
use crate::ui::process_panel::ProcessPanelAction;
use crate::ui::settings::{self, SettingsAction, SettingsState};
use crate::ui::sidebar::{self, SidebarAction, SidebarState};
use crate::ui::terminal::TerminalAction;
use ntx_proto::terminal_input::Input;
use ntx_proto::{SystemInfo, SystemMetrics, TerminalInput, TerminalResize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveView {
    Device,
    GlobalSettings,
}

pub enum AppEvent {
    Connected {
        device_id: String,
        generation: u64,
        client: Box<NtxClient>,
        system_info: Box<SystemInfo>,
        fingerprint: String,
    },
    TlsMismatch {
        device_id: String,
        generation: u64,
        expected: String,
        actual: String,
    },
    ConnectionFailed {
        device_id: String,
        generation: u64,
        error: String,
        is_auth_error: bool,
    },
    MetricsReceived {
        device_id: String,
        generation: u64,
        metrics: SystemMetrics,
    },
    TerminalReady {
        device_id: String,
        generation: u64,
    },
    TerminalOutput {
        device_id: String,
        generation: u64,
        data: Vec<u8>,
    },
    TerminalClosed {
        device_id: String,
        generation: u64,
        error: Option<String>,
    },
    Disconnected {
        device_id: String,
        generation: u64,
    },
    ProcessesReceived {
        device_id: String,
        processes: Vec<ntx_proto::ProcessItem>,
    },
    ProcessesFailed {
        device_id: String,
        error: String,
    },
    ProcessKilled {
        device_id: String,
        pid: u32,
        signal: i32,
        success: bool,
        error: String,
    },
    ServicesReceived {
        device_id: String,
        services: Vec<ntx_proto::ServiceItem>,
    },
    ServicesFailed {
        device_id: String,
        error: String,
    },
    ServiceControlled {
        device_id: String,
        service_name: String,
        action: String,
        success: bool,
        error: String,
    },
}

pub struct NtxApp {
    rt: Arc<tokio::runtime::Runtime>,
    egui_ctx: egui::Context,
    active_view: ActiveView,
    selected_device_id: Option<String>,
    device_panel_state: DevicePanelState,
    sidebar_state: SidebarState,
    devices: HashMap<String, Device>,
    active_clients: HashMap<String, NtxClient>,
    connection_gens: HashMap<String, u64>,
    terminal_gens: HashMap<String, u64>,
    settings: AppSettings,
    settings_state: SettingsState,
    settings_warning: Option<String>,
    discovery_handle: Option<DiscoveryHandle>,
    discovery_rx: mpsc::Receiver<DiscoveryEvent>,
    event_tx: mpsc::Sender<AppEvent>,
    event_rx: mpsc::Receiver<AppEvent>,
    terminal_txs: HashMap<String, mpsc::Sender<TerminalInput>>,
    metrics_cancel_txs: HashMap<String, oneshot::Sender<()>>,
}

impl NtxApp {
    pub fn new(cc: &eframe::CreationContext<'_>, rt: Arc<tokio::runtime::Runtime>) -> Self {
        let egui_ctx = cc.egui_ctx.clone();
        let (disc_tx, disc_rx) = mpsc::channel(64);
        let (event_tx, event_rx) = mpsc::channel(256);

        let discovery_handle =
            match start_discovery(disc_tx, rt.handle().clone(), Some(egui_ctx.clone())) {
                Ok(handle) => Some(handle),
                Err(e) => {
                    warn!("Impossible de démarrer la découverte mDNS : {e}");
                    None
                }
            };

        let (settings, settings_warning) = AppSettings::load();
        egui_ctx.set_visuals(if settings.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });

        let mut app = Self {
            rt,
            egui_ctx,
            active_view: ActiveView::Device,
            selected_device_id: None,
            device_panel_state: DevicePanelState::default(),
            sidebar_state: SidebarState::new(),
            devices: HashMap::new(),
            active_clients: HashMap::new(),
            connection_gens: HashMap::new(),
            terminal_gens: HashMap::new(),
            settings,
            settings_state: SettingsState::default(),
            settings_warning,
            discovery_handle,
            discovery_rx: disc_rx,
            event_tx,
            event_rx,
            terminal_txs: HashMap::new(),
            metrics_cancel_txs: HashMap::new(),
        };

        for known in &app.settings.known_devices.clone() {
            let mut dev = Device::new(
                known.id.clone(),
                known.hostname.clone(),
                known.addresses.clone(),
                known.port,
                known.version.clone(),
            );
            dev.psk = known
                .psk
                .clone()
                .filter(|p| !p.trim().is_empty())
                .or_else(|| {
                    app.settings
                        .device_psks
                        .get(&known.id)
                        .filter(|p| !p.trim().is_empty())
                        .cloned()
                });
            dev.cert_fingerprint = known.cert_fingerprint.clone();
            dev.is_manual = known.is_manual;
            dev.status = ConnectionStatus::Disconnected;
            if let Some(favs) = app.settings.favorite_apps.get(&known.id) {
                dev.favorites = favs.clone();
            }
            app.devices.insert(known.id.clone(), dev);
        }

        if !app.devices.is_empty() {
            let first_id = app.devices.keys().next().cloned();
            app.select_device(first_id);
        }

        app
    }

    fn apply_theme(&self) {
        self.egui_ctx.set_visuals(if self.settings.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
    }

    fn select_device(&mut self, id: Option<String>) {
        self.selected_device_id = id.clone();
        self.device_panel_state.psk_status_message = None;
        self.device_panel_state.show_delete_confirm = false;
        self.device_panel_state.favorites_state.show_add_modal = false;
        self.device_panel_state.favorites_state.pending_confirm = None;
        self.device_panel_state.favorites_state.reset_form();

        if let Some(ref dev_id) = id {
            let (psk_val, should_fetch_proc, should_fetch_srv) =
                if let Some(dev) = self.devices.get(dev_id) {
                    let on_proc_or_fav = matches!(
                        self.device_panel_state.active_tab,
                        DeviceTab::Processes | DeviceTab::Favorites
                    );
                    let fetch_proc = on_proc_or_fav
                        && dev.status.is_connected()
                        && dev.process_state.processes.is_empty()
                        && !dev.process_state.is_loading_processes;
                    let fetch_srv = on_proc_or_fav
                        && dev.status.is_connected()
                        && dev.process_state.services.is_empty()
                        && !dev.process_state.is_loading_services;
                    (dev.psk.clone().unwrap_or_default(), fetch_proc, fetch_srv)
                } else {
                    (String::new(), false, false)
                };

            self.device_panel_state.psk_input = psk_val;
            if should_fetch_proc {
                self.fetch_processes(dev_id);
            }
            if should_fetch_srv {
                self.fetch_services(dev_id);
            }
        } else {
            self.device_panel_state.psk_input.clear();
        }
    }

    fn save_known_devices(&mut self) {
        self.settings.known_devices = self
            .devices
            .values()
            .map(|d| KnownDevice {
                id: d.id.clone(),
                hostname: d.hostname.clone(),
                addresses: d.addresses.clone(),
                port: d.port,
                version: d.version.clone(),
                psk: d.psk.clone(),
                cert_fingerprint: d.cert_fingerprint.clone(),
                is_manual: d.is_manual,
            })
            .collect();
        let _ = self.settings.save();
    }

    fn add_manual_device(&mut self, manual: ManualDevice) {
        let existing_id = self.devices.iter().find_map(|(id, d)| {
            if d.port == manual.port
                && (d.addresses.iter().any(|a| a == &manual.address)
                    || d.hostname.eq_ignore_ascii_case(&manual.hostname))
            {
                Some(id.clone())
            } else {
                None
            }
        });

        let target_id = if let Some(id) = existing_id {
            if let Some(dev) = self.devices.get_mut(&id) {
                dev.hostname = manual.hostname;
                if !dev.addresses.contains(&manual.address) {
                    dev.addresses.insert(0, manual.address);
                }
                dev.is_manual = true;
                if let Some(p) = manual.psk.filter(|s| !s.trim().is_empty()) {
                    dev.psk = Some(p.clone());
                    self.settings.device_psks.insert(id.clone(), p);
                }
            }
            id
        } else {
            let id = format!("{}:{}", manual.address, manual.port);
            let mut dev = Device::new(
                id.clone(),
                manual.hostname,
                vec![manual.address],
                manual.port,
                "manuel".to_string(),
            );
            dev.is_manual = true;
            dev.status = ConnectionStatus::Disconnected;
            if let Some(p) = manual.psk.filter(|s| !s.trim().is_empty()) {
                dev.psk = Some(p.clone());
                self.settings.device_psks.insert(id.clone(), p);
            }
            if let Some(favs) = self.settings.favorite_apps.get(&id) {
                dev.favorites = favs.clone();
            }
            self.devices.insert(id.clone(), dev);
            id
        };

        self.save_known_devices();
        self.select_device(Some(target_id));
    }

    fn remove_device(&mut self, device_id: &str) {
        self.disconnect_device(device_id);
        self.devices.remove(device_id);
        self.settings.device_psks.remove(device_id);
        self.settings.favorite_apps.remove(device_id);
        self.save_known_devices();

        if self.selected_device_id.as_deref() == Some(device_id) {
            let next_id = self.devices.keys().next().cloned();
            self.select_device(next_id);
        }
    }

    fn restart_discovery(&mut self) {
        if let Some(mut handle) = self.discovery_handle.take() {
            handle.stop();
        }

        let (disc_tx, disc_rx) = mpsc::channel(64);
        self.discovery_rx = disc_rx;
        match start_discovery(
            disc_tx,
            self.rt.handle().clone(),
            Some(self.egui_ctx.clone()),
        ) {
            Ok(handle) => self.discovery_handle = Some(handle),
            Err(e) => warn!("Échec du redémarrage de la découverte mDNS : {e}"),
        }
    }

    fn resolve_psk(&self, device: &Device) -> Option<String> {
        device
            .psk
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.settings
                    .device_psks
                    .get(&device.id)
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
            })
            .or_else(|| {
                let d = self.settings.default_psk.trim();
                if d.is_empty() {
                    None
                } else {
                    Some(d)
                }
            })
            .map(|s| s.to_string())
    }

    fn next_connection_gen(&mut self, device_id: &str) -> u64 {
        let entry = self.connection_gens.entry(device_id.to_string()).or_insert(0);
        *entry += 1;
        *entry
    }

    fn next_terminal_gen(&mut self, device_id: &str) -> u64 {
        let entry = self.terminal_gens.entry(device_id.to_string()).or_insert(0);
        *entry += 1;
        *entry
    }

    fn initiate_connect(&mut self, device_id: &str, explicit_psk: Option<String>) {
        if let Some(p) = explicit_psk {
            let trimmed = p.trim().to_string();
            if !trimmed.is_empty() {
                if let Some(d) = self.devices.get_mut(device_id) {
                    d.psk = Some(trimmed.clone());
                }
                self.settings
                    .device_psks
                    .insert(device_id.to_string(), trimmed);
                let _ = self.settings.save();
            }
        }

        let Some(resolved_psk) = self.devices.get(device_id).and_then(|d| self.resolve_psk(d))
        else {
            if let Some(d) = self.devices.get_mut(device_id) {
                d.status = ConnectionStatus::AuthFailed(
                    "Veuillez renseigner une clé PSK dans les paramètres du serveur ou globaux."
                        .to_string(),
                );
            }
            return;
        };

        let gen = self.next_connection_gen(device_id);
        let _ = self.next_terminal_gen(device_id);
        self.active_clients.remove(device_id);
        self.terminal_txs.remove(device_id);
        if let Some(old_tx) = self.metrics_cancel_txs.remove(device_id) {
            let _ = old_tx.send(());
        }

        let (endpoint, expected_fp) = match self.devices.get_mut(device_id) {
            Some(d) => {
                d.status = ConnectionStatus::Connecting;
                d.terminal_state = TerminalSessionState::Idle;
                (d.grpc_endpoint(), d.cert_fingerprint.clone())
            }
            None => return,
        };

        let viewer_id = self.settings.viewer_id.clone();
        let interval_ms = self.settings.refresh_rate_ms;
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();
        let dev_id = device_id.to_string();

        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
        self.metrics_cancel_txs.insert(dev_id.clone(), cancel_tx);

        let rt_handle = self.rt.handle().clone();
        self.rt.spawn(async move {
            info!("Connexion TLS gRPC vers {} ({})", dev_id, endpoint);
            let mut client = match NtxClient::connect_and_authenticate(
                &endpoint,
                &resolved_psk,
                &viewer_id,
                expected_fp,
            )
            .await
            {
                Ok(c) => c,
                Err(ClientConnectError::TlsMismatch { expected, actual }) => {
                    let _ = event_tx
                        .send(AppEvent::TlsMismatch {
                            device_id: dev_id,
                            generation: gen,
                            expected,
                            actual,
                        })
                        .await;
                    ctx.request_repaint();
                    return;
                }
                Err(ClientConnectError::AuthFailed(err)) => {
                    let _ = event_tx
                        .send(AppEvent::ConnectionFailed {
                            device_id: dev_id,
                            generation: gen,
                            error: err,
                            is_auth_error: true,
                        })
                        .await;
                    ctx.request_repaint();
                    return;
                }
                Err(ClientConnectError::RateLimited(err)) => {
                    let _ = event_tx
                        .send(AppEvent::ConnectionFailed {
                            device_id: dev_id,
                            generation: gen,
                            error: format!("Trop de tentatives : {err}"),
                            is_auth_error: true,
                        })
                        .await;
                    ctx.request_repaint();
                    return;
                }
                Err(ClientConnectError::Transport(err)) => {
                    error!("Erreur de connexion vers {}: {}", dev_id, err);
                    let _ = event_tx
                        .send(AppEvent::ConnectionFailed {
                            device_id: dev_id,
                            generation: gen,
                            error: err,
                            is_auth_error: false,
                        })
                        .await;
                    ctx.request_repaint();
                    return;
                }
            };

            let fingerprint = client.cert_fingerprint().to_string();
            let system_info = match client.get_system_info().await {
                Ok(info) => info,
                Err(e) => {
                    warn!("Erreur GetSystemInfo pour {}: {e}", dev_id);
                    SystemInfo::default()
                }
            };

            let stream_client = client.clone();
            let _ = event_tx
                .send(AppEvent::Connected {
                    device_id: dev_id.clone(),
                    generation: gen,
                    client: Box::new(client),
                    system_info: Box::new(system_info),
                    fingerprint,
                })
                .await;
            ctx.request_repaint();

            spawn_metrics_stream_task(
                rt_handle,
                stream_client,
                dev_id,
                gen,
                interval_ms,
                cancel_rx,
                event_tx,
                ctx,
            );
        });
    }

    fn restart_active_metrics_streams(&mut self) {
        let interval_ms = self.settings.refresh_rate_ms;
        let connected_ids: Vec<String> = self
            .active_clients
            .keys()
            .filter(|id| {
                self.devices
                    .get(*id)
                    .map(|d| d.status.is_connected())
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        for dev_id in connected_ids {
            if let Some(old_tx) = self.metrics_cancel_txs.remove(&dev_id) {
                let _ = old_tx.send(());
            }
            if let Some(client) = self.active_clients.get(&dev_id).cloned() {
                let gen = *self.connection_gens.get(&dev_id).unwrap_or(&0);
                let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
                self.metrics_cancel_txs.insert(dev_id.clone(), cancel_tx);
                spawn_metrics_stream_task(
                    self.rt.handle().clone(),
                    client,
                    dev_id,
                    gen,
                    interval_ms,
                    cancel_rx,
                    self.event_tx.clone(),
                    self.egui_ctx.clone(),
                );
            }
        }
    }

    fn disconnect_device(&mut self, device_id: &str) {
        let _ = self.next_connection_gen(device_id);
        let _ = self.next_terminal_gen(device_id);

        if let Some(cancel) = self.metrics_cancel_txs.remove(device_id) {
            let _ = cancel.send(());
        }
        self.active_clients.remove(device_id);
        self.terminal_txs.remove(device_id);
        if let Some(dev) = self.devices.get_mut(device_id) {
            dev.status = ConnectionStatus::Disconnected;
            dev.terminal_state = TerminalSessionState::Idle;
            dev.pending_terminal_command = None;
            dev.process_state.is_loading_processes = false;
            dev.process_state.is_loading_services = false;
        }
    }

    fn open_terminal_session(&mut self, device_id: &str) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            if let Some(dev) = self.devices.get_mut(device_id) {
                dev.terminal_state =
                    TerminalSessionState::Error("Serveur non connecté.".to_string());
            }
            return;
        };

        let gen = self.next_terminal_gen(device_id);
        let (rows, cols) = match self.devices.get_mut(device_id) {
            Some(d) => {
                d.terminal_state = TerminalSessionState::Connecting;
                d.terminal.size()
            }
            None => return,
        };

        let (input_tx, input_rx) = mpsc::channel::<TerminalInput>(128);
        self.terminal_txs
            .insert(device_id.to_string(), input_tx.clone());

        let _ = input_tx.try_send(TerminalInput {
            input: Some(Input::Resize(TerminalResize {
                cols: cols as u32,
                rows: rows as u32,
            })),
        });

        let dev_id = device_id.to_string();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        self.rt.spawn(async move {
            let mut out_stream = match client.open_terminal(input_rx).await {
                Ok(s) => s,
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::TerminalClosed {
                            device_id: dev_id,
                            generation: gen,
                            error: Some(format!("Impossible d'ouvrir le terminal : {e}")),
                        })
                        .await;
                    ctx.request_repaint();
                    return;
                }
            };

            let _ = event_tx
                .send(AppEvent::TerminalReady {
                    device_id: dev_id.clone(),
                    generation: gen,
                })
                .await;
            ctx.request_repaint();

            let mut closed_err = None;
            loop {
                match out_stream.message().await {
                    Ok(Some(output)) => {
                        if !output.data.is_empty() {
                            if event_tx
                                .send(AppEvent::TerminalOutput {
                                    device_id: dev_id.clone(),
                                    generation: gen,
                                    data: output.data,
                                })
                                .await
                                .is_err()
                            {
                                break;
                            }
                            ctx.request_repaint();
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        closed_err = Some(e.to_string());
                        break;
                    }
                }
            }

            let _ = event_tx
                .send(AppEvent::TerminalClosed {
                    device_id: dev_id,
                    generation: gen,
                    error: closed_err,
                })
                .await;
            ctx.request_repaint();
        });
    }

    fn fetch_processes(&mut self, device_id: &str) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            return;
        };

        if let Some(dev) = self.devices.get_mut(device_id) {
            if dev.process_state.is_loading_processes {
                return;
            }
            dev.process_state.is_loading_processes = true;
            dev.process_state.last_process_refresh = Some(std::time::Instant::now());
        }

        let dev_id = device_id.to_string();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        self.rt.spawn(async move {
            match client.list_processes().await {
                Ok(processes) => {
                    let _ = event_tx
                        .send(AppEvent::ProcessesReceived {
                            device_id: dev_id,
                            processes,
                        })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::ProcessesFailed {
                            device_id: dev_id,
                            error: format!("{e:#}"),
                        })
                        .await;
                }
            }
            ctx.request_repaint();
        });
    }

    fn fetch_services(&mut self, device_id: &str) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            return;
        };

        if let Some(dev) = self.devices.get_mut(device_id) {
            if dev.process_state.is_loading_services {
                return;
            }
            dev.process_state.is_loading_services = true;
            dev.process_state.last_service_refresh = Some(std::time::Instant::now());
        }

        let dev_id = device_id.to_string();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        self.rt.spawn(async move {
            match client.list_services().await {
                Ok(services) => {
                    let _ = event_tx
                        .send(AppEvent::ServicesReceived {
                            device_id: dev_id,
                            services,
                        })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::ServicesFailed {
                            device_id: dev_id,
                            error: format!("{e:#}"),
                        })
                        .await;
                }
            }
            ctx.request_repaint();
        });
    }

    fn kill_process(&mut self, device_id: &str, pid: u32, signal: i32) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            if let Some(dev) = self.devices.get_mut(device_id) {
                dev.process_state
                    .set_status("Serveur non connecté.", true);
            }
            return;
        };

        let dev_id = device_id.to_string();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        self.rt.spawn(async move {
            match client.kill_process(pid, signal).await {
                Ok(resp) => {
                    let _ = event_tx
                        .send(AppEvent::ProcessKilled {
                            device_id: dev_id,
                            pid,
                            signal,
                            success: resp.success,
                            error: resp.error_message,
                        })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::ProcessKilled {
                            device_id: dev_id,
                            pid,
                            signal,
                            success: false,
                            error: format!("{e:#}"),
                        })
                        .await;
                }
            }
            ctx.request_repaint();
        });
    }

    fn control_service(&mut self, device_id: &str, service_name: &str, action: &str) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            if let Some(dev) = self.devices.get_mut(device_id) {
                dev.process_state
                    .set_status("Serveur non connecté.", true);
            }
            return;
        };

        let dev_id = device_id.to_string();
        let s_name = service_name.to_string();
        let act = action.to_string();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        self.rt.spawn(async move {
            match client.control_service(&s_name, &act).await {
                Ok(resp) => {
                    let _ = event_tx
                        .send(AppEvent::ServiceControlled {
                            device_id: dev_id,
                            service_name: s_name,
                            action: act,
                            success: resp.success,
                            error: resp.error_message,
                        })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::ServiceControlled {
                            device_id: dev_id,
                            service_name: s_name,
                            action: act,
                            success: false,
                            error: format!("{e:#}"),
                        })
                        .await;
                }
            }
            ctx.request_repaint();
        });
    }

    fn poll_channels(&mut self) {
        while let Ok(event) = self.discovery_rx.try_recv() {
            match event {
                DiscoveryEvent::Added(DiscoveredDevice {
                    fullname,
                    hostname,
                    addresses,
                    port,
                    version,
                    cert_fingerprint,
                }) => {
                    let existing_id = self.devices.iter().find_map(|(id, d)| {
                        if id == &fullname {
                            return Some(id.clone());
                        }
                        if d.port == port {
                            if d.hostname.eq_ignore_ascii_case(&hostname) {
                                return Some(id.clone());
                            }
                            if !addresses.is_empty()
                                && d.addresses.iter().any(|a| addresses.contains(a))
                            {
                                return Some(id.clone());
                            }
                        }
                        None
                    });

                    if let Some(id) = existing_id {
                        if let Some(dev) = self.devices.get_mut(&id) {
                            dev.hostname = hostname;
                            if !addresses.is_empty() {
                                dev.addresses = addresses;
                            }
                            dev.port = port;
                            dev.version = version;
                            if cert_fingerprint.is_some() {
                                dev.mdns_fingerprint = cert_fingerprint;
                            }
                            if dev.status == ConnectionStatus::Disconnected {
                                dev.status = ConnectionStatus::Discovered;
                            }
                        }
                    } else {
                        let id = fullname.clone();
                        let mut dev = Device::new(id.clone(), hostname, addresses, port, version);
                        dev.mdns_fingerprint = cert_fingerprint;
                        if let Some(saved) = self.settings.device_psks.get(&id) {
                            if !saved.trim().is_empty() {
                                dev.psk = Some(saved.clone());
                            }
                        }
                        if let Some(favs) = self.settings.favorite_apps.get(&id) {
                            dev.favorites = favs.clone();
                        }
                        self.devices.insert(id.clone(), dev);
                        if self.selected_device_id.is_none() {
                            self.select_device(Some(id));
                        }
                    }
                    self.save_known_devices();
                }
                DiscoveryEvent::Removed(fullname) => {
                    if let Some(dev) = self.devices.get_mut(&fullname) {
                        if !dev.status.is_connected()
                            && !matches!(dev.status, ConnectionStatus::Connecting)
                        {
                            dev.status = ConnectionStatus::Disconnected;
                        }
                    }
                }
            }
        }

        let cap = self.settings.history_capacity;
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                AppEvent::Connected {
                    device_id,
                    generation,
                    client,
                    system_info,
                    fingerprint,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    self.active_clients.insert(device_id.clone(), *client);
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.status = ConnectionStatus::Connected;
                        dev.system_info = Some(*system_info);
                        dev.cert_fingerprint = Some(fingerprint);
                        if let Some(ref psk) = dev.psk {
                            if !psk.trim().is_empty() {
                                self.settings
                                    .device_psks
                                    .insert(device_id.clone(), psk.clone());
                            }
                        }
                    }
                    self.save_known_devices();

                    if self.selected_device_id.as_deref() == Some(&device_id)
                        && matches!(
                            self.device_panel_state.active_tab,
                            DeviceTab::Processes | DeviceTab::Favorites
                        )
                    {
                        self.fetch_processes(&device_id);
                        self.fetch_services(&device_id);
                    }
                }
                AppEvent::TlsMismatch {
                    device_id,
                    generation,
                    expected,
                    actual,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    self.active_clients.remove(&device_id);
                    self.metrics_cancel_txs.remove(&device_id);
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.status = ConnectionStatus::TlsMismatch { expected, actual };
                    }
                }
                AppEvent::ConnectionFailed {
                    device_id,
                    generation,
                    error,
                    is_auth_error,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    self.active_clients.remove(&device_id);
                    self.metrics_cancel_txs.remove(&device_id);
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.status = if is_auth_error {
                            ConnectionStatus::AuthFailed(error)
                        } else {
                            ConnectionStatus::Error(error)
                        };
                    }
                }
                AppEvent::MetricsReceived {
                    device_id,
                    generation,
                    metrics,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.record_metrics(metrics, cap);
                    }
                }
                AppEvent::TerminalReady {
                    device_id,
                    generation,
                } => {
                    if self.terminal_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.terminal_state = TerminalSessionState::Open;
                        if let Some(cmd) = dev.pending_terminal_command.take() {
                            if !cmd.trim().is_empty() {
                                if let Some(tx) = self.terminal_txs.get(&device_id) {
                                    let mut payload = cmd;
                                    if !payload.ends_with('\n') && !payload.ends_with('\r') {
                                        payload.push('\r');
                                    }
                                    let _ = tx.try_send(TerminalInput {
                                        input: Some(Input::Data(payload.into_bytes())),
                                    });
                                }
                            }
                        }
                    }
                }
                AppEvent::TerminalOutput {
                    device_id,
                    generation,
                    data,
                } => {
                    if self.terminal_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.terminal.process_bytes(&data);
                    }
                }
                AppEvent::TerminalClosed {
                    device_id,
                    generation,
                    error,
                } => {
                    if self.terminal_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    self.terminal_txs.remove(&device_id);
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.terminal_state = match error {
                            Some(err) => {
                                dev.terminal.write_local_notice(&format!(
                                    "\r\n\x1b[31m[Erreur terminal : {err}]\x1b[0m\r\n"
                                ));
                                TerminalSessionState::Error(err)
                            }
                            None => {
                                dev.terminal.write_local_notice(
                                    "\r\n\x1b[33m[Session terminal terminée]\x1b[0m\r\n",
                                );
                                TerminalSessionState::ClosedByUser
                            }
                        };
                    }
                }
                AppEvent::Disconnected {
                    device_id,
                    generation,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    self.active_clients.remove(&device_id);
                    self.terminal_txs.remove(&device_id);
                    self.metrics_cancel_txs.remove(&device_id);
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        if dev.status.is_connected() {
                            dev.status = ConnectionStatus::Disconnected;
                        }
                        if matches!(
                            dev.terminal_state,
                            TerminalSessionState::Open | TerminalSessionState::Connecting
                        ) {
                            dev.terminal_state = TerminalSessionState::ClosedByUser;
                        }
                        dev.process_state.is_loading_processes = false;
                        dev.process_state.is_loading_services = false;
                    }
                }
                AppEvent::ProcessesReceived {
                    device_id,
                    processes,
                } => {
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.process_state.processes = processes;
                        dev.process_state.is_loading_processes = false;
                    }
                }
                AppEvent::ProcessesFailed { device_id, error } => {
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.process_state.is_loading_processes = false;
                        dev.process_state.set_status(
                            format!("Erreur lors de la récupération des processus : {error}"),
                            true,
                        );
                    }
                }
                AppEvent::ProcessKilled {
                    device_id,
                    pid,
                    signal,
                    success,
                    error,
                } => {
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        if success {
                            let sig_name = if signal == 9 { "SIGKILL" } else { "SIGTERM" };
                            dev.process_state.set_status(
                                format!(
                                    "Signal {sig_name} envoyé au processus PID {pid} avec succès."
                                ),
                                false,
                            );
                        } else {
                            dev.process_state.set_status(
                                format!("Échec de l'arrêt du processus PID {pid} : {error}"),
                                true,
                            );
                        }
                    }
                    self.fetch_processes(&device_id);
                }
                AppEvent::ServicesReceived {
                    device_id,
                    services,
                } => {
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.process_state.services = services;
                        dev.process_state.is_loading_services = false;
                    }
                }
                AppEvent::ServicesFailed { device_id, error } => {
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.process_state.is_loading_services = false;
                        dev.process_state.set_status(
                            format!("Erreur lors de la récupération des services : {error}"),
                            true,
                        );
                    }
                }
                AppEvent::ServiceControlled {
                    device_id,
                    service_name,
                    action,
                    success,
                    error,
                } => {
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        if success {
                            dev.process_state.set_status(
                                format!("Action « {action} » appliquée avec succès sur {service_name}."),
                                false,
                            );
                        } else {
                            dev.process_state.set_status(
                                format!("Échec de l'action « {action} » sur {service_name} : {error}"),
                                true,
                            );
                        }
                    }
                    self.fetch_services(&device_id);
                    self.fetch_processes(&device_id);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_metrics_stream_task(
    rt_handle: tokio::runtime::Handle,
    mut client: NtxClient,
    dev_id: String,
    gen: u64,
    interval_ms: u32,
    mut cancel_rx: oneshot::Receiver<()>,
    event_tx: mpsc::Sender<AppEvent>,
    ctx: egui::Context,
) {
    rt_handle.spawn(async move {
        let mut was_cancelled = false;

        match client.stream_metrics(interval_ms).await {
            Ok(mut stream) => loop {
                tokio::select! {
                    _ = &mut cancel_rx => {
                        info!("Streaming de métriques annulé pour {}", dev_id);
                        was_cancelled = true;
                        break;
                    }
                    item = stream.message() => {
                        match item {
                            Ok(Some(metrics)) => {
                                if event_tx.send(AppEvent::MetricsReceived {
                                    device_id: dev_id.clone(),
                                    generation: gen,
                                    metrics,
                                }).await.is_err() {
                                    was_cancelled = true;
                                    break;
                                }
                                ctx.request_repaint();
                            }
                            Ok(None) => {
                                info!("Flux de métriques fermé par le serveur pour {}", dev_id);
                                break;
                            }
                            Err(e) => {
                                warn!("Erreur sur le flux de métriques pour {} : {}", dev_id, e);
                                break;
                            }
                        }
                    }
                }
            },
            Err(e) => {
                warn!("Impossible de démarrer le flux de métriques pour {} : {}", dev_id, e);
            }
        }

        if !was_cancelled {
            let _ = event_tx
                .send(AppEvent::Disconnected {
                    device_id: dev_id,
                    generation: gen,
                })
                .await;
            ctx.request_repaint();
        }
    });
}

impl eframe::App for NtxApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_channels();

        // Auto-refresh processes or favorites on selected connected device
        let mut need_periodic_repaint = self
            .devices
            .values()
            .any(|d| matches!(d.status, ConnectionStatus::Connecting));

        if let Some(ref id) = self.selected_device_id.clone() {
            if let Some(dev) = self.devices.get(id) {
                if dev.status.is_connected() {
                    match self.device_panel_state.active_tab {
                        DeviceTab::Processes => {
                            if dev.process_state.auto_refresh && !dev.process_state.is_paused {
                                need_periodic_repaint = true;
                                if !dev.process_state.is_loading_processes {
                                    let should_refresh = dev
                                        .process_state
                                        .last_process_refresh
                                        .map(|t| {
                                            t.elapsed()
                                                >= Duration::from_secs(
                                                    dev.process_state.auto_refresh_interval_secs,
                                                )
                                        })
                                        .unwrap_or(true);
                                    if should_refresh {
                                        self.fetch_processes(id);
                                    }
                                }
                            }
                        }
                        DeviceTab::Favorites => {
                            need_periodic_repaint = true;
                            let should_proc = !dev.process_state.is_loading_processes
                                && dev
                                    .process_state
                                    .last_process_refresh
                                    .map(|t| t.elapsed() >= Duration::from_secs(5))
                                    .unwrap_or(true);
                            let should_srv = !dev.process_state.is_loading_services
                                && dev
                                    .process_state
                                    .last_service_refresh
                                    .map(|t| t.elapsed() >= Duration::from_secs(5))
                                    .unwrap_or(true);
                            if should_proc {
                                self.fetch_processes(id);
                            }
                            if should_srv {
                                self.fetch_services(id);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if need_periodic_repaint {
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        // 1. Left Sidebar (zero cloning of Device!)
        egui::SidePanel::left("server_sidebar")
            .resizable(true)
            .default_width(260.0)
            .min_width(200.0)
            .max_width(380.0)
            .show(ctx, |ui| {
                let mut device_refs: Vec<&Device> = self.devices.values().collect();
                device_refs.sort_by(|a, b| {
                    let a_conn = a.status.is_connected();
                    let b_conn = b.status.is_connected();
                    b_conn.cmp(&a_conn).then_with(|| {
                        a.hostname.to_lowercase().cmp(&b.hostname.to_lowercase())
                    })
                });

                if let Some(action) = sidebar::render(
                    ui,
                    &mut self.sidebar_state,
                    &device_refs,
                    self.selected_device_id.as_deref(),
                ) {
                    match action {
                        SidebarAction::SelectDevice(id) => {
                            self.active_view = ActiveView::Device;
                            self.select_device(Some(id));
                        }
                        SidebarAction::ConnectDevice(id) => {
                            self.initiate_connect(&id, None);
                        }
                        SidebarAction::DisconnectDevice(id) => {
                            self.disconnect_device(&id);
                        }
                        SidebarAction::RemoveDevice(id) => {
                            self.remove_device(&id);
                        }
                        SidebarAction::AddManualDevice(manual) => {
                            self.add_manual_device(manual);
                        }
                        SidebarAction::RefreshDiscovery => {
                            self.restart_discovery();
                        }
                        SidebarAction::OpenGlobalSettings => {
                            self.active_view = ActiveView::GlobalSettings;
                        }
                    }
                }
            });

        // 2. Central Panel
        egui::CentralPanel::default().show(ctx, |ui| match self.active_view {
            ActiveView::GlobalSettings => {
                if let Some(action) = settings::render(
                    ui,
                    &mut self.settings,
                    &mut self.settings_state,
                    self.settings_warning.as_deref(),
                ) {
                    match action {
                        SettingsAction::BackToPanel => {
                            self.active_view = ActiveView::Device;
                        }
                        SettingsAction::ThemeChanged => {
                            self.apply_theme();
                            let _ = self.settings.save();
                        }
                        SettingsAction::Saved => {
                            self.apply_theme();
                            self.restart_active_metrics_streams();
                        }
                    }
                }
            }
            ActiveView::Device => {
                if let Some(id) = self.selected_device_id.clone() {
                    // Auto-open terminal ONLY when TerminalSessionState::Idle (prevents infinite reopen loop!)
                    let should_open_term = self.device_panel_state.active_tab == DeviceTab::Terminal
                        && self
                            .devices
                            .get(&id)
                            .map(|d| {
                                d.status.is_connected()
                                    && d.terminal_state == TerminalSessionState::Idle
                            })
                            .unwrap_or(false)
                        && !self.terminal_txs.contains_key(&id);

                    if should_open_term {
                        self.open_terminal_session(&id);
                    }

                    let refresh_rate_ms = self.settings.refresh_rate_ms;
                    let history_capacity = self.settings.history_capacity;

                    let action = if let Some(dev) = self.devices.get_mut(&id) {
                        device_panel::render(
                            ui,
                            &mut self.device_panel_state,
                            dev,
                            refresh_rate_ms,
                            history_capacity,
                        )
                    } else {
                        let next = self.devices.keys().next().cloned();
                        self.select_device(next);
                        None
                    };

                    if let Some(action) = action {
                        match action {
                            DevicePanelAction::Connect { id: dev_id, psk } => {
                                self.initiate_connect(&dev_id, psk);
                            }
                            DevicePanelAction::Disconnect(dev_id) => {
                                self.disconnect_device(&dev_id);
                            }
                            DevicePanelAction::Reconnect(dev_id) => {
                                self.disconnect_device(&dev_id);
                                self.initiate_connect(&dev_id, None);
                            }
                            DevicePanelAction::Remove(dev_id) => {
                                self.remove_device(&dev_id);
                            }
                            DevicePanelAction::SavePsk { id: dev_id, psk } => {
                                let trimmed = psk.trim().to_string();
                                if let Some(d) = self.devices.get_mut(&dev_id) {
                                    d.psk = if trimmed.is_empty() {
                                        None
                                    } else {
                                        Some(trimmed.clone())
                                    };
                                }
                                if trimmed.is_empty() {
                                    self.settings.device_psks.remove(&dev_id);
                                } else {
                                    self.settings.device_psks.insert(dev_id, trimmed);
                                }
                                self.save_known_devices();
                            }
                            DevicePanelAction::TrustNewFingerprint {
                                id: dev_id,
                                fingerprint,
                            } => {
                                if let Some(d) = self.devices.get_mut(&dev_id) {
                                    d.cert_fingerprint = Some(fingerprint);
                                }
                                self.save_known_devices();
                                self.initiate_connect(&dev_id, None);
                            }
                            DevicePanelAction::ResetFingerprint(dev_id) => {
                                if let Some(d) = self.devices.get_mut(&dev_id) {
                                    d.cert_fingerprint = None;
                                }
                                self.save_known_devices();
                            }
                            DevicePanelAction::SwitchTab(tab) => {
                                if tab == DeviceTab::Processes || tab == DeviceTab::Favorites {
                                    let (need_proc, need_srv) = if let Some(d) = self.devices.get(&id) {
                                        if d.status.is_connected() {
                                            (
                                                d.process_state.processes.is_empty()
                                                    && !d.process_state.is_loading_processes,
                                                d.process_state.services.is_empty()
                                                    && !d.process_state.is_loading_services,
                                            )
                                        } else {
                                            (false, false)
                                        }
                                    } else {
                                        (false, false)
                                    };
                                    if need_proc {
                                        self.fetch_processes(&id);
                                    }
                                    if need_srv {
                                        self.fetch_services(&id);
                                    }
                                }
                            }
                            DevicePanelAction::ProcessAction(proc_action) => match proc_action {
                                ProcessPanelAction::RefreshProcesses => {
                                    self.fetch_processes(&id);
                                }
                                ProcessPanelAction::RefreshServices => {
                                    self.fetch_services(&id);
                                }
                                ProcessPanelAction::KillProcess { pid, signal } => {
                                    self.kill_process(&id, pid, signal);
                                }
                                ProcessPanelAction::ControlService {
                                    service_name,
                                    action,
                                } => {
                                    self.control_service(&id, &service_name, &action);
                                }
                                ProcessPanelAction::AddFavoriteFromProcess(proc_item) => {
                                    let fav = crate::state::FavoriteApp::new(
                                        format!("fav_{}", uuid::Uuid::new_v4().simple()),
                                        proc_item.name.clone(),
                                        "⚡",
                                        crate::state::FavoriteAppType::Process,
                                        proc_item.name.clone(),
                                        None,
                                        String::new(),
                                        format!("Processus {}", proc_item.name),
                                        None,
                                    );

                                    if let Some(dev) = self.devices.get_mut(&id) {
                                        if !dev
                                            .favorites
                                            .iter()
                                            .any(|f| f.process_filter == fav.process_filter)
                                        {
                                            dev.favorites.push(fav);
                                            self.settings
                                                .favorite_apps
                                                .insert(id.clone(), dev.favorites.clone());
                                            let _ = self.settings.save();
                                        }
                                    }
                                    self.device_panel_state.active_tab = DeviceTab::Favorites;
                                }
                            },
                            DevicePanelAction::FavoritesAction(fav_action) => match fav_action {
                                FavoritesPanelAction::Refresh => {
                                    self.fetch_processes(&id);
                                    self.fetch_services(&id);
                                }
                                FavoritesPanelAction::ClearStatus => {
                                    if let Some(dev) = self.devices.get_mut(&id) {
                                        dev.process_state.clear_status();
                                    }
                                }
                                FavoritesPanelAction::AttachTerminal { command } => {
                                    self.device_panel_state.active_tab = DeviceTab::Terminal;
                                    let is_open = self
                                        .devices
                                        .get(&id)
                                        .map(|d| d.terminal_state == TerminalSessionState::Open)
                                        .unwrap_or(false);

                                    if is_open {
                                        if !command.trim().is_empty() {
                                            if let Some(tx) = self.terminal_txs.get(&id) {
                                                let mut cmd = command.clone();
                                                if !cmd.ends_with('\n') && !cmd.ends_with('\r') {
                                                    cmd.push('\r');
                                                }
                                                let _ = tx.try_send(TerminalInput {
                                                    input: Some(Input::Data(cmd.into_bytes())),
                                                });
                                            }
                                        }
                                    } else {
                                        if let Some(dev) = self.devices.get_mut(&id) {
                                            dev.pending_terminal_command = Some(command);
                                        }
                                        let is_connecting = self
                                            .devices
                                            .get(&id)
                                            .map(|d| {
                                                d.terminal_state
                                                    == TerminalSessionState::Connecting
                                            })
                                            .unwrap_or(false);
                                        if !is_connecting {
                                            self.open_terminal_session(&id);
                                        }
                                    }
                                }
                                FavoritesPanelAction::KillProcess { pid, signal } => {
                                    self.kill_process(&id, pid, signal);
                                }
                                FavoritesPanelAction::ControlService {
                                    service_name,
                                    action,
                                } => {
                                    self.control_service(&id, &service_name, &action);
                                }
                                FavoritesPanelAction::SaveFavorite(fav) => {
                                    if let Some(dev) = self.devices.get_mut(&id) {
                                        if let Some(pos) =
                                            dev.favorites.iter().position(|f| f.id == fav.id)
                                        {
                                            dev.favorites[pos] = fav;
                                        } else {
                                            dev.favorites.push(fav);
                                        }
                                        self.settings
                                            .favorite_apps
                                            .insert(id.clone(), dev.favorites.clone());
                                        let _ = self.settings.save();
                                    }
                                }
                                FavoritesPanelAction::RemoveFavorite(fav_id) => {
                                    if let Some(dev) = self.devices.get_mut(&id) {
                                        dev.favorites.retain(|f| f.id != fav_id);
                                        self.settings
                                            .favorite_apps
                                            .insert(id.clone(), dev.favorites.clone());
                                        let _ = self.settings.save();
                                    }
                                }
                            },
                            DevicePanelAction::TerminalActions(term_actions) => {
                                for term_action in term_actions {
                                    match term_action {
                                        TerminalAction::SendData(data) => {
                                            if let Some(tx) = self.terminal_txs.get(&id) {
                                                let _ = tx.try_send(TerminalInput {
                                                    input: Some(Input::Data(data)),
                                                });
                                            }
                                        }
                                        TerminalAction::Resize { cols, rows } => {
                                            if let Some(tx) = self.terminal_txs.get(&id) {
                                                let _ = tx.try_send(TerminalInput {
                                                    input: Some(Input::Resize(TerminalResize {
                                                        cols: cols as u32,
                                                        rows: rows as u32,
                                                    })),
                                                });
                                            }
                                        }
                                        TerminalAction::Clear => {}
                                        TerminalAction::Connect => {
                                            self.open_terminal_session(&id);
                                        }
                                        TerminalAction::Reconnect => {
                                            self.terminal_txs.remove(&id);
                                            if let Some(d) = self.devices.get_mut(&id) {
                                                d.terminal.clear();
                                            }
                                            self.open_terminal_session(&id);
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.heading("⚡ Bienvenue sur NTX Panel");
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(
                                "Sélectionnez un serveur dans la colonne de gauche ou ajoutez une nouvelle connexion.",
                            )
                            .size(14.0)
                            .weak(),
                        );
                        ui.add_space(20.0);
                        ui.horizontal(|ui| {
                            let total_w = 340.0;
                            ui.add_space((ui.available_width() - total_w).max(0.0) / 2.0);

                            if ui.button("➕ Ajouter un serveur manuellement").clicked() {
                                self.sidebar_state.show_add_modal = true;
                            }

                            if ui.button("🔄 Scanner le réseau (mDNS)").clicked() {
                                self.restart_discovery();
                            }
                        });
                    });
                }
            }
        });
    }
}
