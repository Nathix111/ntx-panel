use crate::state::{is_virtual_disk, is_virtual_interface, ConnectionStatus, Device};
use crate::ui::theme::{self, Palette};
use egui::{Color32, RichText, Stroke, Ui};
use egui_plot::{Corner, HLine, Legend, Line, LineStyle, Plot, PlotPoints};
use std::collections::{BTreeSet, HashSet, VecDeque};
use std::time::{Duration, Instant};

const SPARKLINE_POINTS: usize = 30;
const ALERT_THRESHOLD_PCT: f64 = 85.0;

pub struct MetricsPanelState {
    pub selected_interface: Option<String>,
    pub show_thresholds: bool,
    pub feedback: Option<(String, Instant)>,
    pub copied_ip: Option<(String, Instant)>,
}

impl Default for MetricsPanelState {
    fn default() -> Self {
        Self {
            selected_interface: None,
            show_thresholds: true,
            feedback: None,
            copied_ip: None,
        }
    }
}

impl MetricsPanelState {
    pub fn reset_for_device(&mut self) {
        self.selected_interface = None;
        self.feedback = None;
        self.copied_ip = None;
    }
}

pub enum MetricsPanelAction {
    Connect,
    TogglePause,
    ClearHistory,
    ToggleHideVirtualDisks,
    ToggleHideVirtualInterfaces,
    Ping,
    OpenProcesses,
    OpenTerminal,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeriesStats {
    pub min: f64,
    pub avg: f64,
    pub max: f64,
    pub last: f64,
}

pub fn compute_series_stats(values: &[f64]) -> Option<SeriesStats> {
    let mut min_v = f64::INFINITY;
    let mut max_v = f64::NEG_INFINITY;
    let mut sum = 0.0;
    let mut count = 0usize;
    let mut last = 0.0;

    for &v in values {
        if v.is_finite() {
            min_v = min_v.min(v);
            max_v = max_v.max(v);
            sum += v;
            count += 1;
            last = v;
        }
    }

    (count > 0).then(|| SeriesStats {
        min: min_v,
        avg: sum / count as f64,
        max: max_v,
        last,
    })
}

pub fn choose_rate_unit(peak_bps: f64) -> (f64, &'static str) {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * KB;
    const GB: f64 = 1024.0 * MB;

    let safe = if peak_bps.is_finite() && peak_bps >= 0.0 {
        peak_bps
    } else {
        0.0
    };
    if safe < KB {
        (1.0, "o/s")
    } else if safe < MB {
        (KB, "Ko/s")
    } else if safe < GB {
        (MB, "Mo/s")
    } else {
        (GB, "Go/s")
    }
}

pub fn format_window_duration(secs: f64) -> String {
    let total = secs.max(0.0).round() as u64;
    if total < 60 {
        format!("{total}s")
    } else {
        let mins = total / 60;
        let rem = total % 60;
        if rem == 0 {
            format!("{mins}m")
        } else {
            format!("{mins}m {rem:02}s")
        }
    }
}

pub fn format_axis_seconds(sec: f64) -> String {
    let rounded = sec.round() as i64;
    if rounded == 0 {
        return "0s".to_string();
    }
    let abs = rounded.unsigned_abs();
    let sign = if rounded < 0 { "-" } else { "" };
    if abs < 60 {
        format!("{sign}{abs}s")
    } else {
        let mins = abs / 60;
        let rem = abs % 60;
        if rem == 0 {
            format!("{sign}{mins}m")
        } else {
            format!("{sign}{mins}m{rem:02}s")
        }
    }
}

pub fn extract_network_series(
    device: &Device,
    selected_interface: Option<&str>,
    hide_virtual_interfaces: bool,
) -> (Vec<f64>, Vec<f64>) {
    if !device.net_samples.is_empty() {
        let mut rx_out = Vec::with_capacity(device.net_samples.len());
        let mut tx_out = Vec::with_capacity(device.net_samples.len());
        for tick in &device.net_samples {
            let mut rx: u64 = 0;
            let mut tx: u64 = 0;
            for (name, r, t) in tick {
                let keep = match selected_interface {
                    Some(target) => name == target,
                    None => !hide_virtual_interfaces || !is_virtual_interface(name),
                };
                if keep {
                    rx = rx.saturating_add(*r);
                    tx = tx.saturating_add(*t);
                }
            }
            rx_out.push(rx as f64);
            tx_out.push(tx as f64);
        }
        (rx_out, tx_out)
    } else {
        (
            device.rx_history.iter().copied().collect(),
            device.tx_history.iter().copied().collect(),
        )
    }
}

pub fn available_network_interfaces(
    device: &Device,
    selected_interface: Option<&str>,
    hide_virtual_interfaces: bool,
) -> Vec<String> {
    let mut names = BTreeSet::new();
    if let Some(info) = &device.system_info {
        for n in &info.networks {
            if !n.name.is_empty() {
                names.insert(n.name.clone());
            }
        }
    }
    if let Some(m) = &device.latest_metrics {
        for nm in &m.network_metrics {
            if !nm.interface_name.is_empty() {
                names.insert(nm.interface_name.clone());
            }
        }
    }
    names
        .into_iter()
        .filter(|name| {
            !hide_virtual_interfaces
                || !is_virtual_interface(name)
                || selected_interface == Some(name.as_str())
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiskRow {
    pub mount_point: String,
    pub filesystem: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub avail_bytes: u64,
    pub pct: f64,
    pub read_bps: Option<u64>,
    pub write_bps: Option<u64>,
}

pub fn collect_disk_rows(device: &Device, hide_virtual_disks: bool) -> (Vec<DiskRow>, usize) {
    let mut all_rows: Vec<DiskRow> = Vec::new();
    let mut seen_mounts: HashSet<String> = HashSet::new();

    if let Some(m) = &device.latest_metrics {
        if !m.disk_metrics.is_empty() {
            for dm in &m.disk_metrics {
                if dm.total_bytes == 0 || !seen_mounts.insert(dm.mount_point.clone()) {
                    continue;
                }
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
                let pct = (dm.used_bytes as f64 / dm.total_bytes as f64) * 100.0;
                all_rows.push(DiskRow {
                    mount_point: dm.mount_point.clone(),
                    filesystem: fs_name.to_string(),
                    used_bytes: dm.used_bytes,
                    total_bytes: dm.total_bytes,
                    avail_bytes: dm.total_bytes.saturating_sub(dm.used_bytes),
                    pct,
                    read_bps: Some(dm.read_bytes_per_sec),
                    write_bps: Some(dm.write_bytes_per_sec),
                });
            }
        }
    }

    if all_rows.is_empty() {
        if let Some(info) = &device.system_info {
            for d in &info.disks {
                if d.total_bytes == 0 || !seen_mounts.insert(d.mount_point.clone()) {
                    continue;
                }
                let used_bytes = d.total_bytes.saturating_sub(d.available_bytes);
                let pct = (used_bytes as f64 / d.total_bytes as f64) * 100.0;
                all_rows.push(DiskRow {
                    mount_point: d.mount_point.clone(),
                    filesystem: if d.filesystem.is_empty() {
                        "—".to_string()
                    } else {
                        d.filesystem.clone()
                    },
                    used_bytes,
                    total_bytes: d.total_bytes,
                    avail_bytes: d.available_bytes,
                    pct,
                    read_bps: None,
                    write_bps: None,
                });
            }
        }
    }

    let total_count = all_rows.len();
    let visible_rows = all_rows
        .into_iter()
        .filter(|row| !hide_virtual_disks || !is_virtual_disk(&row.mount_point, &row.filesystem))
        .collect();

    (visible_rows, total_count)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAlert {
    pub critical: bool,
    pub message: String,
}

pub fn collect_server_alerts(
    device: &Device,
    cur_cpu: f64,
    cur_ram: f64,
    visible_disks: &[DiskRow],
) -> Vec<ServerAlert> {
    let mut alerts = Vec::new();
    if cur_cpu >= ALERT_THRESHOLD_PCT {
        alerts.push(ServerAlert {
            critical: cur_cpu >= 92.0,
            message: format!("CPU élevé ({cur_cpu:.0}%)"),
        });
    }
    if cur_ram >= ALERT_THRESHOLD_PCT {
        alerts.push(ServerAlert {
            critical: cur_ram >= 92.0,
            message: format!("RAM élevée ({cur_ram:.0}%)"),
        });
    }
    for d in visible_disks {
        if d.pct >= ALERT_THRESHOLD_PCT {
            alerts.push(ServerAlert {
                critical: d.pct >= 90.0,
                message: format!("Disque {} plein ({:.0}%)", d.mount_point, d.pct),
            });
        }
    }
    if let Some(ms) = device.last_ping_ms {
        if ms >= 150 {
            alerts.push(ServerAlert {
                critical: false,
                message: format!("Latence élevée ({ms} ms)"),
            });
        }
    }
    alerts
}

fn tail_aligned_u64(q: &VecDeque<u64>, i: usize, target_len: usize) -> u64 {
    let offset = target_len.saturating_sub(q.len());
    if i < offset {
        0
    } else {
        q.get(i - offset).copied().unwrap_or(0)
    }
}

fn tail_aligned_f64(q: &VecDeque<f64>, i: usize, target_len: usize) -> f64 {
    let offset = target_len.saturating_sub(q.len());
    if i < offset {
        0.0
    } else {
        q.get(i - offset).copied().unwrap_or(0.0)
    }
}

fn tail_aligned_slice(vals: &[f64], i: usize, target_len: usize) -> f64 {
    let offset = target_len.saturating_sub(vals.len());
    if i < offset {
        0.0
    } else {
        vals.get(i - offset).copied().unwrap_or(0.0)
    }
}

pub fn export_metrics_csv(
    device: &Device,
    selected_interface: Option<&str>,
    hide_virtual_interfaces: bool,
) -> String {
    let mut lines = Vec::with_capacity(device.cpu_history.len() + 1);
    lines.push(
        "timestamp_ms,cpu_percent,ram_percent,ram_used_bytes,net_rx_bps,net_tx_bps,disk_read_bps,disk_write_bps"
            .to_string(),
    );

    let (net_rx, net_tx) =
        extract_network_series(device, selected_interface, hide_virtual_interfaces);
    let n = device.cpu_history.len();

    for i in 0..n {
        let ts = tail_aligned_u64(&device.metrics_timestamps_ms, i, n);
        let cpu = device.cpu_history.get(i).copied().unwrap_or(0.0);
        let ram = tail_aligned_f64(&device.ram_history, i, n);
        let ram_used = tail_aligned_u64(&device.ram_used_history, i, n);
        let rx = tail_aligned_slice(&net_rx, i, n).round() as u64;
        let tx = tail_aligned_slice(&net_tx, i, n).round() as u64;
        let dr = tail_aligned_f64(&device.disk_read_history, i, n).round() as u64;
        let dw = tail_aligned_f64(&device.disk_write_history, i, n).round() as u64;

        lines.push(format!("{ts},{cpu:.2},{ram:.2},{ram_used},{rx},{tx},{dr},{dw}"));
    }

    lines.join("\n")
}

pub fn format_server_summary(
    device: &Device,
    hide_virtual_disks: bool,
    hide_virtual_interfaces: bool,
) -> String {
    let mut out = Vec::new();
    out.push(format!(
        "Serveur : {} ({}:{})",
        device.hostname,
        device.primary_address(),
        device.port
    ));

    if let Some(info) = &device.system_info {
        out.push(format!(
            "Système : {} {} • {} • {} cœur(s)",
            info.os_name, info.os_version, info.arch, info.cpu_count
        ));
    }

    if device.last_ping_ms.is_some() || device.server_uptime_secs.is_some() {
        let ping = device
            .last_ping_ms
            .map(|ms| format!("{ms} ms"))
            .unwrap_or_else(|| "—".to_string());
        let up = device
            .server_uptime_secs
            .map(theme::format_uptime)
            .unwrap_or_else(|| "—".to_string());
        out.push(format!("Santé : Ping {ping} • Uptime {up}"));
    }

    let cur_cpu = device
        .latest_metrics
        .as_ref()
        .map(|m| m.cpu_usage_percent)
        .or_else(|| device.cpu_history.back().copied())
        .unwrap_or(0.0);
    let (ram_used, ram_total, cur_ram) = if let Some(m) = &device.latest_metrics {
        let pct = if m.total_memory_bytes > 0 {
            (m.used_memory_bytes as f64 / m.total_memory_bytes as f64) * 100.0
        } else {
            device.ram_history.back().copied().unwrap_or(0.0)
        };
        (m.used_memory_bytes, m.total_memory_bytes, pct)
    } else {
        (
            0,
            device
                .system_info
                .as_ref()
                .map(|s| s.total_memory_bytes)
                .unwrap_or(0),
            device.ram_history.back().copied().unwrap_or(0.0),
        )
    };
    out.push(format!(
        "CPU : {cur_cpu:.1}% | RAM : {} / {} ({cur_ram:.1}%)",
        theme::format_storage(ram_used),
        theme::format_storage(ram_total)
    ));

    let (rx_series, tx_series) = extract_network_series(device, None, hide_virtual_interfaces);
    let cur_rx = rx_series.last().copied().unwrap_or(0.0);
    let cur_tx = tx_series.last().copied().unwrap_or(0.0);
    out.push(format!(
        "Réseau : ↓ {} | ↑ {}",
        theme::format_bytes_rate(cur_rx),
        theme::format_bytes_rate(cur_tx)
    ));

    let (disks, _) = collect_disk_rows(device, hide_virtual_disks);
    for d in disks {
        out.push(format!(
            "- Disque {} : {} / {} ({:.0}%)",
            d.mount_point,
            theme::format_storage(d.used_bytes),
            theme::format_storage(d.total_bytes),
            d.pct
        ));
    }

    out.join("\n")
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    ui: &mut Ui,
    state: &mut MetricsPanelState,
    device: &Device,
    refresh_rate_ms: u32,
    history_capacity: usize,
    hide_virtual_disks: bool,
    hide_virtual_interfaces: bool,
) -> Option<MetricsPanelAction> {
    let p = Palette::for_ui(ui);
    let mut action: Option<MetricsPanelAction> = None;

    let now = Instant::now();
    let mut needs_timer_repaint = false;
    if let Some((_, until)) = &state.feedback {
        if now < *until {
            needs_timer_repaint = true;
        } else {
            state.feedback = None;
        }
    }
    if let Some((_, until)) = &state.copied_ip {
        if now < *until {
            needs_timer_repaint = true;
        } else {
            state.copied_ip = None;
        }
    }
    if needs_timer_repaint {
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }

    if !device.status.is_connected() {
        render_disconnected_state(ui, &p, device, &mut action);
        return action;
    }

    let step_secs = (refresh_rate_ms.max(250) as f64) / 1000.0;
    let window_secs = (history_capacity.max(10) as f64) * step_secs;

    let (net_rx_series, net_tx_series) = extract_network_series(
        device,
        state.selected_interface.as_deref(),
        hide_virtual_interfaces,
    );
    let (visible_disks, total_disks) = collect_disk_rows(device, hide_virtual_disks);

    let cpu_slice: Vec<f64> = device.cpu_history.iter().copied().collect();
    let ram_slice: Vec<f64> = device.ram_history.iter().copied().collect();
    let disk_r_slice: Vec<f64> = device.disk_read_history.iter().copied().collect();
    let disk_w_slice: Vec<f64> = device.disk_write_history.iter().copied().collect();

    let cur_cpu = device
        .latest_metrics
        .as_ref()
        .map(|m| m.cpu_usage_percent)
        .or_else(|| cpu_slice.last().copied())
        .unwrap_or(0.0);
    let (ram_used_bytes, ram_total_bytes, cur_ram) = if let Some(m) = &device.latest_metrics {
        let pct = if m.total_memory_bytes > 0 {
            (m.used_memory_bytes as f64 / m.total_memory_bytes as f64) * 100.0
        } else {
            ram_slice.last().copied().unwrap_or(0.0)
        };
        (m.used_memory_bytes, m.total_memory_bytes, pct)
    } else {
        (
            0,
            device
                .system_info
                .as_ref()
                .map(|s| s.total_memory_bytes)
                .unwrap_or(0),
            ram_slice.last().copied().unwrap_or(0.0),
        )
    };
    let cur_rx = net_rx_series.last().copied().unwrap_or(0.0);
    let cur_tx = net_tx_series.last().copied().unwrap_or(0.0);
    let cur_disk_r = disk_r_slice.last().copied().unwrap_or(0.0);
    let cur_disk_w = disk_w_slice.last().copied().unwrap_or(0.0);

    let alerts = collect_server_alerts(device, cur_cpu, cur_ram, &visible_disks);

    egui::ScrollArea::vertical()
        .id_salt("device_metrics_scroll")
        .show(ui, |ui| {
            // 1. Live Stream Control & Diagnostic Toolbar
            render_toolbar(
                ui,
                &p,
                state,
                device,
                refresh_rate_ms,
                window_secs,
                hide_virtual_disks,
                hide_virtual_interfaces,
                &mut action,
            );

            // 2. Conditional Server Alert Strip
            if !alerts.is_empty() {
                ui.add_space(8.0);
                render_alert_strip(ui, &p, &alerts, &mut action);
            }

            ui.add_space(10.0);

            // 3. 4 Responsive Enriched KPI Cards
            render_kpi_strip(
                ui,
                &p,
                state,
                device,
                cur_cpu,
                &cpu_slice,
                cur_ram,
                ram_used_bytes,
                ram_total_bytes,
                &ram_slice,
                &visible_disks,
                cur_disk_r,
                cur_disk_w,
                cur_rx,
                cur_tx,
                &net_rx_series,
                &net_tx_series,
                hide_virtual_interfaces,
            );

            ui.add_space(12.0);

            // 4. 2x2 Real-Time Charts Grid
            render_charts_grid(
                ui,
                &p,
                state,
                device,
                step_secs,
                window_secs,
                cur_cpu,
                &cpu_slice,
                cur_ram,
                ram_used_bytes,
                ram_total_bytes,
                &ram_slice,
                &net_rx_series,
                &net_tx_series,
                &disk_r_slice,
                &disk_w_slice,
                hide_virtual_interfaces,
            );

            ui.add_space(10.0);

            // 5. Stockage & Disques Card
            render_disks_card(
                ui,
                &p,
                &visible_disks,
                total_disks,
                hide_virtual_disks,
                &mut action,
            );

            ui.add_space(10.0);

            // 6. Interfaces Réseau Card
            render_networks_card(
                ui,
                &p,
                state,
                device,
                hide_virtual_interfaces,
                &mut action,
            );

            ui.add_space(10.0);

            // 7. Compact System & Hardware Synthesis Card
            render_hardware_synthesis_card(ui, &p, device, &mut action);
        });

    action
}

fn render_disconnected_state(
    ui: &mut Ui,
    p: &Palette,
    device: &Device,
    action: &mut Option<MetricsPanelAction>,
) {
    ui.add_space(20.0);
    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            ui.add_space(14.0);
            ui.label(RichText::new("🖥").size(32.0).color(p.accent));
            ui.add_space(8.0);

            let (title, subtitle) = match &device.status {
                ConnectionStatus::Connecting => (
                    "Connexion en cours...",
                    "Négociation TLS et authentification HMAC-SHA256 avec l'agent.".to_string(),
                ),
                ConnectionStatus::AuthFailed(e) => (
                    "Authentification refusée",
                    format!("Clé PSK invalide ou refusée : {e}"),
                ),
                ConnectionStatus::TlsMismatch { .. } => (
                    "Empreinte TLS modifiée",
                    "Vérifiez et validez la nouvelle empreinte TLS ci-dessus avant de vous connecter."
                        .to_string(),
                ),
                ConnectionStatus::Error(e) => ("Erreur de connexion", e.clone()),
                ConnectionStatus::Discovered | ConnectionStatus::Disconnected => (
                    "Serveur actuellement déconnecté",
                    "Connectez-vous pour suivre CPU, RAM, réseau et I/O disques en temps réel."
                        .to_string(),
                ),
                ConnectionStatus::Connected => ("", String::new()),
            };

            ui.label(RichText::new(title).size(16.0).strong().color(p.text_primary));
            ui.add_space(4.0);
            ui.label(RichText::new(subtitle).size(12.5).color(p.text_secondary));

            if let Some(info) = &device.system_info {
                ui.add_space(8.0);
                theme::pill_badge(
                    ui,
                    &format!(
                        "Dernières specs : {} • {} • {} cœurs • {}",
                        info.os_name,
                        info.arch,
                        info.cpu_count,
                        theme::format_storage(info.total_memory_bytes)
                    ),
                    p.bg_subtle,
                    p.text_secondary,
                    Some(p.border),
                );
            }

            ui.add_space(14.0);
            if matches!(device.status, ConnectionStatus::Connecting) {
                ui.spinner();
            } else if theme::primary_button(ui, p, "⚡ Se connecter maintenant").clicked() {
                *action = Some(MetricsPanelAction::Connect);
            }
            ui.add_space(14.0);
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn render_toolbar(
    ui: &mut Ui,
    p: &Palette,
    state: &mut MetricsPanelState,
    device: &Device,
    refresh_rate_ms: u32,
    window_secs: f64,
    hide_virtual_disks: bool,
    hide_virtual_interfaces: bool,
    action: &mut Option<MetricsPanelAction>,
) {
    let avail_w = ui.available_width();
    let window_lbl = format_window_duration(window_secs);
    let sample_count = device.cpu_history.len();

    let draw_left = |ui: &mut Ui, state: &MetricsPanelState| {
        if device.metrics_paused {
            theme::pill_badge(
                ui,
                &format!("⏸ En pause • {sample_count} pts"),
                p.warning_bg,
                p.warning,
                Some(p.warning_border),
            );
        } else {
            theme::pill_badge(
                ui,
                &format!("● Flux direct • {refresh_rate_ms} ms • {window_lbl}"),
                p.success_bg,
                p.success,
                Some(p.success_border),
            );
        }
        if let Some((msg, _)) = &state.feedback {
            theme::pill_badge(
                ui,
                &format!("✔ {msg}"),
                p.success_bg,
                p.success,
                Some(p.success_border),
            );
        }
    };

    let draw_right = |ui: &mut Ui,
                      rtl: bool,
                      state: &mut MetricsPanelState,
                      action: &mut Option<MetricsPanelAction>| {
        let draw_pause = |ui: &mut Ui, action: &mut Option<MetricsPanelAction>| {
            let lbl = if device.metrics_paused {
                "▶ Reprendre"
            } else {
                "⏸ Figer"
            };
            if ui
                .button(lbl)
                .on_hover_text("Figer ou reprendre l'enregistrement des graphes")
                .clicked()
            {
                *action = Some(MetricsPanelAction::TogglePause);
            }
        };
        let draw_thresh = |ui: &mut Ui, state: &mut MetricsPanelState| {
            if theme::tab_button(ui, p, state.show_thresholds, "⚠ Seuil 85%")
                .on_hover_text("Afficher la ligne de seuil d'alerte à 85 % sur CPU et RAM")
                .clicked()
            {
                state.show_thresholds = !state.show_thresholds;
            }
        };
        let draw_clear = |ui: &mut Ui, action: &mut Option<MetricsPanelAction>| {
            if ui
                .button("♻ Effacer")
                .on_hover_text("Vider l'historique local des graphiques")
                .clicked()
            {
                *action = Some(MetricsPanelAction::ClearHistory);
            }
        };
        let draw_csv = |ui: &mut Ui, state: &mut MetricsPanelState| {
            if ui
                .button("📥 CSV")
                .on_hover_text("Copier l'historique des métriques au format CSV")
                .clicked()
            {
                let csv = export_metrics_csv(
                    device,
                    state.selected_interface.as_deref(),
                    hide_virtual_interfaces,
                );
                ui.ctx().copy_text(csv);
                state.feedback = Some((
                    "CSV copié dans le presse-papier".to_string(),
                    Instant::now() + Duration::from_millis(2500),
                ));
            }
        };
        let draw_summary = |ui: &mut Ui, state: &mut MetricsPanelState| {
            if ui
                .button("📋 Résumé")
                .on_hover_text("Copier un résumé de diagnostic complet")
                .clicked()
            {
                let summary =
                    format_server_summary(device, hide_virtual_disks, hide_virtual_interfaces);
                ui.ctx().copy_text(summary);
                state.feedback = Some((
                    "Résumé copié dans le presse-papier".to_string(),
                    Instant::now() + Duration::from_millis(2500),
                ));
            }
        };

        if rtl {
            draw_summary(ui, state);
            draw_csv(ui, state);
            draw_clear(ui, action);
            draw_thresh(ui, state);
            draw_pause(ui, action);
        } else {
            draw_pause(ui, action);
            draw_thresh(ui, state);
            draw_clear(ui, action);
            draw_csv(ui, state);
            draw_summary(ui, state);
        }
    };

    theme::subtle_bar_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if avail_w >= 820.0 {
            ui.horizontal(|ui| {
                draw_left(ui, state);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    draw_right(ui, true, state, action);
                });
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                draw_left(ui, state);
            });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                draw_right(ui, false, state, action);
            });
        }
    });
}

