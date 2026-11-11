use crate::state::Device;
use egui::{Color32, Key, RichText, Stroke, Ui};

pub enum TerminalAction {
    BackToDashboard,
    SendData(Vec<u8>),
    Clear,
    Reconnect,
}

pub fn render(ui: &mut Ui, device: &mut Device) -> Option<TerminalAction> {
    let mut action = None;

    // Header bar
    ui.horizontal(|ui| {
        if ui.button("⬅ Dashboard").clicked() {
            action = Some(TerminalAction::BackToDashboard);
        }

        ui.separator();
        ui.heading(format!("💻 Remote Terminal — {}", device.hostname));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("🧹 Clear").clicked() {
                device.terminal.clear();
                action = Some(TerminalAction::Clear);
            }

            if ui
                .button("🔄 Reconnect")
                .on_hover_text("Restart terminal session")
                .clicked()
            {
                action = Some(TerminalAction::Reconnect);
            }

            if ui
                .button("⏏ Ctrl+D")
                .on_hover_text("Send EOF (0x04)")
                .clicked()
            {
                action = Some(TerminalAction::SendData(vec![0x04]));
            }

            if ui
                .button("⏹ Ctrl+C")
                .on_hover_text("Send SIGINT (0x03)")
                .clicked()
            {
                action = Some(TerminalAction::SendData(vec![0x03]));
            }

            ui.label(
                RichText::new(format!("{}:{}", device.primary_address(), device.port))
                    .weak()
                    .small(),
            );
        });
    });

    ui.separator();

    // Terminal Screen Area
    let avail_height = (ui.available_height() - 48.0).max(120.0);

    egui::Frame::canvas(ui.style())
        .fill(Color32::from_rgb(18, 18, 20))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(45, 45, 55)))
        .inner_margin(8.0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(avail_height)
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());

                    if device.terminal.lines.is_empty() && device.terminal.current_spans.is_empty()
                    {
                        ui.label(
                            RichText::new(
                                "Terminal ready. Type a command below and press Enter...",
                            )
                            .color(Color32::from_rgb(120, 120, 130))
                            .monospace(),
                        );
                    }

                    // Render completed lines
                    for line in &device.terminal.lines {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            for span in &line.spans {
                                let color =
                                    Color32::from_rgb(span.color[0], span.color[1], span.color[2]);
                                let mut text = RichText::new(&span.text).color(color).monospace();
                                if span.bold {
                                    text = text.strong();
                                }
                                ui.label(text);
                            }
                        });
                    }

                    // Render active line (in-progress output / shell prompt)
                    if !device.terminal.current_spans.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            for span in &device.terminal.current_spans {
                                let color =
                                    Color32::from_rgb(span.color[0], span.color[1], span.color[2]);
                                let mut text = RichText::new(&span.text).color(color).monospace();
                                if span.bold {
                                    text = text.strong();
                                }
                                ui.label(text);
                            }
                        });
                    }
                });
        });

    ui.add_space(4.0);

    // Bottom Input Bar
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("❯")
                .color(Color32::from_rgb(46, 204, 113))
                .monospace()
                .strong(),
        );

        // Keyboard navigation for command history
        if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
            if let Some(idx) = device.terminal.history_idx {
                if idx > 0 {
                    device.terminal.history_idx = Some(idx - 1);
                    if let Some(cmd) = device.terminal.history.get(idx - 1) {
                        device.terminal.input_buffer = cmd.clone();
                    }
                }
            } else if !device.terminal.history.is_empty() {
                let idx = device.terminal.history.len() - 1;
                device.terminal.history_idx = Some(idx);
                if let Some(cmd) = device.terminal.history.get(idx) {
                    device.terminal.input_buffer = cmd.clone();
                }
            }
        }

        if ui.input(|i| i.key_pressed(Key::ArrowDown)) {
            if let Some(idx) = device.terminal.history_idx {
                if idx + 1 < device.terminal.history.len() {
                    device.terminal.history_idx = Some(idx + 1);
                    if let Some(cmd) = device.terminal.history.get(idx + 1) {
                        device.terminal.input_buffer = cmd.clone();
                    }
                } else {
                    device.terminal.history_idx = None;
                    device.terminal.input_buffer.clear();
                }
            }
        }

        let is_windows = device
            .system_info
            .as_ref()
            .map(|i| i.os_name.to_lowercase().contains("win"))
            .unwrap_or(false);

        let input_resp = ui.add(
            egui::TextEdit::singleline(&mut device.terminal.input_buffer)
                .font(egui::TextStyle::Monospace)
                .desired_width(ui.available_width() - 80.0)
                .hint_text("Type command and press Enter..."),
        );

        let enter_pressed = ui.input(|i| i.key_pressed(Key::Enter));
        let send_clicked = ui.button("Send ⏎").clicked();

        if (input_resp.lost_focus() && enter_pressed)
            || (input_resp.has_focus() && enter_pressed)
            || send_clicked
        {
            let cmd = std::mem::take(&mut device.terminal.input_buffer);
            let newline: &[u8] = if is_windows { b"\r\n" } else { b"\n" };

            if !cmd.trim().is_empty() {
                device.terminal.history.push(cmd.clone());
                device.terminal.history_idx = None;

                let mut data = cmd.into_bytes();
                data.extend_from_slice(newline);
                action = Some(TerminalAction::SendData(data));
            } else {
                // Send lone newline to get prompt
                action = Some(TerminalAction::SendData(newline.to_vec()));
            }

            input_resp.request_focus();
        }
    });

    action
}
