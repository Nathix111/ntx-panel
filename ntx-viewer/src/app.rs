use crate::client::NtxClient;
use crate::discovery::{start_discovery, DiscoveredDevice, DiscoveryEvent, DiscoveryHandle};
use crate::state::{AppSettings, ConnectionStatus, Device, ManualDevice};
use crate::ui::device_panel::{self, DevicePanelAction, DevicePanelState, DeviceTab};
use crate::ui::process_panel::ProcessPanelAction;
use crate::ui::settings::{self, SettingsAction, SettingsState};
use crate::ui::sidebar::{self, SidebarAction, SidebarState};
use crate::ui::terminal::TerminalAction;
use ntx_proto::terminal_input::Input;
use ntx_proto::{SystemInfo, SystemMetrics, TerminalInput, TerminalResize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveView {
    Device,
    GlobalSettings,
}

pub enum AppEvent {
    Connected {
        device_id: String,
        system_info: SystemInfo,
        session_token: String,
    },
    ConnectionFailed {
        device_id: String,
        error: String,
        is_auth_error: bool,
    },
    MetricsReceived {
        device_id: String,
        metrics: SystemMetrics,
    },
    TerminalOutput {
        device_id: String,
        data: Vec<u8>,
    },
    TerminalClosed {
        device_id: String,
    },
    Disconnected {
        device_id: String,
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
    active_view: ActiveView,
    selected_device_id: Option<String>,
    device_panel_state: DevicePanelState,
    sidebar_state: SidebarState,
    devices: HashMap<String, Device>,
    settings: AppSettings,
    settings_state: SettingsState,
    discovery_handle: Option<DiscoveryHandle>,
    discovery_rx: mpsc::Receiver<DiscoveryEvent>,
    event_tx: mpsc::Sender<AppEvent>,
    event_rx: mpsc::Receiver<AppEvent>,
    terminal_txs: HashMap<String, mpsc::Sender<TerminalInput>>,
    metrics_cancel_txs: HashMap<String, tokio::sync::oneshot::Sender<()>>,
}

impl NtxApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, rt: Arc<tokio::runtime::Runtime>) -> Self {
        let (disc_tx, disc_rx) = mpsc::channel(64);
        let (event_tx, event_rx) = mpsc::channel(128);

        let discovery_handle = match start_discovery(disc_tx, rt.handle().clone()) {
            Ok(handle) => Some(handle),
            Err(e) => {
                warn!("Could not start mDNS discovery: {e}");
                None
            }
        };

        let settings = AppSettings::load();

        let mut app = Self {
            rt,
            active_view: ActiveView::Device,
            selected_device_id: None,
            device_panel_state: DevicePanelState::default(),
            sidebar_state: SidebarState::new(),
            devices: HashMap::new(),
            settings,
            settings_state: SettingsState::default(),
            discovery_handle,
            discovery_rx: disc_rx,
            event_tx,
            event_rx,
            terminal_txs: HashMap::new(),
            metrics_cancel_txs: HashMap::new(),
        };

        // 1. Load any saved known devices from settings (persisted across restarts)
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
                .or_else(|| app.settings.device_psks.get(&known.id).cloned());
            dev.status = ConnectionStatus::Disconnected;
            app.devices.insert(known.id.clone(), dev);
        }

        // 2. Load any saved manual devices from settings
        for manual in &app.settings.manual_devices.clone() {
            app.add_manual_device(manual.clone());
        }

        // 3. Auto-select first device if available
        if !app.devices.is_empty() {
            let first_id = app.devices.keys().next().cloned();
            if let Some(ref id) = first_id {
                if let Some(dev) = app.devices.get(id) {
                    app.device_panel_state.psk_input = dev.psk.clone().unwrap_or_default();
                }
            }
            app.selected_device_id = first_id;
        }

        app
    }

    fn save_known_devices(&mut self) {
        self.settings.known_devices = self
            .devices
            .values()
            .map(|d| crate::state::KnownDevice {
                id: d.id.clone(),
                hostname: d.hostname.clone(),
                addresses: d.addresses.clone(),
                port: d.port,
                version: d.version.clone(),
                psk: d.psk.clone(),
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

        if let Some(id) = existing_id {
            if let Some(dev) = self.devices.get_mut(&id) {
                dev.hostname = manual.hostname;
                if manual.psk.is_some() {
                    dev.psk = manual.psk;
                }
            }
            if self.selected_device_id.is_none() {
                self.selected_device_id = Some(id);
            }
        } else {
            let id = format!("{}:{}", manual.address, manual.port);
            let mut dev = Device::new(
                id.clone(),
                manual.hostname,
                vec![manual.address],
                manual.port,
                "manual".to_string(),
            );
            dev.psk = manual.psk;
            self.devices.insert(id.clone(), dev);
            if self.selected_device_id.is_none() {
                self.selected_device_id = Some(id);
            }
        }
        self.save_known_devices();
    }

    fn restart_discovery(&mut self) {
        if let Some(mut handle) = self.discovery_handle.take() {
            handle.stop();
        }

        let (disc_tx, disc_rx) = mpsc::channel(64);
        self.discovery_rx = disc_rx;
        match start_discovery(disc_tx, self.rt.handle().clone()) {
            Ok(handle) => self.discovery_handle = Some(handle),
            Err(e) => warn!("Failed to restart discovery: {e}"),
        }
    }

    fn get_effective_psk(&self, device: &Device) -> String {
        if let Some(dev_psk) = &device.psk {
            if !dev_psk.is_empty() {
                return dev_psk.clone();
            }
        }
        if let Some(saved_psk) = self.settings.device_psks.get(&device.id) {
            if !saved_psk.is_empty() {
                return saved_psk.clone();
            }
        }
        self.settings.default_psk.clone()
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

        let (endpoint, psk) = match self.devices.get_mut(device_id) {
            Some(d) => {
                let resolved_psk = d
                    .psk
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| {
                        self.settings
                            .device_psks
                            .get(device_id)
                            .map(|s| s.as_str())
                    })
                    .or_else(|| {
                        if !self.settings.default_psk.trim().is_empty() {
                            Some(self.settings.default_psk.as_str())
                        } else {
                            None
                        }
                    });

                if let Some(p) = resolved_psk {
                    let psk_str = p.to_string();
                    d.status = ConnectionStatus::Connecting;
                    (d.grpc_endpoint(), psk_str)
                } else {
                    d.status =
                        ConnectionStatus::AuthFailed("Please enter the PSK first".to_string());
                    return;
                }
            }
            None => return,
        };

        let viewer_id = self.settings.viewer_id.clone();
        let interval_ms = self.settings.refresh_rate_ms;
        let event_tx = self.event_tx.clone();
        let dev_id = device_id.to_string();

        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
        if let Some(old_tx) = self.metrics_cancel_txs.insert(dev_id.clone(), cancel_tx) {
            let _ = old_tx.send(());
        }

        self.rt.spawn(async move {
            info!("Connecting to device {} at {}", dev_id, endpoint);
            let mut client = match NtxClient::connect(&endpoint).await {
                Ok(c) => c,
                Err(e) => {
                    error!("Connection error to {}: {e}", dev_id);
                    let _ = event_tx
                        .send(AppEvent::ConnectionFailed {
                            device_id: dev_id,
                            error: format!("Failed to connect: {e}"),
                            is_auth_error: false,
                        })
                        .await;
                    return;
                }
            };

            let session_token = match client.authenticate(&psk, &viewer_id).await {
                Ok(token) => token,
                Err(e) => {
                    let err_str = e.to_string();
                    let is_auth = err_str.to_lowercase().contains("auth")
                        || err_str.to_lowercase().contains("psk")
                        || err_str.to_lowercase().contains("rejected");
                    let _ = event_tx
                        .send(AppEvent::ConnectionFailed {
                            device_id: dev_id,
                            error: err_str,
                            is_auth_error: is_auth,
                        })
                        .await;
                    return;
                }
            };

            let system_info = match client.get_system_info().await {
                Ok(info) => info,
                Err(e) => {
                    warn!("GetSystemInfo error for {}: {e}", dev_id);
                    SystemInfo::default()
                }
            };

            let _ = event_tx
                .send(AppEvent::Connected {
                    device_id: dev_id.clone(),
                    system_info,
                    session_token,
                })
                .await;

            match client.stream_metrics(interval_ms).await {
                Ok(mut stream) => loop {
                    tokio::select! {
                        _ = &mut cancel_rx => {
                            info!("Metrics streaming cancelled for {}", dev_id);
                            break;
                        }
                        item = stream.message() => {
                            match item {
                                Ok(Some(metrics)) => {
                                    if event_tx.send(AppEvent::MetricsReceived {
                                        device_id: dev_id.clone(),
                                        metrics,
                                    }).await.is_err() {
                                        break;
                                    }
                                }
                                Ok(None) => {
                                    info!("Metrics stream ended by server for {}", dev_id);
                                    break;
                                }
                                Err(e) => {
                                    warn!("Metrics stream error for {}: {e}", dev_id);
                                    break;
                                }
                            }
                        }
                    }
                },
                Err(e) => {
                    warn!("Failed to initiate metrics stream for {}: {e}", dev_id);
                }
            }

            let _ = event_tx
                .send(AppEvent::Disconnected { device_id: dev_id })
                .await;
        });
    }

    fn disconnect_device(&mut self, device_id: &str) {
        if let Some(cancel) = self.metrics_cancel_txs.remove(device_id) {
            let _ = cancel.send(());
        }
        self.terminal_txs.remove(device_id);
        if let Some(dev) = self.devices.get_mut(device_id) {
            dev.status = ConnectionStatus::Disconnected;
            dev.session_token = None;
        }
    }

    fn open_terminal_session(&mut self, device_id: &str) {
        if self.terminal_txs.contains_key(device_id) {
            return;
        }

        let dev = match self.devices.get(device_id) {
            Some(d) => d,
            None => return,
        };

        let endpoint = dev.grpc_endpoint();
        let psk = self.get_effective_psk(dev);
        let viewer_id = self.settings.viewer_id.clone();
        let dev_id = device_id.to_string();
        let event_tx = self.event_tx.clone();

        let (input_tx, input_rx) = mpsc::channel::<TerminalInput>(64);
        self.terminal_txs.insert(dev_id.clone(), input_tx.clone());

        let _ = input_tx.try_send(TerminalInput {
            input: Some(Input::Resize(TerminalResize {
                cols: 80,
                rows: 24,
            })),
        });

        self.rt.spawn(async move {
            let mut client = match NtxClient::connect(&endpoint).await {
                Ok(c) => c,
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::TerminalOutput {
                            device_id: dev_id.clone(),
                            data: format!("Failed to connect to terminal: {e}\r\n").into_bytes(),
                        })
                        .await;
                    let _ = event_tx
                        .send(AppEvent::TerminalClosed { device_id: dev_id })
                        .await;
                    return;
                }
            };

            if let Err(e) = client.authenticate(&psk, &viewer_id).await {
                let _ = event_tx
                    .send(AppEvent::TerminalOutput {
                        device_id: dev_id.clone(),
                        data: format!("Terminal authentication failed: {e}\r\n").into_bytes(),
                    })
                    .await;
                let _ = event_tx
                    .send(AppEvent::TerminalClosed { device_id: dev_id })
                    .await;
                return;
            }

            let mut out_stream = match client.open_terminal(input_rx).await {
                Ok(s) => s,
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::TerminalOutput {
                            device_id: dev_id.clone(),
                            data: format!("Failed to open terminal stream: {e}\r\n").into_bytes(),
                        })
                        .await;
                    let _ = event_tx
                        .send(AppEvent::TerminalClosed { device_id: dev_id })
                        .await;
                    return;
                }
            };

            while let Ok(Some(output)) = out_stream.message().await {
                if !output.data.is_empty()
                    && event_tx
                        .send(AppEvent::TerminalOutput {
                            device_id: dev_id.clone(),
                            data: output.data,
                        })
                        .await
                        .is_err()
                {
                    break;
                }
            }

            let _ = event_tx
                .send(AppEvent::TerminalClosed { device_id: dev_id })
                .await;
        });
    }

    fn get_client_credentials(&self, device_id: &str) -> Option<(String, String, String)> {
        let dev = self.devices.get(device_id)?;
        let endpoint = dev.grpc_endpoint();
        let psk = self.get_effective_psk(dev);
        let viewer_id = self.settings.viewer_id.clone();
        Some((endpoint, psk, viewer_id))
    }

    fn fetch_processes(&mut self, device_id: &str) {
        if let Some(dev) = self.devices.get_mut(device_id) {
            if dev.process_state.is_loading_processes {
                return;
            }
            dev.process_state.is_loading_processes = true;
            dev.process_state.last_process_refresh = Some(std::time::Instant::now());
        }

        let (endpoint, psk, viewer_id) = match self.get_client_credentials(device_id) {
            Some(c) => c,
            None => return,
        };

        let dev_id = device_id.to_string();
        let event_tx = self.event_tx.clone();

        self.rt.spawn(async move {
            let mut client = match NtxClient::connect(&endpoint).await {
                Ok(c) => c,
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::ProcessesFailed {
                            device_id: dev_id,
                            error: format!("Failed to connect: {e}"),
                        })
                        .await;
                    return;
                }
            };

            if let Err(e) = client.authenticate(&psk, &viewer_id).await {
                let _ = event_tx
                    .send(AppEvent::ProcessesFailed {
                        device_id: dev_id,
                        error: format!("Authentication failed: {e}"),
                    })
                    .await;
                return;
            }

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
                            error: format!("ListProcesses failed: {e}"),
                        })
                        .await;
                }
            }
        });
    }

    fn fetch_services(&mut self, device_id: &str) {
        if let Some(dev) = self.devices.get_mut(device_id) {
            if dev.process_state.is_loading_services {
                return;
            }
            dev.process_state.is_loading_services = true;
            dev.process_state.last_service_refresh = Some(std::time::Instant::now());
        }

        let (endpoint, psk, viewer_id) = match self.get_client_credentials(device_id) {
            Some(c) => c,
            None => return,
        };

        let dev_id = device_id.to_string();
        let event_tx = self.event_tx.clone();

        self.rt.spawn(async move {
            let mut client = match NtxClient::connect(&endpoint).await {
                Ok(c) => c,
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::ServicesFailed {
                            device_id: dev_id,
                            error: format!("Failed to connect: {e}"),
                        })
                        .await;
                    return;
                }
            };

            if let Err(e) = client.authenticate(&psk, &viewer_id).await {
                let _ = event_tx
                    .send(AppEvent::ServicesFailed {
                        device_id: dev_id,
                        error: format!("Authentication failed: {e}"),
                    })
                    .await;
                return;
            }

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
                            error: format!("ListServices failed: {e}"),
                        })
                        .await;
                }
            }
        });
    }

    fn kill_process(&mut self, device_id: &str, pid: u32, signal: i32) {
        let (endpoint, psk, viewer_id) = match self.get_client_credentials(device_id) {
            Some(c) => c,
            None => return,
        };

        let dev_id = device_id.to_string();
        let event_tx = self.event_tx.clone();

        self.rt.spawn(async move {
            let mut client = match NtxClient::connect(&endpoint).await {
                Ok(c) => c,
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::ProcessKilled {
                            device_id: dev_id,
                            pid,
                            signal,
                            success: false,
                            error: format!("Failed to connect: {e}"),
                        })
                        .await;
                    return;
                }
            };

            if let Err(e) = client.authenticate(&psk, &viewer_id).await {
                let _ = event_tx
                    .send(AppEvent::ProcessKilled {
                        device_id: dev_id,
                        pid,
                        signal,
                        success: false,
                        error: format!("Authentication failed: {e}"),
                    })
                    .await;
                return;
            }

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
                            error: format!("KillProcess failed: {e}"),
                        })
                        .await;
                }
            }
        });
    }

    fn control_service(&mut self, device_id: &str, service_name: &str, action: &str) {
        let (endpoint, psk, viewer_id) = match self.get_client_credentials(device_id) {
            Some(c) => c,
            None => return,
        };

        let dev_id = device_id.to_string();
        let s_name = service_name.to_string();
        let act = action.to_string();
        let event_tx = self.event_tx.clone();

        self.rt.spawn(async move {
            let mut client = match NtxClient::connect(&endpoint).await {
                Ok(c) => c,
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::ServiceControlled {
                            device_id: dev_id,
                            service_name: s_name,
                            action: act,
                            success: false,
                            error: format!("Failed to connect: {e}"),
                        })
                        .await;
                    return;
                }
            };

            if let Err(e) = client.authenticate(&psk, &viewer_id).await {
                let _ = event_tx
                    .send(AppEvent::ServiceControlled {
                        device_id: dev_id,
                        service_name: s_name,
                        action: act,
                        success: false,
                        error: format!("Authentication failed: {e}"),
                    })
                    .await;
                return;
            }

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
                            error: format!("ControlService failed: {e}"),
                        })
                        .await;
                }
            }
        });
    }

    fn poll_channels(&mut self) {
        // Poll discovery
        while let Ok(event) = self.discovery_rx.try_recv() {
            match event {
                DiscoveryEvent::Added(DiscoveredDevice {
                    fullname,
                    hostname,
                    addresses,
                    port,
                    version,
                }) => {
                    let existing_id = self.devices.iter().find_map(|(id, d)| {
                        if id == &fullname {
                            return Some(id.clone());
                        }
                        if d.port == port {
                            if d.hostname.eq_ignore_ascii_case(&hostname) {
                                return Some(id.clone());
                            }
                            if d.addresses.iter().any(|a| addresses.contains(a)) {
                                return Some(id.clone());
                            }
                        }
                        None
                    });

                    if let Some(id) = existing_id {
                        if let Some(dev) = self.devices.get_mut(&id) {
                            dev.hostname = hostname;
                            dev.addresses = addresses;
                            dev.port = port;
                            dev.version = version;
                            if dev.status == ConnectionStatus::Disconnected {
                                dev.status = ConnectionStatus::Discovered;
                            }
                        }
                    } else {
                        let id = fullname.clone();
                        let mut dev = Device::new(id.clone(), hostname, addresses, port, version);
                        if let Some(saved) = self.settings.device_psks.get(&id) {
                            dev.psk = Some(saved.clone());
                        }
                        self.devices.insert(id.clone(), dev);
                        if self.selected_device_id.is_none() {
                            self.selected_device_id = Some(id);
                        }
                    }
                    self.save_known_devices();
                }
                DiscoveryEvent::Removed(fullname) => {
                    if let Some(dev) = self.devices.get_mut(&fullname) {
                        if !dev.status.is_connected() {
                            dev.status = ConnectionStatus::Disconnected;
                        }
                    }
                }
            }
        }

        // Poll app events
        let cap = self.settings.history_capacity;
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                AppEvent::Connected {
                    device_id,
                    system_info,
                    session_token,
                } => {
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.status = ConnectionStatus::Connected;
                        dev.session_token = Some(session_token);
                        dev.system_info = Some(system_info);
                        if let Some(ref psk) = dev.psk {
                            self.settings
                                .device_psks
                                .insert(device_id.clone(), psk.clone());
                            let _ = self.settings.save();
                        }
                    }
                    if self.device_panel_state.active_tab == DeviceTab::Processes {
                        self.fetch_processes(&device_id);
                        self.fetch_services(&device_id);
                    }
                }
                AppEvent::ConnectionFailed {
                    device_id,
                    error,
                    is_auth_error,
                } => {
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.status = if is_auth_error {
                            ConnectionStatus::AuthFailed(error)
                        } else {
                            ConnectionStatus::Error(error)
                        };
                    }
                }
                AppEvent::MetricsReceived { device_id, metrics } => {
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.record_metrics(metrics, cap);
                    }
                }
                AppEvent::TerminalOutput { device_id, data } => {
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.terminal.push_bytes(&data);
                    }
                }
                AppEvent::TerminalClosed { device_id } => {
                    self.terminal_txs.remove(&device_id);
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        dev.terminal
                            .push_str("\r\n[Terminal session disconnected]\r\n");
                    }
                }
                AppEvent::Disconnected { device_id } => {
                    if let Some(dev) = self.devices.get_mut(&device_id) {
                        if dev.status.is_connected() {
                            dev.status = ConnectionStatus::Disconnected;
                        }
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
                            format!("Erreur lors de la récupération des processus: {error}"),
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
                                format!("Échec de l'arrêt du processus PID {pid}: {error}"),
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
                            format!("Erreur lors de la récupération des services: {error}"),
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
                                format!("Action '{action}' appliquée avec succès sur {service_name}."),
                                false,
                            );
                        } else {
                            dev.process_state.set_status(
                                format!("Échec de l'action '{action}' sur {service_name}: {error}"),
                                true,
                            );
                        }
                    }
                    self.fetch_services(&device_id);
                }
            }
        }
    }
}