fn render_alert_strip(
    ui: &mut Ui,
    p: &Palette,
    alerts: &[ServerAlert],
    action: &mut Option<MetricsPanelAction>,
) {
    let any_critical = alerts.iter().any(|a| a.critical);
    let (bg, border, fg) = if any_critical {
        (p.danger_bg, p.danger_border, p.danger)
    } else {
        (p.warning_bg, p.warning_border, p.warning)
    };
    let joined = alerts
        .iter()
        .map(|a| a.message.as_str())
        .collect::<Vec<_>>()
        .join("  •  ");
    let text = format!("⚠ Alertes actives : {joined}");
    let avail_w = ui.available_width();

    egui::Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0_f32, border))
        .corner_radius(7)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let draw_shortcuts = |ui: &mut Ui, rtl: bool, action: &mut Option<MetricsPanelAction>| {
                let draw_term = |ui: &mut Ui, action: &mut Option<MetricsPanelAction>| {
                    if ui.small_button("💻 Terminal").clicked() {
                        *action = Some(MetricsPanelAction::OpenTerminal);
                    }
                };
                let draw_proc = |ui: &mut Ui, action: &mut Option<MetricsPanelAction>| {
                    if ui.small_button("⚡ Processus").clicked() {
                        *action = Some(MetricsPanelAction::OpenProcesses);
                    }
                };
                if rtl {
                    draw_term(ui, action);
                    draw_proc(ui, action);
                } else {
                    draw_proc(ui, action);
                    draw_term(ui, action);
                }
            };

            if avail_w >= 720.0 {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&text).size(12.0).strong().color(fg))
                            .truncate(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        draw_shortcuts(ui, true, action);
                    });
                });
            } else {
                ui.label(RichText::new(&text).size(12.0).strong().color(fg));
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    draw_shortcuts(ui, false, action);
                });
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn render_kpi_strip(
    ui: &mut Ui,
    p: &Palette,
    state: &MetricsPanelState,
    device: &Device,
    cur_cpu: f64,
    cpu_slice: &[f64],
    cur_ram: f64,
    ram_used_bytes: u64,
    ram_total_bytes: u64,
    ram_slice: &[f64],
    visible_disks: &[DiskRow],
    cur_disk_r: f64,
    cur_disk_w: f64,
    cur_rx: f64,
    cur_tx: f64,
    net_rx_series: &[f64],
    net_tx_series: &[f64],
    hide_virtual_interfaces: bool,
) {
    let avail_w = ui.available_width();

    if avail_w >= 880.0 {
        ui.columns(4, |cols| {
            render_kpi_cpu(&mut cols[0], p, device, cur_cpu, cpu_slice);
            render_kpi_ram(
                &mut cols[1],
                p,
                cur_ram,
                ram_used_bytes,
                ram_total_bytes,
                ram_slice,
            );
            render_kpi_storage(&mut cols[2], p, visible_disks, cur_disk_r, cur_disk_w);
            render_kpi_network(
                &mut cols[3],
                p,
                state,
                cur_rx,
                cur_tx,
                net_rx_series,
                net_tx_series,
                hide_virtual_interfaces,
            );
        });
    } else if avail_w >= 480.0 {
        ui.columns(2, |cols| {
            render_kpi_cpu(&mut cols[0], p, device, cur_cpu, cpu_slice);
            render_kpi_ram(
                &mut cols[1],
                p,
                cur_ram,
                ram_used_bytes,
                ram_total_bytes,
                ram_slice,
            );
        });
        ui.add_space(8.0);
        ui.columns(2, |cols| {
            render_kpi_storage(&mut cols[0], p, visible_disks, cur_disk_r, cur_disk_w);
            render_kpi_network(
                &mut cols[1],
                p,
                state,
                cur_rx,
                cur_tx,
                net_rx_series,
                net_tx_series,
                hide_virtual_interfaces,
            );
        });
    } else {
        render_kpi_cpu(ui, p, device, cur_cpu, cpu_slice);
        ui.add_space(8.0);
        render_kpi_ram(
            ui,
            p,
            cur_ram,
            ram_used_bytes,
            ram_total_bytes,
            ram_slice,
        );
        ui.add_space(8.0);
        render_kpi_storage(ui, p, visible_disks, cur_disk_r, cur_disk_w);
        ui.add_space(8.0);
        render_kpi_network(
            ui,
            p,
            state,
            cur_rx,
            cur_tx,
            net_rx_series,
            net_tx_series,
            hide_virtual_interfaces,
        );
    }
}

