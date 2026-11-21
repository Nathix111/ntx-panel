use crate::state::{
    is_virtual_disk, is_virtual_interface, ConnectionStatus, Device, OverviewViewMode,
    SortDirection,
};
use crate::ui::theme::{self, Palette};
use egui::{Color32, RichText, Stroke, Ui};
use egui_extras::{Column, TableBuilder};
use std::cmp::Ordering;
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

const FLEET_SPARKLINE_POINTS: usize = 30;
const CARD_SPARKLINE_POINTS: usize = 24;

pub enum OverviewAction {
    SelectDevice(String),
    ConnectDevice(String),
    DisconnectDevice(String),
    ReconnectDevice(String),
    PingDevice(String),
    PingAll,
    OpenTerminal(String),
    OpenProcesses(String),
    OpenFavorites(String),
    ConfigureDevice(String),
    RemoveDevice(String),
    ConnectAll,
    DisconnectAll,
    OpenAddModal,
    RefreshDiscovery,
    SetViewMode(OverviewViewMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverviewStatusFilter {
    #[default]
    All,
    Online,
    Offline,
    Alerts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverviewSortBy {
    #[default]
    StatusThenName,
    Name,
    Cpu,
    Ram,
    Disk,
    Latency,
    Uptime,
}

impl OverviewSortBy {
    pub fn label(self) -> &'static str {
        match self {
            OverviewSortBy::StatusThenName => "Statut & nom",
            OverviewSortBy::Name => "Nom d'hôte",
            OverviewSortBy::Cpu => "Utilisation CPU",
            OverviewSortBy::Ram => "Mémoire RAM",
            OverviewSortBy::Disk => "Occupation disque",
            OverviewSortBy::Latency => "Latence Ping",
            OverviewSortBy::Uptime => "Uptime",
        }
    }
}

pub struct OverviewState {
    pub search_query: String,
    pub status_filter: OverviewStatusFilter,
    pub sort_by: OverviewSortBy,
    pub sort_dir: SortDirection,
    pub copied_device_id: Option<(String, Instant)>,
    pub pending_delete: Option<(String, String)>,
}

impl Default for OverviewState {
    fn default() -> Self {
        Self {
            search_query: String::new(),
            status_filter: OverviewStatusFilter::All,
            sort_by: OverviewSortBy::StatusThenName,
            sort_dir: SortDirection::Ascending,
            copied_device_id: None,
            pending_delete: None,
        }
    }
}

impl OverviewState {
    pub fn toggle_sort(&mut self, col: OverviewSortBy) {
        if self.sort_by == col {
            self.sort_dir = match self.sort_dir {
                SortDirection::Ascending => SortDirection::Descending,
                SortDirection::Descending => SortDirection::Ascending,
            };
        } else {
            self.sort_by = col;
            self.sort_dir = match col {
                OverviewSortBy::Cpu
                | OverviewSortBy::Ram
                | OverviewSortBy::Disk
                | OverviewSortBy::Latency
                | OverviewSortBy::Uptime => SortDirection::Descending,
                OverviewSortBy::StatusThenName | OverviewSortBy::Name => SortDirection::Ascending,
            };
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiskMountSummary {
    pub mount_point: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub pct: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiskUsageSummary {
    pub primary: DiskMountSummary,
    pub visible_disks: Vec<DiskMountSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertSeverity {
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAlert {
    pub severity: AlertSeverity,
    pub message: String,
}

pub struct DeviceSnapshot<'a> {
    pub device: &'a Device,
    pub cpu_pct: Option<f64>,
    pub cpu_cores: u32,
    pub ram_pct: Option<f64>,
    pub ram_used_bytes: u64,
    pub ram_total_bytes: u64,
    pub disk: Option<DiskUsageSummary>,
    pub rx_bps: f64,
    pub tx_bps: f64,
    pub alerts: Vec<DeviceAlert>,
}

pub struct FleetSummary {
    pub total: usize,
    pub online: usize,
    pub connecting: usize,
    pub offline: usize,
    pub discovered: usize,
    pub alert_devices: usize,
    pub critical_alerts: usize,
    pub total_cores: u32,
    pub avg_cpu: Option<f64>,
    pub max_cpu: Option<(String, f64)>,
    pub cpu_sparkline: Vec<f64>,
    pub fleet_ram_pct: Option<f64>,
    pub ram_used_bytes: u64,
    pub ram_total_bytes: u64,
    pub ram_sparkline: Vec<f64>,
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
    pub disk_pct: Option<f64>,
    pub total_rx_bps: f64,
    pub total_tx_bps: f64,
}

pub fn device_network_rates(dev: &Device, hide_virtual_interfaces: bool) -> (f64, f64) {
    if let Some(m) = &dev.latest_metrics {
        let mut rx = 0.0;
        let mut tx = 0.0;
        for iface in &m.network_metrics {
            if hide_virtual_interfaces && is_virtual_interface(&iface.interface_name) {
                continue;
            }
            rx += iface.rx_bytes_per_sec as f64;
            tx += iface.tx_bytes_per_sec as f64;
        }
        (rx, tx)
    } else {
        (
            dev.rx_history.back().copied().unwrap_or(0.0),
            dev.tx_history.back().copied().unwrap_or(0.0),
        )
    }
}

pub fn primary_disk_usage(dev: &Device, hide_virtual_disks: bool) -> Option<DiskUsageSummary> {
    let mut visible_disks: Vec<DiskMountSummary> = Vec::new();

    if let Some(m) = &dev.latest_metrics {
        if !m.disk_metrics.is_empty() {
            for dm in &m.disk_metrics {
                if dm.total_bytes == 0 {
                    continue;
                }
                let fs_name = dev
                    .system_info
                    .as_ref()
                    .and_then(|info| {
                        info.disks
                            .iter()
                            .find(|d| d.mount_point == dm.mount_point)
                            .map(|d| d.filesystem.as_str())
                    })
                    .unwrap_or("");
                if hide_virtual_disks && is_virtual_disk(&dm.mount_point, fs_name) {
                    continue;
                }
                let pct = (dm.used_bytes as f64 / dm.total_bytes as f64) * 100.0;
                visible_disks.push(DiskMountSummary {
                    mount_point: dm.mount_point.clone(),
                    used_bytes: dm.used_bytes,
                    total_bytes: dm.total_bytes,
                    pct,
                });
            }
        }
    }

    if visible_disks.is_empty() {
        if let Some(info) = &dev.system_info {
            for d in &info.disks {
                if d.total_bytes == 0 {
                    continue;
                }
                if hide_virtual_disks && is_virtual_disk(&d.mount_point, &d.filesystem) {
                    continue;
                }
                let used_bytes = d.total_bytes.saturating_sub(d.available_bytes);
                let pct = (used_bytes as f64 / d.total_bytes as f64) * 100.0;
                visible_disks.push(DiskMountSummary {
                    mount_point: d.mount_point.clone(),
                    used_bytes,
                    total_bytes: d.total_bytes,
                    pct,
                });
            }
        }
    }

    if visible_disks.is_empty() {
        return None;
    }

    let primary = visible_disks
        .iter()
        .filter(|d| d.pct >= 85.0)
        .max_by(|a, b| a.pct.partial_cmp(&b.pct).unwrap_or(Ordering::Equal))
        .or_else(|| {
            visible_disks.iter().find(|d| {
                let mp = d.mount_point.trim();
                mp == "/" || mp.eq_ignore_ascii_case("C:\\") || mp.eq_ignore_ascii_case("C:/")
            })
        })
        .or_else(|| visible_disks.iter().max_by_key(|d| d.total_bytes))?
        .clone();

    Some(DiskUsageSummary {
        primary,
        visible_disks,
    })
}

pub fn device_alerts(
    dev: &Device,
    cpu: Option<f64>,
    ram: Option<f64>,
    disk: Option<&DiskUsageSummary>,
) -> Vec<DeviceAlert> {
    let mut alerts = Vec::new();
    match &dev.status {
        ConnectionStatus::TlsMismatch { .. } => alerts.push(DeviceAlert {
            severity: AlertSeverity::Critical,
            message: "Certificat TLS modifié".to_string(),
        }),
        ConnectionStatus::AuthFailed(_) => alerts.push(DeviceAlert {
            severity: AlertSeverity::Critical,
            message: "Authentification PSK refusée".to_string(),
        }),
        ConnectionStatus::Error(_) => alerts.push(DeviceAlert {
            severity: AlertSeverity::Warning,
            message: "Erreur de connexion".to_string(),
        }),
        ConnectionStatus::Connected => {
            if let Some(c) = cpu {
                if c >= 85.0 {
                    alerts.push(DeviceAlert {
                        severity: if c >= 92.0 {
                            AlertSeverity::Critical
                        } else {
                            AlertSeverity::Warning
                        },
                        message: format!("CPU élevé ({c:.0}%)"),
                    });
                }
            }
            if let Some(r) = ram {
                if r >= 85.0 {
                    alerts.push(DeviceAlert {
                        severity: if r >= 92.0 {
                            AlertSeverity::Critical
                        } else {
                            AlertSeverity::Warning
                        },
                        message: format!("RAM élevée ({r:.0}%)"),
                    });
                }
            }
            if let Some(d) = disk {
                if d.primary.pct >= 85.0 {
                    alerts.push(DeviceAlert {
                        severity: if d.primary.pct >= 90.0 {
                            AlertSeverity::Critical
                        } else {
                            AlertSeverity::Warning
                        },
                        message: format!(
                            "Disque {} plein ({:.0}%)",
                            d.primary.mount_point, d.primary.pct
                        ),
                    });
                }
            }
        }
        ConnectionStatus::Connecting
        | ConnectionStatus::Discovered
        | ConnectionStatus::Disconnected => {}
    }
    alerts
}

pub fn compute_fleet_sparkline(
    online: &[&Device],
    select: fn(&Device) -> &VecDeque<f64>,
    max_points: usize,
) -> Vec<f64> {
    if max_points == 0 {
        return Vec::new();
    }
    let series: Vec<&VecDeque<f64>> = online
        .iter()
        .map(|d| select(d))
        .filter(|q| !q.is_empty())
        .collect();
    if series.is_empty() {
        return Vec::new();
    }
    let len = series
        .iter()
        .map(|q| q.len().min(max_points))
        .max()
        .unwrap_or(0);
    let mut out = Vec::with_capacity(len);
    for idx in 0..len {
        let dist_from_end = len - 1 - idx;
        let mut sum = 0.0;
        let mut count = 0usize;
        for q in &series {
            if q.len() > dist_from_end {
                let val = q[q.len() - 1 - dist_from_end];
                if val.is_finite() {
                    sum += val;
                    count += 1;
                }
            }
        }
        if count > 0 {
            out.push(sum / count as f64);
        }
    }
    out
}

pub fn build_snapshot<'a>(
    dev: &'a Device,
    hide_virtual_disks: bool,
    hide_virtual_interfaces: bool,
) -> DeviceSnapshot<'a> {
    let cpu_cores = dev.system_info.as_ref().map(|s| s.cpu_count).unwrap_or(0);

    if dev.status.is_connected() {
        let cpu_pct = dev
            .latest_metrics
            .as_ref()
            .map(|m| m.cpu_usage_percent)
            .or_else(|| dev.cpu_history.back().copied());

        let (ram_pct, ram_used_bytes, ram_total_bytes) = if let Some(m) = &dev.latest_metrics {
            if m.total_memory_bytes > 0 {
                let pct = (m.used_memory_bytes as f64 / m.total_memory_bytes as f64) * 100.0;
                (Some(pct), m.used_memory_bytes, m.total_memory_bytes)
            } else {
                (
                    dev.ram_history.back().copied(),
                    0,
                    dev.system_info
                        .as_ref()
                        .map(|s| s.total_memory_bytes)
                        .unwrap_or(0),
                )
            }
        } else {
            (
                dev.ram_history.back().copied(),
                0,
                dev.system_info
                    .as_ref()
                    .map(|s| s.total_memory_bytes)
                    .unwrap_or(0),
            )
        };

        let disk = primary_disk_usage(dev, hide_virtual_disks);
        let (rx_bps, tx_bps) = device_network_rates(dev, hide_virtual_interfaces);
        let alerts = device_alerts(dev, cpu_pct, ram_pct, disk.as_ref());

        DeviceSnapshot {
            device: dev,
            cpu_pct,
            cpu_cores,
            ram_pct,
            ram_used_bytes,
            ram_total_bytes,
            disk,
            rx_bps,
            tx_bps,
            alerts,
        }
    } else {
        let alerts = device_alerts(dev, None, None, None);
        DeviceSnapshot {
            device: dev,
            cpu_pct: None,
            cpu_cores,
            ram_pct: None,
            ram_used_bytes: 0,
            ram_total_bytes: dev
                .system_info
                .as_ref()
                .map(|s| s.total_memory_bytes)
                .unwrap_or(0),
            disk: None,
            rx_bps: 0.0,
            tx_bps: 0.0,
            alerts,
        }
    }
}

pub fn summarize_fleet(snapshots: &[DeviceSnapshot<'_>]) -> FleetSummary {
    let total = snapshots.len();
    let mut online = 0usize;
    let mut connecting = 0usize;
    let mut offline = 0usize;
    let mut discovered = 0usize;
    let mut alert_devices = 0usize;
    let mut critical_alerts = 0usize;
    let mut total_cores = 0u32;

    let mut online_devs: Vec<&Device> = Vec::new();
    let mut cpu_sum = 0.0;
    let mut cpu_n = 0usize;
    let mut max_cpu: Option<(String, f64)> = None;

    let mut ram_used_bytes = 0u64;
    let mut ram_total_bytes = 0u64;
    let mut ram_pct_sum = 0.0;
    let mut ram_pct_n = 0usize;

    let mut disk_used_bytes = 0u64;
    let mut disk_total_bytes = 0u64;

    let mut total_rx_bps = 0.0;
    let mut total_tx_bps = 0.0;

    for snap in snapshots {
        if !snap.alerts.is_empty() {
            alert_devices += 1;
            if snap
                .alerts
                .iter()
                .any(|a| a.severity == AlertSeverity::Critical)
            {
                critical_alerts += 1;
            }
        }

        if snap.device.status.is_connected() {
            online += 1;
            online_devs.push(snap.device);
            total_cores = total_cores.saturating_add(snap.cpu_cores);

            if let Some(cpu) = snap.cpu_pct {
                cpu_sum += cpu;
                cpu_n += 1;
                if max_cpu
                    .as_ref()
                    .is_none_or(|(_, best)| cpu > *best)
                {
                    max_cpu = Some((snap.device.hostname.clone(), cpu));
                }
            }

            if snap.ram_total_bytes > 0 {
                ram_used_bytes = ram_used_bytes.saturating_add(snap.ram_used_bytes);
                ram_total_bytes = ram_total_bytes.saturating_add(snap.ram_total_bytes);
            }
            if let Some(rp) = snap.ram_pct {
                ram_pct_sum += rp;
                ram_pct_n += 1;
            }

            if let Some(disk) = &snap.disk {
                let mut seen_mounts: HashSet<&str> = HashSet::new();
                for mount in &disk.visible_disks {
                    if seen_mounts.insert(mount.mount_point.as_str()) {
                        disk_used_bytes = disk_used_bytes.saturating_add(mount.used_bytes);
                        disk_total_bytes = disk_total_bytes.saturating_add(mount.total_bytes);
                    }
                }
            }

            total_rx_bps += snap.rx_bps;
            total_tx_bps += snap.tx_bps;
        } else {
            offline += 1;
            match snap.device.status {
                ConnectionStatus::Connecting => connecting += 1,
                ConnectionStatus::Discovered => discovered += 1,
                _ => {}
            }
        }
    }

    let avg_cpu = (cpu_n > 0).then(|| cpu_sum / cpu_n as f64);
    let fleet_ram_pct = if ram_total_bytes > 0 {
        Some((ram_used_bytes as f64 / ram_total_bytes as f64) * 100.0)
    } else if ram_pct_n > 0 {
        Some(ram_pct_sum / ram_pct_n as f64)
    } else {
        None
    };
    let disk_pct = (disk_total_bytes > 0)
        .then(|| (disk_used_bytes as f64 / disk_total_bytes as f64) * 100.0);

    let cpu_sparkline =
        compute_fleet_sparkline(&online_devs, |d| &d.cpu_history, FLEET_SPARKLINE_POINTS);
    let ram_sparkline =
        compute_fleet_sparkline(&online_devs, |d| &d.ram_history, FLEET_SPARKLINE_POINTS);

    FleetSummary {
        total,
        online,
        connecting,
        offline,
        discovered,
        alert_devices,
        critical_alerts,
        total_cores,
        avg_cpu,
        max_cpu,
        cpu_sparkline,
        fleet_ram_pct,
        ram_used_bytes,
        ram_total_bytes,
        ram_sparkline,
        disk_used_bytes,
        disk_total_bytes,
        disk_pct,
        total_rx_bps,
        total_tx_bps,
    }
}

fn status_sort_rank(status: &ConnectionStatus) -> u8 {
    match status {
        ConnectionStatus::Connected => 0,
        ConnectionStatus::TlsMismatch { .. }
        | ConnectionStatus::AuthFailed(_)
        | ConnectionStatus::Error(_)
        | ConnectionStatus::Connecting => 1,
        ConnectionStatus::Discovered => 2,
        ConnectionStatus::Disconnected => 3,
    }
}

fn cmp_opt_metric<T: PartialOrd>(
    va: Option<T>,
    vb: Option<T>,
    a_name: &str,
    b_name: &str,
    dir: SortDirection,
) -> Ordering {
    match (va, vb) {
        (Some(x), Some(y)) => {
            let c = x.partial_cmp(&y).unwrap_or(Ordering::Equal);
            let directed = match dir {
                SortDirection::Ascending => c,
                SortDirection::Descending => c.reverse(),
            };
            directed.then_with(|| a_name.cmp(b_name))
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a_name.cmp(b_name),
    }
}

pub fn filter_and_sort_snapshots<'a>(
    snapshots: &'a [DeviceSnapshot<'a>],
    state: &OverviewState,
) -> Vec<&'a DeviceSnapshot<'a>> {
    let q = state.search_query.trim().to_lowercase();

    let mut filtered: Vec<&'a DeviceSnapshot<'a>> = snapshots
        .iter()
        .filter(|snap| {
            let status_ok = match state.status_filter {
                OverviewStatusFilter::All => true,
                OverviewStatusFilter::Online => snap.device.status.is_connected(),
                OverviewStatusFilter::Offline => !snap.device.status.is_connected(),
                OverviewStatusFilter::Alerts => !snap.alerts.is_empty(),
            };
            if !status_ok {
                return false;
            }
            if q.is_empty() {
                return true;
            }
            let dev = snap.device;
            dev.hostname.to_lowercase().contains(&q)
                || dev.primary_address().to_lowercase().contains(&q)
                || dev
                    .addresses
                    .iter()
                    .any(|a| a.to_lowercase().contains(&q))
                || dev.version.to_lowercase().contains(&q)
                || dev.system_info.as_ref().is_some_and(|info| {
                    info.os_name.to_lowercase().contains(&q)
                        || info.arch.to_lowercase().contains(&q)
                })
                || dev
                    .favorites
                    .iter()
                    .any(|f| f.name.to_lowercase().contains(&q))
                || dev.managed_services.services.iter().any(|s| {
                    s.config
                        .as_ref()
                        .is_some_and(|c| c.name.to_lowercase().contains(&q))
                })
                || dev
                    .docker_state
                    .containers
                    .iter()
                    .any(|c| c.name.to_lowercase().contains(&q))
        })
        .collect();

    filtered.sort_by(|a, b| {
        let a_name = a.device.hostname.to_lowercase();
        let b_name = b.device.hostname.to_lowercase();

        match state.sort_by {
            OverviewSortBy::StatusThenName => {
                let cmp = status_sort_rank(&a.device.status)
                    .cmp(&status_sort_rank(&b.device.status))
                    .then_with(|| a_name.cmp(&b_name));
                match state.sort_dir {
                    SortDirection::Ascending => cmp,
                    SortDirection::Descending => cmp.reverse(),
                }
            }
            OverviewSortBy::Name => {
                let cmp = a_name.cmp(&b_name);
                match state.sort_dir {
                    SortDirection::Ascending => cmp,
                    SortDirection::Descending => cmp.reverse(),
                }
            }
            OverviewSortBy::Cpu => {
                cmp_opt_metric(a.cpu_pct, b.cpu_pct, &a_name, &b_name, state.sort_dir)
            }
            OverviewSortBy::Ram => {
                cmp_opt_metric(a.ram_pct, b.ram_pct, &a_name, &b_name, state.sort_dir)
            }
            OverviewSortBy::Disk => cmp_opt_metric(
                a.disk.as_ref().map(|d| d.primary.pct),
                b.disk.as_ref().map(|d| d.primary.pct),
                &a_name,
                &b_name,
                state.sort_dir,
            ),
            OverviewSortBy::Latency => cmp_opt_metric(
                a.device.last_ping_ms,
                b.device.last_ping_ms,
                &a_name,
                &b_name,
                state.sort_dir,
            ),
            OverviewSortBy::Uptime => cmp_opt_metric(
                a.device.server_uptime_secs,
                b.device.server_uptime_secs,
                &a_name,
                &b_name,
                state.sort_dir,
            ),
        }
    });

    filtered
}

pub fn render(
    ui: &mut Ui,
    state: &mut OverviewState,
    devices: &[&Device],
    view_mode: OverviewViewMode,
    hide_virtual_disks: bool,
    hide_virtual_interfaces: bool,
) -> Option<OverviewAction> {
    let p = Palette::for_ui(ui);
    let mut action: Option<OverviewAction> = None;

    if let Some((_, until)) = &state.copied_device_id {
        if Instant::now() < *until {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        } else {
            state.copied_device_id = None;
        }
    }

    let snapshots: Vec<DeviceSnapshot<'_>> = devices
        .iter()
        .map(|d| build_snapshot(d, hide_virtual_disks, hide_virtual_interfaces))
        .collect();
    let fleet = summarize_fleet(&snapshots);

    // 1. Header bar
    render_header(ui, &p, devices, &fleet, &mut action);
    ui.add_space(10.0);

    // 2. Responsive Enriched KPI Strip
    render_kpi_strip(
        ui,
        &p,
        state,
        &fleet,
        hide_virtual_disks || hide_virtual_interfaces,
    );

    // 3. Conditional Alert Strip
    if fleet.alert_devices > 0 && state.status_filter != OverviewStatusFilter::Alerts {
        ui.add_space(8.0);
        render_alert_strip(ui, &p, state, &snapshots, &fleet);
    }

    ui.add_space(12.0);

    // 4. Empty Fleet State
    if devices.is_empty() {
        render_empty_fleet(ui, &p, &mut action);
        return action;
    }

    // 5. Fleet Control Toolbar
    render_control_toolbar(ui, &p, state, &fleet, view_mode, &mut action);
    ui.add_space(10.0);

    // 6. Filtered view
    let filtered = filter_and_sort_snapshots(&snapshots, state);
    if filtered.is_empty() {
        render_empty_filtered(ui, &p, state);
    } else {
        match view_mode {
            OverviewViewMode::Cards => {
                render_cards_grid(ui, &p, state, &filtered, &mut action);
            }
            OverviewViewMode::Table => {
                render_compact_table(ui, &p, state, &filtered, &mut action);
            }
        }
    }

    // 8. Delete confirmation modal
    render_delete_modal(ui, &p, state, &mut action);

    action
}

fn render_header(
    ui: &mut Ui,
    p: &Palette,
    devices: &[&Device],
    fleet: &FleetSummary,
    action: &mut Option<OverviewAction>,
) {
    let any_connected = fleet.online > 0;
    let any_disconnected = devices
        .iter()
        .any(|d| !d.status.is_connected() && !matches!(d.status, ConnectionStatus::Connecting));
    let avail_w = ui.available_width();

    let subtitle = if fleet.total == 0 {
        "Supervision centralisée, latence TLS et accès rapide à vos serveurs locaux.".to_string()
    } else {
        let mut parts = vec![format!("{}/{} en ligne", fleet.online, fleet.total)];
        if fleet.total_cores > 0 {
            parts.push(format!("{} cœurs actifs", fleet.total_cores));
        }
        if fleet.alert_devices > 0 {
            parts.push(format!("{} alerte(s)", fleet.alert_devices));
        } else {
            parts.push("Découverte mDNS & TLS".to_string());
        }
        parts.join("  •  ")
    };

    let render_title = |ui: &mut Ui| {
        ui.vertical(|ui| {
            ui.label(
                RichText::new("Vue d'ensemble du parc")
                    .size(20.0)
                    .strong()
                    .color(p.text_primary),
            );
            ui.label(RichText::new(&subtitle).size(12.5).color(p.text_secondary));
        });
    };

    let render_header_buttons = |ui: &mut Ui, rtl: bool, action: &mut Option<OverviewAction>| {
        let draw_add = |ui: &mut Ui, action: &mut Option<OverviewAction>| {
            if theme::primary_button(ui, p, "➕ Ajouter").clicked() {
                *action = Some(OverviewAction::OpenAddModal);
            }
        };
        let draw_scan = |ui: &mut Ui, action: &mut Option<OverviewAction>| {
            if ui
                .button("🔄 Scanner mDNS")
                .on_hover_text("Relancer la recherche automatique sur le réseau local")
                .clicked()
            {
                *action = Some(OverviewAction::RefreshDiscovery);
            }
        };
        let draw_ping_all = |ui: &mut Ui, action: &mut Option<OverviewAction>| {
            if any_connected
                && ui
                    .button("📡 Ping tous")
                    .on_hover_text("Mesurer la latence de tous les serveurs connectés")
                    .clicked()
            {
                *action = Some(OverviewAction::PingAll);
            }
        };
        let draw_disconn_all = |ui: &mut Ui, action: &mut Option<OverviewAction>| {
            if any_connected
                && ui
                    .button("🔌 Tout déconnecter")
                    .on_hover_text("Fermer toutes les sessions actives")
                    .clicked()
            {
                *action = Some(OverviewAction::DisconnectAll);
            }
        };
        let draw_conn_all = |ui: &mut Ui, action: &mut Option<OverviewAction>| {
            if any_disconnected
                && ui
                    .button("⚡ Tout connecter")
                    .on_hover_text("Connecter tous les serveurs ayant une clé PSK configurée")
                    .clicked()
            {
                *action = Some(OverviewAction::ConnectAll);
            }
        };

        if rtl {
            draw_add(ui, action);
            draw_scan(ui, action);
            draw_ping_all(ui, action);
            draw_disconn_all(ui, action);
            draw_conn_all(ui, action);
        } else {
            draw_conn_all(ui, action);
            draw_disconn_all(ui, action);
            draw_ping_all(ui, action);
            draw_scan(ui, action);
            draw_add(ui, action);
        }
    };

    if avail_w >= 880.0 {
        ui.horizontal(|ui| {
            render_title(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                render_header_buttons(ui, true, action);
            });
        });
    } else {
        render_title(ui);
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            render_header_buttons(ui, false, action);
        });
    }
}

fn render_kpi_strip(
    ui: &mut Ui,
    p: &Palette,
    state: &mut OverviewState,
    fleet: &FleetSummary,
    virtual_filtered: bool,
) {
    let avail_w = ui.available_width();

    if avail_w >= 880.0 {
        ui.columns(4, |cols| {
            render_kpi_fleet(&mut cols[0], p, state, fleet);
            render_kpi_cpu(&mut cols[1], p, fleet);
            render_kpi_ram(&mut cols[2], p, fleet);
            render_kpi_net_storage(&mut cols[3], p, fleet, virtual_filtered);
        });
    } else if avail_w >= 480.0 {
        ui.columns(2, |cols| {
            render_kpi_fleet(&mut cols[0], p, state, fleet);
            render_kpi_cpu(&mut cols[1], p, fleet);
        });
        ui.add_space(8.0);
        ui.columns(2, |cols| {
            render_kpi_ram(&mut cols[0], p, fleet);
            render_kpi_net_storage(&mut cols[1], p, fleet, virtual_filtered);
        });
    } else {
        render_kpi_fleet(ui, p, state, fleet);
        ui.add_space(8.0);
        render_kpi_cpu(ui, p, fleet);
        ui.add_space(8.0);
        render_kpi_ram(ui, p, fleet);
        ui.add_space(8.0);
        render_kpi_net_storage(ui, p, fleet, virtual_filtered);
    }
}

fn render_kpi_fleet(ui: &mut Ui, p: &Palette, state: &mut OverviewState, fleet: &FleetSummary) {
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(92.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("ÉTAT DU PARC")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if fleet.alert_devices > 0 {
                    let (bg, fg, border) = if fleet.critical_alerts > 0 {
                        (p.danger_bg, p.danger, p.danger_border)
                    } else {
                        (p.warning_bg, p.warning, p.warning_border)
                    };
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("⚠ {}", fleet.alert_devices))
                                    .size(10.5)
                                    .strong()
                                    .color(fg),
                            )
                            .fill(bg)
                            .stroke(Stroke::new(1.0_f32, border))
                            .corner_radius(10),
                        )
                        .on_hover_text("Filtrer sur les serveurs en alerte")
                        .clicked()
                    {
                        state.status_filter = OverviewStatusFilter::Alerts;
                    }
                } else if fleet.online > 0 {
                    theme::pill_badge(
                        ui,
                        "✔ Nominal",
                        p.success_bg,
                        p.success,
                        Some(p.success_border),
                    );
                } else if fleet.total > 0 {
                    theme::pill_badge(ui, "○ En veille", p.bg_subtle, p.text_muted, Some(p.border));
                }
            });
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("{} / {}", fleet.online, fleet.total))
                .size(21.0)
                .strong()
                .color(if fleet.online > 0 {
                    p.success
                } else {
                    p.text_secondary
                }),
        );

        ui.add_space(4.0);
        let ratio = if fleet.total > 0 {
            fleet.online as f32 / fleet.total as f32
        } else {
            0.0
        };
        theme::mini_progress_bar(ui, p, ratio, p.success, ui.available_width(), 5.0);
        ui.add_space(4.0);

        let caption = if fleet.total == 0 {
            "Aucun serveur enregistré".to_string()
        } else if fleet.online == fleet.total {
            "Tous les serveurs sont en ligne".to_string()
        } else {
            let mut parts = Vec::new();
            if fleet.connecting > 0 {
                parts.push(format!("{} en cours", fleet.connecting));
            }
            if fleet.discovered > 0 {
                parts.push(format!("{} découvert(s)", fleet.discovered));
            }
            let idle = fleet
                .offline
                .saturating_sub(fleet.connecting)
                .saturating_sub(fleet.discovered);
            if idle > 0 {
                parts.push(format!("{idle} hors ligne"));
            }
            parts.join(" • ")
        };
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_kpi_cpu(ui: &mut Ui, p: &Palette, fleet: &FleetSummary) {
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(92.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("CPU MOYEN")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            if fleet.total_cores > 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{} cœurs", fleet.total_cores))
                            .size(10.5)
                            .color(p.text_muted),
                    );
                });
            }
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let (label, col) = match fleet.avg_cpu {
                Some(avg) => (format!("{avg:.1}%"), theme::usage_color(p, avg)),
                None => ("—".to_string(), p.text_muted),
            };
            ui.label(RichText::new(label).size(21.0).strong().color(col));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::sparkline(
                    ui,
                    p,
                    &fleet.cpu_sparkline,
                    0.0,
                    100.0,
                    p.accent,
                    72.0,
                    22.0,
                );
            });
        });

        ui.add_space(4.0);
        let (frac, bar_col) = match fleet.avg_cpu {
            Some(avg) => ((avg / 100.0) as f32, theme::usage_color(p, avg)),
            None => (0.0, p.accent),
        };
        theme::mini_progress_bar(ui, p, frac, bar_col, ui.available_width(), 5.0);
        ui.add_space(4.0);

        let caption = match &fleet.max_cpu {
            Some((host, peak)) => format!("Pic : {peak:.0}% ({host})"),
            None => "En attente de télémétrie".to_string(),
        };
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_kpi_ram(ui: &mut Ui, p: &Palette, fleet: &FleetSummary) {
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(92.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("MÉMOIRE RAM")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            if fleet.ram_total_bytes > 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} / {}",
                            theme::format_storage(fleet.ram_used_bytes),
                            theme::format_storage(fleet.ram_total_bytes)
                        ))
                        .size(10.5)
                        .color(p.text_muted),
                    );
                });
            }
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let (label, col) = match fleet.fleet_ram_pct {
                Some(pct) => (format!("{pct:.1}%"), theme::usage_color(p, pct)),
                None => ("—".to_string(), p.text_muted),
            };
            ui.label(RichText::new(label).size(21.0).strong().color(col));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::sparkline(
                    ui,
                    p,
                    &fleet.ram_sparkline,
                    0.0,
                    100.0,
                    p.purple,
                    72.0,
                    22.0,
                );
            });
        });

        ui.add_space(4.0);
        let frac = fleet
            .fleet_ram_pct
            .map(|pct| (pct / 100.0) as f32)
            .unwrap_or(0.0);
        theme::mini_progress_bar(ui, p, frac, p.purple, ui.available_width(), 5.0);
        ui.add_space(4.0);

        let caption = if fleet.ram_total_bytes > 0 {
            format!(
                "{} utilisés sur {}",
                theme::format_storage(fleet.ram_used_bytes),
                theme::format_storage(fleet.ram_total_bytes)
            )
        } else {
            "En attente de télémétrie".to_string()
        };
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_kpi_net_storage(
    ui: &mut Ui,
    p: &Palette,
    fleet: &FleetSummary,
    virtual_filtered: bool,
) {
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(92.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("RÉSEAU & STOCKAGE")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            if virtual_filtered {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("hors virt.").size(10.0).color(p.text_muted));
                });
            }
        });

        ui.add_space(4.0);
        if fleet.online == 0 {
            ui.label(RichText::new("—").size(21.0).strong().color(p.text_muted));
        } else {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("↓ {}", theme::format_bytes_rate(fleet.total_rx_bps)))
                        .size(13.5)
                        .strong()
                        .color(p.success),
                );
                ui.label(RichText::new("•").size(12.0).color(p.text_muted));
                ui.label(
                    RichText::new(format!("↑ {}", theme::format_bytes_rate(fleet.total_tx_bps)))
                        .size(13.5)
                        .strong()
                        .color(p.orange),
                );
            });
            ui.add_space(3.0);
        }

        ui.add_space(4.0);
        let (frac, bar_col) = match fleet.disk_pct {
            Some(pct) => ((pct / 100.0) as f32, theme::usage_color(p, pct)),
            None => (0.0, p.text_muted),
        };
        theme::mini_progress_bar(ui, p, frac, bar_col, ui.available_width(), 5.0);
        ui.add_space(4.0);

        let caption = if fleet.disk_total_bytes > 0 {
            format!(
                "💾 {} / {} ({:.0}%)",
                theme::format_storage(fleet.disk_used_bytes),
                theme::format_storage(fleet.disk_total_bytes),
                fleet.disk_pct.unwrap_or(0.0)
            )
        } else if fleet.online == 0 {
            "Aucun serveur connecté".to_string()
        } else {
            "Stockage indisponible".to_string()
        };
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_alert_strip(
    ui: &mut Ui,
    p: &Palette,
    state: &mut OverviewState,
    snapshots: &[DeviceSnapshot<'_>],
    fleet: &FleetSummary,
) {
    let (bg, border, fg) = if fleet.critical_alerts > 0 {
        (p.danger_bg, p.danger_border, p.danger)
    } else {
        (p.warning_bg, p.warning_border, p.warning)
    };

    let mut summary_items: Vec<String> = Vec::new();
    let mut total_alerting = 0usize;
    for snap in snapshots {
        if !snap.alerts.is_empty() {
            total_alerting += 1;
            if summary_items.len() < 2 {
                let first_msg = &snap.alerts[0].message;
                summary_items.push(format!("{} ({first_msg})", snap.device.hostname));
            }
        }
    }
    let extra = total_alerting.saturating_sub(2);
    let summary_text = if extra > 0 {
        format!(
            "{} alerte(s) active(s) : {}  + {extra} autre(s)",
            total_alerting,
            summary_items.join("  •  ")
        )
    } else {
        format!(
            "{} alerte(s) active(s) : {}",
            total_alerting,
            summary_items.join("  •  ")
        )
    };

    let avail_w = ui.available_width();
    egui::Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0_f32, border))
        .corner_radius(7)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if avail_w >= 720.0 {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("⚠").color(fg).strong());
                    ui.add(
                        egui::Label::new(RichText::new(&summary_text).size(12.0).color(fg))
                            .truncate(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Voir les alertes").clicked() {
                            state.status_filter = OverviewStatusFilter::Alerts;
                        }
                    });
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("⚠").color(fg).strong());
                    ui.label(RichText::new(&summary_text).size(12.0).color(fg));
                });
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("Voir les alertes").clicked() {
                        state.status_filter = OverviewStatusFilter::Alerts;
                    }
                });
            }
        });
}

