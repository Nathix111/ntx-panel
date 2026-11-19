use crate::state::{Device, TerminalSessionState};
use egui::{text::LayoutJob, Color32, EventFilter, Key, RichText, Stroke, TextFormat, Ui};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalAction {
    SendData(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Clear,
    Connect,
    Reconnect,
}

const DEFAULT_FG: Color32 = Color32::from_rgb(225, 228, 232);
const DEFAULT_BG: Color32 = Color32::from_rgb(18, 18, 20);

pub fn render(ui: &mut Ui, device: &mut Device) -> Vec<TerminalAction> {
    let mut actions = Vec::new();
    let (cur_rows, cur_cols) = device.terminal.size();

    // Terminal Toolbar
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("🐚 Terminal VT100 ({cur_cols}×{cur_rows})"))
                .monospace()
                .size(12.0)
                .color(Color32::from_rgb(170, 178, 190)),
        );

        let (st_text, st_color) = match &device.terminal_state {
            TerminalSessionState::Open => ("● Ouvert", Color32::from_rgb(46, 204, 113)),
            TerminalSessionState::Connecting => ("◌ Connexion...", Color32::from_rgb(52, 152, 219)),
            TerminalSessionState::ClosedByUser => ("○ Fermé", Color32::from_rgb(149, 165, 166)),
            TerminalSessionState::Error(_) => ("✕ Erreur", Color32::from_rgb(231, 76, 60)),
            TerminalSessionState::Idle => ("○ Prêt", Color32::from_rgb(241, 196, 15)),
        };
        ui.label(RichText::new(st_text).size(11.5).strong().color(st_color));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("🧹 Effacer")
                .on_hover_text("Effacer l'écran et le tampon du terminal")
                .clicked()
            {
                device.terminal.clear();
                actions.push(TerminalAction::Clear);
            }

            let is_active = matches!(
                device.terminal_state,
                TerminalSessionState::Open | TerminalSessionState::Connecting
            );
            if is_active {
                if ui
                    .button("🔄 Reconnecter")
                    .on_hover_text("Fermer et relancer une nouvelle session terminal")
                    .clicked()
                {
                    actions.push(TerminalAction::Reconnect);
                }
            } else if ui
                .button("▶ Ouvrir")
                .on_hover_text("Ouvrir une nouvelle session terminal")
                .clicked()
            {
                actions.push(TerminalAction::Connect);
            }

            if ui
                .button("📋 Copier")
                .on_hover_text("Copier le texte visible du terminal (Ctrl+Maj+C)")
                .clicked()
            {
                ui.ctx().copy_text(device.terminal.visible_plain_text());
            }

            if ui
                .button("⏏ Ctrl+D")
                .on_hover_text("Envoyer EOF (0x04)")
                .clicked()
            {
                actions.push(TerminalAction::SendData(vec![0x04]));
            }

            if ui
                .button("⏹ Ctrl+C")
                .on_hover_text("Envoyer SIGINT (0x03)")
                .clicked()
            {
                actions.push(TerminalAction::SendData(vec![0x03]));
            }
        });
    });

    ui.add_space(4.0);

    // Banner if terminal was closed or errored (no auto-reconnect loop!)
    match &device.terminal_state {
        TerminalSessionState::ClosedByUser => {
            egui::Frame::new()
                .fill(Color32::from_rgb(30, 34, 42))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(70, 80, 100)))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("La session terminal est terminée.")
                                .color(Color32::from_rgb(200, 210, 225)),
                        );
                        if ui.button("🔄 Relancer une session").clicked() {
                            actions.push(TerminalAction::Reconnect);
                        }
                    });
                });
            ui.add_space(4.0);
        }
        TerminalSessionState::Error(err) => {
            egui::Frame::new()
                .fill(Color32::from_rgb(60, 24, 24))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(180, 70, 70)))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("Erreur terminal : {err}"))
                                .color(Color32::from_rgb(255, 160, 160)),
                        );
                        if ui.button("🔄 Réessayer").clicked() {
                            actions.push(TerminalAction::Reconnect);
                        }
                    });
                });
            ui.add_space(4.0);
        }
        _ => {}
    }

    // VT100 Screen Canvas
    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    let (char_width, row_height) = ui.fonts(|fonts| {
        let cw = fonts.glyph_width(&font_id, 'M').max(6.0);
        let rh = fonts.row_height(&font_id).max(13.0);
        (cw, rh)
    });

    let avail = ui.available_size();
    let inner_w = (avail.x - 16.0).max(160.0);
    let inner_h = (avail.y - 24.0).max(120.0);
    let cols = (inner_w / char_width).floor().clamp(20.0, 300.0) as u16;
    let rows = (inner_h / row_height).floor().clamp(5.0, 200.0) as u16;

    if device.terminal.resize(rows, cols) {
        actions.push(TerminalAction::Resize { cols, rows });
    }

    let canvas_size = egui::vec2(cols as f32 * char_width, rows as f32 * row_height);

    // Reserve focus stroke based on previous frame focus; checked cleanly via Frame
    let frame_resp = egui::Frame::canvas(ui.style())
        .fill(DEFAULT_BG)
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(52, 60, 76)))
        .inner_margin(8.0)
        .show(ui, |ui| {
            let (rect, response) = ui.allocate_exact_size(canvas_size, egui::Sense::click());

            if response.clicked() {
                response.request_focus();
            }

            if response.has_focus() {
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        response.id,
                        EventFilter {
                            tab: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: true,
                        },
                    );
                });
            }

            // Mouse wheel scrollback
            if response.hovered() {
                let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll_y.abs() >= 4.0 {
                    let delta_lines = (scroll_y / row_height).round() as isize;
                    if delta_lines != 0 {
                        device.terminal.scroll_by(delta_lines);
                    }
                }
            }

            // Keyboard input capture when focused
            if response.has_focus() {
                let mut out_bytes = Vec::new();
                let (app_cursor, bracketed_paste) = {
                    let s = device.terminal.screen();
                    (s.application_cursor(), s.bracketed_paste())
                };
                let (events, cur_mods) =
                    ui.input(|i| (i.events.clone(), i.modifiers));

                for ev in events {
                    match ev {
                        egui::Event::Copy => {
                            if cur_mods.shift {
                                ui.ctx().copy_text(device.terminal.visible_plain_text());
                            } else {
                                out_bytes.push(0x03);
                            }
                        }
                        egui::Event::Cut => {
                            out_bytes.push(0x18);
                        }
                        egui::Event::Paste(text) => {
                            if !text.is_empty() {
                                if bracketed_paste {
                                    out_bytes.extend_from_slice(b"\x1b[200~");
                                    out_bytes.extend_from_slice(text.as_bytes());
                                    out_bytes.extend_from_slice(b"\x1b[201~");
                                } else {
                                    out_bytes.extend_from_slice(text.as_bytes());
                                }
                            }
                        }
                        egui::Event::Text(text) => {
                            // Ignore Ctrl+<char>, EXCEPT Windows AltGr which sets both ctrl && alt
                            let is_pure_ctrl = cur_mods.ctrl && !cur_mods.alt;
                            if !is_pure_ctrl && !cur_mods.mac_cmd {
                                for ch in text.chars() {
                                    if ch >= ' ' {
                                        let mut buf = [0u8; 4];
                                        let s = ch.encode_utf8(&mut buf);
                                        out_bytes.extend_from_slice(s.as_bytes());
                                    }
                                }
                            }
                        }
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } => {
                            if let Some(seq) = map_key_to_vt100(key, modifiers, app_cursor) {
                                out_bytes.extend_from_slice(&seq);
                            }
                        }
                        _ => {}
                    }
                }

                if !out_bytes.is_empty() {
                    device.terminal.reset_scrollback();
                    actions.push(TerminalAction::SendData(out_bytes));
                }
            }

            // Render VT100 grid
            let painter = ui.painter_at(rect);
            let screen = device.terminal.screen();

            for r in 0..rows {
                let row_y = rect.min.y + r as f32 * row_height;
                let mut job = LayoutJob::default();
                let mut run_text = String::new();
                let mut run_style: Option<(Color32, bool, bool)> = None;

                for c in 0..cols {
                    let Some(cell) = screen.cell(r, c) else {
                        continue;
                    };
                    if cell.is_wide_continuation() {
                        continue;
                    }

                    let mut fg = vt100_color_to_egui(cell.fgcolor(), DEFAULT_FG, cell.bold());
                    let mut bg = vt100_color_to_egui(cell.bgcolor(), DEFAULT_BG, false);
                    if cell.inverse() {
                        std::mem::swap(&mut fg, &mut bg);
                    }

                    if bg != DEFAULT_BG {
                        let cell_w = if cell.is_wide() {
                            char_width * 2.0
                        } else {
                            char_width
                        };
                        let cell_rect = egui::Rect::from_min_size(
                            egui::pos2(rect.min.x + c as f32 * char_width, row_y),
                            egui::vec2(cell_w, row_height),
                        );
                        painter.rect_filled(cell_rect, 0.0, bg);
                    }

                    let style = (fg, cell.italic(), cell.underline());
                    if run_style != Some(style) {
                        if let Some((rfg, ritalic, runder)) = run_style.take() {
                            if !run_text.is_empty() {
                                append_run(&mut job, &run_text, &font_id, rfg, ritalic, runder);
                                run_text.clear();
                            }
                        }
                        run_style = Some(style);
                    }

                    if cell.has_contents() {
                        run_text.push_str(&cell.contents());
                    } else {
                        run_text.push(' ');
                    }
                }

                if let Some((rfg, ritalic, runder)) = run_style {
                    if !run_text.is_empty() {
                        append_run(&mut job, &run_text, &font_id, rfg, ritalic, runder);
                    }
                }

                if !job.is_empty() {
                    let galley = ui.fonts(|f| f.layout_job(job));
                    painter.galley(egui::pos2(rect.min.x, row_y), galley, DEFAULT_FG);
                }
            }

            // Render cursor
            if !screen.hide_cursor() && screen.scrollback() == 0 {
                let (cur_r, cur_c) = screen.cursor_position();
                if cur_r < rows && cur_c < cols {
                    let cur_rect = egui::Rect::from_min_size(
                        egui::pos2(
                            rect.min.x + cur_c as f32 * char_width,
                            rect.min.y + cur_r as f32 * row_height,
                        ),
                        egui::vec2(char_width, row_height),
                    );
                    if response.has_focus() {
                        painter.rect_filled(
                            cur_rect,
                            1.0,
                            Color32::from_rgba_unmultiplied(80, 220, 140, 125),
                        );
                    } else {
                        painter.rect_stroke(
                            cur_rect,
                            1.0,
                            Stroke::new(1.0_f32, Color32::from_rgb(130, 140, 160)),
                            egui::StrokeKind::Inside,
                        );
                    }
                }
            }

            // Focus highlight border + scrollback badge
            if response.has_focus() {
                painter.rect_stroke(
                    rect.expand(4.0),
                    2.0,
                    Stroke::new(1.5_f32, Color32::from_rgb(52, 152, 219)),
                    egui::StrokeKind::Outside,
                );
            } else {
                let hint_pos = egui::pos2(rect.right() - 8.0, rect.bottom() - 4.0);
                painter.text(
                    hint_pos,
                    egui::Align2::RIGHT_BOTTOM,
                    "Cliquez ici pour saisir (Tab, Flèches, Ctrl+C, vim, htop...)",
                    egui::FontId::proportional(11.0),
                    Color32::from_rgba_unmultiplied(180, 190, 210, 140),
                );
            }

            if screen.scrollback() > 0 {
                let sb_pos = egui::pos2(rect.right() - 8.0, rect.top() + 4.0);
                painter.text(
                    sb_pos,
                    egui::Align2::RIGHT_TOP,
                    format!("↑ Historique (+{} lignes)", screen.scrollback()),
                    egui::FontId::proportional(11.0),
                    Color32::from_rgb(241, 196, 15),
                );
            }
        });
    let _ = frame_resp;

    actions
}