fn tail_values(values: &[f64], limit: usize) -> &[f64] {
    let start = values.len().saturating_sub(limit);
    &values[start..]
}

fn render_kpi_cpu(
    ui: &mut Ui,
    p: &Palette,
    device: &Device,
    cur_cpu: f64,
    cpu_slice: &[f64],
) {
    let cpu_cores = device.system_info.as_ref().map(|s| s.cpu_count).unwrap_or(0);
    let col = theme::usage_color(p, cur_cpu);
    let stats = compute_series_stats(cpu_slice);

    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(94.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("PROCESSEUR")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            if cpu_cores > 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{cpu_cores} cœurs"))
                            .size(10.5)
                            .color(p.text_muted),
                    );
                });
            }
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{cur_cpu:.1}%"))
                    .size(21.0)
                    .strong()
                    .color(col),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::sparkline(
                    ui,
                    p,
                    tail_values(cpu_slice, SPARKLINE_POINTS),
                    0.0,
                    100.0,
                    col,
                    68.0,
                    22.0,
                );
            });
        });

        ui.add_space(4.0);
        theme::mini_progress_bar(ui, p, (cur_cpu / 100.0) as f32, col, ui.available_width(), 5.0);
        ui.add_space(4.0);

        let caption = match stats {
            Some(s) => format!("Moy {:.1}% • Pic {:.1}%", s.avg, s.max),
            None => "En attente de données".to_string(),
        };
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn render_kpi_ram(
    ui: &mut Ui,
    p: &Palette,
    cur_ram: f64,
    ram_used_bytes: u64,
    ram_total_bytes: u64,
    ram_slice: &[f64],
) {
    let val_col = if cur_ram >= ALERT_THRESHOLD_PCT {
        p.danger
    } else {
        p.purple
    };
    let stats = compute_series_stats(ram_slice);

    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(94.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("MÉMOIRE VIVE")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            if ram_total_bytes > 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} / {}",
                            theme::format_storage(ram_used_bytes),
                            theme::format_storage(ram_total_bytes)
                        ))
                        .size(10.5)
                        .color(p.text_muted),
                    );
                });
            }
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{cur_ram:.1}%"))
                    .size(21.0)
                    .strong()
                    .color(val_col),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::sparkline(
                    ui,
                    p,
                    tail_values(ram_slice, SPARKLINE_POINTS),
                    0.0,
                    100.0,
                    p.purple,
                    68.0,
                    22.0,
                );
            });
        });

        ui.add_space(4.0);
        theme::mini_progress_bar(
            ui,
            p,
            (cur_ram / 100.0) as f32,
            val_col,
            ui.available_width(),
            5.0,
        );
        ui.add_space(4.0);

        let avail = ram_total_bytes.saturating_sub(ram_used_bytes);
        let caption = match (ram_total_bytes > 0, stats) {
            (true, Some(s)) => format!("{} libre • Pic {:.1}%", theme::format_storage(avail), s.max),
            (true, None) => format!("{} libre", theme::format_storage(avail)),
            (false, Some(s)) => format!("Moy {:.1}% • Pic {:.1}%", s.avg, s.max),
            (false, None) => "En attente de données".to_string(),
        };
        ui.add(egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate());
    });
}