fn render_empty_fleet(ui: &mut Ui, p: &Palette, action: &mut Option<OverviewAction>) {
    ui.add_space(20.0);
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            ui.add_space(16.0);
            ui.label(RichText::new("🖥").size(34.0).color(p.accent));
            ui.add_space(8.0);
            ui.label(
                RichText::new("Aucun serveur enregistré pour le moment")
                    .size(16.0)
                    .strong()
                    .color(p.text_primary),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Lancez ntx-server sur votre Raspberry Pi / serveur Linux ou ajoutez son adresse manuellement.",
                )
                .size(12.5)
                .color(p.text_secondary),
            );
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                let total_w = 340.0;
                ui.add_space(((ui.available_width() - total_w) * 0.5).max(0.0));
                if theme::primary_button(ui, p, "➕ Ajouter manuellement").clicked() {
                    *action = Some(OverviewAction::OpenAddModal);
                }
                if ui.button("🔄 Scanner le réseau (mDNS)").clicked() {
                    *action = Some(OverviewAction::RefreshDiscovery);
                }
            });
            ui.add_space(14.0);
        });
    });
}

fn render_control_toolbar(
    ui: &mut Ui,
    p: &Palette,
    state: &mut OverviewState,
    fleet: &FleetSummary,
    view_mode: OverviewViewMode,
    action: &mut Option<OverviewAction>,
) {
    let avail_w = ui.available_width();

    let draw_search_and_filters = |ui: &mut Ui, state: &mut OverviewState| {
        ui.label(RichText::new("🔍").color(p.text_muted));
        ui.add(
            egui::TextEdit::singleline(&mut state.search_query)
                .hint_text("Rechercher hôte, IP, OS, service...")
                .desired_width(210.0),
        );
        if !state.search_query.is_empty()
            && ui
                .small_button("✖")
                .on_hover_text("Effacer la recherche")
                .clicked()
        {
            state.search_query.clear();
        }

        ui.separator();

        if theme::tab_button(
            ui,
            p,
            state.status_filter == OverviewStatusFilter::All,
            &format!("Tous ({})", fleet.total),
        )
        .clicked()
        {
            state.status_filter = OverviewStatusFilter::All;
        }
        if theme::tab_button(
            ui,
            p,
            state.status_filter == OverviewStatusFilter::Online,
            &format!("● En ligne ({})", fleet.online),
        )
        .clicked()
        {
            state.status_filter = OverviewStatusFilter::Online;
        }
        if theme::tab_button(
            ui,
            p,
            state.status_filter == OverviewStatusFilter::Offline,
            &format!("○ Hors ligne ({})", fleet.offline),
        )
        .clicked()
        {
            state.status_filter = OverviewStatusFilter::Offline;
        }
        if (fleet.alert_devices > 0 || state.status_filter == OverviewStatusFilter::Alerts)
            && theme::tab_button(
                ui,
                p,
                state.status_filter == OverviewStatusFilter::Alerts,
                &format!("⚠ Alertes ({})", fleet.alert_devices),
            )
            .clicked()
        {
            state.status_filter = OverviewStatusFilter::Alerts;
        }
    };

    let draw_view_switch =
        |ui: &mut Ui, rtl: bool, action: &mut Option<OverviewAction>| {
            let draw_cards = |ui: &mut Ui, action: &mut Option<OverviewAction>| {
                if theme::tab_button(ui, p, view_mode == OverviewViewMode::Cards, "⊞ Cartes")
                    .on_hover_text("Affichage en grille de cartes enrichies")
                    .clicked()
                    && view_mode != OverviewViewMode::Cards
                {
                    *action = Some(OverviewAction::SetViewMode(OverviewViewMode::Cards));
                }
            };
            let draw_table = |ui: &mut Ui, action: &mut Option<OverviewAction>| {
                if theme::tab_button(ui, p, view_mode == OverviewViewMode::Table, "☰ Tableau")
                    .on_hover_text("Affichage en tableau compact triable")
                    .clicked()
                    && view_mode != OverviewViewMode::Table
                {
                    *action = Some(OverviewAction::SetViewMode(OverviewViewMode::Table));
                }
            };
            if rtl {
                draw_table(ui, action);
                draw_cards(ui, action);
            } else {
                draw_cards(ui, action);
                draw_table(ui, action);
            }
        };

    let draw_sort_controls = |ui: &mut Ui, rtl: bool, state: &mut OverviewState| {
        let dir_icon = match state.sort_dir {
            SortDirection::Ascending => "▲",
            SortDirection::Descending => "▼",
        };
        let dir_tip = match state.sort_dir {
            SortDirection::Ascending => "Tri croissant (cliquer pour inverser)",
            SortDirection::Descending => "Tri décroissant (cliquer pour inverser)",
        };

        let draw_dir = |ui: &mut Ui, state: &mut OverviewState| {
            if ui.button(dir_icon).on_hover_text(dir_tip).clicked() {
                state.sort_dir = match state.sort_dir {
                    SortDirection::Ascending => SortDirection::Descending,
                    SortDirection::Descending => SortDirection::Ascending,
                };
            }
        };
        let draw_combo = |ui: &mut Ui, state: &mut OverviewState| {
            egui::ComboBox::from_id_salt("overview_sort_combo")
                .selected_text(state.sort_by.label())
                .width(145.0)
                .show_ui(ui, |ui| {
                    let options = [
                        OverviewSortBy::StatusThenName,
                        OverviewSortBy::Name,
                        OverviewSortBy::Cpu,
                        OverviewSortBy::Ram,
                        OverviewSortBy::Disk,
                        OverviewSortBy::Latency,
                        OverviewSortBy::Uptime,
                    ];
                    for opt in options {
                        if ui.selectable_label(state.sort_by == opt, opt.label()).clicked() {
                            state.toggle_sort(opt);
                        }
                    }
                });
        };

        if rtl {
            draw_dir(ui, state);
            draw_combo(ui, state);
        } else {
            draw_combo(ui, state);
            draw_dir(ui, state);
        }
    };

    theme::subtle_bar_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if avail_w >= 960.0 {
            ui.horizontal(|ui| {
                draw_search_and_filters(ui, state);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    draw_view_switch(ui, true, action);
                    ui.separator();
                    draw_sort_controls(ui, true, state);
                });
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                draw_search_and_filters(ui, state);
            });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                draw_sort_controls(ui, false, state);
                ui.separator();
                draw_view_switch(ui, false, action);
            });
        }
    });
}

