use crate::client::NtxClient;
use crate::discovery::{start_discovery, DiscoveredDevice, DiscoveryEvent, DiscoveryHandle};
use crate::state::{AppSettings, ConnectionStatus, Device, ManualDevice};
use crate::ui::dashboard::{self, DashboardAction, DashboardState};
use crate::ui::device_panel::{self, DevicePanelAction};
use crate::ui::settings::{self, SettingsAction, SettingsState};
use crate::ui::terminal::{self, TerminalAction};
use ntx_proto::terminal_input::Input;
use ntx_proto::{SystemInfo, SystemMetrics, TerminalInput, TerminalResize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    Dashboard,
    DeviceDetail(String),
    Terminal(String),
    Settings,
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
}

pub struct NtxApp {
    rt: Arc<tokio::runtime::Runtime>,
    current_view: View,
    devices: HashMap<String, Device>,
    settings: AppSettings,
    dashboard_state: DashboardState,
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
            current_view: View::Dashboard,
            devices: HashMap::new(),
            settings,
            dashboard_state: DashboardState::new(),
            settings_state: SettingsState::default(),
            discovery_handle,
            discovery_rx: disc_rx,
            event_tx,
            event_rx,
            terminal_txs: HashMap::new(),
            metrics_cancel_txs: HashMap::new(),
        };

        // Load any saved manual devices from settings
        for manual in &app.settings.manual_devices.clone() {
            app.add_manual_device(manual.clone());
        }

        app
    }

    fn add_manual_device(&mut self, manual: ManualDevice) {
        let id = format!("{}:{}", manual.address, manual.port);
        let mut dev = Device::new(
            id.clone(),
            manual.hostname,
            vec![manual.address],
            manual.port,
            "manual".to_string(),
        );
        dev.psk = manual.psk;
        self.devices.insert(id, dev);
    }

    fn restart_discovery(&mut self) {
        if let Some(handle) = self.discovery_handle.take() {
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
        self.metrics_cancel_txs.insert(dev_id.clone(), cancel_tx);

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

            // Authenticate
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

            // Get static system info
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

            // Stream metrics in a loop
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

        // Send initial resize
        let _ = input_tx.try_send(TerminalInput {
            input: Some(Input::Resize(TerminalResize { cols: 80, rows: 24 })),
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

            match client.open_terminal(input_rx).await {
                Ok(mut stream) => {
                    while let Ok(Some(output)) = stream.message().await {
                        if event_tx
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
                }
                Err(e) => {
                    let _ = event_tx
                        .send(AppEvent::TerminalOutput {
                            device_id: dev_id.clone(),
                            data: format!("Terminal stream error: {e}\r\n").into_bytes(),
                        })
                        .await;
                }
            }

            let _ = event_tx
                .send(AppEvent::TerminalClosed { device_id: dev_id })
                .await;
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
                    let id = fullname.clone();
                    if let Some(dev) = self.devices.get_mut(&id) {
                        dev.addresses = addresses;
                        dev.port = port;
                        dev.version = version;
                    } else {
                        let mut dev = Device::new(id.clone(), hostname, addresses, port, version);
                        if let Some(saved) = self.settings.device_psks.get(&id) {
                            dev.psk = Some(saved.clone());
                        }
                        self.devices.insert(id, dev);
                    }
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
            }
        }
    }
}

impl eframe::App for NtxApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_channels();

        // Repaint periodically for live charts and terminal
        ctx.request_repaint_after(Duration::from_millis(100));

        // Top Navigation Bar
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("⚡ NTX Panel");

                ui.separator();

                let mut to_dash = false;
                let mut to_settings = false;

                if ui
                    .selectable_label(matches!(self.current_view, View::Dashboard), "📊 Dashboard")
                    .clicked()
                {
                    to_dash = true;
                }

                if ui
                    .selectable_label(matches!(self.current_view, View::Settings), "⚙ Settings")
                    .clicked()
                {
                    to_settings = true;
                }

                if to_dash {
                    self.current_view = View::Dashboard;
                } else if to_settings {
                    self.current_view = View::Settings;
                }

                // Active view breadcrumbs
                match &self.current_view {
                    View::DeviceDetail(id) => {
                        ui.separator();
                        let name = self
                            .devices
                            .get(id)
                            .map(|d| d.hostname.as_str())
                            .unwrap_or(id.as_str());
                        ui.label(format!("Monitor: {}", name));
                    }
                    View::Terminal(id) => {
                        ui.separator();
                        let name = self
                            .devices
                            .get(id)
                            .map(|d| d.hostname.as_str())
                            .unwrap_or(id.as_str());
                        ui.label(format!("Terminal: {}", name));
                    }
                    _ => {}
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let connected_count = self
                        .devices
                        .values()
                        .filter(|d| matches!(d.status, ConnectionStatus::Connected))
                        .count();
                    ui.label(format!(
                        "Online: {}/{}",
                        connected_count,
                        self.devices.len()
                    ));
                });
            });
        });

        // Main Content Panel
        egui::CentralPanel::default().show(ctx, |ui| match &self.current_view {
            View::Dashboard => {
                let devices_vec: Vec<Device> = self.devices.values().cloned().collect();
                if let Some(action) = dashboard::render(ui, &mut self.dashboard_state, &devices_vec)
                {
                    match action {
                        DashboardAction::Connect { id, psk } => {
                            self.initiate_connect(&id, psk);
                        }
                        DashboardAction::Disconnect(id) => {
                            self.disconnect_device(&id);
                        }
                        DashboardAction::OpenMonitor(id) => {
                            self.current_view = View::DeviceDetail(id);
                        }
                        DashboardAction::OpenTerminal(id) => {
                            self.open_terminal_session(&id);
                            self.current_view = View::Terminal(id);
                        }
                        DashboardAction::AddManualDevice(manual) => {
                            self.settings.manual_devices.push(manual.clone());
                            let _ = self.settings.save();
                            self.add_manual_device(manual);
                        }
                        DashboardAction::RemoveDevice(id) => {
                            self.disconnect_device(&id);
                            self.devices.remove(&id);
                            self.settings
                                .manual_devices
                                .retain(|m| format!("{}:{}", m.address, m.port) != id);
                            let _ = self.settings.save();
                        }
                        DashboardAction::RefreshDiscovery => {
                            self.restart_discovery();
                        }
                    }
                }
            }
            View::DeviceDetail(id) => {
                let dev_opt = self.devices.get(id).cloned();
                if let Some(dev) = dev_opt {
                    if let Some(action) = device_panel::render(ui, &dev) {
                        match action {
                            DevicePanelAction::BackToDashboard => {
                                self.current_view = View::Dashboard;
                            }
                            DevicePanelAction::OpenTerminal(dev_id) => {
                                self.open_terminal_session(&dev_id);
                                self.current_view = View::Terminal(dev_id);
                            }
                            DevicePanelAction::Disconnect(dev_id) => {
                                self.disconnect_device(&dev_id);
                            }
                            DevicePanelAction::Reconnect(dev_id) => {
                                self.initiate_connect(&dev_id, None);
                            }
                        }
                    }
                } else {
                    ui.label("Device not found.");
                    if ui.button("Back to Dashboard").clicked() {
                        self.current_view = View::Dashboard;
                    }
                }
            }
            View::Terminal(id) => {
                let id_clone = id.clone();
                if let Some(dev) = self.devices.get_mut(&id_clone) {
                    if let Some(action) = terminal::render(ui, dev) {
                        match action {
                            TerminalAction::BackToDashboard => {
                                self.current_view = View::Dashboard;
                            }
                            TerminalAction::SendData(data) => {
                                if let Some(tx) = self.terminal_txs.get(&id_clone) {
                                    let _ = tx.try_send(TerminalInput {
                                        input: Some(Input::Data(data)),
                                    });
                                }
                            }
                            TerminalAction::Clear => {}
                        }
                    }
                } else {
                    ui.label("Device not found.");
                    if ui.button("Back to Dashboard").clicked() {
                        self.current_view = View::Dashboard;
                    }
                }
            }
            View::Settings => {
                if let Some(action) =
                    settings::render(ui, &mut self.settings, &mut self.settings_state)
                {
                    match action {
                        SettingsAction::BackToDashboard => {
                            self.current_view = View::Dashboard;
                        }
                        SettingsAction::Saved => {}
                    }
                }
            }
        });
    }
}