fn select_primary_disk(visible_disks: &[DiskRow]) -> Option<&DiskRow> {
    visible_disks
        .iter()
        .filter(|d| d.pct >= ALERT_THRESHOLD_PCT)
        .max_by(|a, b| {
            a.pct
                .partial_cmp(&b.pct)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .or_else(|| {
            visible_disks.iter().find(|d| {
                let mp = d.mount_point.trim();
                mp == "/" || mp.eq_ignore_ascii_case("C:\\") || mp.eq_ignore_ascii_case("C:/")
            })
        })
        .or_else(|| visible_disks.iter().max_by_key(|d| d.total_bytes))
}

fn render_kpi_storage(
    ui: &mut Ui,
    p: &Palette,
    visible_disks: &[DiskRow],
    cur_disk_r: f64,
    cur_disk_w: f64,
) {
    let primary = select_primary_disk(visible_disks);
    let tooltip = if visible_disks.is_empty() {
        "Aucun volume visible".to_string()
    } else {
        visible_disks
            .iter()
            .map(|d| {
                format!(
                    "{} ({}) : {} / {} ({:.0}%)",
                    d.mount_point,
                    d.filesystem,
                    theme::format_storage(d.used_bytes),
                    theme::format_storage(d.total_bytes),
                    d.pct
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let resp = theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(94.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("STOCKAGE & I/O")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            if let Some(d) = primary {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&d.mount_point)
                                .monospace()
                                .size(10.5)
                                .color(p.text_muted),
                        )
                        .truncate(),
                    );
                });
            }
        });

        ui.add_space(4.0);
        if let Some(d) = primary {
            let col = theme::usage_color(p, d.pct);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{:.0}%", d.pct))
                        .size(21.0)
                        .strong()
                        .color(col),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(format!("É {}", theme::format_bytes_rate(cur_disk_w)))
                                .size(10.5)
                                .color(p.orange),
                        );
                        ui.label(
                            RichText::new(format!("L {}", theme::format_bytes_rate(cur_disk_r)))
                                .size(10.5)
                                .color(p.accent),
                        );
                    });
                });
            });

            ui.add_space(4.0);
            theme::mini_progress_bar(
                ui,
                p,
                (d.pct / 100.0) as f32,
                col,
                ui.available_width(),
                5.0,
            );
            ui.add_space(4.0);

            let caption = if visible_disks.len() > 1 {
                format!(
                    "{} / {} ({} vol.)",
                    theme::format_storage(d.used_bytes),
                    theme::format_storage(d.total_bytes),
                    visible_disks.len()
                )
            } else {
                format!(
                    "{} / {}",
                    theme::format_storage(d.used_bytes),
                    theme::format_storage(d.total_bytes)
                )
            };
            ui.add(
                egui::Label::new(RichText::new(caption).size(11.0).color(p.text_muted)).truncate(),
            );
        } else {
            ui.label(RichText::new("—").size(21.0).strong().color(p.text_muted));
            ui.add_space(4.0);
            theme::mini_progress_bar(ui, p, 0.0, p.text_muted, ui.available_width(), 5.0);
            ui.add_space(4.0);
            ui.label(
                RichText::new("Aucun volume visible")
                    .size(11.0)
                    .color(p.text_muted),
            );
        }
    });

    resp.response.on_hover_text(tooltip);
}

#[allow(clippy::too_many_arguments)]
fn render_kpi_network(
    ui: &mut Ui,
    p: &Palette,
    state: &MetricsPanelState,
    cur_rx: f64,
    cur_tx: f64,
    net_rx_series: &[f64],
    net_tx_series: &[f64],
    hide_virtual_interfaces: bool,
) {
    let combined: Vec<f64> = net_rx_series
        .iter()
        .zip(net_tx_series.iter())
        .map(|(&r, &t)| r + t)
        .collect();
    let tail = tail_values(&combined, SPARKLINE_POINTS);
    let peak_comb = combined.iter().copied().fold(0.0_f64, f64::max).max(1.0);
    let max_rx = net_rx_series.iter().copied().fold(0.0_f64, f64::max);
    let max_tx = net_tx_series.iter().copied().fold(0.0_f64, f64::max);
    let cur_comb = cur_rx + cur_tx;

    let scope = match &state.selected_interface {
        Some(iface) => iface.as_str(),
        None if hide_virtual_interfaces => "physiques",
        None => "toutes",
    };

    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(94.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("RÉSEAU ACTIF")
                    .size(10.5)
                    .strong()
                    .color(p.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(RichText::new(scope).size(10.5).color(p.text_muted))
                        .truncate(),
                );
            });
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(format!("↓ {}", theme::format_bytes_rate(cur_rx)))
                        .size(13.0)
                        .strong()
                        .color(p.success),
                );
                ui.label(
                    RichText::new(format!("↑ {}", theme::format_bytes_rate(cur_tx)))
                        .size(12.5)
                        .strong()
                        .color(p.orange),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::sparkline(ui, p, tail, 0.0, peak_comb, p.accent, 68.0, 22.0);
            });
        });

        ui.add_space(3.0);
        let ratio = (cur_comb / peak_comb).clamp(0.0, 1.0) as f32;
        theme::mini_progress_bar(ui, p, ratio, p.success, ui.available_width(), 5.0);
        ui.add_space(4.0);

        ui.add(
            egui::Label::new(
                RichText::new(format!(
                    "Pic ↓ {} • ↑ {}",
                    theme::format_bytes_rate(max_rx),
                    theme::format_bytes_rate(max_tx)
                ))
                .size(11.0)
                .color(p.text_muted),
            )
            .truncate(),
        );
    });
}