fn render_empty_filtered(ui: &mut Ui, p: &Palette, state: &mut OverviewState) {
    ui.add_space(16.0);
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            ui.add_space(14.0);
            ui.label(
                RichText::new("Aucun serveur ne correspond aux filtres actuels.")
                    .size(14.5)
                    .strong()
                    .color(p.text_primary),
            );
            ui.add_space(6.0);
            if theme::primary_button(ui, p, "Réinitialiser les filtres").clicked() {
                state.search_query.clear();
                state.status_filter = OverviewStatusFilter::All;
            }
            ui.add_space(14.0);
        });
    });
}

fn status_presentation(
    p: &Palette,
    status: &ConnectionStatus,
) -> (&'static str, Color32, Color32, Color32, Color32) {
    match status {
        ConnectionStatus::Connected => {
            ("● Connecté", p.success_bg, p.success, p.success_border, p.success)
        }
        ConnectionStatus::Connecting => (
            "● Connexion...",
            p.accent_bg,
            p.accent,
            p.accent_border,
            p.accent,
        ),
        ConnectionStatus::AuthFailed(_) => {
            ("✖ Auth refusée", p.danger_bg, p.danger, p.danger_border, p.danger)
        }
        ConnectionStatus::TlsMismatch { .. } => (
            "⚠ Certificat modifié",
            p.danger_bg,
            p.danger,
            p.danger_border,
            p.danger,
        ),
        ConnectionStatus::Error(_) => {
            ("⚠ Erreur", p.warning_bg, p.warning, p.warning_border, p.warning)
        }
        ConnectionStatus::Discovered => (
            "○ Découvert",
            p.warning_bg,
            p.warning,
            p.warning_border,
            p.warning,
        ),
        ConnectionStatus::Disconnected => (
            "○ Déconnecté",
            p.bg_subtle,
            p.text_muted,
            p.border,
            p.border_strong,
        ),
    }
}

