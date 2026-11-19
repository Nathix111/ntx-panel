use crate::state::{Device, TerminalSessionState};
use crate::ui::theme::{self, Palette};
use egui::{text::LayoutJob, Color32, EventFilter, Key, RichText, Stroke, TextFormat, Ui};

#[derive(Debug, Clone, PartialEq)]
pub enum TerminalAction {
    SendData(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Clear,
    Connect,
    Reconnect,
    SetFontSize(f32),
}

const DEFAULT_FG: Color32 = Color32::from_rgb(230, 236, 244);
const DEFAULT_BG: Color32 = Color32::from_rgb(13, 17, 23);
const CURSOR_BG: Color32 = Color32::from_rgb(52, 211, 153);

fn snap_to_pixel(val: f32, ppp: f32) -> f32 {
    (val * ppp).round() / ppp
}

fn grid_cell_metrics(raw_char_width: f32, raw_row_height: f32, ppp: f32) -> (f32, f32) {
    let safe_ppp = ppp.max(0.5);
    let cw = snap_to_pixel(raw_char_width.max(5.5), safe_ppp).max(5.5);
    let rh = snap_to_pixel(raw_row_height.max(12.0), safe_ppp).max(12.0);
    (cw, rh)
}

fn is_grid_ascii(contents: &str, is_wide: bool) -> bool {
    !is_wide && contents.len() == 1 && matches!(contents.as_bytes()[0], 0x20..=0x7E)
}

const QUICK_COMMANDS: &[(&str, &str)] = &[
    ("📊 htop (moniteur système)", "htop"),
    ("💾 df -h (disques)", "df -h"),
    ("⚡ free -h (mémoire)", "free -h"),
    ("ℹ uptime && uname -a", "uptime && uname -a"),
    ("🌐 ip -br a (réseau)", "ip -br a"),
    ("🔌 ss -tulpn (ports ouverts)", "ss -tulpn"),
    ("📜 journalctl (50 dernières lignes)", "journalctl -n 50 --no-pager"),
    ("⚠ systemctl --failed", "systemctl --failed"),
];

pub fn render(ui: &mut Ui, device: &mut Device, font_size: f32) -> Vec<TerminalAction> {
    let p = Palette::for_ui(ui);
    let mut actions = Vec::new();
    let (cur_rows, cur_cols) = device.terminal.size();
    let clamped_font = font_size.clamp(10.0, 20.0);

    let canvas_id = ui.make_persistent_id(("vt100_canvas", &device.id));
    let has_focus = ui.memory(|m| m.has_focus(canvas_id));

    // 1. Terminal Toolbar
    theme::subtle_bar_frame(&p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let avail_w = ui.available_width();

        if avail_w >= 1060.0 {
            ui.horizontal(|ui| {
                render_status_badges(ui, &p, device, cur_cols, cur_rows, has_focus);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    render_session_button(ui, &p, device, &mut actions);
                    render_toolbar_controls(
                        ui,
                        &p,
                        device,
                        clamped_font,
                        canvas_id,
                        &mut actions,
                        true,
                    );
                });
            });
        } else {
            if avail_w >= 540.0 {
                ui.horizontal(|ui| {
                    render_status_badges(ui, &p, device, cur_cols, cur_rows, has_focus);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        render_session_button(ui, &p, device, &mut actions);
                    });
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    render_status_badges(ui, &p, device, cur_cols, cur_rows, has_focus);
                    render_session_button(ui, &p, device, &mut actions);
                });
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                render_toolbar_controls(
                    ui,
                    &p,
                    device,
                    clamped_font,
                    canvas_id,
                    &mut actions,
                    false,
                );
            });
        }
    });

    ui.add_space(6.0);

    // 2. Banner if terminal was closed or errored (no auto-reconnect loop!)
    match &device.terminal_state {
        TerminalSessionState::ClosedByUser => {
            theme::subtle_bar_frame(&p).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("La session terminal est terminée.")
                            .color(p.text_secondary),
                    );
                    if theme::primary_button(ui, &p, "🔄 Relancer une session").clicked() {
                        actions.push(TerminalAction::Reconnect);
                    }
                });
            });
            ui.add_space(6.0);
        }
        TerminalSessionState::Error(err) => {
            egui::Frame::new()
                .fill(p.danger_bg)
                .stroke(Stroke::new(1.0_f32, p.danger_border))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("✖ Erreur terminal : {err}")).color(p.danger),
                        );
                        if ui.button("🔄 Réessayer").clicked() {
                            actions.push(TerminalAction::Reconnect);
                        }
                    });
                });
            ui.add_space(6.0);
        }
        _ => {}
    }

    // 3. VT100 Screen Canvas
    let font_id = egui::FontId::monospace(clamped_font);
    let ppp = ui.ctx().pixels_per_point().max(0.5);
    let (char_width, row_height) = ui.fonts(|fonts| {
        let raw_cw = fonts.glyph_width(&font_id, 'M');
        let raw_rh = fonts.row_height(&font_id);
        grid_cell_metrics(raw_cw, raw_rh, ppp)
    });

    let avail = ui.available_size();
    let inner_w = (avail.x - 18.0).max(160.0);
    let inner_h = (avail.y - 24.0).max(120.0);
    let cols = (inner_w / char_width).floor().clamp(20.0, 300.0) as u16;
    let rows = (inner_h / row_height).floor().clamp(5.0, 200.0) as u16;

    if device.terminal.resize(rows, cols) {
        actions.push(TerminalAction::Resize { cols, rows });
    }

    let canvas_size = egui::vec2(cols as f32 * char_width, rows as f32 * row_height);
    let canvas_stroke = if has_focus {
        Stroke::new(1.5_f32, p.accent)
    } else {
        Stroke::new(1.0_f32, p.border_strong)
    };

    let frame_resp = egui::Frame::canvas(ui.style())
        .fill(DEFAULT_BG)
        .stroke(canvas_stroke)
        .corner_radius(8)
        .inner_margin(8.0)
        .show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(canvas_size, egui::Sense::hover());
            let response = ui.interact(rect, canvas_id, egui::Sense::click());

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
                let (events, cur_mods) = ui.input(|i| (i.events.clone(), i.modifiers));

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

            // Render VT100 grid anchored on physical pixel cell boundaries
            let origin_x = snap_to_pixel(rect.min.x, ppp);
            let origin_y = snap_to_pixel(rect.min.y, ppp);
            let painter = ui.painter_at(rect.expand(2.0));
            let screen = device.terminal.screen();
            let cursor_cell = if !screen.hide_cursor() && screen.scrollback() == 0 {
                let (cur_r, cur_c) = screen.cursor_position();
                if cur_r < rows && cur_c < cols {
                    Some((cur_r, cur_c))
                } else {
                    None
                }
            } else {
                None
            };

            let mut run_text = String::new();
            for r in 0..rows {
                let row_y = origin_y + r as f32 * row_height;
                let mut run_start_col: u16 = 0;
                let mut run_next_col: u16 = 0;
                let mut run_style: Option<(Color32, bool, bool)> = None;
                run_text.clear();

                for c in 0..cols {
                    let Some(cell) = screen.cell(r, c) else {
                        flush_ascii_run(
                            ui,
                            &painter,
                            origin_x,
                            row_y,
                            char_width,
                            &font_id,
                            run_start_col,
                            &mut run_text,
                            &mut run_style,
                        );
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

                    let is_focused_cursor = response.has_focus() && cursor_cell == Some((r, c));
                    if is_focused_cursor {
                        bg = CURSOR_BG;
                        fg = DEFAULT_BG;
                    }

                    if bg != DEFAULT_BG {
                        let cell_w = if cell.is_wide() {
                            char_width * 2.0
                        } else {
                            char_width
                        };
                        let cell_rect = egui::Rect::from_min_size(
                            egui::pos2(origin_x + c as f32 * char_width, row_y),
                            egui::vec2(cell_w, row_height),
                        );
                        painter.rect_filled(
                            cell_rect,
                            if is_focused_cursor { 1.0 } else { 0.0 },
                            bg,
                        );
                    }

                    if !cell.has_contents() {
                        if cell.underline() {
                            let style = (fg, cell.italic(), true);
                            if run_style == Some(style) && c == run_next_col {
                                run_text.push(' ');
                                run_next_col = c + 1;
                            } else {
                                flush_ascii_run(
                                    ui,
                                    &painter,
                                    origin_x,
                                    row_y,
                                    char_width,
                                    &font_id,
                                    run_start_col,
                                    &mut run_text,
                                    &mut run_style,
                                );
                                run_start_col = c;
                                run_next_col = c + 1;
                                run_text.push(' ');
                                run_style = Some(style);
                            }
                        } else {
                            flush_ascii_run(
                                ui,
                                &painter,
                                origin_x,
                                row_y,
                                char_width,
                                &font_id,
                                run_start_col,
                                &mut run_text,
                                &mut run_style,
                            );
                        }
                        continue;
                    }

                    let contents = cell.contents();
                    let style = (fg, cell.italic(), cell.underline());
                    if is_grid_ascii(&contents, cell.is_wide()) {
                        if run_style == Some(style) && c == run_next_col {
                            run_text.push_str(&contents);
                            run_next_col = c + 1;
                        } else {
                            flush_ascii_run(
                                ui,
                                &painter,
                                origin_x,
                                row_y,
                                char_width,
                                &font_id,
                                run_start_col,
                                &mut run_text,
                                &mut run_style,
                            );
                            run_start_col = c;
                            run_next_col = c + 1;
                            run_text.push_str(&contents);
                            run_style = Some(style);
                        }
                    } else {
                        flush_ascii_run(
                            ui,
                            &painter,
                            origin_x,
                            row_y,
                            char_width,
                            &font_id,
                            run_start_col,
                            &mut run_text,
                            &mut run_style,
                        );
                        paint_text_at(
                            ui,
                            &painter,
                            origin_x + c as f32 * char_width,
                            row_y,
                            &contents,
                            &font_id,
                            fg,
                            cell.italic(),
                            cell.underline(),
                        );
                    }
                }

                flush_ascii_run(
                    ui,
                    &painter,
                    origin_x,
                    row_y,
                    char_width,
                    &font_id,
                    run_start_col,
                    &mut run_text,
                    &mut run_style,
                );
            }

            // Unfocused cursor outline (focused cursor was drawn with crisp inverted cell colors)
            if !response.has_focus() {
                if let Some((cur_r, cur_c)) = cursor_cell {
                    let cur_rect = egui::Rect::from_min_size(
                        egui::pos2(
                            origin_x + cur_c as f32 * char_width,
                            origin_y + cur_r as f32 * row_height,
                        ),
                        egui::vec2(char_width, row_height),
                    );
                    painter.rect_stroke(
                        cur_rect,
                        1.0,
                        Stroke::new(1.0_f32, Color32::from_rgb(130, 145, 165)),
                        egui::StrokeKind::Inside,
                    );
                }
            }

            if !response.has_focus() {
                let hint_pos = egui::pos2(rect.right() - 8.0, rect.bottom() - 4.0);
                painter.text(
                    hint_pos,
                    egui::Align2::RIGHT_BOTTOM,
                    "Cliquez ici pour saisir (Tab, Flèches, Ctrl+C, vim, htop...)",
                    egui::FontId::proportional(11.0),
                    Color32::from_rgba_unmultiplied(180, 195, 215, 150),
                );
            }

            if screen.scrollback() > 0 {
                let sb_pos = egui::pos2(rect.right() - 8.0, rect.top() + 4.0);
                painter.text(
                    sb_pos,
                    egui::Align2::RIGHT_TOP,
                    format!("↑ Historique (+{} lignes)", screen.scrollback()),
                    egui::FontId::proportional(11.0),
                    Color32::from_rgb(251, 191, 36),
                );
            }
        });
    let _ = frame_resp;

    actions
}