#[allow(clippy::too_many_arguments)]
fn render_charts_grid(
    ui: &mut Ui,
    p: &Palette,
    state: &mut MetricsPanelState,
    device: &Device,
    step_secs: f64,
    window_secs: f64,
    cur_cpu: f64,
    cpu_slice: &[f64],
    cur_ram: f64,
    ram_used_bytes: u64,
    ram_total_bytes: u64,
    ram_slice: &[f64],
    net_rx_series: &[f64],
    net_tx_series: &[f64],
    disk_r_slice: &[f64],
    disk_w_slice: &[f64],
    hide_virtual_interfaces: bool,
) {
    let avail_w = ui.available_width();
    if avail_w >= 800.0 {
        ui.columns(2, |cols| {
            render_cpu_plot_card(
                &mut cols[0],
                p,
                state.show_thresholds,
                step_secs,
                window_secs,
                cur_cpu,
                cpu_slice,
            );
            render_ram_plot_card(
                &mut cols[1],
                p,
                state.show_thresholds,
                step_secs,
                window_secs,
                cur_ram,
                ram_used_bytes,
                ram_total_bytes,
                ram_slice,
            );
        });
        ui.add_space(10.0);
        ui.columns(2, |cols| {
            render_network_plot_card(
                &mut cols[0],
                p,
                state,
                device,
                step_secs,
                window_secs,
                net_rx_series,
                net_tx_series,
                hide_virtual_interfaces,
            );
            render_disk_io_plot_card(
                &mut cols[1],
                p,
                step_secs,
                window_secs,
                disk_r_slice,
                disk_w_slice,
            );
        });
    } else {
        render_cpu_plot_card(
            ui,
            p,
            state.show_thresholds,
            step_secs,
            window_secs,
            cur_cpu,
            cpu_slice,
        );
        ui.add_space(10.0);
        render_ram_plot_card(
            ui,
            p,
            state.show_thresholds,
            step_secs,
            window_secs,
            cur_ram,
            ram_used_bytes,
            ram_total_bytes,
            ram_slice,
        );
        ui.add_space(10.0);
        render_network_plot_card(
            ui,
            p,
            state,
            device,
            step_secs,
            window_secs,
            net_rx_series,
            net_tx_series,
            hide_virtual_interfaces,
        );
        ui.add_space(10.0);
        render_disk_io_plot_card(ui, p, step_secs, window_secs, disk_r_slice, disk_w_slice);
    }
}

fn build_points(values: &[f64], step_secs: f64, scale: f64) -> Vec<[f64; 2]> {
    let n = values.len();
    values
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let sec_ago = if n > 0 {
                -((n - 1 - i) as f64 * step_secs)
            } else {
                0.0
            };
            [sec_ago, v * scale]
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn render_percent_plot(
    ui: &mut Ui,
    p: &Palette,
    plot_id: &str,
    series_name: &str,
    color: Color32,
    values: &[f64],
    step_secs: f64,
    window_secs: f64,
    show_thresholds: bool,
) {
    let points = build_points(values, step_secs, 1.0);
    let line = Line::new(PlotPoints::new(points))
        .name(series_name)
        .color(color)
        .fill(0.0_f32)
        .width(2.0_f32);

    Plot::new(plot_id)
        .height(172.0)
        .include_x(-window_secs)
        .include_x(0.0)
        .include_y(0.0)
        .include_y(100.0)
        .allow_zoom(false)
        .allow_drag(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .x_axis_formatter(|mark, _range| format_axis_seconds(mark.value))
        .y_axis_formatter(|mark, _range| format!("{:.0}%", mark.value))
        .label_formatter(|name, value| {
            let t = format_axis_seconds(value.x);
            if name.is_empty() {
                format!("{t}\n{:.1}%", value.y)
            } else {
                format!("{name}\n{t} : {:.1}%", value.y)
            }
        })
        .legend(Legend::default().position(Corner::LeftTop))
        .show(ui, |plot_ui| {
            if show_thresholds {
                plot_ui.hline(
                    HLine::new(ALERT_THRESHOLD_PCT)
                        .color(p.danger.gamma_multiply(0.55))
                        .style(LineStyle::dashed_dense())
                        .width(1.0_f32)
                        .allow_hover(false),
                );
            }
            plot_ui.line(line);
        });
}

fn render_cpu_plot_card(
    ui: &mut Ui,
    p: &Palette,
    show_thresholds: bool,
    step_secs: f64,
    window_secs: f64,
    cur_cpu: f64,
    cpu_slice: &[f64],
) {
    let stats = compute_series_stats(cpu_slice);
    theme::card_frame(p).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("📈 Processeur (%)")
                    .strong()
                    .size(13.5)
                    .color(p.text_primary),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{cur_cpu:.1}%"))
                        .strong()
                        .size(12.5)
                        .color(theme::usage_color(p, cur_cpu)),
                );
                if let Some(s) = stats {
                    ui.label(
                        RichText::new(format!(
                            "Min {:.0}% • Moy {:.0}% • Max {:.0}%",
                            s.min, s.avg, s.max
                        ))
                        .size(11.0)
                        .color(p.text_muted),
                    );
                }
            });
        });
        ui.add_space(4.0);
        render_percent_plot(
            ui,
            p,
            "cpu_graph",
            "CPU %",
            p.accent,
            cpu_slice,
            step_secs,
            window_secs,
            show_thresholds,
        );
    });
}

#[allow(clippy::too_many_arguments)]
fn render_ram_plot_card(
    ui: &mut Ui,
    p: &Palette,
    show_thresholds: bool,
    step_secs: f64,
    window_secs: f64,
    cur_ram: f64,
    ram_used_bytes: u64,
    ram_total_bytes: u64,
    ram_slice: &[f64],
) {
    let stats = compute_series_stats(ram_slice);
    theme::card_frame(p).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("📈 Mémoire RAM (%)")
                    .strong()
                    .size(13.5)
                    .color(p.text_primary),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{cur_ram:.1}%"))
                        .strong()
                        .size(12.5)
                        .color(p.purple),
                );
                let mut right_parts = Vec::new();
                if ram_total_bytes > 0 {
                    right_parts.push(format!(
                        "{}/{}",
                        theme::format_storage(ram_used_bytes),
                        theme::format_storage(ram_total_bytes)
                    ));
                }
                if let Some(s) = stats {
                    right_parts.push(format!(
                        "Min {:.0}% • Moy {:.0}% • Max {:.0}%",
                        s.min, s.avg, s.max
                    ));
                }
                if !right_parts.is_empty() {
                    ui.label(
                        RichText::new(right_parts.join("  •  "))
                            .size(11.0)
                            .color(p.text_muted),
                    );
                }
            });
        });
        ui.add_space(4.0);
        render_percent_plot(
            ui,
            p,
            "ram_graph",
            "RAM %",
            p.purple,
            ram_slice,
            step_secs,
            window_secs,
            show_thresholds,
        );
    });
}

#[allow(clippy::too_many_arguments)]
fn render_network_plot_card(
    ui: &mut Ui,
    p: &Palette,
    state: &mut MetricsPanelState,
    device: &Device,
    step_secs: f64,
    window_secs: f64,
    net_rx_series: &[f64],
    net_tx_series: &[f64],
    hide_virtual_interfaces: bool,
) {
    let peak_bps = net_rx_series
        .iter()
        .chain(net_tx_series.iter())
        .copied()
        .fold(0.0_f64, f64::max);
    let (divisor, unit) = choose_rate_unit(peak_bps);
    let max_rx = net_rx_series.iter().copied().fold(0.0_f64, f64::max);
    let max_tx = net_tx_series.iter().copied().fold(0.0_f64, f64::max);

    let ifaces = available_network_interfaces(
        device,
        state.selected_interface.as_deref(),
        hide_virtual_interfaces,
    );
    let all_label = if hide_virtual_interfaces {
        "Toutes (physiques)"
    } else {
        "Toutes"
    };
    let selected_lbl = state
        .selected_interface
        .as_deref()
        .unwrap_or(all_label)
        .to_string();

    theme::card_frame(p).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("🌐 Débit Réseau ({unit})"))
                    .strong()
                    .size(13.5)
                    .color(p.text_primary),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::ComboBox::from_id_salt("metrics_net_iface_combo")
                    .selected_text(&selected_lbl)
                    .width(135.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(state.selected_interface.is_none(), all_label)
                            .clicked()
                        {
                            state.selected_interface = None;
                        }
                        for iface in &ifaces {
                            let is_sel = state.selected_interface.as_deref() == Some(iface.as_str());
                            if ui.selectable_label(is_sel, iface).clicked() {
                                state.selected_interface = Some(iface.clone());
                            }
                        }
                    });
                ui.label(
                    RichText::new(format!(
                        "Pic ↓ {} • ↑ {}",
                        theme::format_bytes_rate(max_rx),
                        theme::format_bytes_rate(max_tx)
                    ))
                    .size(11.0)
                    .color(p.text_muted),
                );
            });
        });
        ui.add_space(4.0);

        let scale = 1.0 / divisor;
        let rx_points = build_points(net_rx_series, step_secs, scale);
        let tx_points = build_points(net_tx_series, step_secs, scale);
        let top_y = (peak_bps * scale).max(if divisor == 1.0 { 10.0 } else { 1.0 }) * 1.15;

        let rx_line = Line::new(PlotPoints::new(rx_points))
            .name(format!("Entrant ↓ ({unit})"))
            .color(p.success)
            .fill(0.0_f32)
            .width(2.0_f32);
        let tx_line = Line::new(PlotPoints::new(tx_points))
            .name(format!("Sortant ↑ ({unit})"))
            .color(p.orange)
            .width(2.0_f32);

        Plot::new("net_graph")
            .height(172.0)
            .include_x(-window_secs)
            .include_x(0.0)
            .include_y(0.0)
            .include_y(top_y)
            .allow_zoom(false)
            .allow_drag(false)
            .allow_scroll(false)
            .allow_boxed_zoom(false)
            .x_axis_formatter(|mark, _range| format_axis_seconds(mark.value))
            .y_axis_formatter(move |mark, _range| {
                if mark.value.abs() >= 10.0 || mark.value == 0.0 {
                    format!("{:.0} {unit}", mark.value)
                } else {
                    format!("{:.1} {unit}", mark.value)
                }
            })
            .label_formatter(move |name, value| {
                let t = format_axis_seconds(value.x);
                if name.is_empty() {
                    format!("{t}\n{:.2} {unit}", value.y)
                } else {
                    format!("{name}\n{t} : {:.2} {unit}", value.y)
                }
            })
            .legend(Legend::default().position(Corner::LeftTop))
            .show(ui, |plot_ui| {
                plot_ui.line(rx_line);
                plot_ui.line(tx_line);
            });
    });
}