fn copy_address_button(ui: &mut Ui, p: &Palette, dev: &Device, state: &mut OverviewState) {
    let endpoint = format!("{}:{}", dev.primary_address(), dev.port);
    let is_copied = state
        .copied_device_id
        .as_ref()
        .is_some_and(|(id, until)| id == &dev.id && Instant::now() < *until);
    let (label, text_col) = if is_copied {
        (format!("✔ {endpoint}"), p.success)
    } else {
        (format!("📋 {endpoint}"), p.text_secondary)
    };

    if ui
        .add(
            egui::Button::new(RichText::new(label).monospace().size(11.5).color(text_col))
                .fill(p.bg_subtle)
                .stroke(Stroke::new(1.0_f32, p.border))
                .corner_radius(5),
        )
        .on_hover_text("Cliquer pour copier l'adresse réseau")
        .clicked()
    {
        ui.ctx().copy_text(endpoint);
        state.copied_device_id = Some((dev.id.clone(), Instant::now() + Duration::from_secs(2)));
    }
}

fn disk_tooltip_text(disk: &DiskUsageSummary) -> String {
    disk.visible_disks
        .iter()
        .map(|d| {
            format!(
                "{} : {} / {} ({:.0}%)",
                d.mount_point,
                theme::format_storage(d.used_bytes),
                theme::format_storage(d.total_bytes),
                d.pct
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn series_stats_tooltip(label: &str, values: &[f64], extra: Option<&str>) -> String {
    let mut out = if values.is_empty() {
        format!("{label} : aucune donnée historique")
    } else {
        let mut min_v = f64::INFINITY;
        let mut max_v = f64::NEG_INFINITY;
        let mut sum = 0.0;
        let mut count = 0usize;
        for &v in values {
            if v.is_finite() {
                min_v = min_v.min(v);
                max_v = max_v.max(v);
                sum += v;
                count += 1;
            }
        }
        if count > 0 {
            let avg = sum / count as f64;
            format!("{label} — Min {min_v:.1}% • Moy {avg:.1}% • Max {max_v:.1}%")
        } else {
            format!("{label} : aucune donnée historique")
        }
    };
    if let Some(ex) = extra {
        if !ex.is_empty() {
            out.push('\n');
            out.push_str(ex);
        }
    }
    out
}

fn tail_slice_vec(q: &VecDeque<f64>, limit: usize) -> Vec<f64> {
    let skip = q.len().saturating_sub(limit);
    q.iter().skip(skip).copied().collect()
}

fn render_device_context_menu(
    ui: &mut Ui,
    dev: &Device,
    state: &mut OverviewState,
    action: &mut Option<OverviewAction>,
) {
    let dev_id = dev.id.clone();
    let endpoint = format!("{}:{}", dev.primary_address(), dev.port);

    if dev.status.is_connected() {
        if ui.button("📈 Ouvrir la vue détaillée").clicked() {
            *action = Some(OverviewAction::SelectDevice(dev_id.clone()));
            ui.close_menu();
        }
        if ui.button("💻 Terminal").clicked() {
            *action = Some(OverviewAction::OpenTerminal(dev_id.clone()));
            ui.close_menu();
        }
        if ui.button("⚡ Système").clicked() {
            *action = Some(OverviewAction::OpenProcesses(dev_id.clone()));
            ui.close_menu();
        }
        if ui.button("⭐ Mes Services").clicked() {
            *action = Some(OverviewAction::OpenFavorites(dev_id.clone()));
            ui.close_menu();
        }
        if ui.button("📡 Ping").clicked() {
            *action = Some(OverviewAction::PingDevice(dev_id.clone()));
            ui.close_menu();
        }
        if ui.button("🔄 Reconnecter").clicked() {
            *action = Some(OverviewAction::ReconnectDevice(dev_id.clone()));
            ui.close_menu();
        }
        if ui.button("🔌 Déconnecter").clicked() {
            *action = Some(OverviewAction::DisconnectDevice(dev_id.clone()));
            ui.close_menu();
        }
    } else {
        if !matches!(dev.status, ConnectionStatus::Connecting)
            && ui.button("⚡ Connecter").clicked()
        {
            *action = Some(OverviewAction::ConnectDevice(dev_id.clone()));
            ui.close_menu();
        }
        if ui.button("📈 Ouvrir la fiche").clicked() {
            *action = Some(OverviewAction::SelectDevice(dev_id.clone()));
            ui.close_menu();
        }
        if ui.button("⚙ Configurer").clicked() {
            *action = Some(OverviewAction::ConfigureDevice(dev_id.clone()));
            ui.close_menu();
        }
    }

    ui.separator();
    if ui.button("📋 Copier l'adresse").clicked() {
        ui.ctx().copy_text(endpoint);
        state.copied_device_id = Some((dev_id.clone(), Instant::now() + Duration::from_secs(2)));
        ui.close_menu();
    }

    ui.separator();
    if ui.button("🗑 Supprimer de la liste").clicked() {
        state.pending_delete = Some((dev_id, dev.hostname.clone()));
        ui.close_menu();
    }
}

fn render_overflow_menu(
    ui: &mut Ui,
    dev: &Device,
    state: &mut OverviewState,
    action: &mut Option<OverviewAction>,
) {
    let dev_id = dev.id.clone();
    let endpoint = format!("{}:{}", dev.primary_address(), dev.port);

    ui.menu_button("⋯", |ui| {
        if dev.status.is_connected() {
            if ui.button("⭐ Mes Services").clicked() {
                *action = Some(OverviewAction::OpenFavorites(dev_id.clone()));
                ui.close_menu();
            }
            if ui.button("🔄 Reconnecter").clicked() {
                *action = Some(OverviewAction::ReconnectDevice(dev_id.clone()));
                ui.close_menu();
            }
            if ui.button("⚙ Configurer").clicked() {
                *action = Some(OverviewAction::ConfigureDevice(dev_id.clone()));
                ui.close_menu();
            }
        } else if ui.button("📋 Copier l'adresse").clicked() {
            ui.ctx().copy_text(endpoint);
            state.copied_device_id = Some((dev_id.clone(), Instant::now() + Duration::from_secs(2)));
            ui.close_menu();
        }

        ui.separator();
        if ui.button("🗑 Supprimer").clicked() {
            state.pending_delete = Some((dev_id, dev.hostname.clone()));
            ui.close_menu();
        }
    });
}

fn render_cards_grid(
    ui: &mut Ui,
    p: &Palette,
    state: &mut OverviewState,
    snapshots: &[&DeviceSnapshot<'_>],
    action: &mut Option<OverviewAction>,
) {
    let avail_w = ui.available_width();
    let num_cols = if avail_w >= 1120.0 {
        3
    } else if avail_w >= 660.0 {
        2
    } else {
        1
    };

    egui::ScrollArea::vertical()
        .id_salt("overview_cards_scroll")
        .show(ui, |ui| {
            for chunk in snapshots.chunks(num_cols) {
                ui.columns(num_cols, |cols| {
                    for (i, snap) in chunk.iter().enumerate() {
                        if let Some(card_act) = render_server_card(&mut cols[i], p, state, snap) {
                            *action = Some(card_act);
                        }
                    }
                });
                ui.add_space(10.0);
            }
        });
}

fn render_server_card(
    ui: &mut Ui,
    p: &Palette,
    state: &mut OverviewState,
    snap: &DeviceSnapshot<'_>,
) -> Option<OverviewAction> {
    let dev = snap.device;
    let mut card_action: Option<OverviewAction> = None;

    let (_, _, _, status_border, accent_strip) = status_presentation(p, &dev.status);
    let base_border = if dev.status.is_connected() {
        p.border_strong
    } else if !snap.alerts.is_empty() {
        status_border
    } else {
        p.border
    };
    let hover_border = if !snap.alerts.is_empty() {
        status_border
    } else {
        p.accent_border
    };

    let card_resp = ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt(("overview_card", &dev.id))
            .sense(egui::Sense::click()),
        |ui| {
            ui.style_mut().interaction.selectable_labels = false;
            let mut prepared = egui::Frame::new()
                .fill(p.bg_card)
                .stroke(Stroke::new(1.0_f32, base_border))
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(14, 12))
                .begin(ui);

            {
                let cui = &mut prepared.content_ui;
                cui.set_width(cui.available_width());
                cui.set_min_height(176.0);
                render_card_contents(cui, p, state, snap, &mut card_action);
            }

            let outer_resp = prepared.allocate_space(ui);
            let is_hovered = ui.response().hovered() || outer_resp.hovered();
            if is_hovered {
                prepared.frame.fill = p.bg_card_hover;
                prepared.frame.stroke = Stroke::new(1.2_f32, hover_border);
            }
            prepared.paint(ui);

            let card_rect = outer_resp.rect;
            if ui.is_rect_visible(card_rect) {
                let strip_rect = egui::Rect::from_min_max(
                    egui::pos2(card_rect.left() + 1.0, card_rect.top() + 8.0),
                    egui::pos2(card_rect.left() + 4.0, card_rect.bottom() - 8.0),
                );
                ui.painter().rect_filled(strip_rect, 1.5, accent_strip);
            }
        },
    );

    if card_action.is_none() && card_resp.response.clicked() {
        card_action = Some(OverviewAction::SelectDevice(dev.id.clone()));
    }
    card_resp.response.context_menu(|ui| {
        render_device_context_menu(ui, dev, state, &mut card_action);
    });

    card_action
}

fn render_card_contents(
    ui: &mut Ui,
    p: &Palette,
    state: &mut OverviewState,
    snap: &DeviceSnapshot<'_>,
    action: &mut Option<OverviewAction>,
) {
    let dev = snap.device;
    let card_w = ui.available_width();
    let (status_label, status_bg, status_fg, status_border, _) =
        status_presentation(p, &dev.status);
    let os_name = dev.system_info.as_ref().map(|i| i.os_name.as_str());
    let icon = theme::os_icon(os_name);

    // 1. Header row: status pill anchored right first, truncated hostname fills left
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 22.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            theme::pill_badge(ui, status_label, status_bg, status_fg, Some(status_border));
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("{icon} {}", dev.hostname))
                            .size(15.5)
                            .strong()
                            .color(p.text_primary),
                    )
                    .truncate(),
                );
            });
        },
    );

    ui.add_space(6.0);

    // 2. Metadata badges row
    ui.horizontal_wrapped(|ui| {
        copy_address_button(ui, p, dev, state);

        if let Some(info) = &dev.system_info {
            let mut hw_parts: Vec<String> = Vec::new();
            if !info.os_name.is_empty() {
                hw_parts.push(info.os_name.clone());
            }
            if !info.arch.is_empty() {
                hw_parts.push(info.arch.clone());
            }
            if info.cpu_count > 0 {
                hw_parts.push(format!("{}c", info.cpu_count));
            }
            if !hw_parts.is_empty() {
                theme::pill_badge(
                    ui,
                    &hw_parts.join(" • "),
                    p.bg_subtle,
                    p.text_secondary,
                    None,
                );
            }
        }

        if dev.auto_connect {
            theme::pill_badge(ui, "Auto", p.accent_bg, p.accent, Some(p.accent_border));
        }

        if let Some(ms) = dev.last_ping_ms {
            theme::pill_badge(
                ui,
                &format!("📡 {ms} ms"),
                p.bg_subtle,
                theme::latency_color(p, ms),
                None,
            );
        }

        if let Some(up) = dev.server_uptime_secs {
            theme::pill_badge(
                ui,
                &format!("⏱ {}", theme::format_uptime(up)),
                p.bg_subtle,
                p.text_secondary,
                None,
            );
        }

        let services_badge_count = if !dev.managed_services.services.is_empty() {
            dev.managed_services.services.len()
        } else {
            dev.favorites.len()
        };
        if services_badge_count > 0
            && ui
                .add(
                    egui::Button::new(
                        RichText::new(format!("⭐ {services_badge_count}"))
                            .size(11.0)
                            .strong()
                            .color(p.warning),
                    )
                    .fill(p.bg_subtle)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .corner_radius(10),
                )
                .on_hover_text("Ouvrir Mes Services")
                .clicked()
        {
            *action = Some(OverviewAction::OpenFavorites(dev.id.clone()));
        }
    });

    ui.add_space(8.0);

    // 3. Telemetry body or connection diagnostic
    if dev.status.is_connected() {
        let cpu = snap.cpu_pct.unwrap_or(0.0);
        let cpu_col = theme::usage_color(p, cpu);
        let cpu_hist = tail_slice_vec(&dev.cpu_history, CARD_SPARKLINE_POINTS);
        let bar_w = (card_w - 154.0).clamp(42.0, 130.0);

        // CPU row
        ui.horizontal(|ui| {
            ui.allocate_ui(egui::vec2(28.0, 18.0), |ui| {
                ui.label(RichText::new("CPU").size(11.0).color(p.text_muted));
            });
            ui.allocate_ui(egui::vec2(38.0, 18.0), |ui| {
                let txt = snap
                    .cpu_pct
                    .map(|v| format!("{v:.0}%"))
                    .unwrap_or_else(|| "—".to_string());
                ui.label(RichText::new(txt).size(11.5).strong().color(cpu_col));
            });
            theme::mini_progress_bar(ui, p, (cpu / 100.0) as f32, cpu_col, bar_w, 6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let cores_note = (snap.cpu_cores > 0).then(|| format!("{} cœur(s)", snap.cpu_cores));
                let tip = series_stats_tooltip("CPU", &cpu_hist, cores_note.as_deref());
                theme::sparkline(ui, p, &cpu_hist, 0.0, 100.0, cpu_col, 60.0, 18.0)
                    .on_hover_text(tip);
            });
        });

        ui.add_space(3.0);

        // RAM row
        let ram = snap.ram_pct.unwrap_or(0.0);
        let ram_hist = tail_slice_vec(&dev.ram_history, CARD_SPARKLINE_POINTS);
        ui.horizontal(|ui| {
            ui.allocate_ui(egui::vec2(28.0, 18.0), |ui| {
                ui.label(RichText::new("RAM").size(11.0).color(p.text_muted));
            });
            ui.allocate_ui(egui::vec2(38.0, 18.0), |ui| {
                let txt = snap
                    .ram_pct
                    .map(|v| format!("{v:.0}%"))
                    .unwrap_or_else(|| "—".to_string());
                ui.label(RichText::new(txt).size(11.5).strong().color(p.purple));
            });
            theme::mini_progress_bar(ui, p, (ram / 100.0) as f32, p.purple, bar_w, 6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mem_note = (snap.ram_total_bytes > 0).then(|| {
                    format!(
                        "{} / {}",
                        theme::format_storage(snap.ram_used_bytes),
                        theme::format_storage(snap.ram_total_bytes)
                    )
                });
                let tip = series_stats_tooltip("RAM", &ram_hist, mem_note.as_deref());
                theme::sparkline(ui, p, &ram_hist, 0.0, 100.0, p.purple, 60.0, 18.0)
                    .on_hover_text(tip);
            });
        });

        // Disk row (if visible disk exists)
        if let Some(disk) = &snap.disk {
            ui.add_space(3.0);
            let tip = disk_tooltip_text(disk);
            let disk_col = theme::usage_color(p, disk.primary.pct);
            let disk_bar_w = (card_w - 200.0).clamp(36.0, 100.0);
            ui.horizontal(|ui| {
                ui.allocate_ui(egui::vec2(58.0, 18.0), |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("💾 {}", disk.primary.mount_point))
                                .size(11.0)
                                .color(p.text_muted),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&tip);
                });
                ui.allocate_ui(egui::vec2(34.0, 18.0), |ui| {
                    ui.label(
                        RichText::new(format!("{:.0}%", disk.primary.pct))
                            .size(11.5)
                            .strong()
                            .color(disk_col),
                    )
                    .on_hover_text(&tip);
                });
                theme::mini_progress_bar(
                    ui,
                    p,
                    (disk.primary.pct / 100.0) as f32,
                    disk_col,
                    disk_bar_w,
                    6.0,
                )
                .on_hover_text(&tip);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "{}/{}",
                                theme::format_storage(disk.primary.used_bytes),
                                theme::format_storage(disk.primary.total_bytes)
                            ))
                            .size(10.5)
                            .color(p.text_secondary),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&tip);
                });
            });
        }

        ui.add_space(5.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!(
                    "↓ {}  •  ↑ {}",
                    theme::format_bytes_rate(snap.rx_bps),
                    theme::format_bytes_rate(snap.tx_bps)
                ))
                .size(11.0)
                .color(p.text_secondary),
            );
            if let Some(alert) = snap.alerts.first() {
                let (abg, afg, aborder) = if alert.severity == AlertSeverity::Critical {
                    (p.danger_bg, p.danger, p.danger_border)
                } else {
                    (p.warning_bg, p.warning, p.warning_border)
                };
                let label = if snap.alerts.len() > 1 {
                    format!("⚠ {} (+{})", alert.message, snap.alerts.len() - 1)
                } else {
                    format!("⚠ {}", alert.message)
                };
                theme::pill_badge(ui, &label, abg, afg, Some(aborder));
            }
        });
    } else {
        let (diag_bg, diag_border, diag_fg, msg) = match &dev.status {
            ConnectionStatus::TlsMismatch { .. } => (
                p.danger_bg,
                p.danger_border,
                p.danger,
                "Empreinte TLS modifiée — vérification manuelle requise.".to_string(),
            ),
            ConnectionStatus::AuthFailed(e) => {
                (p.danger_bg, p.danger_border, p.danger, e.clone())
            }
            ConnectionStatus::Error(e) => (p.warning_bg, p.warning_border, p.warning, e.clone()),
            ConnectionStatus::Connecting => (
                p.bg_subtle,
                p.border,
                p.accent,
                "Négociation TLS & challenge HMAC en cours...".to_string(),
            ),
            ConnectionStatus::Discovered => (
                p.bg_subtle,
                p.border,
                p.text_secondary,
                "Découvert sur le réseau local. Prêt pour la connexion sécurisée.".to_string(),
            ),
            ConnectionStatus::Disconnected => (
                p.bg_subtle,
                p.border,
                p.text_secondary,
                if dev.psk.as_deref().is_some_and(|s| !s.trim().is_empty()) {
                    "Prêt à se connecter avec la clé PSK mémorisée.".to_string()
                } else {
                    "Utilisera la clé PSK par défaut lors de la connexion.".to_string()
                },
            ),
            ConnectionStatus::Connected => (p.bg_subtle, p.border, p.text_secondary, String::new()),
        };

        egui::Frame::new()
            .fill(diag_bg)
            .stroke(Stroke::new(1.0_f32, diag_border))
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(10, 7))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(msg).size(11.5).color(diag_fg));
            });

        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            let has_psk = dev.psk.as_deref().is_some_and(|s| !s.trim().is_empty());
            if has_psk {
                theme::pill_badge(
                    ui,
                    "🔑 PSK prête",
                    p.success_bg,
                    p.success,
                    Some(p.success_border),
                );
            } else {
                theme::pill_badge(ui, "🔑 PSK par défaut", p.bg_subtle, p.text_muted, None);
            }
            theme::pill_badge(
                ui,
                if dev.is_manual { "Manuel" } else { "mDNS" },
                p.bg_subtle,
                p.text_secondary,
                None,
            );
            if !dev.version.is_empty() {
                theme::pill_badge(
                    ui,
                    &format!("v{}", dev.version.trim_start_matches('v')),
                    p.bg_subtle,
                    p.text_muted,
                    None,
                );
            }
        });
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    // 4. Footer actions (adaptive labels to avoid overflow at narrow widths)
    let show_details_lbl = card_w >= 300.0;
    let show_term_lbl = card_w >= 365.0;
    let show_cfg_lbl = card_w >= 345.0;

    ui.horizontal(|ui| {
        if dev.status.is_connected() {
            if theme::primary_button(
                ui,
                p,
                if show_details_lbl {
                    "📈 Détails"
                } else {
                    "📈"
                },
            )
            .on_hover_text("Ouvrir la vue détaillée du serveur")
            .clicked()
            {
                *action = Some(OverviewAction::SelectDevice(dev.id.clone()));
            }
            if ui
                .button(if show_term_lbl {
                    "💻 Terminal"
                } else {
                    "💻"
                })
                .on_hover_text("Ouvrir le terminal interactif")
                .clicked()
            {
                *action = Some(OverviewAction::OpenTerminal(dev.id.clone()));
            }
            if ui
                .button("⚡")
                .on_hover_text("Système (Processus & Services systemd)")
                .clicked()
            {
                *action = Some(OverviewAction::OpenProcesses(dev.id.clone()));
            }
            if ui
                .button("📡")
                .on_hover_text("Mesurer la latence (Ping)")
                .clicked()
            {
                *action = Some(OverviewAction::PingDevice(dev.id.clone()));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                render_overflow_menu(ui, dev, state, action);
                if ui
                    .button("🔌")
                    .on_hover_text("Fermer la connexion gRPC TLS")
                    .clicked()
                {
                    *action = Some(OverviewAction::DisconnectDevice(dev.id.clone()));
                }
            });
        } else {
            if matches!(dev.status, ConnectionStatus::Connecting) {
                ui.spinner();
                ui.label(RichText::new("Connexion...").size(11.5).color(p.accent));
            } else if theme::primary_button(ui, p, "⚡ Connecter").clicked() {
                *action = Some(OverviewAction::ConnectDevice(dev.id.clone()));
            }

            if ui
                .button(if show_cfg_lbl {
                    "⚙ Configurer"
                } else {
                    "⚙"
                })
                .on_hover_text("Configurer le serveur et la clé PSK")
                .clicked()
            {
                *action = Some(OverviewAction::ConfigureDevice(dev.id.clone()));
            }

            if ui.button("Ouvrir").clicked() {
                *action = Some(OverviewAction::SelectDevice(dev.id.clone()));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                render_overflow_menu(ui, dev, state, action);
            });
        }
    });
}