#[derive(Clone, Copy)]
enum ControlGroup {
    CommandsMenu,
    FontZoom,
    ClipboardCopy,
    ClipboardPaste,
    CtrlC,
    CtrlD,
    ClearScreen,
    Sep,
}

const TOOLBAR_CONTROLS_LTR: &[ControlGroup] = &[
    ControlGroup::CommandsMenu,
    ControlGroup::Sep,
    ControlGroup::FontZoom,
    ControlGroup::Sep,
    ControlGroup::ClipboardCopy,
    ControlGroup::ClipboardPaste,
    ControlGroup::Sep,
    ControlGroup::CtrlC,
    ControlGroup::CtrlD,
    ControlGroup::ClearScreen,
];

fn render_status_badges(
    ui: &mut Ui,
    p: &Palette,
    device: &Device,
    cur_cols: u16,
    cur_rows: u16,
    has_focus: bool,
) {
    ui.label(
        RichText::new(format!("🐚 VT100 ({cur_cols}×{cur_rows})"))
            .monospace()
            .size(12.0)
            .strong()
            .color(p.text_primary),
    );

    let (st_text, st_bg, st_fg) = match &device.terminal_state {
        TerminalSessionState::Open => ("● Ouvert", p.success_bg, p.success),
        TerminalSessionState::Connecting => ("● Connexion...", p.accent_bg, p.accent),
        TerminalSessionState::ClosedByUser => ("○ Fermé", p.bg_card, p.text_muted),
        TerminalSessionState::Error(_) => ("✖ Erreur", p.danger_bg, p.danger),
        TerminalSessionState::Idle => ("○ Prêt", p.warning_bg, p.warning),
    };
    theme::pill_badge(ui, st_text, st_bg, st_fg, None);

    if has_focus {
        theme::pill_badge(ui, "⌨ Clavier actif", p.accent_bg, p.accent, Some(p.accent_border));
    } else {
        theme::pill_badge(
            ui,
            "Cliquer pour saisir",
            p.bg_card,
            p.text_muted,
            Some(p.border),
        );
    }
}

