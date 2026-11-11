use crate::state::{ConnectionStatus, Device};
use egui::{Color32, ProgressBar, RichText, Ui};
use egui_plot::{Corner, Legend, Line, Plot, PlotPoints};

pub enum DevicePanelAction {
    BackToDashboard,
    OpenTerminal(String),
    Disconnect(String),
    Reconnect(String),
}

pub fn render(ui: &mut Ui, device: &Device) -> Option<DevicePanelAction> {
    let mut action = None;

    // Header bar
    ui.horizontal(|ui| {
        if ui.button("⬅ Dashboard").clicked() {
            action = Some(DevicePanelAction::BackToDashboard);
        }

        ui.separator();

        let icon = if let Some(info) = &device.system_info {
            let os = info.os_name.to_lowercase();
            if os.contains("win") {
                "🪟"
            } else if os.contains("raspbian") || os.contains("debian") || os.contains("ubuntu") {
                "🐧"
            } else if os.contains("mac") || os.contains("darwin") {
                "🍎"
            } else {
                "🖥️"
            }
        } else {
            "🖥️"
        };

        ui.heading(format!("{} {}", icon, device.hostname));

        let status_color = match &device.status {
            ConnectionStatus::Connected => Color32::from_rgb(46, 204, 113),
            ConnectionStatus::Connecting => Color32::from_rgb(52, 152, 219),
            _ => Color32::from_rgb(231, 76, 60),
        };
        ui.label(
            RichText::new(device.status.display_text())
                .color(status_color)
                .strong(),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if matches!(device.status, ConnectionStatus::Connected) {
                if ui.button("💻 Terminal").clicked() {
                    action = Some(DevicePanelAction::OpenTerminal(device.id.clone()));
                }
                if ui.button("Disconnect").clicked() {
                    action = Some(DevicePanelAction::Disconnect(device.id.clone()));
                }
            } else {
                if ui.button("Reconnect").clicked() {
                    action = Some(DevicePanelAction::Reconnect(device.id.clone()));
                }
            }
        });
    });

    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        // System Information Summary
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Target:").strong());
                ui.label(format!("{}:{}", device.primary_address(), device.port));

                if let Some(info) = &device.system_info {
                    ui.separator();
                    ui.label(RichText::new("OS:").strong());
                    ui.label(format!(
                        "{} {} ({})",
                        info.os_name, info.os_version, info.arch
                    ));

                    ui.separator();
                    ui.label(RichText::new("CPUs:").strong());
                    ui.label(format!("{} cores", info.cpu_count));

                    ui.separator();
                    ui.label(RichText::new("Total RAM:").strong());
                    let total_gb = info.total_memory_bytes as f64 / 1_073_741_824.0;
                    ui.label(format!("{:.2} GB", total_gb));
                }

                if !device.version.is_empty() {
                    ui.separator();
                    ui.label(RichText::new("Agent Version:").strong());
                    ui.label(&device.version);
                }
            });
        });

        ui.add_space(10.0);

        // Real-time Graphs (CPU & RAM)
        ui.columns(2, |columns| {
            // CPU Plot
            columns[0].vertical(|ui| {
                let cur_cpu = device.cpu_history.back().copied().unwrap_or(0.0);
                ui.label(
                    RichText::new(format!("📈 CPU Utilization: {:.1}%", cur_cpu))
                        .strong()
                        .size(14.0),
                );

                let n = device.cpu_history.len();
                let points: Vec<[f64; 2]> = device
                    .cpu_history
                    .iter()
                    .enumerate()
                    .map(|(i, &val)| {
                        let sec_ago = if n > 0 { -((n - 1 - i) as f64) } else { 0.0 };
                        [sec_ago, val]
                    })
                    .collect();

                let line = Line::new(PlotPoints::new(points))
                    .name("CPU %")
                    .color(Color32::from_rgb(0, 200, 255))
                    .fill(0.0_f32)
                    .width(2.0_f32);

                Plot::new("cpu_graph")
                    .height(180.0)
                    .include_x(-60.0)
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

            // RAM Plot
            columns[1].vertical(|ui| {
                let cur_ram = device.ram_history.back().copied().unwrap_or(0.0);
                let (used_gb, total_gb) = if let Some(m) = &device.latest_metrics {
                    (
                        m.used_memory_bytes as f64 / 1_073_741_824.0,
                        m.total_memory_bytes as f64 / 1_073_741_824.0,
                    )
                } else {
                    (0.0, 0.0)
                };

                ui.label(
                    RichText::new(format!(
                        "📈 RAM Utilization: {:.1}% ({:.1}/{:.1} GB)",
                        cur_ram, used_gb, total_gb
                    ))
                    .strong()
                    .size(14.0),
                );

                let n = device.ram_history.len();
                let points: Vec<[f64; 2]> = device
                    .ram_history
                    .iter()
                    .enumerate()
                    .map(|(i, &val)| {
                        let sec_ago = if n > 0 { -((n - 1 - i) as f64) } else { 0.0 };
                        [sec_ago, val]
                    })
                    .collect();

                let line = Line::new(PlotPoints::new(points))
                    .name("RAM %")
                    .color(Color32::from_rgb(180, 100, 240))
                    .fill(0.0_f32)
                    .width(2.0_f32);

                Plot::new("ram_graph")
                    .height(180.0)
                    .include_x(-60.0)
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

        ui.add_space(10.0);

        // Network I/O Plot
        let cur_rx = device.rx_history.back().copied().unwrap_or(0.0);
        let cur_tx = device.tx_history.back().copied().unwrap_or(0.0);
        let rx_str = format_bytes_rate(cur_rx);
        let tx_str = format_bytes_rate(cur_tx);

        ui.label(
            RichText::new(format!(
                "🌐 Network Throughput — Download: {} | Upload: {}",
                rx_str, tx_str
            ))
            .strong()
            .size(14.0),
        );

        let n = device.rx_history.len();
        let rx_points: Vec<[f64; 2]> = device
            .rx_history
            .iter()
            .enumerate()
            .map(|(i, &val)| {
                let sec_ago = if n > 0 { -((n - 1 - i) as f64) } else { 0.0 };
                [sec_ago, val / 1024.0] // in KB/s
            })
            .collect();

        let tx_points: Vec<[f64; 2]> = device
            .tx_history
            .iter()
            .enumerate()
            .map(|(i, &val)| {
                let sec_ago = if n > 0 { -((n - 1 - i) as f64) } else { 0.0 };
                [sec_ago, val / 1024.0] // in KB/s
            })
            .collect();

        let rx_line = Line::new(PlotPoints::new(rx_points))
            .name("Download (KB/s)")
            .color(Color32::from_rgb(46, 204, 113))
            .fill(0.0_f32)
            .width(2.0_f32);

        let tx_line = Line::new(PlotPoints::new(tx_points))
            .name("Upload (KB/s)")
            .color(Color32::from_rgb(230, 126, 34))
            .width(2.0_f32);

        Plot::new("net_graph")
            .height(140.0)
            .include_x(-60.0)
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

        ui.add_space(14.0);

        // Disks / Storage Section
        if let Some(info) = &device.system_info {
            if !info.disks.is_empty() {
                ui.heading("💾 Storage & Disks");
                egui::Grid::new("disks_grid")
                    .striped(true)
                    .min_col_width(80.0)
                    .spacing([20.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Mount Point").strong());
                        ui.label(RichText::new("Filesystem").strong());
                        ui.label(RichText::new("Total").strong());
                        ui.label(RichText::new("Used").strong());
                        ui.label(RichText::new("Free").strong());
                        ui.label(RichText::new("Usage").strong());
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
                            ui.label(format!("{:.1} GB", total_gb));
                            ui.label(format!("{:.1} GB", used_gb));
                            ui.label(format!("{:.1} GB", avail_gb));

                            let bar = ProgressBar::new(pct.clamp(0.0, 1.0))
                                .text(format!("{:.0}%", pct * 100.0))
                                .desired_width(120.0);
                            ui.add(bar);
                            ui.end_row();
                        }
                    });

                ui.add_space(14.0);
            }

            // Network Interfaces Section
            if !info.networks.is_empty() {
                ui.heading("🖧 Network Interfaces");
                egui::Grid::new("networks_grid")
                    .striped(true)
                    .min_col_width(100.0)
                    .spacing([20.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Interface").strong());
                        ui.label(RichText::new("MAC Address").strong());
                        ui.label(RichText::new("IP Addresses").strong());
                        ui.label(RichText::new("Current Throughput").strong());
                        ui.end_row();

                        for net in &info.networks {
                            ui.label(&net.name);
                            ui.label(if net.mac_address.is_empty() {
                                "—".to_string()
                            } else {
                                net.mac_address.clone()
                            });
                            ui.label(net.ip_addresses.join(", "));

                            // Find throughput for this interface
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

    action
}

fn format_bytes_rate(bytes_per_sec: f64) -> String {
    if bytes_per_sec < 1024.0 {
        format!("{:.0} B/s", bytes_per_sec)
    } else if bytes_per_sec < 1024.0 * 1024.0 {
        format!("{:.1} KB/s", bytes_per_sec / 1024.0)
    } else if bytes_per_sec < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} MB/s", bytes_per_sec / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB/s", bytes_per_sec / (1024.0 * 1024.0 * 1024.0))
    }
}