fn render_disk_io_plot_card(
    ui: &mut Ui,
    p: &Palette,
    step_secs: f64,
    window_secs: f64,
    disk_r_slice: &[f64],
    disk_w_slice: &[f64],
) {
    let peak_bps = disk_r_slice
        .iter()
        .chain(disk_w_slice.iter())
        .copied()
        .fold(0.0_f64, f64::max);
    let (divisor, unit) = choose_rate_unit(peak_bps);
    let max_r = disk_r_slice.iter().copied().fold(0.0_f64, f64::max);
    let max_w = disk_w_slice.iter().copied().fold(0.0_f64, f64::max);

    theme::card_frame(p).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("💾 Activité I/O Disque ({unit})"))
                    .strong()
                    .size(13.5)
                    .color(p.text_primary),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!(
                        "Pic L {} • É {}",
                        theme::format_bytes_rate(max_r),
                        theme::format_bytes_rate(max_w)
                    ))
                    .size(11.0)
                    .color(p.text_muted),
                );
            });
        });
        ui.add_space(4.0);

        let scale = 1.0 / divisor;
        let r_points = build_points(disk_r_slice, step_secs, scale);
        let w_points = build_points(disk_w_slice, step_secs, scale);
        let top_y = (peak_bps * scale).max(if divisor == 1.0 { 10.0 } else { 1.0 }) * 1.15;

        let r_line = Line::new(PlotPoints::new(r_points))
            .name(format!("Lecture ({unit})"))
            .color(p.accent)
            .fill(0.0_f32)
            .width(2.0_f32);
        let w_line = Line::new(PlotPoints::new(w_points))
            .name(format!("Écriture ({unit})"))
            .color(p.orange)
            .width(2.0_f32);

        Plot::new("disk_io_graph")
            .height(172.0)
            .include_x(-window_secs)
            .include_x(0.0)
            .include_y(0.0)
            .include_y(top_y)
            .allow_zoom(false)
            .allow_drag(false)
            .allow_scroll(false)
            .allow_boxed_zoom(false)
            .x_axis_formatter(|mark, _range| format_axis_seconds(mark.value))
            .y_axis_formatter(move |mark, _range| {
                if mark.value.abs() >= 10.0 || mark.value == 0.0 {
                    format!("{:.0} {unit}", mark.value)
                } else {
                    format!("{:.1} {unit}", mark.value)
                }
            })
            .label_formatter(move |name, value| {
                let t = format_axis_seconds(value.x);
                if name.is_empty() {
                    format!("{t}\n{:.2} {unit}", value.y)
                } else {
                    format!("{name}\n{t} : {:.2} {unit}", value.y)
                }
            })
            .legend(Legend::default().position(Corner::LeftTop))
            .show(ui, |plot_ui| {
                plot_ui.line(r_line);
                plot_ui.line(w_line);
            });
    });
}

fn render_disks_card(
    ui: &mut Ui,
    p: &Palette,
    visible_disks: &[DiskRow],
    total_disks: usize,
    hide_virtual_disks: bool,
    action: &mut Option<MetricsPanelAction>,
) {
    if total_disks == 0 && visible_disks.is_empty() {
        return;
    }

    let sum_used: u64 = visible_disks
        .iter()
        .fold(0u64, |acc, d| acc.saturating_add(d.used_bytes));
    let sum_total: u64 = visible_disks
        .iter()
        .fold(0u64, |acc, d| acc.saturating_add(d.total_bytes));
    let sum_pct = if sum_total > 0 {
        (sum_used as f64 / sum_total as f64) * 100.0
    } else {
        0.0
    };

    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let avail_w = ui.available_width();

        let draw_title = |ui: &mut Ui| {
            ui.label(
                RichText::new("💾 Stockage & Disques")
                    .strong()
                    .size(14.0)
                    .color(p.text_primary),
            );
            theme::pill_badge(
                ui,
                &format!("{}/{total_disks}", visible_disks.len()),
                p.bg_subtle,
                p.text_secondary,
                Some(p.border),
            );
            if sum_total > 0 {
                ui.label(
                    RichText::new(format!(
                        "{} / {} ({sum_pct:.0}%)",
                        theme::format_storage(sum_used),
                        theme::format_storage(sum_total)
                    ))
                    .size(12.0)
                    .color(p.text_secondary),
                );
            }
        };

        let draw_toggle = |ui: &mut Ui, action: &mut Option<MetricsPanelAction>| {
            let mut hide = hide_virtual_disks;
            if ui
                .checkbox(&mut hide, "Masquer virtuels (tmpfs/loop/boot)")
                .clicked()
            {
                *action = Some(MetricsPanelAction::ToggleHideVirtualDisks);
            }
        };

        if avail_w >= 620.0 {
            ui.horizontal(|ui| {
                draw_title(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    draw_toggle(ui, action);
                });
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                draw_title(ui);
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                draw_toggle(ui, action);
            });
        }

        ui.add_space(8.0);

        if visible_disks.is_empty() {
            ui.label(
                RichText::new(
                    "Aucun volume physique visible (décochez « Masquer virtuels » pour tout afficher).",
                )
                .size(12.0)
                .color(p.text_secondary),
            );
            return;
        }

        let num_cols = if avail_w >= 780.0 { 2 } else { 1 };
        for (row_idx, chunk) in visible_disks.chunks(num_cols).enumerate() {
            if row_idx > 0 {
                ui.add_space(8.0);
            }
            if num_cols == 1 {
                render_disk_mini_card(ui, p, &chunk[0]);
            } else {
                ui.columns(2, |cols| {
                    for (i, disk) in chunk.iter().enumerate() {
                        render_disk_mini_card(&mut cols[i], p, disk);
                    }
                });
            }
        }
    });
}

fn render_disk_mini_card(ui: &mut Ui, p: &Palette, disk: &DiskRow) {
    let pct_col = theme::usage_color(p, disk.pct);
    theme::subtle_bar_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());

        // Row 1: Mount point + filesystem pill + right-aligned percentage
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 20.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.label(
                    RichText::new(format!("{:.0}%", disk.pct))
                        .size(13.0)
                        .strong()
                        .color(pct_col),
                );
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&disk.mount_point)
                                .monospace()
                                .strong()
                                .size(12.5)
                                .color(p.text_primary),
                        )
                        .truncate(),
                    );
                    theme::pill_badge(
                        ui,
                        &disk.filesystem,
                        p.bg_card,
                        p.text_muted,
                        Some(p.border),
                    );
                });
            },
        );

        ui.add_space(5.0);
        theme::mini_progress_bar(
            ui,
            p,
            (disk.pct / 100.0) as f32,
            pct_col,
            ui.available_width(),
            7.0,
        );
        ui.add_space(5.0);

        // Row 3: Capacity breakdown + live I/O
        let cap_text = format!(
            "{} utilisés • {} libres / {}",
            theme::format_storage(disk.used_bytes),
            theme::format_storage(disk.avail_bytes),
            theme::format_storage(disk.total_bytes)
        );
        let has_io = disk.read_bps.is_some() || disk.write_bps.is_some();

        if ui.available_width() >= 340.0 && has_io {
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(RichText::new(&cap_text).size(11.0).color(p.text_secondary))
                        .truncate(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let w = disk.write_bps.unwrap_or(0) as f64;
                    let r = disk.read_bps.unwrap_or(0) as f64;
                    ui.label(
                        RichText::new(format!("É {}", theme::format_bytes_rate(w)))
                            .size(11.0)
                            .color(p.orange),
                    );
                    ui.label(RichText::new("•").size(10.5).color(p.text_muted));
                    ui.label(
                        RichText::new(format!("L {}", theme::format_bytes_rate(r)))
                            .size(11.0)
                            .color(p.accent),
                    );
                });
            });
        } else {
            ui.add(
                egui::Label::new(RichText::new(&cap_text).size(11.0).color(p.text_secondary))
                    .truncate(),
            );
            if has_io {
                let w = disk.write_bps.unwrap_or(0) as f64;
                let r = disk.read_bps.unwrap_or(0) as f64;
                ui.label(
                    RichText::new(format!(
                        "L {}  •  É {}",
                        theme::format_bytes_rate(r),
                        theme::format_bytes_rate(w)
                    ))
                    .size(11.0)
                    .color(p.text_muted),
                );
            }
        }
    });
}