fn sort_indicator(active: bool, dir: SortDirection) -> &'static str {
    if !active {
        ""
    } else {
        match dir {
            SortDirection::Ascending => " ▲",
            SortDirection::Descending => " ▼",
        }
    }
}

fn render_compact_table(
    ui: &mut Ui,
    p: &Palette,
    state: &mut OverviewState,
    snapshots: &[&DeviceSnapshot<'_>],
    action: &mut Option<OverviewAction>,
) {
    let mut sort_to_toggle: Option<OverviewSortBy> = None;
    let active_sort = state.sort_by;
    let active_dir = state.sort_dir;

    theme::card_frame(p).show(ui, |ui| {
        let outer_avail_w = ui.available_width();
        let avail_h = ui.available_height().max(220.0);
        let table_w = outer_avail_w.max(940.0);

        egui::ScrollArea::horizontal()
            .id_salt("overview_table_hscroll")
            .show(ui, |ui| {
                let max_rect = egui::Rect::from_min_size(
                    ui.cursor().min,
                    egui::vec2(table_w, avail_h),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(max_rect), |ui| {
                    TableBuilder::new(ui)
                        .id_salt("overview_fleet_table")
                        .striped(true)
                        .resizable(true)
                        .sense(egui::Sense::click())
                        .auto_shrink([false, true])
                        .min_scrolled_height(0.0)
                        .max_scroll_height((avail_h - 30.0).max(160.0))
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::initial(175.0).at_least(120.0))
                        .column(Column::initial(150.0).at_least(110.0))
                        .column(Column::initial(125.0).at_least(95.0))
                        .column(Column::initial(110.0).at_least(92.0))
                        .column(Column::initial(95.0).at_least(75.0))
                        .column(Column::initial(95.0).at_least(75.0))
                        .column(Column::remainder().at_least(120.0))
                        .column(Column::initial(115.0).at_least(90.0))
                        .column(Column::exact(185.0))
                        .header(26.0, |mut header| {
                            let mut col_hdr =
                                |ui: &mut Ui, label: &str, col: Option<OverviewSortBy>| {
                                    let is_active = col == Some(active_sort);
                                    let text = format!(
                                        "{label}{}",
                                        sort_indicator(is_active, active_dir)
                                    );
                                    if let Some(c) = col {
                                        if ui.selectable_label(is_active, text).clicked() {
                                            sort_to_toggle = Some(c);
                                        }
                                    } else {
                                        ui.label(
                                            RichText::new(text).strong().color(p.text_secondary),
                                        );
                                    }
                                };

                            header.col(|ui| col_hdr(ui, "Serveur", Some(OverviewSortBy::Name)));
                            header.col(|ui| col_hdr(ui, "Adresse", None));
                            header.col(|ui| {
                                col_hdr(ui, "Statut", Some(OverviewSortBy::StatusThenName))
                            });
                            header.col(|ui| col_hdr(ui, "CPU", Some(OverviewSortBy::Cpu)));
                            header.col(|ui| col_hdr(ui, "RAM", Some(OverviewSortBy::Ram)));
                            header.col(|ui| col_hdr(ui, "Disque", Some(OverviewSortBy::Disk)));
                            header.col(|ui| col_hdr(ui, "Réseau", None));
                            header.col(|ui| {
                                col_hdr(ui, "Ping / Uptime", Some(OverviewSortBy::Latency))
                            });
                            header.col(|ui| col_hdr(ui, "Actions", None));
                        })
                        .body(|body| {
                            body.rows(34.0, snapshots.len(), |mut row| {
                                let snap = snapshots[row.index()];
                                let dev = snap.device;
                                let (status_label, status_bg, status_fg, status_border, dot_col) =
                                    status_presentation(p, &dev.status);
                                let os_icon = theme::os_icon(
                                    dev.system_info.as_ref().map(|i| i.os_name.as_str()),
                                );

                                // 1. Serveur
                                row.col(|ui| {
                                    let (dot_rect, _) = ui.allocate_exact_size(
                                        egui::vec2(8.0, 8.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().circle_filled(dot_rect.center(), 3.5, dot_col);
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new(format!("{os_icon} {}", dev.hostname))
                                                    .strong()
                                                    .color(p.text_primary),
                                            )
                                            .frame(false),
                                        )
                                        .on_hover_text("Ouvrir la vue détaillée du serveur")
                                        .clicked()
                                    {
                                        *action = Some(OverviewAction::SelectDevice(dev.id.clone()));
                                    }
                                    if dev.auto_connect {
                                        theme::pill_badge(
                                            ui,
                                            "Auto",
                                            p.accent_bg,
                                            p.accent,
                                            None,
                                        );
                                    }
                                });

                                // 2. Adresse
                                row.col(|ui| {
                                    copy_address_button(ui, p, dev, state);
                                });

                                // 3. Statut
                                row.col(|ui| {
                                    theme::pill_badge(
                                        ui,
                                        status_label,
                                        status_bg,
                                        status_fg,
                                        Some(status_border),
                                    );
                                    if !snap.alerts.is_empty() {
                                        let tip = snap
                                            .alerts
                                            .iter()
                                            .map(|a| a.message.as_str())
                                            .collect::<Vec<_>>()
                                            .join("\n");
                                        ui.label(RichText::new("⚠").color(p.warning))
                                            .on_hover_text(tip);
                                    }
                                });

                                // 4. CPU
                                row.col(|ui| {
                                    if let Some(cpu) = snap.cpu_pct {
                                        let cpu_col = theme::usage_color(p, cpu);
                                        let hist = tail_slice_vec(&dev.cpu_history, 20);
                                        let tip = (snap.cpu_cores > 0)
                                            .then(|| format!("{} cœur(s)", snap.cpu_cores));
                                        let full_tip =
                                            series_stats_tooltip("CPU", &hist, tip.as_deref());
                                        ui.label(
                                            RichText::new(format!("{cpu:.1}%"))
                                                .strong()
                                                .size(11.5)
                                                .color(cpu_col),
                                        )
                                        .on_hover_text(&full_tip);
                                        theme::sparkline(
                                            ui, p, &hist, 0.0, 100.0, cpu_col, 44.0, 16.0,
                                        )
                                        .on_hover_text(&full_tip);
                                    } else {
                                        ui.label(RichText::new("—").color(p.text_muted));
                                    }
                                });

                                // 5. RAM
                                row.col(|ui| {
                                    if let Some(ram) = snap.ram_pct {
                                        let tip = if snap.ram_total_bytes > 0 {
                                            format!(
                                                "{} / {}",
                                                theme::format_storage(snap.ram_used_bytes),
                                                theme::format_storage(snap.ram_total_bytes)
                                            )
                                        } else {
                                            format!("{ram:.1}%")
                                        };
                                        ui.label(
                                            RichText::new(format!("{ram:.0}%"))
                                                .strong()
                                                .size(11.5)
                                                .color(p.purple),
                                        )
                                        .on_hover_text(&tip);
                                        theme::mini_progress_bar(
                                            ui,
                                            p,
                                            (ram / 100.0) as f32,
                                            p.purple,
                                            42.0,
                                            5.0,
                                        )
                                        .on_hover_text(&tip);
                                    } else {
                                        ui.label(RichText::new("—").color(p.text_muted));
                                    }
                                });

                                // 6. Disque
                                row.col(|ui| {
                                    if let Some(disk) = &snap.disk {
                                        let tip = disk_tooltip_text(disk);
                                        let dcol = theme::usage_color(p, disk.primary.pct);
                                        ui.label(
                                            RichText::new(format!("{:.0}%", disk.primary.pct))
                                                .strong()
                                                .size(11.5)
                                                .color(dcol),
                                        )
                                        .on_hover_text(&tip);
                                        theme::mini_progress_bar(
                                            ui,
                                            p,
                                            (disk.primary.pct / 100.0) as f32,
                                            dcol,
                                            42.0,
                                            5.0,
                                        )
                                        .on_hover_text(&tip);
                                    } else {
                                        ui.label(RichText::new("—").color(p.text_muted));
                                    }
                                });

                                // 7. Réseau
                                row.col(|ui| {
                                    if dev.status.is_connected() {
                                        ui.label(
                                            RichText::new(format!(
                                                "↓ {} / ↑ {}",
                                                theme::format_bytes_rate(snap.rx_bps),
                                                theme::format_bytes_rate(snap.tx_bps)
                                            ))
                                            .size(11.0)
                                            .color(p.text_secondary),
                                        );
                                    } else {
                                        ui.label(RichText::new("—").color(p.text_muted));
                                    }
                                });

                                // 8. Ping / Uptime
                                row.col(|ui| {
                                    if dev.last_ping_ms.is_none()
                                        && dev.server_uptime_secs.is_none()
                                    {
                                        ui.label(RichText::new("—").color(p.text_muted));
                                    } else {
                                        if let Some(ms) = dev.last_ping_ms {
                                            ui.label(
                                                RichText::new(format!("📡 {ms}ms"))
                                                    .size(11.0)
                                                    .color(theme::latency_color(p, ms)),
                                            );
                                        }
                                        if let Some(up) = dev.server_uptime_secs {
                                            ui.label(
                                                RichText::new(format!(
                                                    "⏱ {}",
                                                    theme::format_uptime(up)
                                                ))
                                                .size(11.0)
                                                .color(p.text_secondary),
                                            );
                                        }
                                    }
                                });

                                // 9. Actions
                                row.col(|ui| {
                                    if dev.status.is_connected() {
                                        if ui
                                            .small_button("📈")
                                            .on_hover_text("Détails & métriques")
                                            .clicked()
                                        {
                                            *action =
                                                Some(OverviewAction::SelectDevice(dev.id.clone()));
                                        }
                                        if ui
                                            .small_button("💻")
                                            .on_hover_text("Terminal")
                                            .clicked()
                                        {
                                            *action =
                                                Some(OverviewAction::OpenTerminal(dev.id.clone()));
                                        }
                                        if ui
                                            .small_button("⚡")
                                            .on_hover_text("Système")
                                            .clicked()
                                        {
                                            *action =
                                                Some(OverviewAction::OpenProcesses(dev.id.clone()));
                                        }
                                        if ui.small_button("📡").on_hover_text("Ping").clicked() {
                                            *action =
                                                Some(OverviewAction::PingDevice(dev.id.clone()));
                                        }
                                        if ui
                                            .small_button("🔌")
                                            .on_hover_text("Déconnecter")
                                            .clicked()
                                        {
                                            *action = Some(OverviewAction::DisconnectDevice(
                                                dev.id.clone(),
                                            ));
                                        }
                                    } else {
                                        if !matches!(dev.status, ConnectionStatus::Connecting)
                                            && ui
                                                .small_button("⚡")
                                                .on_hover_text("Connecter")
                                                .clicked()
                                        {
                                            *action = Some(OverviewAction::ConnectDevice(
                                                dev.id.clone(),
                                            ));
                                        }
                                        if ui
                                            .small_button("⚙")
                                            .on_hover_text("Configurer")
                                            .clicked()
                                        {
                                            *action = Some(OverviewAction::ConfigureDevice(
                                                dev.id.clone(),
                                            ));
                                        }
                                        if ui
                                            .small_button("📈")
                                            .on_hover_text("Ouvrir la fiche")
                                            .clicked()
                                        {
                                            *action =
                                                Some(OverviewAction::SelectDevice(dev.id.clone()));
                                        }
                                    }
                                    render_overflow_menu(ui, dev, state, action);
                                });

                                let resp = row.response();
                                if action.is_none() && resp.clicked() {
                                    *action = Some(OverviewAction::SelectDevice(dev.id.clone()));
                                }
                                resp.context_menu(|ui| {
                                    render_device_context_menu(ui, dev, state, action);
                                });
                            });
                        });
                });
            });
    });

    if let Some(col) = sort_to_toggle {
        state.toggle_sort(col);
    }
}