fn append_run(
    job: &mut LayoutJob,
    text: &str,
    font_id: &egui::FontId,
    fg: Color32,
    italic: bool,
    underline: bool,
) {
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font_id.clone(),
            color: fg,
            italics: italic,
            underline: if underline {
                Stroke::new(1.0_f32, fg)
            } else {
                Stroke::NONE
            },
            ..Default::default()
        },
    );
}

fn map_key_to_vt100(
    key: Key,
    modifiers: egui::Modifiers,
    application_cursor: bool,
) -> Option<Vec<u8>> {
    // Ctrl+<letter> (excluding AltGr where ctrl && alt are both active)
    if modifiers.ctrl && !modifiers.alt && !modifiers.shift {
        if let Some(ctrl_byte) = key_to_ctrl_byte(key) {
            return Some(vec![ctrl_byte]);
        }
    }

    let bytes: &[u8] = match key {
        Key::Enter => b"\r",
        Key::Backspace => b"\x7f",
        Key::Tab => {
            if modifiers.shift {
                b"\x1b[Z"
            } else {
                b"\t"
            }
        }
        Key::Escape => b"\x1b",
        Key::ArrowUp => {
            if application_cursor {
                b"\x1bOA"
            } else {
                b"\x1b[A"
            }
        }
        Key::ArrowDown => {
            if application_cursor {
                b"\x1bOB"
            } else {
                b"\x1b[B"
            }
        }
        Key::ArrowRight => {
            if application_cursor {
                b"\x1bOC"
            } else {
                b"\x1b[C"
            }
        }
        Key::ArrowLeft => {
            if application_cursor {
                b"\x1bOD"
            } else {
                b"\x1b[D"
            }
        }
        Key::Home => b"\x1b[H",
        Key::End => b"\x1b[F",
        Key::Insert => b"\x1b[2~",
        Key::Delete => b"\x1b[3~",
        Key::PageUp => b"\x1b[5~",
        Key::PageDown => b"\x1b[6~",
        Key::F1 => b"\x1bOP",
        Key::F2 => b"\x1bOQ",
        Key::F3 => b"\x1bOR",
        Key::F4 => b"\x1bOS",
        Key::F5 => b"\x1b[15~",
        Key::F6 => b"\x1b[17~",
        Key::F7 => b"\x1b[18~",
        Key::F8 => b"\x1b[19~",
        Key::F9 => b"\x1b[20~",
        Key::F10 => b"\x1b[21~",
        Key::F11 => b"\x1b[23~",
        Key::F12 => b"\x1b[24~",
        _ => return None,
    };

    Some(bytes.to_vec())
}