fn render_networks_card(
    ui: &mut Ui,
    p: &Palette,
    state: &mut MetricsPanelState,
    device: &Device,
    hide_virtual_interfaces: bool,
    action: &mut Option<MetricsPanelAction>,
) {
    let Some(info) = &device.system_info else {
        return;
    };
    if info.networks.is_empty() {
        return;
    }

    let total_ifaces = info.networks.len();
    let filtered_nets: Vec<_> = info
        .networks
        .iter()
        .filter(|n| !hide_virtual_interfaces || !is_virtual_interface(&n.name))
        .collect();

    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let avail_w = ui.available_width();

        let draw_title = |ui: &mut Ui| {
            ui.label(
                RichText::new("🌐 Interfaces Réseau")
                    .strong()
                    .size(14.0)
                    .color(p.text_primary),
            );
            theme::pill_badge(
                ui,
                &format!("{}/{total_ifaces}", filtered_nets.len()),
                p.bg_subtle,
                p.text_secondary,
                Some(p.border),
            );
        };

        let draw_toggle = |ui: &mut Ui, action: &mut Option<MetricsPanelAction>| {
            let mut hide = hide_virtual_interfaces;
            if ui
                .checkbox(&mut hide, "Masquer virtuelles (lo/docker/veth)")
                .clicked()
            {
                *action = Some(MetricsPanelAction::ToggleHideVirtualInterfaces);
            }
        };

        if avail_w >= 640.0 {
            ui.horizontal(|ui| {
                draw_title(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    draw_toggle(ui, action);
                });
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                draw_title(ui);
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                draw_toggle(ui, action);
            });
        }

        ui.add_space(8.0);

        if filtered_nets.is_empty() {
            ui.label(
                RichText::new(
                    "Aucune interface physique visible (décochez « Masquer virtuelles » pour tout afficher).",
                )
                .size(12.0)
                .color(p.text_secondary),
            );
            return;
        }

        egui::ScrollArea::horizontal()
            .id_salt("metrics_networks_hscroll")
            .show(ui, |ui| {
                egui::Grid::new("networks_grid")
                    .striped(true)
                    .min_col_width(110.0)
                    .spacing([20.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Interface").strong().color(p.text_secondary));
                        ui.label(RichText::new("Adresse MAC").strong().color(p.text_secondary));
                        ui.label(RichText::new("Adresses IP").strong().color(p.text_secondary));
                        ui.label(RichText::new("Débit actuel").strong().color(p.text_secondary));
                        ui.end_row();

                        for net in filtered_nets {
                            let rates = device.latest_metrics.as_ref().and_then(|m| {
                                m.network_metrics
                                    .iter()
                                    .find(|nm| nm.interface_name == net.name)
                                    .map(|nm| (nm.rx_bytes_per_sec, nm.tx_bytes_per_sec))
                            });
                            let has_traffic = rates.is_some_and(|(r, t)| r > 0 || t > 0);
                            let is_filtered =
                                state.selected_interface.as_deref() == Some(net.name.as_str());

                            // 1. Interface + live dot + filter button
                            ui.horizontal(|ui| {
                                let dot_col = if has_traffic { p.success } else { p.text_muted };
                                let (dot_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(8.0, 8.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().circle_filled(dot_rect.center(), 3.5, dot_col);
                                ui.label(RichText::new(&net.name).strong().color(p.text_primary));

                                let filter_btn = egui::Button::new(
                                    RichText::new("📈").size(11.0).color(if is_filtered {
                                        p.accent
                                    } else {
                                        p.text_secondary
                                    }),
                                )
                                .fill(if is_filtered {
                                    p.accent_bg
                                } else {
                                    p.bg_subtle
                                })
                                .stroke(Stroke::new(
                                    1.0_f32,
                                    if is_filtered {
                                        p.accent_border
                                    } else {
                                        p.border
                                    },
                                ))
                                .corner_radius(5);

                                if ui
                                    .add(filter_btn)
                                    .on_hover_text(if is_filtered {
                                        "Afficher toutes les interfaces sur le graphe"
                                    } else {
                                        "Isoler cette interface sur le graphe réseau"
                                    })
                                    .clicked()
                                {
                                    if is_filtered {
                                        state.selected_interface = None;
                                    } else {
                                        state.selected_interface = Some(net.name.clone());
                                    }
                                }
                            });

                            // 2. MAC
                            ui.label(
                                RichText::new(if net.mac_address.is_empty() {
                                    "—".to_string()
                                } else {
                                    net.mac_address.clone()
                                })
                                .monospace()
                                .color(p.text_secondary),
                            );

                            // 3. Copyable IP chips
                            if net.ip_addresses.is_empty() {
                                ui.label(RichText::new("—").color(p.text_muted));
                            } else {
                                ui.horizontal(|ui| {
                                    for ip in &net.ip_addresses {
                                        let is_copied = state.copied_ip.as_ref().is_some_and(
                                            |(val, until)| val == ip && Instant::now() < *until,
                                        );
                                        let (lbl, col) = if is_copied {
                                            (format!("✔ {ip}"), p.success)
                                        } else {
                                            (format!("📋 {ip}"), p.text_primary)
                                        };
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new(lbl)
                                                        .monospace()
                                                        .size(11.5)
                                                        .color(col),
                                                )
                                                .fill(p.bg_subtle)
                                                .stroke(Stroke::new(1.0_f32, p.border))
                                                .corner_radius(5),
                                            )
                                            .on_hover_text("Cliquer pour copier cette adresse IP")
                                            .clicked()
                                        {
                                            ui.ctx().copy_text(ip.clone());
                                            state.copied_ip = Some((
                                                ip.clone(),
                                                Instant::now() + Duration::from_secs(2),
                                            ));
                                        }
                                    }
                                });
                            }

                            // 4. Colored live throughput
                            if let Some((rx, tx)) = rates {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!(
                                            "↓ {}",
                                            theme::format_bytes_rate(rx as f64)
                                        ))
                                        .size(11.5)
                                        .color(if rx > 0 { p.success } else { p.text_muted }),
                                    );
                                    ui.label(RichText::new("/").size(11.0).color(p.text_muted));
                                    ui.label(
                                        RichText::new(format!(
                                            "↑ {}",
                                            theme::format_bytes_rate(tx as f64)
                                        ))
                                        .size(11.5)
                                        .color(if tx > 0 { p.orange } else { p.text_muted }),
                                    );
                                });
                            } else {
                                ui.label(RichText::new("—").color(p.text_muted));
                            }

                            ui.end_row();
                        }
                    });
            });
    });
}

fn render_spec_tile(ui: &mut Ui, p: &Palette, title: &str, main_val: &str, sub_val: &str) {
    theme::subtle_bar_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(56.0);
        ui.label(
            RichText::new(title)
                .size(10.0)
                .strong()
                .color(p.text_muted),
        );
        ui.add_space(2.0);
        ui.add(
            egui::Label::new(
                RichText::new(main_val)
                    .size(12.5)
                    .strong()
                    .color(p.text_primary),
            )
            .truncate(),
        );
        ui.add(
            egui::Label::new(RichText::new(sub_val).size(11.0).color(p.text_secondary)).truncate(),
        );
    });
}