impl eframe::App for NtxApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_channels();

        // Auto-refresh processes if enabled
        if let Some(ref id) = self.selected_device_id {
            if self.device_panel_state.active_tab == DeviceTab::Processes {
                if let Some(dev) = self.devices.get(id) {
                    if dev.status.is_connected()
                        && dev.process_state.auto_refresh
                        && !dev.process_state.is_loading_processes
                    {
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
                            let id_clone = id.clone();
                            self.fetch_processes(&id_clone);
                        }
                    }
                }
            }
        }

        // Repaint periodically for live charts and terminal
        ctx.request_repaint_after(Duration::from_millis(100));

        // 1. Left Sidebar: Connections & Servers list
        egui::SidePanel::left("server_sidebar")
            .resizable(true)
            .default_width(260.0)
            .min_width(200.0)
            .max_width(380.0)
            .show(ctx, |ui| {
                let devices_vec: Vec<Device> = {
                    let mut v: Vec<Device> = self.devices.values().cloned().collect();
                    // Sort: connected first, then alphabetical by hostname
                    v.sort_by(|a, b| {
                        let a_conn = a.status.is_connected();
                        let b_conn = b.status.is_connected();
                        b_conn.cmp(&a_conn).then_with(|| {
                            a.hostname.to_lowercase().cmp(&b.hostname.to_lowercase())
                        })
                    });
                    v
                };

                if let Some(action) = sidebar::render(
                    ui,
                    &mut self.sidebar_state,
                    &devices_vec,
                    self.selected_device_id.as_deref(),
                ) {
                    match action {
                        SidebarAction::SelectDevice(id) => {
                            self.selected_device_id = Some(id.clone());
                            self.active_view = ActiveView::Device;
                            if let Some(dev) = self.devices.get(&id) {
                                self.device_panel_state.psk_input =
                                    dev.psk.clone().unwrap_or_default();
                            }
                        }
                        SidebarAction::ConnectDevice(id) => {
                            self.initiate_connect(&id, None);
                        }
                        SidebarAction::DisconnectDevice(id) => {
                            self.disconnect_device(&id);
                        }
                        SidebarAction::RemoveDevice(id) => {
                            self.disconnect_device(&id);
                            self.devices.remove(&id);
                            self.settings.device_psks.remove(&id);
                            self.settings
                                .manual_devices
                                .retain(|m| format!("{}:{}", m.address, m.port) != id);
                            self.save_known_devices();
                            if self.selected_device_id.as_deref() == Some(&id) {
                                self.selected_device_id = self.devices.keys().next().cloned();
                            }
                        }
                        SidebarAction::AddManualDevice(manual) => {
                            self.settings.manual_devices.push(manual.clone());
                            let _ = self.settings.save();
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

        // 2. Central Panel: Device Management (with top tabs) or Settings
        egui::CentralPanel::default().show(ctx, |ui| match self.active_view {
            ActiveView::GlobalSettings => {
                if let Some(action) =
                    settings::render(ui, &mut self.settings, &mut self.settings_state)
                {
                    match action {
                        SettingsAction::BackToDashboard => {
                            self.active_view = ActiveView::Device;
                        }
                        SettingsAction::Saved => {}
                    }
                }
            }
            ActiveView::Device => {
                if let Some(id) = self.selected_device_id.clone() {
                    // Check if we need to auto-open terminal session before mutable borrow of devices
                    let should_open_term = self.device_panel_state.active_tab == DeviceTab::Terminal
                        && self
                            .devices
                            .get(&id)
                            .map(|d| d.status.is_connected())
                            .unwrap_or(false)
                        && !self.terminal_txs.contains_key(&id);

                    if should_open_term {
                        self.open_terminal_session(&id);
                    }

                    let action = if let Some(dev) = self.devices.get_mut(&id) {
                        device_panel::render(ui, &mut self.device_panel_state, dev)
                    } else {
                        self.selected_device_id = self.devices.keys().next().cloned();
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
                                self.disconnect_device(&dev_id);
                                self.devices.remove(&dev_id);
                                self.settings.device_psks.remove(&dev_id);
                                self.settings
                                    .manual_devices
                                    .retain(|m| format!("{}:{}", m.address, m.port) != dev_id);
                                self.save_known_devices();
                                self.selected_device_id = self.devices.keys().next().cloned();
                            }
                            DevicePanelAction::SavePsk { id: dev_id, psk } => {
                                if let Some(d) = self.devices.get_mut(&dev_id) {
                                    d.psk = Some(psk.clone());
                                }
                                self.settings.device_psks.insert(dev_id, psk);
                                self.save_known_devices();
                            }
                            DevicePanelAction::SwitchTab(tab) => {
                                if tab == DeviceTab::Processes {
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
                                if tab == DeviceTab::Terminal {
                                    // Terminal will be auto-opened on next frame
                                }
                            }
                            DevicePanelAction::ProcessAction(proc_action) => {
                                match proc_action {
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
                                }
                            }
                            DevicePanelAction::TerminalAction(term_action) => {
                                match term_action {
                                    TerminalAction::SendData(data) => {
                                        if let Some(tx) = self.terminal_txs.get(&id) {
                                            let _ = tx.try_send(TerminalInput {
                                                input: Some(Input::Data(data)),
                                            });
                                        }
                                    }
                                    TerminalAction::Clear => {}
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
                } else {
                    // Empty state when no device exists or none is selected
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
                            let total_w = 320.0;
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