fn key_to_ctrl_byte(key: Key) -> Option<u8> {
    let code = match key {
        Key::A => 1,
        Key::B => 2,
        Key::C => 3,
        Key::D => 4,
        Key::E => 5,
        Key::F => 6,
        Key::G => 7,
        Key::H => 8,
        Key::I => 9,
        Key::J => 10,
        Key::K => 11,
        Key::L => 12,
        Key::M => 13,
        Key::N => 14,
        Key::O => 15,
        Key::P => 16,
        Key::Q => 17,
        Key::R => 18,
        Key::S => 19,
        Key::T => 20,
        Key::U => 21,
        Key::V => 22,
        Key::W => 23,
        Key::X => 24,
        Key::Y => 25,
        Key::Z => 26,
        _ => return None,
    };
    Some(code)
}

fn vt100_color_to_egui(color: vt100::Color, default: Color32, bold_bright: bool) -> Color32 {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
        vt100::Color::Idx(mut idx) => {
            if bold_bright && idx < 8 {
                idx += 8;
            }
            ansi_idx_to_color(idx)
        }
    }
}

fn ansi_idx_to_color(idx: u8) -> Color32 {
    match idx {
        0 => Color32::from_rgb(30, 32, 38),
        1 => Color32::from_rgb(224, 85, 85),
        2 => Color32::from_rgb(76, 200, 100),
        3 => Color32::from_rgb(230, 195, 70),
        4 => Color32::from_rgb(85, 145, 240),
        5 => Color32::from_rgb(195, 100, 220),
        6 => Color32::from_rgb(70, 200, 220),
        7 => Color32::from_rgb(215, 220, 226),
        8 => Color32::from_rgb(110, 118, 130),
        9 => Color32::from_rgb(255, 115, 115),
        10 => Color32::from_rgb(110, 235, 135),
        11 => Color32::from_rgb(250, 230, 110),
        12 => Color32::from_rgb(125, 180, 255),
        13 => Color32::from_rgb(235, 135, 255),
        14 => Color32::from_rgb(110, 235, 250),
        15 => Color32::from_rgb(255, 255, 255),
        16..=231 => {
            let v = idx - 16;
            let r_i = v / 36;
            let g_i = (v % 36) / 6;
            let b_i = v % 6;
            let map = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
            Color32::from_rgb(map(r_i), map(g_i), map(b_i))
        }
        232..=255 => {
            let gray = 8 + (idx - 232) * 10;
            Color32::from_rgb(gray, gray, gray)
        }
    }
}