fn render_hardware_synthesis_card(
    ui: &mut Ui,
    p: &Palette,
    device: &Device,
    action: &mut Option<MetricsPanelAction>,
) {
    let sys_main = device
        .system_info
        .as_ref()
        .map(|i| {
            let joined = format!("{} {}", i.os_name, i.os_version).trim().to_string();
            if joined.is_empty() {
                "—".to_string()
            } else {
                joined
            }
        })
        .unwrap_or_else(|| "—".to_string());
    let sys_sub = device
        .system_info
        .as_ref()
        .map(|i| format!("Hôte : {}", i.hostname))
        .unwrap_or_else(|| format!("Hôte : {}", device.hostname));

    let hw_main = device
        .system_info
        .as_ref()
        .map(|i| {
            format!(
                "{} • {} cœurs",
                if i.arch.is_empty() { "—" } else { &i.arch },
                i.cpu_count
            )
        })
        .unwrap_or_else(|| "—".to_string());
    let hw_sub = device
        .system_info
        .as_ref()
        .map(|i| format!("RAM : {}", theme::format_storage(i.total_memory_bytes)))
        .unwrap_or_else(|| "Matériel inconnu".to_string());

    let ver_str = if device.version.is_empty() {
        "Version inconnue".to_string()
    } else {
        format!("v{}", device.version.trim_start_matches('v'))
    };
    let agent_main = format!(
        "{ver_str} • {}",
        if device.is_manual { "Manuel" } else { "mDNS" }
    );
    let agent_sub = format!("{} adresse(s) configurée(s)", device.addresses.len());

    let sec_main = if device.cert_fingerprint.is_some() {
        "🛡 TLS TOFU épinglé".to_string()
    } else {
        "🛡 TLS en attente".to_string()
    };
    let ping_txt = device
        .last_ping_ms
        .map(|ms| format!("{ms} ms"))
        .unwrap_or_else(|| "—".to_string());
    let up_txt = device
        .server_uptime_secs
        .map(theme::format_uptime)
        .unwrap_or_else(|| "—".to_string());
    let sec_sub = format!("Ping {ping_txt} • Uptime {up_txt}");

    theme::card_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let avail_w = ui.available_width();

        if avail_w >= 540.0 {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("ℹ Synthèse Système & Matériel")
                        .strong()
                        .size(14.0)
                        .color(p.text_primary),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("📡 Tester le Ping")
                        .on_hover_text("Mesurer la latence gRPC actuelle")
                        .clicked()
                    {
                        *action = Some(MetricsPanelAction::Ping);
                    }
                });
            });
        } else {
            ui.label(
                RichText::new("ℹ Synthèse Système & Matériel")
                    .strong()
                    .size(14.0)
                    .color(p.text_primary),
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("📡 Tester le Ping").clicked() {
                    *action = Some(MetricsPanelAction::Ping);
                }
            });
        }

        ui.add_space(8.0);

        if avail_w >= 860.0 {
            ui.columns(4, |cols| {
                render_spec_tile(&mut cols[0], p, "SYSTÈME", &sys_main, &sys_sub);
                render_spec_tile(&mut cols[1], p, "MATÉRIEL", &hw_main, &hw_sub);
                render_spec_tile(&mut cols[2], p, "AGENT NTX", &agent_main, &agent_sub);
                render_spec_tile(&mut cols[3], p, "SÉCURITÉ & SANTÉ", &sec_main, &sec_sub);
            });
        } else if avail_w >= 480.0 {
            ui.columns(2, |cols| {
                render_spec_tile(&mut cols[0], p, "SYSTÈME", &sys_main, &sys_sub);
                render_spec_tile(&mut cols[1], p, "MATÉRIEL", &hw_main, &hw_sub);
            });
            ui.add_space(8.0);
            ui.columns(2, |cols| {
                render_spec_tile(&mut cols[0], p, "AGENT NTX", &agent_main, &agent_sub);
                render_spec_tile(&mut cols[1], p, "SÉCURITÉ & SANTÉ", &sec_main, &sec_sub);
            });
        } else {
            render_spec_tile(ui, p, "SYSTÈME", &sys_main, &sys_sub);
            ui.add_space(6.0);
            render_spec_tile(ui, p, "MATÉRIEL", &hw_main, &hw_sub);
            ui.add_space(6.0);
            render_spec_tile(ui, p, "AGENT NTX", &agent_main, &agent_sub);
            ui.add_space(6.0);
            render_spec_tile(ui, p, "SÉCURITÉ & SANTÉ", &sec_main, &sec_sub);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_connected_device() -> Device {
        let mut dev = Device::new(
            "d1".to_string(),
            "pi-node".to_string(),
            vec!["192.168.1.42".to_string()],
            9850,
            "0.3.0".to_string(),
        );
        dev.status = ConnectionStatus::Connected;
        dev.last_ping_ms = Some(12);
        dev.server_uptime_secs = Some(3720);
        dev.system_info = Some(ntx_proto::SystemInfo {
            hostname: "pi-node".to_string(),
            os_name: "Debian".to_string(),
            os_version: "12".to_string(),
            arch: "aarch64".to_string(),
            total_memory_bytes: 4_000_000_000,
            cpu_count: 4,
            disks: vec![
                ntx_proto::DiskInfo {
                    name: "mmcblk0p2".to_string(),
                    mount_point: "/".to_string(),
                    total_bytes: 32_000_000_000,
                    available_bytes: 16_000_000_000,
                    filesystem: "ext4".to_string(),
                },
                ntx_proto::DiskInfo {
                    name: "tmpfs".to_string(),
                    mount_point: "/run".to_string(),
                    total_bytes: 500_000_000,
                    available_bytes: 450_000_000,
                    filesystem: "tmpfs".to_string(),
                },
            ],
            networks: vec![
                ntx_proto::NetworkInterface {
                    name: "eth0".to_string(),
                    mac_address: "aa:bb:cc:dd:ee:ff".to_string(),
                    ip_addresses: vec!["192.168.1.42".to_string()],
                },
                ntx_proto::NetworkInterface {
                    name: "lo".to_string(),
                    mac_address: "".to_string(),
                    ip_addresses: vec!["127.0.0.1".to_string()],
                },
            ],
        });
        dev
    }

    #[test]
    fn test_choose_rate_unit_scales_cleanly() {
        assert_eq!(choose_rate_unit(0.0), (1.0, "o/s"));
        assert_eq!(choose_rate_unit(512.0), (1.0, "o/s"));
        assert_eq!(choose_rate_unit(2048.0), (1024.0, "Ko/s"));
        assert_eq!(
            choose_rate_unit(5.0 * 1024.0 * 1024.0),
            (1024.0 * 1024.0, "Mo/s")
        );
        assert_eq!(
            choose_rate_unit(2.0 * 1024.0 * 1024.0 * 1024.0),
            (1024.0 * 1024.0 * 1024.0, "Go/s")
        );
    }

    #[test]
    fn test_compute_series_stats() {
        assert!(compute_series_stats(&[]).is_none());
        assert!(compute_series_stats(&[f64::NAN, f64::INFINITY]).is_none());

        let stats = compute_series_stats(&[10.0, f64::NAN, 30.0, 20.0]).unwrap();
        assert_eq!(stats.min, 10.0);
        assert_eq!(stats.max, 30.0);
        assert!((stats.avg - 20.0).abs() < 1e-6);
        assert_eq!(stats.last, 20.0);
    }

    #[test]
    fn test_extract_network_series_filters_virtual_and_specific_interface() {
        let mut dev = make_connected_device();
        dev.net_samples.push_back(vec![
            ("eth0".to_string(), 1000, 200),
            ("wlan0".to_string(), 500, 100),
            ("lo".to_string(), 90_000, 90_000),
        ]);
        dev.net_samples.push_back(vec![
            ("eth0".to_string(), 2000, 400),
            ("docker0".to_string(), 50_000, 50_000),
        ]);

        let (rx_phys, tx_phys) = extract_network_series(&dev, None, true);
        assert_eq!(rx_phys, vec![1500.0, 2000.0]);
        assert_eq!(tx_phys, vec![300.0, 400.0]);

        let (rx_all, tx_all) = extract_network_series(&dev, None, false);
        assert_eq!(rx_all, vec![91_500.0, 52_000.0]);
        assert_eq!(tx_all, vec![90_300.0, 50_400.0]);

        let (rx_eth, tx_eth) = extract_network_series(&dev, Some("eth0"), true);
        assert_eq!(rx_eth, vec![1000.0, 2000.0]);
        assert_eq!(tx_eth, vec![200.0, 400.0]);

        let ifaces = available_network_interfaces(&dev, Some("lo"), true);
        assert_eq!(ifaces, vec!["eth0".to_string(), "lo".to_string()]);
    }

    #[test]
    fn test_collect_disk_rows_live_and_static_with_virtual_filter() {
        let mut dev = make_connected_device();

        // Static fallback when latest_metrics is None
        let (vis_static, total_static) = collect_disk_rows(&dev, true);
        assert_eq!(total_static, 2);
        assert_eq!(vis_static.len(), 1);
        assert_eq!(vis_static[0].mount_point, "/");
        assert!(vis_static[0].read_bps.is_none());

        // Live metrics with duplicate mount_point
        dev.latest_metrics = Some(ntx_proto::SystemMetrics {
            disk_metrics: vec![
                ntx_proto::DiskMetrics {
                    mount_point: "/".to_string(),
                    used_bytes: 20_000_000_000,
                    total_bytes: 32_000_000_000,
                    read_bytes_per_sec: 4096,
                    write_bytes_per_sec: 2048,
                },
                ntx_proto::DiskMetrics {
                    mount_point: "/".to_string(),
                    used_bytes: 20_000_000_000,
                    total_bytes: 32_000_000_000,
                    read_bytes_per_sec: 1,
                    write_bytes_per_sec: 1,
                },
                ntx_proto::DiskMetrics {
                    mount_point: "/run".to_string(),
                    used_bytes: 100_000_000,
                    total_bytes: 500_000_000,
                    read_bytes_per_sec: 0,
                    write_bytes_per_sec: 0,
                },
            ],
            ..Default::default()
        });

        let (vis_live, total_live) = collect_disk_rows(&dev, true);
        assert_eq!(total_live, 2);
        assert_eq!(vis_live.len(), 1);
        assert_eq!(vis_live[0].filesystem, "ext4");
        assert_eq!(vis_live[0].read_bps, Some(4096));
        assert_eq!(vis_live[0].write_bps, Some(2048));

        let (all_live, _) = collect_disk_rows(&dev, false);
        assert_eq!(all_live.len(), 2);
    }

    #[test]
    fn test_collect_server_alerts_thresholds() {
        let mut dev = make_connected_device();
        dev.last_ping_ms = Some(180);
        let disks = vec![DiskRow {
            mount_point: "/".to_string(),
            filesystem: "ext4".to_string(),
            used_bytes: 91,
            total_bytes: 100,
            avail_bytes: 9,
            pct: 91.0,
            read_bps: None,
            write_bps: None,
        }];

        let alerts = collect_server_alerts(&dev, 93.0, 86.0, &disks);
        assert_eq!(alerts.len(), 4);
        assert!(alerts[0].critical); // CPU >= 92%
        assert!(!alerts[1].critical); // RAM 86%
        assert!(alerts[2].critical); // Disk >= 90%
        assert!(!alerts[3].critical); // Ping 180 ms
    }

    #[test]
    fn test_export_metrics_csv_and_server_summary() {
        let mut dev = make_connected_device();
        dev.record_metrics(
            ntx_proto::SystemMetrics {
                cpu_usage_percent: 42.5,
                used_memory_bytes: 2_000_000_000,
                total_memory_bytes: 4_000_000_000,
                disk_metrics: vec![ntx_proto::DiskMetrics {
                    mount_point: "/".to_string(),
                    used_bytes: 16_000_000_000,
                    total_bytes: 32_000_000_000,
                    read_bytes_per_sec: 1024,
                    write_bytes_per_sec: 512,
                }],
                network_metrics: vec![
                    ntx_proto::NetworkMetrics {
                        interface_name: "eth0".to_string(),
                        rx_bytes_per_sec: 4000,
                        tx_bytes_per_sec: 1000,
                    },
                    ntx_proto::NetworkMetrics {
                        interface_name: "lo".to_string(),
                        rx_bytes_per_sec: 99999,
                        tx_bytes_per_sec: 99999,
                    },
                ],
                timestamp_ms: 1_700_000_000_000,
            },
            10,
        );

        let csv = export_metrics_csv(&dev, None, true);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            "timestamp_ms,cpu_percent,ram_percent,ram_used_bytes,net_rx_bps,net_tx_bps,disk_read_bps,disk_write_bps"
        );
        assert_eq!(
            lines[1],
            "1700000000000,42.50,50.00,2000000000,4000,1000,1024,512"
        );

        let summary = format_server_summary(&dev, true, true);
        assert!(summary.contains("pi-node"));
        assert!(summary.contains("Debian 12"));
        assert!(summary.contains("42.5%"));
        assert!(summary.contains("Disque /"));
    }

    #[test]
    fn test_format_window_and_axis_seconds() {
        assert_eq!(format_window_duration(45.0), "45s");
        assert_eq!(format_window_duration(90.0), "1m 30s");
        assert_eq!(format_window_duration(120.0), "2m");

        assert_eq!(format_axis_seconds(0.0), "0s");
        assert_eq!(format_axis_seconds(-30.0), "-30s");
        assert_eq!(format_axis_seconds(-90.0), "-1m30s");
        assert_eq!(format_axis_seconds(-120.0), "-2m");
    }
}