fn render_session_button(
    ui: &mut Ui,
    p: &Palette,
    device: &Device,
    actions: &mut Vec<TerminalAction>,
) {
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
    } else if theme::primary_button(ui, p, "▶ Ouvrir")
        .on_hover_text("Ouvrir une nouvelle session terminal")
        .clicked()
    {
        actions.push(TerminalAction::Connect);
    }
}

fn render_toolbar_controls(
    ui: &mut Ui,
    p: &Palette,
    device: &mut Device,
    clamped_font: f32,
    canvas_id: egui::Id,
    actions: &mut Vec<TerminalAction>,
    rtl: bool,
) {
    let iter: Box<dyn Iterator<Item = &ControlGroup>> = if rtl {
        Box::new(TOOLBAR_CONTROLS_LTR.iter().rev())
    } else {
        Box::new(TOOLBAR_CONTROLS_LTR.iter())
    };

    for item in iter {
        match item {
            ControlGroup::CommandsMenu => {
                ui.menu_button("⚡ Commandes", |ui| {
                    for &(label, cmd) in QUICK_COMMANDS {
                        if ui.button(label).clicked() {
                            let mut payload = cmd.as_bytes().to_vec();
                            payload.push(b'\r');
                            actions.push(TerminalAction::SendData(payload));
                            ui.memory_mut(|m| m.request_focus(canvas_id));
                            ui.close_menu();
                        }
                    }
                });
            }
            ControlGroup::FontZoom => {
                render_font_zoom_controls(ui, p, clamped_font, rtl, actions);
            }
            ControlGroup::ClipboardCopy => {
                if ui
                    .button("📋 Copier")
                    .on_hover_text("Copier le texte visible du terminal (Ctrl+Maj+C)")
                    .clicked()
                {
                    ui.ctx().copy_text(device.terminal.visible_plain_text());
                }
            }
            ControlGroup::ClipboardPaste => {
                if ui
                    .button("📥 Coller")
                    .on_hover_text("Coller le contenu du presse-papier dans le terminal")
                    .clicked()
                {
                    ui.memory_mut(|m| m.request_focus(canvas_id));
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                }
            }
            ControlGroup::CtrlC => {
                if ui
                    .button("⏹ Ctrl+C")
                    .on_hover_text("Envoyer SIGINT (0x03)")
                    .clicked()
                {
                    actions.push(TerminalAction::SendData(vec![0x03]));
                }
            }
            ControlGroup::CtrlD => {
                if ui
                    .button("⏏ Ctrl+D")
                    .on_hover_text("Envoyer EOF (0x04)")
                    .clicked()
                {
                    actions.push(TerminalAction::SendData(vec![0x04]));
                }
            }
            ControlGroup::ClearScreen => {
                if ui
                    .button("♻ Effacer")
                    .on_hover_text("Effacer l'écran et le tampon du terminal")
                    .clicked()
                {
                    device.terminal.clear();
                    actions.push(TerminalAction::Clear);
                }
            }
            ControlGroup::Sep => {
                ui.separator();
            }
        }
    }
}

