use crate::client::{is_unimplemented_error, ClientConnectError, NtxClient};
use crate::discovery::{start_discovery, DiscoveredDevice, DiscoveryEvent, DiscoveryHandle};
use crate::state::{
    favorite_app_to_managed_config, AppSettings, ConnectionStatus, ConsoleStreamState, Device,
    DockerConsoleMode, KnownDevice, ManualDevice, TerminalSessionState,
};
use crate::ui::device_panel::{self, DevicePanelAction, DevicePanelState, DeviceTab};
use crate::ui::docker_panel::DockerPanelAction;
use crate::ui::overview::{self, OverviewAction, OverviewState};
use crate::ui::process_panel::ProcessPanelAction;
use crate::ui::services_panel::ServicesPanelAction;
use crate::ui::settings::{self, SettingsAction, SettingsState};
use crate::ui::sidebar::{self, SidebarAction, SidebarState};
use crate::ui::terminal::TerminalAction;
use crate::ui::theme::{self, Palette};
use ntx_proto::terminal_input::Input;
use ntx_proto::{SystemInfo, SystemMetrics, TerminalInput, TerminalResize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveView {
    Overview,
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
        connected_address: String,
    },
    PingResult {
        device_id: String,
        generation: u64,
        latency_ms: u32,
        uptime_seconds: u64,
        version: String,
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
    ManagedServicesReceived {
        device_id: String,
        generation: u64,
        services: Vec<ntx_proto::ManagedServiceStatus>,
    },
    ManagedServicesFailed {
        device_id: String,
        generation: u64,
        error: String,
        is_unimplemented: bool,
    },
    ManagedServiceSaved {
        device_id: String,
        generation: u64,
        service_name: String,
        service_id: Option<String>,
        success: bool,
        error: String,
        is_unimplemented: bool,
    },
    ManagedServiceDeleted {
        device_id: String,
        generation: u64,
        service_id: String,
        success: bool,
        error: String,
    },
    ManagedServiceControlled {
        device_id: String,
        generation: u64,
        service_id: String,
        action: String,
        success: bool,
        error: String,
    },
    ManagedServiceInputSent {
        device_id: String,
        generation: u64,
        service_id: String,
        success: bool,
        error: String,
    },
    ServiceConsoleReady {
        device_id: String,
        service_id: String,
        generation: u64,
    },
    ServiceConsoleChunk {
        device_id: String,
        service_id: String,
        generation: u64,
        data: Vec<u8>,
        is_history: bool,
    },
    ServiceConsoleClosed {
        device_id: String,
        service_id: String,
        generation: u64,
        error: Option<String>,
    },
    DockerContainersReceived {
        device_id: String,
        generation: u64,
        response: Box<ntx_proto::ListDockerContainersResponse>,
    },
    DockerContainersFailed {
        device_id: String,
        generation: u64,
        error: String,
        is_unimplemented: bool,
    },
    DockerContainerControlled {
        device_id: String,
        generation: u64,
        container_id: String,
        action: String,
        success: bool,
        error: String,
    },
    DockerCommandExecuted {
        device_id: String,
        generation: u64,
        container_id: String,
        command: String,
        success: bool,
        exit_code: i32,
        stdout: String,
        stderr: String,
        error: String,
    },
    DockerConsoleReady {
        device_id: String,
        container_id: String,
        generation: u64,
    },
    DockerConsoleChunk {
        device_id: String,
        container_id: String,
        generation: u64,
        data: Vec<u8>,
        is_history: bool,
    },
    DockerConsoleClosed {
        device_id: String,
        container_id: String,
        generation: u64,
        error: Option<String>,
    },
}

pub struct NtxApp {
    rt: Arc<tokio::runtime::Runtime>,
    egui_ctx: egui::Context,
    active_view: ActiveView,
    selected_device_id: Option<String>,
    device_panel_state: DevicePanelState,
    overview_state: OverviewState,
    sidebar_state: SidebarState,
    devices: HashMap<String, Device>,
    active_clients: HashMap<String, NtxClient>,
    connection_gens: HashMap<String, u64>,
    terminal_gens: HashMap<String, u64>,
    service_console_gens: HashMap<String, u64>,
    service_console_cancel_txs: HashMap<String, oneshot::Sender<()>>,
    docker_console_gens: HashMap<String, u64>,
    docker_console_cancel_txs: HashMap<String, oneshot::Sender<()>>,
    docker_console_input_txs: HashMap<String, mpsc::Sender<ntx_proto::DockerConsoleInput>>,
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
        #[cfg(target_os = "windows")]
        crate::win_dpi_fix::install(cc);

        let egui_ctx = cc.egui_ctx.clone();
        let (disc_tx, disc_rx) = mpsc::channel(64);
        let (event_tx, event_rx) = mpsc::channel(512);

        let discovery_handle =
            match start_discovery(disc_tx, rt.handle().clone(), Some(egui_ctx.clone())) {
                Ok(handle) => Some(handle),
                Err(e) => {
                    warn!("Impossible de démarrer la découverte mDNS : {e}");
                    None
                }
            };

        let (settings, settings_warning) = AppSettings::load();
        theme::apply_theme(&egui_ctx, settings.dark_mode);

        let mut app = Self {
            rt,
            egui_ctx,
            active_view: ActiveView::Overview,
            selected_device_id: None,
            device_panel_state: DevicePanelState::default(),
            overview_state: OverviewState::default(),
            sidebar_state: SidebarState::new(),
            devices: HashMap::new(),
            active_clients: HashMap::new(),
            connection_gens: HashMap::new(),
            terminal_gens: HashMap::new(),
            service_console_gens: HashMap::new(),
            service_console_cancel_txs: HashMap::new(),
            docker_console_gens: HashMap::new(),
            docker_console_cancel_txs: HashMap::new(),
            docker_console_input_txs: HashMap::new(),
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

        let mut healed_known = false;
        let mut auto_connect_ids = Vec::new();
        for known in &app.settings.known_devices.clone() {
            let cleaned_addrs = crate::discovery::sanitize_addresses(&known.addresses);
            if cleaned_addrs.is_empty() {
                healed_known = true;
                continue;
            }
            if cleaned_addrs != known.addresses {
                healed_known = true;
            }

            let mut dev = Device::new(
                known.id.clone(),
                known.hostname.clone(),
                cleaned_addrs,
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
            dev.auto_connect = known.auto_connect;
            dev.status = ConnectionStatus::Disconnected;
            if let Some(favs) = app.settings.favorite_apps.get(&known.id) {
                dev.favorites = favs.clone();
            }
            if dev.auto_connect {
                auto_connect_ids.push(known.id.clone());
            }
            app.devices.insert(known.id.clone(), dev);
        }
        if healed_known {
            app.save_known_devices();
        }

        if !app.devices.is_empty() {
            let first_id = app.devices.keys().next().cloned();
            app.select_device(first_id);
        }

        for id in auto_connect_ids {
            app.initiate_connect(&id, None);
        }

        app
    }

    fn apply_theme(&self) {
        theme::apply_theme(&self.egui_ctx, self.settings.dark_mode);
    }

    fn select_device(&mut self, id: Option<String>) {
        self.selected_device_id = id.clone();

        if let Some(ref dev_id) = id {
            if let Some(dev) = self.devices.get(dev_id) {
                self.device_panel_state.sync_from_device(dev);
            }
            self.maybe_auto_load_active_tab(dev_id);
        } else {
            self.device_panel_state.edit_device_id = None;
            self.device_panel_state.psk_input.clear();
            self.device_panel_state.services_state.reset_for_device();
            self.device_panel_state.docker_state.reset_for_device();
        }
    }

    fn maybe_auto_load_active_tab(&mut self, device_id: &str) {
        let Some(dev) = self.devices.get(device_id) else {
            return;
        };
        if !dev.status.is_connected() {
            return;
        }
        match self.device_panel_state.active_tab {
            DeviceTab::System => {
                let need_proc = dev.process_state.processes.is_empty()
                    && !dev.process_state.is_loading_processes;
                let need_srv = dev.process_state.services.is_empty()
                    && !dev.process_state.is_loading_services;
                if need_proc {
                    self.fetch_processes(device_id);
                }
                if need_srv {
                    self.fetch_services(device_id);
                }
            }
            DeviceTab::Services => {
                let should_fetch = !dev.managed_services.unsupported_by_server
                    && !dev.managed_services.is_loading;
                if should_fetch {
                    self.fetch_managed_services(device_id);
                }
            }
            DeviceTab::Docker => {
                let should_fetch = !dev.docker_state.unsupported_by_server
                    && dev.docker_state.docker_available
                    && !dev.docker_state.is_loading;
                let include_all = dev.docker_state.include_all;
                if should_fetch {
                    self.fetch_docker_containers(device_id, include_all);
                }
            }
            DeviceTab::Metrics | DeviceTab::Terminal | DeviceTab::Config => {}
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
                auto_connect: d.auto_connect,
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
                let merged = crate::discovery::merge_and_prioritize_addresses(
                    std::slice::from_ref(&manual.address),
                    &dev.addresses,
                    true,
                );
                if !merged.is_empty() {
                    dev.addresses = merged;
                } else if !dev.addresses.contains(&manual.address) {
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
            let cleaned =
                crate::discovery::sanitize_addresses(std::slice::from_ref(&manual.address));
            let dev_addrs = if cleaned.is_empty() {
                vec![manual.address]
            } else {
                cleaned
            };
            let mut dev = Device::new(
                id.clone(),
                manual.hostname,
                dev_addrs,
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
        let (disc_tx, disc_rx) = mpsc::channel(64);
        let rt_handle = self.rt.handle().clone();
        let ctx = Some(self.egui_ctx.clone());

        let refresh_ok = if let Some(ref mut handle) = self.discovery_handle {
            match handle.refresh(disc_tx.clone(), rt_handle.clone(), ctx.clone()) {
                Ok(()) => true,
                Err(e) => {
                    warn!("Échec du rafraîchissement mDNS sur le daemon actif : {e}");
                    handle.stop();
                    self.discovery_handle = None;
                    false
                }
            }
        } else {
            false
        };

        if !refresh_ok {
            match start_discovery(disc_tx, rt_handle, ctx) {
                Ok(handle) => self.discovery_handle = Some(handle),
                Err(e) => warn!("Échec du redémarrage de la découverte mDNS : {e}"),
            }
        }

        self.discovery_rx = disc_rx;
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

    fn next_service_console_gen(&mut self, device_id: &str) -> u64 {
        let entry = self
            .service_console_gens
            .entry(device_id.to_string())
            .or_insert(0);
        *entry += 1;
        *entry
    }

    fn next_docker_console_gen(&mut self, device_id: &str) -> u64 {
        let entry = self
            .docker_console_gens
            .entry(device_id.to_string())
            .or_insert(0);
        *entry += 1;
        *entry
    }

    fn cancel_service_console(&mut self, device_id: &str) {
        let _ = self.next_service_console_gen(device_id);
        if let Some(cancel) = self.service_console_cancel_txs.remove(device_id) {
            let _ = cancel.send(());
        }
    }

    fn cancel_docker_console(&mut self, device_id: &str) {
        let _ = self.next_docker_console_gen(device_id);
        if let Some(cancel) = self.docker_console_cancel_txs.remove(device_id) {
            let _ = cancel.send(());
        }
        self.docker_console_input_txs.remove(device_id);
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
        self.cancel_service_console(device_id);
        self.cancel_docker_console(device_id);
        self.active_clients.remove(device_id);
        self.terminal_txs.remove(device_id);
        if let Some(old_tx) = self.metrics_cancel_txs.remove(device_id) {
            let _ = old_tx.send(());
        }

        let (candidates, expected_fp) = match self.devices.get_mut(device_id) {
            Some(d) => {
                let eps = d.grpc_endpoints();
                if eps.is_empty() {
                    d.status = ConnectionStatus::Error(
                        "Aucune adresse réseau IPv4/IPv6 routable disponible pour ce serveur."
                            .to_string(),
                    );
                    return;
                }
                d.status = ConnectionStatus::Connecting;
                d.terminal_state = TerminalSessionState::Idle;
                d.managed_services.reset_on_disconnect();
                d.docker_state.reset_on_disconnect();
                (eps, d.cert_fingerprint.clone())
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
            let mut connected = None;
            let mut last_transport_err = None;

            for (addr, endpoint) in candidates {
                info!("Connexion TLS gRPC vers {} ({})", dev_id, endpoint);
                match NtxClient::connect_and_authenticate(
                    &endpoint,
                    &resolved_psk,
                    &viewer_id,
                    expected_fp.clone(),
                )
                .await
                {
                    Ok(c) => {
                        connected = Some((addr, c));
                        break;
                    }
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
                        warn!(
                            "Échec de connexion vers {} via {} : {}",
                            dev_id, endpoint, err
                        );
                        last_transport_err = Some(err);
                    }
                }
            }

            let Some((connected_address, mut client)) = connected else {
                let err = last_transport_err.unwrap_or_else(|| {
                    "Impossible de joindre le serveur sur aucune adresse.".to_string()
                });
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
            };

            let fingerprint = client.cert_fingerprint().to_string();
            let system_info = match client.get_system_info().await {
                Ok(info) => info,
                Err(e) => {
                    warn!("Erreur GetSystemInfo pour {}: {e}", dev_id);
                    SystemInfo::default()
                }
            };

            let mut ping_client = client.clone();
            let ping_start = std::time::Instant::now();
            let ping_res = ping_client.ping().await;

            let stream_client = client.clone();
            let _ = event_tx
                .send(AppEvent::Connected {
                    device_id: dev_id.clone(),
                    generation: gen,
                    client: Box::new(client),
                    system_info: Box::new(system_info),
                    fingerprint,
                    connected_address,
                })
                .await;

            if let Ok(ping_resp) = ping_res {
                let latency_ms = ping_start.elapsed().as_millis().min(u32::MAX as u128) as u32;
                let _ = event_tx
                    .send(AppEvent::PingResult {
                        device_id: dev_id.clone(),
                        generation: gen,
                        latency_ms,
                        uptime_seconds: ping_resp.uptime_seconds,
                        version: ping_resp.version,
                    })
                    .await;
            }
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
        self.cancel_service_console(device_id);
        self.cancel_docker_console(device_id);

        if let Some(cancel) = self.metrics_cancel_txs.remove(device_id) {
            let _ = cancel.send(());
        }
        self.active_clients.remove(device_id);
        self.terminal_txs.remove(device_id);
        if let Some(dev) = self.devices.get_mut(device_id) {
            dev.status = ConnectionStatus::Disconnected;
            dev.last_ping_ms = None;
            dev.server_uptime_secs = None;
            dev.terminal_state = TerminalSessionState::Idle;
            dev.pending_terminal_command = None;
            dev.process_state.is_loading_processes = false;
            dev.process_state.is_loading_services = false;
            dev.managed_services.reset_on_disconnect();
            dev.docker_state.reset_on_disconnect();
        }
    }

    fn ping_device(&mut self, device_id: &str) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            return;
        };
        let gen = *self.connection_gens.get(device_id).unwrap_or(&0);
        let dev_id = device_id.to_string();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        self.rt.spawn(async move {
            let start = std::time::Instant::now();
            if let Ok(resp) = client.ping().await {
                let latency_ms = start.elapsed().as_millis().min(u32::MAX as u128) as u32;
                let _ = event_tx
                    .send(AppEvent::PingResult {
                        device_id: dev_id,
                        generation: gen,
                        latency_ms,
                        uptime_seconds: resp.uptime_seconds,
                        version: resp.version,
                    })
                    .await;
                ctx.request_repaint();
            }
        });
    }

    fn open_or_run_terminal_command(&mut self, device_id: &str, command: String) {
        self.active_view = ActiveView::Device;
        self.device_panel_state.active_tab = DeviceTab::Terminal;
        let is_open = self
            .devices
            .get(device_id)
            .map(|d| d.terminal_state == TerminalSessionState::Open)
            .unwrap_or(false);

        if is_open {
            if !command.trim().is_empty() {
                if let Some(tx) = self.terminal_txs.get(device_id) {
                    let mut cmd = command;
                    if !cmd.ends_with('\n') && !cmd.ends_with('\r') {
                        cmd.push('\r');
                    }
                    let _ = tx.try_send(TerminalInput {
                        input: Some(Input::Data(cmd.into_bytes())),
                    });
                }
            }
        } else {
            if let Some(dev) = self.devices.get_mut(device_id) {
                if !command.trim().is_empty() {
                    dev.pending_terminal_command = Some(command);
                }
            }
            let is_connecting = self
                .devices
                .get(device_id)
                .map(|d| d.terminal_state == TerminalSessionState::Connecting)
                .unwrap_or(false);
            if !is_connecting {
                self.open_terminal_session(device_id);
            }
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

    fn fetch_managed_services(&mut self, device_id: &str) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            return;
        };
        let gen = *self.connection_gens.get(device_id).unwrap_or(&0);

        if let Some(dev) = self.devices.get_mut(device_id) {
            if dev.managed_services.is_loading {
                return;
            }
            dev.managed_services.is_loading = true;
            dev.managed_services.last_refresh = Some(std::time::Instant::now());
        }

        let dev_id = device_id.to_string();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        self.rt.spawn(async move {
            match client.list_managed_services().await {
                Ok(services) => {
                    let _ = event_tx
                        .send(AppEvent::ManagedServicesReceived {
                            device_id: dev_id,
                            generation: gen,
                            services,
                        })
                        .await;
                }
                Err(e) => {
                    let is_unimplemented = is_unimplemented_error(&e);
                    let _ = event_tx
                        .send(AppEvent::ManagedServicesFailed {
                            device_id: dev_id,
                            generation: gen,
                            error: format!("{e:#}"),
                            is_unimplemented,
                        })
                        .await;
                }
            }
            ctx.request_repaint();
        });
    }

    fn save_managed_service(&mut self, device_id: &str, config: ntx_proto::ManagedServiceConfig) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            if let Some(dev) = self.devices.get_mut(device_id) {
                dev.managed_services
                    .set_status("Serveur non connecté.", true);
            }
            return;
        };
        let gen = *self.connection_gens.get(device_id).unwrap_or(&0);
        let dev_id = device_id.to_string();
        let svc_name = config.name.clone();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        self.rt.spawn(async move {
            match client.save_managed_service(config).await {
                Ok(resp) => {
                    let _ = event_tx
                        .send(AppEvent::ManagedServiceSaved {
                            device_id: dev_id,
                            generation: gen,
                            service_name: svc_name,
                            service_id: resp.service.map(|s| s.id),
                            success: resp.success,
                            error: resp.error_message,
                            is_unimplemented: false,
                        })
                        .await;
                }
                Err(e) => {
                    let is_unimplemented = is_unimplemented_error(&e);
                    let _ = event_tx
                        .send(AppEvent::ManagedServiceSaved {
                            device_id: dev_id,
                            generation: gen,
                            service_name: svc_name,
                            service_id: None,
                            success: false,
                            error: format!("{e:#}"),
                            is_unimplemented,
                        })
                        .await;
                }
            }
            ctx.request_repaint();
        });
    }

    fn import_legacy_favorites(&mut self, device_id: &str) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            if let Some(dev) = self.devices.get_mut(device_id) {
                dev.managed_services
                    .set_status("Serveur non connecté.", true);
            }
            return;
        };
        let gen = *self.connection_gens.get(device_id).unwrap_or(&0);

        let configs: Vec<ntx_proto::ManagedServiceConfig> = {
            let Some(dev) = self.devices.get(device_id) else {
                return;
            };
            let mut existing_names: HashSet<String> = dev
                .managed_services
                .services
                .iter()
                .filter_map(|s| s.config.as_ref())
                .map(|c| c.name.trim().to_lowercase())
                .collect();
            let mut existing_ids: HashSet<String> = dev
                .managed_services
                .services
                .iter()
                .filter_map(|s| s.config.as_ref())
                .map(|c| c.id.trim().to_lowercase())
                .collect();

            let mut out = Vec::new();
            for fav in &dev.favorites {
                let mut cfg = favorite_app_to_managed_config(fav);
                let name_key = cfg.name.trim().to_lowercase();
                if existing_names.contains(&name_key) {
                    continue;
                }
                if !cfg.id.is_empty() && existing_ids.contains(&cfg.id.to_lowercase()) {
                    cfg.id.clear();
                }
                existing_names.insert(name_key);
                if !cfg.id.is_empty() {
                    existing_ids.insert(cfg.id.to_lowercase());
                }
                out.push(cfg);
            }
            out
        };

        if configs.is_empty() {
            if let Some(dev) = self.devices.get_mut(device_id) {
                dev.managed_services.set_status(
                    "Tous les anciens favoris sont déjà présents dans Mes Services.",
                    false,
                );
            }
            return;
        }

        let dev_id = device_id.to_string();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        self.rt.spawn(async move {
            let mut imported = 0usize;
            let mut last_id = None;
            let mut last_err = String::new();
            let mut is_unimplemented = false;

            for cfg in configs {
                match client.save_managed_service(cfg).await {
                    Ok(resp) if resp.success => {
                        imported += 1;
                        if let Some(svc) = resp.service {
                            last_id = Some(svc.id);
                        }
                    }
                    Ok(resp) => {
                        last_err = resp.error_message;
                    }
                    Err(e) => {
                        is_unimplemented = is_unimplemented_error(&e);
                        last_err = format!("{e:#}");
                        if is_unimplemented {
                            break;
                        }
                    }
                }
            }

            let _ = event_tx
                .send(AppEvent::ManagedServiceSaved {
                    device_id: dev_id,
                    generation: gen,
                    service_name: format!("{imported} service(s) importé(s)"),
                    service_id: last_id,
                    success: imported > 0,
                    error: last_err,
                    is_unimplemented,
                })
                .await;
            ctx.request_repaint();
        });
    }

    fn delete_managed_service(&mut self, device_id: &str, service_id: &str) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            if let Some(dev) = self.devices.get_mut(device_id) {
                dev.managed_services
                    .set_status("Serveur non connecté.", true);
            }
            return;
        };

        if let Some(dev) = self.devices.get_mut(device_id) {
            if dev.managed_services.console_service_id.as_deref() == Some(service_id) {
                dev.managed_services.console_service_id = None;
                dev.managed_services.console_state = ConsoleStreamState::Idle;
                dev.managed_services.console_buffer.clear();
                self.cancel_service_console(device_id);
            }
        }

        let gen = *self.connection_gens.get(device_id).unwrap_or(&0);
        let dev_id = device_id.to_string();
        let svc_id = service_id.to_string();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        self.rt.spawn(async move {
            match client.delete_managed_service(&svc_id).await {
                Ok(resp) => {
                    let _ = event_tx
                        .send(AppEvent::ManagedServiceDeleted {
                            device_id: dev_id,
                            generation: gen,
                            service_id: svc_id,
                            success: resp.success,
                            error: resp.error_message,
                        })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::ManagedServiceDeleted {
                            device_id: dev_id,
                            generation: gen,
                            service_id: svc_id,
                            success: false,
                            error: format!("{e:#}"),
                        })
                        .await;
                }
            }
            ctx.request_repaint();
        });
    }

    fn control_managed_service(&mut self, device_id: &str, service_id: &str, action: &str) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            if let Some(dev) = self.devices.get_mut(device_id) {
                dev.managed_services
                    .set_status("Serveur non connecté.", true);
            }
            return;
        };
        let gen = *self.connection_gens.get(device_id).unwrap_or(&0);
        let dev_id = device_id.to_string();
        let svc_id = service_id.to_string();
        let act = action.to_string();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        self.rt.spawn(async move {
            match client.control_managed_service(&svc_id, &act).await {
                Ok(resp) => {
                    let _ = event_tx
                        .send(AppEvent::ManagedServiceControlled {
                            device_id: dev_id,
                            generation: gen,
                            service_id: svc_id,
                            action: act,
                            success: resp.success,
                            error: resp.error_message,
                        })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::ManagedServiceControlled {
                            device_id: dev_id,
                            generation: gen,
                            service_id: svc_id,
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

    fn send_managed_service_input(&mut self, device_id: &str, service_id: &str, input: &str) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            if let Some(dev) = self.devices.get_mut(device_id) {
                dev.managed_services
                    .set_status("Serveur non connecté.", true);
            }
            return;
        };
        let gen = *self.connection_gens.get(device_id).unwrap_or(&0);
        let dev_id = device_id.to_string();
        let svc_id = service_id.to_string();
        let payload = input.to_string();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        if let Some(dev) = self.devices.get_mut(device_id) {
            if dev.managed_services.console_service_id.as_deref() == Some(service_id) {
                dev.managed_services.console_buffer.push_system_line(
                    &format!("> {payload}"),
                    Some(Palette::dark().accent),
                );
            }
        }

        self.rt.spawn(async move {
            match client
                .send_managed_service_input(&svc_id, &payload, true)
                .await
            {
                Ok(resp) => {
                    let _ = event_tx
                        .send(AppEvent::ManagedServiceInputSent {
                            device_id: dev_id,
                            generation: gen,
                            service_id: svc_id,
                            success: resp.success,
                            error: resp.error_message,
                        })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::ManagedServiceInputSent {
                            device_id: dev_id,
                            generation: gen,
                            service_id: svc_id,
                            success: false,
                            error: format!("{e:#}"),
                        })
                        .await;
                }
            }
            ctx.request_repaint();
        });
    }

    fn open_service_console(&mut self, device_id: &str, service_id: &str) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            return;
        };

        let gen = self.next_service_console_gen(device_id);
        if let Some(old_cancel) = self.service_console_cancel_txs.remove(device_id) {
            let _ = old_cancel.send(());
        }

        if let Some(dev) = self.devices.get_mut(device_id) {
            dev.managed_services.console_service_id = Some(service_id.to_string());
            dev.managed_services.console_state = ConsoleStreamState::Connecting;
            dev.managed_services.console_buffer.clear();
        } else {
            return;
        }

        let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
        self.service_console_cancel_txs
            .insert(device_id.to_string(), cancel_tx);

        let dev_id = device_id.to_string();
        let svc_id = service_id.to_string();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        self.rt.spawn(async move {
            let mut stream = match client.stream_managed_service_console(&svc_id, 200).await {
                Ok(s) => s,
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::ServiceConsoleClosed {
                            device_id: dev_id,
                            service_id: svc_id,
                            generation: gen,
                            error: Some(format!("{e:#}")),
                        })
                        .await;
                    ctx.request_repaint();
                    return;
                }
            };

            let _ = event_tx
                .send(AppEvent::ServiceConsoleReady {
                    device_id: dev_id.clone(),
                    service_id: svc_id.clone(),
                    generation: gen,
                })
                .await;
            ctx.request_repaint();

            let mut was_cancelled = false;
            let mut closed_err = None;
            loop {
                tokio::select! {
                    _ = &mut cancel_rx => {
                        was_cancelled = true;
                        break;
                    }
                    item = stream.message() => {
                        match item {
                            Ok(Some(chunk)) => {
                                if !chunk.data.is_empty() {
                                    if event_tx
                                        .send(AppEvent::ServiceConsoleChunk {
                                            device_id: dev_id.clone(),
                                            service_id: svc_id.clone(),
                                            generation: gen,
                                            data: chunk.data,
                                            is_history: chunk.is_history,
                                        })
                                        .await
                                        .is_err()
                                    {
                                        was_cancelled = true;
                                        break;
                                    }
                                    ctx.request_repaint();
                                }
                            }
                            Ok(None) => break,
                            Err(status) => {
                                closed_err = Some(status.message().to_string());
                                break;
                            }
                        }
                    }
                }
            }

            if !was_cancelled {
                let _ = event_tx
                    .send(AppEvent::ServiceConsoleClosed {
                        device_id: dev_id,
                        service_id: svc_id,
                        generation: gen,
                        error: closed_err,
                    })
                    .await;
                ctx.request_repaint();
            }
        });
    }

    fn fetch_docker_containers(&mut self, device_id: &str, include_all: bool) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            return;
        };
        let gen = *self.connection_gens.get(device_id).unwrap_or(&0);

        if let Some(dev) = self.devices.get_mut(device_id) {
            if dev.docker_state.is_loading {
                return;
            }
            dev.docker_state.is_loading = true;
            dev.docker_state.include_all = include_all;
            dev.docker_state.last_refresh = Some(std::time::Instant::now());
        }

        let dev_id = device_id.to_string();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        self.rt.spawn(async move {
            match client.list_docker_containers(include_all).await {
                Ok(response) => {
                    let _ = event_tx
                        .send(AppEvent::DockerContainersReceived {
                            device_id: dev_id,
                            generation: gen,
                            response: Box::new(response),
                        })
                        .await;
                }
                Err(e) => {
                    let is_unimplemented = is_unimplemented_error(&e);
                    let _ = event_tx
                        .send(AppEvent::DockerContainersFailed {
                            device_id: dev_id,
                            generation: gen,
                            error: format!("{e:#}"),
                            is_unimplemented,
                        })
                        .await;
                }
            }
            ctx.request_repaint();
        });
    }

    fn control_docker_container(
        &mut self,
        device_id: &str,
        container_id: &str,
        action: &str,
        force: bool,
    ) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            if let Some(dev) = self.devices.get_mut(device_id) {
                dev.docker_state.set_status("Serveur non connecté.", true);
            }
            return;
        };
        let gen = *self.connection_gens.get(device_id).unwrap_or(&0);
        let dev_id = device_id.to_string();
        let cid = container_id.to_string();
        let act = action.to_string();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        self.rt.spawn(async move {
            match client.control_docker_container(&cid, &act, force).await {
                Ok(resp) => {
                    let _ = event_tx
                        .send(AppEvent::DockerContainerControlled {
                            device_id: dev_id,
                            generation: gen,
                            container_id: cid,
                            action: act,
                            success: resp.success,
                            error: resp.error_message,
                        })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::DockerContainerControlled {
                            device_id: dev_id,
                            generation: gen,
                            container_id: cid,
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

    fn exec_docker_command(&mut self, device_id: &str, container_id: &str, command: &str) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            if let Some(dev) = self.devices.get_mut(device_id) {
                dev.docker_state.set_status("Serveur non connecté.", true);
            }
            return;
        };

        if let Some(dev) = self.devices.get_mut(device_id) {
            if dev.docker_state.console_container_id.as_deref() == Some(container_id) {
                dev.docker_state.console_buffer.push_system_line(
                    &format!("$ {command}"),
                    Some(Palette::dark().accent),
                );
            }
        }

        let gen = *self.connection_gens.get(device_id).unwrap_or(&0);
        let dev_id = device_id.to_string();
        let cid = container_id.to_string();
        let cmd = command.to_string();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        self.rt.spawn(async move {
            match client.exec_docker_command(&cid, &cmd).await {
                Ok(resp) => {
                    let _ = event_tx
                        .send(AppEvent::DockerCommandExecuted {
                            device_id: dev_id,
                            generation: gen,
                            container_id: cid,
                            command: cmd,
                            success: resp.success,
                            exit_code: resp.exit_code,
                            stdout: resp.stdout,
                            stderr: resp.stderr,
                            error: resp.error_message,
                        })
                        .await;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::DockerCommandExecuted {
                            device_id: dev_id,
                            generation: gen,
                            container_id: cid,
                            command: cmd,
                            success: false,
                            exit_code: -1,
                            stdout: String::new(),
                            stderr: String::new(),
                            error: format!("{e:#}"),
                        })
                        .await;
                }
            }
            ctx.request_repaint();
        });
    }

    fn open_docker_console(
        &mut self,
        device_id: &str,
        container_id: &str,
        mode: DockerConsoleMode,
    ) {
        let Some(mut client) = self.active_clients.get(device_id).cloned() else {
            return;
        };

        let gen = self.next_docker_console_gen(device_id);
        if let Some(old_cancel) = self.docker_console_cancel_txs.remove(device_id) {
            let _ = old_cancel.send(());
        }
        self.docker_console_input_txs.remove(device_id);

        if let Some(dev) = self.devices.get_mut(device_id) {
            dev.docker_state.console_mode = mode;
            dev.docker_state.console_container_id = Some(container_id.to_string());
            dev.docker_state.console_state = ConsoleStreamState::Connecting;
            dev.docker_state.console_buffer.clear();
        } else {
            return;
        }

        let (input_tx, input_rx) = mpsc::channel::<ntx_proto::DockerConsoleInput>(64);
        let _ = input_tx.try_send(ntx_proto::DockerConsoleInput {
            input: Some(ntx_proto::docker_console_input::Input::Init(
                ntx_proto::DockerConsoleInit {
                    container_id: container_id.to_string(),
                    mode: mode.as_proto_mode().to_string(),
                    tail_lines: 200,
                },
            )),
        });
        self.docker_console_input_txs
            .insert(device_id.to_string(), input_tx);

        let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
        self.docker_console_cancel_txs
            .insert(device_id.to_string(), cancel_tx);

        let dev_id = device_id.to_string();
        let cid = container_id.to_string();
        let event_tx = self.event_tx.clone();
        let ctx = self.egui_ctx.clone();

        self.rt.spawn(async move {
            let mut stream = match client.stream_docker_console(input_rx).await {
                Ok(s) => s,
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::DockerConsoleClosed {
                            device_id: dev_id,
                            container_id: cid,
                            generation: gen,
                            error: Some(format!("{e:#}")),
                        })
                        .await;
                    ctx.request_repaint();
                    return;
                }
            };

            let _ = event_tx
                .send(AppEvent::DockerConsoleReady {
                    device_id: dev_id.clone(),
                    container_id: cid.clone(),
                    generation: gen,
                })
                .await;
            ctx.request_repaint();

            let mut was_cancelled = false;
            let mut closed_err = None;
            loop {
                tokio::select! {
                    _ = &mut cancel_rx => {
                        was_cancelled = true;
                        break;
                    }
                    item = stream.message() => {
                        match item {
                            Ok(Some(chunk)) => {
                                if !chunk.data.is_empty() {
                                    if event_tx
                                        .send(AppEvent::DockerConsoleChunk {
                                            device_id: dev_id.clone(),
                                            container_id: cid.clone(),
                                            generation: gen,
                                            data: chunk.data,
                                            is_history: chunk.is_history,
                                        })
                                        .await
                                        .is_err()
                                    {
                                        was_cancelled = true;
                                        break;
                                    }
                                    ctx.request_repaint();
                                }
                            }
                            Ok(None) => break,
                            Err(status) => {
                                closed_err = Some(status.message().to_string());
                                break;
                            }
                        }
                    }
                }
            }

            if !was_cancelled {
                let _ = event_tx
                    .send(AppEvent::DockerConsoleClosed {
                        device_id: dev_id,
                        container_id: cid,
                        generation: gen,
                        error: closed_err,
                    })
                    .await;
                ctx.request_repaint();
            }
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
                            if !dev.is_manual {
                                dev.hostname = hostname;
                            }
                            let merged = crate::discovery::merge_and_prioritize_addresses(
                                &dev.addresses,
                                &addresses,
                                dev.is_manual,
                            );
                            if !merged.is_empty() {
                                dev.addresses = merged;
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
                    connected_address,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    self.active_clients.insert(device_id.clone(), *client);
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.status = ConnectionStatus::Connected;
                        dev.system_info = Some(*system_info);
                        dev.cert_fingerprint = Some(fingerprint);
                        if !connected_address.is_empty() {
                            dev.addresses
                                .retain(|a| !a.eq_ignore_ascii_case(&connected_address));
                            dev.addresses.insert(0, connected_address);
                        }
                        if let Some(ref psk) = dev.psk {
                            if !psk.trim().is_empty() {
                                self.settings
                                    .device_psks
                                    .insert(device_id.clone(), psk.clone());
                            }
                        }
                    }
                    self.save_known_devices();

                    if self.selected_device_id.as_deref() == Some(&device_id) {
                        self.maybe_auto_load_active_tab(&device_id);
                    }
                }
                AppEvent::PingResult {
                    device_id,
                    generation,
                    latency_ms,
                    uptime_seconds,
                    version,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.last_ping_ms = Some(latency_ms);
                        dev.server_uptime_secs = Some(uptime_seconds);
                        if !version.is_empty() {
                            dev.version = version;
                        }
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
                    let _ = self.next_connection_gen(&device_id);
                    let _ = self.next_terminal_gen(&device_id);
                    self.active_clients.remove(&device_id);
                    self.terminal_txs.remove(&device_id);
                    self.metrics_cancel_txs.remove(&device_id);
                    self.cancel_service_console(&device_id);
                    self.cancel_docker_console(&device_id);
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
                        dev.managed_services.reset_on_disconnect();
                        dev.docker_state.reset_on_disconnect();
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
                AppEvent::ManagedServicesReceived {
                    device_id,
                    generation,
                    services,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    let mut stream_to_open: Option<String> = None;
                    let mut should_cancel_stream = false;
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.managed_services.services = services;
                        dev.managed_services.is_loading = false;
                        dev.managed_services.unsupported_by_server = false;

                        let still_exists = dev
                            .managed_services
                            .selected_service_id
                            .as_deref()
                            .is_some_and(|sel| {
                                dev.managed_services.services.iter().any(|s| {
                                    s.config.as_ref().is_some_and(|c| c.id == sel)
                                })
                            });
                        if !still_exists {
                            dev.managed_services.selected_service_id = dev
                                .managed_services
                                .services
                                .first()
                                .and_then(|s| s.config.as_ref())
                                .map(|c| c.id.clone());
                        }

                        if let Some(sel_id) = dev.managed_services.selected_service_id.clone() {
                            let same_stream = dev
                                .managed_services
                                .console_service_id
                                .as_deref()
                                == Some(sel_id.as_str());
                            if !same_stream
                                || dev.managed_services.console_state == ConsoleStreamState::Idle
                            {
                                stream_to_open = Some(sel_id);
                            }
                        } else {
                            dev.managed_services.console_service_id = None;
                            dev.managed_services.console_state = ConsoleStreamState::Idle;
                            should_cancel_stream = true;
                        }
                    }
                    if should_cancel_stream {
                        self.cancel_service_console(&device_id);
                    } else if let Some(svc_id) = stream_to_open {
                        self.open_service_console(&device_id, &svc_id);
                    }
                }
                AppEvent::ManagedServicesFailed {
                    device_id,
                    generation,
                    error,
                    is_unimplemented,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.managed_services.is_loading = false;
                        if is_unimplemented {
                            dev.managed_services.unsupported_by_server = true;
                        } else {
                            dev.managed_services.set_status(
                                format!(
                                    "Erreur lors de la récupération des services gérés : {error}"
                                ),
                                true,
                            );
                        }
                    }
                }
                AppEvent::ManagedServiceSaved {
                    device_id,
                    generation,
                    service_name,
                    service_id,
                    success,
                    error,
                    is_unimplemented,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        if is_unimplemented {
                            dev.managed_services.unsupported_by_server = true;
                        } else if success {
                            if let Some(saved_id) = service_id {
                                dev.managed_services.selected_service_id = Some(saved_id);
                            }
                            dev.managed_services.set_status(
                                format!("Service « {service_name} » enregistré."),
                                false,
                            );
                        } else {
                            dev.managed_services.set_status(
                                format!(
                                    "Échec de l'enregistrement de « {service_name} » : {error}"
                                ),
                                true,
                            );
                        }
                    }
                    if success {
                        self.fetch_managed_services(&device_id);
                    }
                }
                AppEvent::ManagedServiceDeleted {
                    device_id,
                    generation,
                    service_id,
                    success,
                    error,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        if success {
                            if dev.managed_services.selected_service_id.as_deref()
                                == Some(service_id.as_str())
                            {
                                dev.managed_services.selected_service_id = None;
                            }
                            dev.managed_services
                                .set_status("Service supprimé avec succès.", false);
                        } else {
                            dev.managed_services.set_status(
                                format!("Échec de la suppression du service : {error}"),
                                true,
                            );
                        }
                    }
                    if success {
                        self.fetch_managed_services(&device_id);
                    }
                }
                AppEvent::ManagedServiceControlled {
                    device_id,
                    generation,
                    service_id,
                    action,
                    success,
                    error,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    let mut reconnect_svc: Option<String> = None;
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        if success {
                            dev.managed_services.set_status(
                                format!("Action « {action} » appliquée avec succès."),
                                false,
                            );
                            if matches!(action.as_str(), "start" | "restart")
                                && dev.managed_services.selected_service_id.as_deref()
                                    == Some(service_id.as_str())
                                && !matches!(
                                    dev.managed_services.console_state,
                                    ConsoleStreamState::Streaming | ConsoleStreamState::Connecting
                                )
                            {
                                reconnect_svc = Some(service_id.clone());
                            }
                        } else {
                            dev.managed_services.set_status(
                                format!("Échec de l'action « {action} » : {error}"),
                                true,
                            );
                        }
                    }
                    if let Some(svc_id) = reconnect_svc {
                        self.open_service_console(&device_id, &svc_id);
                    }
                    self.fetch_managed_services(&device_id);
                }
                AppEvent::ManagedServiceInputSent {
                    device_id,
                    generation,
                    service_id,
                    success,
                    error,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    if !success {
                        if let Some(dev) = self.devices.get_mut(&device_id) {
                            let msg = format!("Échec de l'envoi de la commande : {error}");
                            dev.managed_services.set_status(msg.clone(), true);
                            if dev.managed_services.console_service_id.as_deref()
                                == Some(service_id.as_str())
                            {
                                dev.managed_services
                                    .console_buffer
                                    .push_system_line(&format!("[{msg}]"), Some(Palette::dark().danger));
                            }
                        }
                    }
                }
                AppEvent::ServiceConsoleReady {
                    device_id,
                    service_id,
                    generation,
                } => {
                    if self.service_console_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        if dev.managed_services.console_service_id.as_deref()
                            == Some(service_id.as_str())
                        {
                            dev.managed_services.console_state = ConsoleStreamState::Streaming;
                        }
                    }
                }
                AppEvent::ServiceConsoleChunk {
                    device_id,
                    service_id,
                    generation,
                    data,
                    is_history: _,
                } => {
                    if self.service_console_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        if dev.managed_services.console_service_id.as_deref()
                            == Some(service_id.as_str())
                        {
                            dev.managed_services.console_buffer.push_bytes(&data);
                        }
                    }
                }
                AppEvent::ServiceConsoleClosed {
                    device_id,
                    service_id,
                    generation,
                    error,
                } => {
                    if self.service_console_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    self.service_console_cancel_txs.remove(&device_id);
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        if dev.managed_services.console_service_id.as_deref()
                            == Some(service_id.as_str())
                        {
                            let dark = Palette::dark();
                            dev.managed_services.console_state = match error {
                                Some(err) => {
                                    dev.managed_services.console_buffer.push_system_line(
                                        &format!("[Flux console interrompu : {err}]"),
                                        Some(dark.danger),
                                    );
                                    ConsoleStreamState::Error(err)
                                }
                                None => {
                                    dev.managed_services.console_buffer.push_system_line(
                                        "[Flux console fermé]",
                                        Some(dark.warning),
                                    );
                                    ConsoleStreamState::Closed
                                }
                            };
                        }
                    }
                }
                AppEvent::DockerContainersReceived {
                    device_id,
                    generation,
                    response,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    let resp = *response;
                    let mut stream_to_open: Option<(String, DockerConsoleMode)> = None;
                    let mut should_cancel_stream = false;

                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.docker_state.docker_available = resp.docker_available;
                        dev.docker_state.docker_version = resp.docker_version;
                        dev.docker_state.docker_error = if resp.error_message.trim().is_empty() {
                            None
                        } else {
                            Some(resp.error_message)
                        };
                        dev.docker_state.containers = resp.containers;
                        dev.docker_state.is_loading = false;
                        dev.docker_state.unsupported_by_server = false;

                        if !dev.docker_state.docker_available || dev.docker_state.containers.is_empty()
                        {
                            dev.docker_state.selected_container_id = None;
                            dev.docker_state.console_container_id = None;
                            dev.docker_state.console_state = ConsoleStreamState::Idle;
                            should_cancel_stream = true;
                        } else {
                            let still_exists = dev
                                .docker_state
                                .selected_container_id
                                .as_deref()
                                .is_some_and(|sel| {
                                    dev.docker_state.containers.iter().any(|c| c.id == sel)
                                });
                            if !still_exists {
                                dev.docker_state.selected_container_id = dev
                                    .docker_state
                                    .containers
                                    .iter()
                                    .find(|c| c.state.eq_ignore_ascii_case("running"))
                                    .or_else(|| dev.docker_state.containers.first())
                                    .map(|c| c.id.clone());
                            }

                            if let Some(cid) = dev.docker_state.selected_container_id.clone() {
                                let same_stream = dev
                                    .docker_state
                                    .console_container_id
                                    .as_deref()
                                    == Some(cid.as_str());
                                if !same_stream
                                    || dev.docker_state.console_state == ConsoleStreamState::Idle
                                {
                                    stream_to_open = Some((cid, dev.docker_state.console_mode));
                                }
                            }
                        }
                    }

                    if should_cancel_stream {
                        self.cancel_docker_console(&device_id);
                    } else if let Some((cid, mode)) = stream_to_open {
                        self.open_docker_console(&device_id, &cid, mode);
                    }
                }
                AppEvent::DockerContainersFailed {
                    device_id,
                    generation,
                    error,
                    is_unimplemented,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.docker_state.is_loading = false;
                        if is_unimplemented {
                            dev.docker_state.unsupported_by_server = true;
                        } else {
                            dev.docker_state.set_status(
                                format!(
                                    "Erreur lors de la récupération des conteneurs Docker : {error}"
                                ),
                                true,
                            );
                        }
                    }
                }
                AppEvent::DockerContainerControlled {
                    device_id,
                    generation,
                    container_id,
                    action,
                    success,
                    error,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    let mut include_all = true;
                    let mut cancel_console = false;
                    let mut reconnect_console: Option<(String, DockerConsoleMode)> = None;

                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        include_all = dev.docker_state.include_all;
                        if success {
                            dev.docker_state.set_status(
                                format!("Action « {action} » exécutée sur le conteneur."),
                                false,
                            );
                            if action == "remove"
                                && dev.docker_state.console_container_id.as_deref()
                                    == Some(container_id.as_str())
                            {
                                dev.docker_state.console_container_id = None;
                                dev.docker_state.console_state = ConsoleStreamState::Idle;
                                dev.docker_state.selected_container_id = None;
                                cancel_console = true;
                            } else if matches!(action.as_str(), "start" | "restart" | "unpause")
                                && dev.docker_state.selected_container_id.as_deref()
                                    == Some(container_id.as_str())
                                && !matches!(
                                    dev.docker_state.console_state,
                                    ConsoleStreamState::Streaming | ConsoleStreamState::Connecting
                                )
                            {
                                reconnect_console =
                                    Some((container_id.clone(), dev.docker_state.console_mode));
                            }
                        } else {
                            dev.docker_state.set_status(
                                format!("Échec de l'action « {action} » : {error}"),
                                true,
                            );
                        }
                    }

                    if cancel_console {
                        self.cancel_docker_console(&device_id);
                    } else if let Some((cid, mode)) = reconnect_console {
                        self.open_docker_console(&device_id, &cid, mode);
                    }
                    self.fetch_docker_containers(&device_id, include_all);
                }
                AppEvent::DockerCommandExecuted {
                    device_id,
                    generation,
                    container_id,
                    command,
                    success,
                    exit_code,
                    stdout,
                    stderr,
                    error,
                } => {
                    if self.connection_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        let same_console = dev.docker_state.console_container_id.as_deref()
                            == Some(container_id.as_str());
                        if same_console {
                            let dark = Palette::dark();
                            if !stdout.is_empty() {
                                dev.docker_state.console_buffer.push_bytes(stdout.as_bytes());
                                if !stdout.ends_with('\n') {
                                    dev.docker_state.console_buffer.push_bytes(b"\n");
                                }
                            }
                            if !stderr.is_empty() {
                                for line in stderr.lines() {
                                    dev.docker_state
                                        .console_buffer
                                        .push_system_line(line, Some(dark.danger));
                                }
                            }
                            if !error.is_empty() {
                                dev.docker_state.console_buffer.push_system_line(
                                    &format!("[Erreur exec : {error}]"),
                                    Some(dark.danger),
                                );
                            } else if !success || exit_code != 0 {
                                dev.docker_state.console_buffer.push_system_line(
                                    &format!("[Code de sortie : {exit_code}]"),
                                    Some(dark.warning),
                                );
                            } else {
                                dev.docker_state.console_buffer.push_system_line(
                                    &format!("[✔ Terminé (code {exit_code})]"),
                                    Some(dark.success),
                                );
                            }
                        }
                        if !error.is_empty() {
                            dev.docker_state.set_status(
                                format!("Échec de « {command} » : {error}"),
                                true,
                            );
                        }
                    }
                }
                AppEvent::DockerConsoleReady {
                    device_id,
                    container_id,
                    generation,
                } => {
                    if self.docker_console_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        if dev.docker_state.console_container_id.as_deref()
                            == Some(container_id.as_str())
                        {
                            dev.docker_state.console_state = ConsoleStreamState::Streaming;
                        }
                    }
                }
                AppEvent::DockerConsoleChunk {
                    device_id,
                    container_id,
                    generation,
                    data,
                    is_history: _,
                } => {
                    if self.docker_console_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        if dev.docker_state.console_container_id.as_deref()
                            == Some(container_id.as_str())
                        {
                            dev.docker_state.console_buffer.push_bytes(&data);
                        }
                    }
                }
                AppEvent::DockerConsoleClosed {
                    device_id,
                    container_id,
                    generation,
                    error,
                } => {
                    if self.docker_console_gens.get(&device_id) != Some(&generation) {
                        continue;
                    }
                    self.docker_console_cancel_txs.remove(&device_id);
                    self.docker_console_input_txs.remove(&device_id);
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        if dev.docker_state.console_container_id.as_deref()
                            == Some(container_id.as_str())
                        {
                            let dark = Palette::dark();
                            dev.docker_state.console_state = match error {
                                Some(err) => {
                                    dev.docker_state.console_buffer.push_system_line(
                                        &format!("[Flux console Docker interrompu : {err}]"),
                                        Some(dark.danger),
                                    );
                                    ConsoleStreamState::Error(err)
                                }
                                None => {
                                    dev.docker_state.console_buffer.push_system_line(
                                        "[Flux console Docker fermé]",
                                        Some(dark.warning),
                                    );
                                    ConsoleStreamState::Closed
                                }
                            };
                        }
                    }
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
                        DeviceTab::System => {
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
                        DeviceTab::Services if !dev.managed_services.unsupported_by_server => {
                            need_periodic_repaint = true;
                            if !dev.managed_services.is_loading {
                                let should_refresh = dev
                                    .managed_services
                                    .last_refresh
                                    .map(|t| t.elapsed() >= Duration::from_secs(3))
                                    .unwrap_or(true);
                                if should_refresh {
                                    self.fetch_managed_services(id);
                                }
                            }
                        }
                        DeviceTab::Docker
                            if !dev.docker_state.unsupported_by_server
                                && dev.docker_state.docker_available =>
                        {
                            need_periodic_repaint = true;
                            if !dev.docker_state.is_loading {
                                let should_refresh = dev
                                    .docker_state
                                    .last_refresh
                                    .map(|t| t.elapsed() >= Duration::from_secs(4))
                                    .unwrap_or(true);
                                let include_all = dev.docker_state.include_all;
                                if should_refresh {
                                    self.fetch_docker_containers(id, include_all);
                                }
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

        let p = if self.settings.dark_mode {
            Palette::dark()
        } else {
            Palette::light()
        };

        // 1. Left Sidebar (zero cloning of Device!)
        egui::SidePanel::left("server_sidebar")
            .resizable(true)
            .default_width(275.0)
            .min_width(220.0)
            .max_width(390.0)
            .frame(
                egui::Frame::new()
                    .fill(p.bg_sidebar)
                    .inner_margin(egui::Margin::same(12))
                    .stroke(egui::Stroke::new(1.0_f32, p.border)),
            )
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
                    self.active_view == ActiveView::Overview,
                    self.active_view == ActiveView::GlobalSettings,
                    self.settings.dark_mode,
                ) {
                    match action {
                        SidebarAction::OpenOverview => {
                            self.active_view = ActiveView::Overview;
                        }
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
                            self.active_view = ActiveView::Device;
                            self.add_manual_device(manual);
                        }
                        SidebarAction::RefreshDiscovery => {
                            self.restart_discovery();
                        }
                        SidebarAction::OpenGlobalSettings => {
                            self.active_view = ActiveView::GlobalSettings;
                        }
                        SidebarAction::OpenTerminal(id) => {
                            self.active_view = ActiveView::Device;
                            self.select_device(Some(id));
                            self.device_panel_state.active_tab = DeviceTab::Terminal;
                        }
                        SidebarAction::ConfigureDevice(id) => {
                            self.active_view = ActiveView::Device;
                            self.select_device(Some(id));
                            self.device_panel_state.active_tab = DeviceTab::Config;
                        }
                        SidebarAction::ToggleTheme => {
                            self.settings.dark_mode = !self.settings.dark_mode;
                            self.apply_theme();
                            let _ = self.settings.save();
                        }
                    }
                }
            });

        // 2. Central Panel
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p.bg_app)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ctx, |ui| match self.active_view {
                ActiveView::Overview => {
                    let mut device_refs: Vec<&Device> = self.devices.values().collect();
                    device_refs.sort_by(|a, b| {
                        let a_conn = a.status.is_connected();
                        let b_conn = b.status.is_connected();
                        b_conn.cmp(&a_conn).then_with(|| {
                            a.hostname.to_lowercase().cmp(&b.hostname.to_lowercase())
                        })
                    });

                    if let Some(action) = overview::render(
                        ui,
                        &mut self.overview_state,
                        &device_refs,
                        self.settings.overview_view_mode,
                        self.settings.hide_virtual_disks,
                        self.settings.hide_virtual_interfaces,
                    ) {
                        match action {
                            OverviewAction::SelectDevice(id) => {
                                self.active_view = ActiveView::Device;
                                self.device_panel_state.active_tab = DeviceTab::Metrics;
                                self.select_device(Some(id));
                            }
                            OverviewAction::ConnectDevice(id) => {
                                self.initiate_connect(&id, None);
                            }
                            OverviewAction::DisconnectDevice(id) => {
                                self.disconnect_device(&id);
                            }
                            OverviewAction::ReconnectDevice(id) => {
                                self.disconnect_device(&id);
                                self.initiate_connect(&id, None);
                            }
                            OverviewAction::PingDevice(id) => {
                                self.ping_device(&id);
                            }
                            OverviewAction::PingAll => {
                                let ids: Vec<String> = self
                                    .devices
                                    .iter()
                                    .filter(|(_, d)| d.status.is_connected())
                                    .map(|(id, _)| id.clone())
                                    .collect();
                                for id in ids {
                                    self.ping_device(&id);
                                }
                            }
                            OverviewAction::OpenTerminal(id) => {
                                self.active_view = ActiveView::Device;
                                self.device_panel_state.active_tab = DeviceTab::Terminal;
                                self.select_device(Some(id));
                            }
                            OverviewAction::OpenProcesses(id) => {
                                self.active_view = ActiveView::Device;
                                self.device_panel_state.active_tab = DeviceTab::System;
                                self.select_device(Some(id));
                            }
                            OverviewAction::OpenFavorites(id) => {
                                self.active_view = ActiveView::Device;
                                self.device_panel_state.active_tab = DeviceTab::Services;
                                self.select_device(Some(id));
                            }
                            OverviewAction::ConfigureDevice(id) => {
                                self.active_view = ActiveView::Device;
                                self.device_panel_state.active_tab = DeviceTab::Config;
                                self.select_device(Some(id));
                            }
                            OverviewAction::RemoveDevice(id) => {
                                self.remove_device(&id);
                            }
                            OverviewAction::ConnectAll => {
                                let with_psk: Vec<String> = self
                                    .devices
                                    .iter()
                                    .filter(|(_, d)| {
                                        !d.status.is_connected()
                                            && !matches!(d.status, ConnectionStatus::Connecting)
                                            && self.resolve_psk(d).is_some()
                                    })
                                    .map(|(id, _)| id.clone())
                                    .collect();
                                let targets = if !with_psk.is_empty() {
                                    with_psk
                                } else {
                                    self.devices
                                        .iter()
                                        .filter(|(_, d)| {
                                            !d.status.is_connected()
                                                && !matches!(d.status, ConnectionStatus::Connecting)
                                        })
                                        .map(|(id, _)| id.clone())
                                        .collect()
                                };
                                for id in targets {
                                    self.initiate_connect(&id, None);
                                }
                            }
                            OverviewAction::DisconnectAll => {
                                let ids: Vec<String> = self
                                    .devices
                                    .iter()
                                    .filter(|(_, d)| d.status.is_connected())
                                    .map(|(id, _)| id.clone())
                                    .collect();
                                for id in ids {
                                    self.disconnect_device(&id);
                                }
                            }
                            OverviewAction::OpenAddModal => {
                                self.sidebar_state.show_add_modal = true;
                                self.sidebar_state.add_error = None;
                            }
                            OverviewAction::RefreshDiscovery => {
                                self.restart_discovery();
                            }
                            OverviewAction::SetViewMode(mode) => {
                                self.settings.overview_view_mode = mode;
                                let _ = self.settings.save();
                            }
                        }
                    }
                }
                ActiveView::GlobalSettings => {
                    let device_names: HashMap<String, String> = self
                        .devices
                        .iter()
                        .map(|(id, d)| (id.clone(), d.hostname.clone()))
                        .collect();
                    if let Some(action) = settings::render(
                        ui,
                        &mut self.settings,
                        &mut self.settings_state,
                        self.settings_warning.as_deref(),
                        &device_names,
                    ) {
                        match action {
                            SettingsAction::BackToPanel => {
                                self.active_view = if self.selected_device_id.is_some() {
                                    ActiveView::Device
                                } else {
                                    ActiveView::Overview
                                };
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
                        let should_open_term =
                            self.device_panel_state.active_tab == DeviceTab::Terminal
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
                        let hide_virtual_disks = self.settings.hide_virtual_disks;
                        let hide_virtual_interfaces = self.settings.hide_virtual_interfaces;
                        let terminal_font_size = self.settings.terminal_font_size;

                        let action = if let Some(dev) = self.devices.get_mut(&id) {
                            device_panel::render(
                                ui,
                                &mut self.device_panel_state,
                                dev,
                                refresh_rate_ms,
                                history_capacity,
                                hide_virtual_disks,
                                hide_virtual_interfaces,
                                terminal_font_size,
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
                                    if self.devices.is_empty() {
                                        self.active_view = ActiveView::Overview;
                                    }
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
                                DevicePanelAction::Ping(dev_id) => {
                                    self.ping_device(&dev_id);
                                }
                                DevicePanelAction::UpdateDeviceConfig {
                                    id: dev_id,
                                    hostname,
                                    address,
                                    port,
                                    auto_connect,
                                } => {
                                    if let Some(d) = self.devices.get_mut(&dev_id) {
                                        d.hostname = hostname;
                                        d.auto_connect = auto_connect;
                                        if d.is_manual {
                                            if let Some(addr) = address {
                                                let merged =
                                                    crate::discovery::merge_and_prioritize_addresses(
                                                        std::slice::from_ref(&addr),
                                                        &d.addresses,
                                                        true,
                                                    );
                                                d.addresses = if merged.is_empty() {
                                                    vec![addr]
                                                } else {
                                                    merged
                                                };
                                            }
                                            if let Some(prt) = port {
                                                d.port = prt;
                                            }
                                        }
                                    }
                                    self.save_known_devices();
                                }
                                DevicePanelAction::ToggleHideVirtualDisks => {
                                    self.settings.hide_virtual_disks =
                                        !self.settings.hide_virtual_disks;
                                    let _ = self.settings.save();
                                }
                                DevicePanelAction::ToggleHideVirtualInterfaces => {
                                    self.settings.hide_virtual_interfaces =
                                        !self.settings.hide_virtual_interfaces;
                                    let _ = self.settings.save();
                                }
                                DevicePanelAction::ToggleMetricsPause(dev_id) => {
                                    if let Some(d) = self.devices.get_mut(&dev_id) {
                                        d.metrics_paused = !d.metrics_paused;
                                    }
                                }
                                DevicePanelAction::ClearMetricsHistory(dev_id) => {
                                    if let Some(d) = self.devices.get_mut(&dev_id) {
                                        d.clear_metrics_history();
                                    }
                                }
                                DevicePanelAction::SwitchTab(_tab) => {
                                    self.maybe_auto_load_active_tab(&id);
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
                                    ProcessPanelAction::ViewServiceLogs(service_name) => {
                                        let safe_unit =
                                            service_name.trim().replace('\'', "'\\''");
                                        let cmd = format!("journalctl -u '{safe_unit}' -n 100 -f");
                                        self.open_or_run_terminal_command(&id, cmd);
                                    }
                                    ProcessPanelAction::AddFavoriteFromProcess(proc_item) => {
                                        let command = if proc_item.command_line.trim().is_empty() {
                                            proc_item.name.clone()
                                        } else {
                                            proc_item.command_line.clone()
                                        };
                                        let cfg = ntx_proto::ManagedServiceConfig {
                                            id: String::new(),
                                            name: proc_item.name.clone(),
                                            icon: "⚡".to_string(),
                                            description: format!("Processus {}", proc_item.name),
                                            command,
                                            working_dir: String::new(),
                                            stop_command: String::new(),
                                            auto_start: false,
                                            auto_restart: false,
                                            env_vars: Vec::new(),
                                            quick_commands: Vec::new(),
                                            systemd_unit: String::new(),
                                            attach_command: String::new(),
                                        };
                                        self.device_panel_state.active_tab = DeviceTab::Services;
                                        self.device_panel_state
                                            .services_state
                                            .open_prefilled(cfg);
                                        self.maybe_auto_load_active_tab(&id);
                                    }
                                    ProcessPanelAction::AddFavoriteFromService(srv_item) => {
                                        let display_name = srv_item
                                            .name
                                            .strip_suffix(".service")
                                            .unwrap_or(&srv_item.name)
                                            .to_string();
                                        let cfg = ntx_proto::ManagedServiceConfig {
                                            id: String::new(),
                                            name: display_name,
                                            icon: "⚙".to_string(),
                                            description: srv_item.description.clone(),
                                            command: String::new(),
                                            working_dir: String::new(),
                                            stop_command: String::new(),
                                            auto_start: false,
                                            auto_restart: false,
                                            env_vars: Vec::new(),
                                            quick_commands: Vec::new(),
                                            systemd_unit: srv_item.name.clone(),
                                            attach_command: String::new(),
                                        };
                                        self.device_panel_state.active_tab = DeviceTab::Services;
                                        self.device_panel_state
                                            .services_state
                                            .open_prefilled(cfg);
                                        self.maybe_auto_load_active_tab(&id);
                                    }
                                },
                                DevicePanelAction::ServicesAction(svc_action) => match svc_action {
                                    ServicesPanelAction::Refresh => {
                                        if let Some(dev) = self.devices.get_mut(&id) {
                                            dev.managed_services.unsupported_by_server = false;
                                        }
                                        self.fetch_managed_services(&id);
                                    }
                                    ServicesPanelAction::SelectService(svc_id) => {
                                        let needs_stream = if let Some(dev) =
                                            self.devices.get_mut(&id)
                                        {
                                            dev.managed_services.selected_service_id =
                                                Some(svc_id.clone());
                                            dev.managed_services.console_service_id.as_deref()
                                                != Some(svc_id.as_str())
                                                || !matches!(
                                                    dev.managed_services.console_state,
                                                    ConsoleStreamState::Streaming
                                                        | ConsoleStreamState::Connecting
                                                )
                                        } else {
                                            false
                                        };
                                        if needs_stream {
                                            self.open_service_console(&id, &svc_id);
                                        }
                                    }
                                    ServicesPanelAction::SaveService(config) => {
                                        self.save_managed_service(&id, config);
                                    }
                                    ServicesPanelAction::DeleteService(svc_id) => {
                                        self.delete_managed_service(&id, &svc_id);
                                    }
                                    ServicesPanelAction::ControlService {
                                        id: svc_id,
                                        action: act,
                                    } => {
                                        self.control_managed_service(&id, &svc_id, &act);
                                    }
                                    ServicesPanelAction::SendInput { id: svc_id, input } => {
                                        self.send_managed_service_input(&id, &svc_id, &input);
                                    }
                                    ServicesPanelAction::ReconnectConsole(svc_id) => {
                                        self.open_service_console(&id, &svc_id);
                                    }
                                    ServicesPanelAction::ClearConsole => {
                                        if let Some(dev) = self.devices.get_mut(&id) {
                                            dev.managed_services.console_buffer.clear();
                                        }
                                    }
                                    ServicesPanelAction::AttachExternalTerminal { command } => {
                                        self.open_or_run_terminal_command(&id, command);
                                    }
                                    ServicesPanelAction::ImportLegacyFavorites => {
                                        self.import_legacy_favorites(&id);
                                    }
                                    ServicesPanelAction::ClearStatus => {
                                        if let Some(dev) = self.devices.get_mut(&id) {
                                            dev.managed_services.clear_status();
                                        }
                                    }
                                },
                                DevicePanelAction::DockerAction(dk_action) => match dk_action {
                                    DockerPanelAction::Refresh { include_all } => {
                                        if let Some(dev) = self.devices.get_mut(&id) {
                                            dev.docker_state.unsupported_by_server = false;
                                            dev.docker_state.docker_available = true;
                                            dev.docker_state.docker_error = None;
                                            dev.docker_state.include_all = include_all;
                                        }
                                        self.fetch_docker_containers(&id, include_all);
                                    }
                                    DockerPanelAction::ToggleIncludeAll(include_all) => {
                                        if let Some(dev) = self.devices.get_mut(&id) {
                                            dev.docker_state.include_all = include_all;
                                        }
                                        self.fetch_docker_containers(&id, include_all);
                                    }
                                    DockerPanelAction::SelectContainer(cid) => {
                                        let open_req = if let Some(dev) = self.devices.get_mut(&id)
                                        {
                                            dev.docker_state.selected_container_id =
                                                Some(cid.clone());
                                            let needs = dev
                                                .docker_state
                                                .console_container_id
                                                .as_deref()
                                                != Some(cid.as_str())
                                                || !matches!(
                                                    dev.docker_state.console_state,
                                                    ConsoleStreamState::Streaming
                                                        | ConsoleStreamState::Connecting
                                                );
                                            needs.then_some(dev.docker_state.console_mode)
                                        } else {
                                            None
                                        };
                                        if let Some(mode) = open_req {
                                            self.open_docker_console(&id, &cid, mode);
                                        }
                                    }
                                    DockerPanelAction::SetConsoleMode { container_id, mode } => {
                                        if let Some(dev) = self.devices.get_mut(&id) {
                                            dev.docker_state.console_mode = mode;
                                            dev.docker_state.console_buffer.clear();
                                        }
                                        self.open_docker_console(&id, &container_id, mode);
                                    }
                                    DockerPanelAction::ControlContainer {
                                        container_id,
                                        action: act,
                                        force,
                                    } => {
                                        self.control_docker_container(
                                            &id,
                                            &container_id,
                                            &act,
                                            force,
                                        );
                                    }
                                    DockerPanelAction::SendConsoleInput {
                                        container_id,
                                        input,
                                    } => {
                                        let mut payload = input;
                                        if !payload.ends_with('\n') && !payload.ends_with('\r') {
                                            payload.push('\n');
                                        }
                                        let matches_stream = self
                                            .devices
                                            .get(&id)
                                            .and_then(|d| {
                                                d.docker_state.console_container_id.as_deref()
                                            })
                                            == Some(container_id.as_str());
                                        if matches_stream {
                                            let send_ok = self
                                                .docker_console_input_txs
                                                .get(&id)
                                                .is_some_and(|tx| {
                                                    tx.try_send(ntx_proto::DockerConsoleInput {
                                                        input: Some(
                                                            ntx_proto::docker_console_input::Input::Data(
                                                                payload.into_bytes(),
                                                            ),
                                                        ),
                                                    })
                                                    .is_ok()
                                                });
                                            if !send_ok {
                                                if let Some(dev) = self.devices.get_mut(&id) {
                                                    dev.docker_state.set_status(
                                                        "Impossible d'envoyer l'entrée : flux console Docker fermé.",
                                                        true,
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    DockerPanelAction::ExecCommand {
                                        container_id,
                                        command,
                                    } => {
                                        self.exec_docker_command(&id, &container_id, &command);
                                    }
                                    DockerPanelAction::ReconnectConsole { container_id, mode } => {
                                        self.open_docker_console(&id, &container_id, mode);
                                    }
                                    DockerPanelAction::ClearConsole => {
                                        if let Some(dev) = self.devices.get_mut(&id) {
                                            dev.docker_state.console_buffer.clear();
                                        }
                                    }
                                    DockerPanelAction::ClearStatus => {
                                        if let Some(dev) = self.devices.get_mut(&id) {
                                            dev.docker_state.clear_status();
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
                                                        input: Some(Input::Resize(
                                                            TerminalResize {
                                                                cols: cols as u32,
                                                                rows: rows as u32,
                                                            },
                                                        )),
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
                                            TerminalAction::SetFontSize(sz) => {
                                                self.settings.terminal_font_size =
                                                    sz.clamp(10.0, 20.0);
                                                let _ = self.settings.save();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        self.active_view = ActiveView::Overview;
                    }
                }
            });
    }
}