fn render_delete_modal(
    ui: &mut Ui,
    p: &Palette,
    state: &mut OverviewState,
    action: &mut Option<OverviewAction>,
) {
    let Some((dev_id, hostname)) = state.pending_delete.clone() else {
        return;
    };

    egui::Window::new("⚠ Confirmer la suppression")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            ui.label(format!(
                "Voulez-vous vraiment supprimer le serveur « {hostname} » de NTX Panel ?"
            ));
            ui.label(
                RichText::new("Les données locales et l'empreinte TLS associées seront supprimées.")
                    .color(p.text_secondary)
                    .size(12.0),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if theme::danger_button(ui, p, "🗑 Supprimer définitivement").clicked() {
                    state.pending_delete = None;
                    *action = Some(OverviewAction::RemoveDevice(dev_id));
                }
                if ui.button("Annuler").clicked() {
                    state.pending_delete = None;
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_device(id: &str, hostname: &str, status: ConnectionStatus) -> Device {
        let mut dev = Device::new(
            id.to_string(),
            hostname.to_string(),
            vec!["192.168.1.10".to_string()],
            9850,
            "0.3.0".to_string(),
        );
        dev.status = status;
        dev
    }

    #[test]
    fn test_device_network_rates_filters_virtual_interfaces() {
        let mut dev = make_device("d1", "node-1", ConnectionStatus::Connected);
        dev.latest_metrics = Some(ntx_proto::SystemMetrics {
            cpu_usage_percent: 10.0,
            used_memory_bytes: 100,
            total_memory_bytes: 1000,
            disk_metrics: vec![],
            network_metrics: vec![
                ntx_proto::NetworkMetrics {
                    interface_name: "eth0".to_string(),
                    rx_bytes_per_sec: 2000,
                    tx_bytes_per_sec: 500,
                },
                ntx_proto::NetworkMetrics {
                    interface_name: "lo".to_string(),
                    rx_bytes_per_sec: 100_000,
                    tx_bytes_per_sec: 100_000,
                },
                ntx_proto::NetworkMetrics {
                    interface_name: "docker0".to_string(),
                    rx_bytes_per_sec: 50_000,
                    tx_bytes_per_sec: 50_000,
                },
            ],
            timestamp_ms: 1,
        });

        assert_eq!(device_network_rates(&dev, true), (2000.0, 500.0));
        assert_eq!(device_network_rates(&dev, false), (152_000.0, 150_500.0));
    }

    #[test]
    fn test_primary_disk_usage_prefers_root_and_prioritizes_critical() {
        let mut dev = make_device("d1", "pi", ConnectionStatus::Connected);
        dev.system_info = Some(ntx_proto::SystemInfo {
            disks: vec![
                ntx_proto::DiskInfo {
                    name: "tmpfs".to_string(),
                    mount_point: "/run".to_string(),
                    total_bytes: 1000,
                    available_bytes: 100,
                    filesystem: "tmpfs".to_string(),
                },
                ntx_proto::DiskInfo {
                    name: "sda1".to_string(),
                    mount_point: "/mnt/data".to_string(),
                    total_bytes: 500_000,
                    available_bytes: 250_000,
                    filesystem: "ext4".to_string(),
                },
                ntx_proto::DiskInfo {
                    name: "mmcblk0p2".to_string(),
                    mount_point: "/".to_string(),
                    total_bytes: 100_000,
                    available_bytes: 60_000,
                    filesystem: "ext4".to_string(),
                },
            ],
            ..Default::default()
        });

        let summary = primary_disk_usage(&dev, true).unwrap();
        assert_eq!(summary.visible_disks.len(), 2);
        assert_eq!(summary.primary.mount_point, "/");

        // Now mark /mnt/data critical (>= 85%) via live metrics -> should prioritize it
        dev.latest_metrics = Some(ntx_proto::SystemMetrics {
            disk_metrics: vec![
                ntx_proto::DiskMetrics {
                    mount_point: "/".to_string(),
                    used_bytes: 40_000,
                    total_bytes: 100_000,
                    read_bytes_per_sec: 0,
                    write_bytes_per_sec: 0,
                },
                ntx_proto::DiskMetrics {
                    mount_point: "/mnt/data".to_string(),
                    used_bytes: 460_000,
                    total_bytes: 500_000,
                    read_bytes_per_sec: 0,
                    write_bytes_per_sec: 0,
                },
            ],
            ..Default::default()
        });
        let critical = primary_disk_usage(&dev, true).unwrap();
        assert_eq!(critical.primary.mount_point, "/mnt/data");
        assert!(critical.primary.pct >= 90.0);
    }

    #[test]
    fn test_fleet_sparkline_tail_alignment_and_averaging() {
        let mut d1 = make_device("d1", "a", ConnectionStatus::Connected);
        d1.cpu_history.extend([10.0, 20.0, 30.0, 40.0]);
        let mut d2 = make_device("d2", "b", ConnectionStatus::Connected);
        d2.cpu_history.extend([50.0, 60.0]);
        let d3 = make_device("d3", "c", ConnectionStatus::Connected); // empty

        let fleet = [&d1, &d2, &d3];
        let spark = compute_fleet_sparkline(&fleet, |d| &d.cpu_history, 10);
        assert_eq!(spark, vec![10.0, 20.0, 40.0, 50.0]);

        let capped = compute_fleet_sparkline(&fleet, |d| &d.cpu_history, 2);
        assert_eq!(capped, vec![40.0, 50.0]);
    }

    #[test]
    fn test_device_alerts_and_fleet_summary() {
        let mut d1 = make_device("d1", "alpha", ConnectionStatus::Connected);
        d1.system_info = Some(ntx_proto::SystemInfo {
            cpu_count: 4,
            total_memory_bytes: 4_000_000_000,
            ..Default::default()
        });
        d1.latest_metrics = Some(ntx_proto::SystemMetrics {
            cpu_usage_percent: 93.0,
            used_memory_bytes: 2_000_000_000,
            total_memory_bytes: 4_000_000_000,
            disk_metrics: vec![
                ntx_proto::DiskMetrics {
                    mount_point: "/".to_string(),
                    used_bytes: 90,
                    total_bytes: 100,
                    read_bytes_per_sec: 0,
                    write_bytes_per_sec: 0,
                },
                ntx_proto::DiskMetrics {
                    mount_point: "/".to_string(),
                    used_bytes: 90,
                    total_bytes: 100,
                    read_bytes_per_sec: 0,
                    write_bytes_per_sec: 0,
                },
            ],
            network_metrics: vec![ntx_proto::NetworkMetrics {
                interface_name: "eth0".to_string(),
                rx_bytes_per_sec: 1500,
                tx_bytes_per_sec: 500,
            }],
            timestamp_ms: 0,
        });

        let d2 = make_device(
            "d2",
            "beta",
            ConnectionStatus::AuthFailed("Mauvaise PSK".to_string()),
        );
        let d3 = make_device("d3", "gamma", ConnectionStatus::Discovered);

        let snapshots = vec![
            build_snapshot(&d1, true, true),
            build_snapshot(&d2, true, true),
            build_snapshot(&d3, true, true),
        ];

        assert_eq!(snapshots[0].alerts.len(), 2); // CPU >= 92% + Disk >= 90%
        assert_eq!(snapshots[0].alerts[0].severity, AlertSeverity::Critical);
        assert_eq!(snapshots[1].alerts.len(), 1);
        assert_eq!(snapshots[2].alerts.len(), 0);

        let summary = summarize_fleet(&snapshots);
        assert_eq!(summary.total, 3);
        assert_eq!(summary.online, 1);
        assert_eq!(summary.offline, 2);
        assert_eq!(summary.discovered, 1);
        assert_eq!(summary.alert_devices, 2);
        assert_eq!(summary.critical_alerts, 2);
        assert_eq!(summary.total_cores, 4);
        assert_eq!(summary.disk_used_bytes, 90);
        assert_eq!(summary.disk_total_bytes, 100);
        assert_eq!(summary.total_rx_bps, 1500.0);
        assert_eq!(summary.total_tx_bps, 500.0);
    }

    #[test]
    fn test_filter_and_sort_snapshots() {
        let mut d1 = make_device("d1", "web-01", ConnectionStatus::Connected);
        d1.cpu_history.push_back(20.0);
        let mut d2 = make_device("d2", "db-01", ConnectionStatus::Connected);
        d2.cpu_history.push_back(90.0);
        let d3 = make_device("d3", "backup", ConnectionStatus::Disconnected);

        let snaps = vec![
            build_snapshot(&d1, true, true),
            build_snapshot(&d2, true, true),
            build_snapshot(&d3, true, true),
        ];

        let mut state = OverviewState::default();
        state.toggle_sort(OverviewSortBy::Cpu); // Descending by default
        let res = filter_and_sort_snapshots(&snaps, &state);
        assert_eq!(res.len(), 3);
        assert_eq!(res[0].device.hostname, "db-01");
        assert_eq!(res[1].device.hostname, "web-01");
        assert_eq!(res[2].device.hostname, "backup"); // None always last

        // Toggle to Ascending: None still last!
        state.toggle_sort(OverviewSortBy::Cpu);
        let res_asc = filter_and_sort_snapshots(&snaps, &state);
        assert_eq!(res_asc[0].device.hostname, "web-01");
        assert_eq!(res_asc[1].device.hostname, "db-01");
        assert_eq!(res_asc[2].device.hostname, "backup");

        state.status_filter = OverviewStatusFilter::Alerts;
        let alerts_only = filter_and_sort_snapshots(&snaps, &state);
        assert_eq!(alerts_only.len(), 1);
        assert_eq!(alerts_only[0].device.hostname, "db-01");

        state.status_filter = OverviewStatusFilter::All;
        state.search_query = "web".to_string();
        let search_res = filter_and_sort_snapshots(&snaps, &state);
        assert_eq!(search_res.len(), 1);
        assert_eq!(search_res[0].device.hostname, "web-01");
    }
}