fn render_font_zoom_controls(
    ui: &mut Ui,
    p: &Palette,
    clamped_font: f32,
    rtl: bool,
    actions: &mut Vec<TerminalAction>,
) {
    let dec_btn = |ui: &mut Ui, actions: &mut Vec<TerminalAction>| {
        if ui
            .small_button("A-")
            .on_hover_text("Réduire la police du terminal")
            .clicked()
        {
            actions.push(TerminalAction::SetFontSize(
                (clamped_font - 1.0).clamp(10.0, 20.0),
            ));
        }
    };
    let inc_btn = |ui: &mut Ui, actions: &mut Vec<TerminalAction>| {
        if ui
            .small_button("A+")
            .on_hover_text("Agrandir la police du terminal")
            .clicked()
        {
            actions.push(TerminalAction::SetFontSize(
                (clamped_font + 1.0).clamp(10.0, 20.0),
            ));
        }
    };
    let label = |ui: &mut Ui| {
        ui.label(
            RichText::new(format!("{clamped_font:.0}px"))
                .monospace()
                .size(11.0)
                .color(p.text_secondary),
        );
    };

    if rtl {
        inc_btn(ui, actions);
        label(ui);
        dec_btn(ui, actions);
    } else {
        dec_btn(ui, actions);
        label(ui);
        inc_btn(ui, actions);
    }
}

#[allow(clippy::too_many_arguments)]
fn flush_ascii_run(
    ui: &Ui,
    painter: &egui::Painter,
    origin_x: f32,
    row_y: f32,
    char_width: f32,
    font_id: &egui::FontId,
    start_col: u16,
    run_text: &mut String,
    run_style: &mut Option<(Color32, bool, bool)>,
) {
    if let Some((fg, italic, underline)) = run_style.take() {
        if !run_text.is_empty() {
            paint_text_at(
                ui,
                painter,
                origin_x + start_col as f32 * char_width,
                row_y,
                run_text,
                font_id,
                fg,
                italic,
                underline,
            );
            run_text.clear();
        }
    } else {
        run_text.clear();
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_text_at(
    ui: &Ui,
    painter: &egui::Painter,
    x: f32,
    row_y: f32,
    text: &str,
    font_id: &egui::FontId,
    fg: Color32,
    italic: bool,
    underline: bool,
) {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    append_run(&mut job, text, font_id, fg, italic, underline);
    let galley = ui.fonts(|f| f.layout_job(job));
    painter.galley(egui::pos2(x, row_y), galley, DEFAULT_FG);
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
        0 => Color32::from_rgb(30, 34, 42),
        1 => Color32::from_rgb(240, 92, 92),
        2 => Color32::from_rgb(76, 209, 130),
        3 => Color32::from_rgb(245, 200, 75),
        4 => Color32::from_rgb(88, 155, 245),
        5 => Color32::from_rgb(200, 115, 235),
        6 => Color32::from_rgb(56, 210, 228),
        7 => Color32::from_rgb(215, 222, 230),
        8 => Color32::from_rgb(110, 120, 136),
        9 => Color32::from_rgb(255, 125, 125),
        10 => Color32::from_rgb(110, 240, 155),
        11 => Color32::from_rgb(255, 230, 120),
        12 => Color32::from_rgb(130, 185, 255),
        13 => Color32::from_rgb(235, 145, 255),
        14 => Color32::from_rgb(115, 235, 250),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grid_cell_metrics_snaps_to_physical_pixels() {
        let raw_cw = 13.0 * 1233.0 / 2048.0; // 7.82666pt in Hack-Regular at 13px
        let raw_rh = 15.2;

        for ppp in [1.0_f32, 1.25, 1.5, 2.0] {
            let (cw, rh) = grid_cell_metrics(raw_cw, raw_rh, ppp);
            assert!(
                (cw - 8.0).abs() < 1e-4,
                "expected snapped cell width ~8.0pt at ppp={ppp}, got {cw}"
            );
            assert!((rh * ppp - (rh * ppp).round()).abs() < 1e-4);
            assert!(cw >= 5.5);
            assert!(rh >= 12.0);
        }

        let (min_cw, min_rh) = grid_cell_metrics(1.0, 2.0, 0.1);
        assert!(min_cw >= 5.5);
        assert!(min_rh >= 12.0);
    }

    #[test]
    fn test_is_grid_ascii() {
        assert!(is_grid_ascii("A", false));
        assert!(is_grid_ascii(" ", false));
        assert!(is_grid_ascii("~", false));
        assert!(!is_grid_ascii("é", false));
        assert!(!is_grid_ascii("🐧", false));
        assert!(!is_grid_ascii("W", true));
        assert!(!is_grid_ascii("ab", false));
        assert!(!is_grid_ascii("", false));
    }
}
