use crate::ui::theme::{self, Palette};
use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{Color32, CornerRadius, FontId, Key, Margin, Modifiers, RichText, Stroke, Ui};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Default ring-buffer capacity for live service & container console output.
pub const DEFAULT_MAX_LINES: usize = 2000;

/// Maximum number of commands retained in [`ConsoleWidgetState::command_history`].
const MAX_COMMAND_HISTORY: usize = 100;

/// Safety cap for a single console line (in bytes) to protect the UI against unbounded binary dumps.
const MAX_LINE_BYTES: usize = 16_384;

/// Safety cap for CSI/OSC escape sequence buffering against malformed streams.
const MAX_CSI_PARAMS: usize = 128;
const MAX_OSC_LEN: usize = 2048;

/// Dedicated dark console canvas & log foreground palette (matching `terminal.rs`).
const CONSOLE_BG: Color32 = Color32::from_rgb(11, 15, 20);
const CONSOLE_DEFAULT_FG: Color32 = Color32::from_rgb(220, 228, 238);
const CONSOLE_BOLD_FG: Color32 = Color32::from_rgb(248, 250, 252);
const CONSOLE_MUTED_FG: Color32 = Color32::from_rgb(122, 134, 152);
const CONSOLE_SEARCH_HL_BG: Color32 = Color32::from_rgb(180, 120, 20);
const CONSOLE_SEARCH_HL_FG: Color32 = Color32::from_rgb(255, 255, 255);

/// Styled text segment within a [`ConsoleLine`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleSpan {
    pub text: String,
    pub color: Option<Color32>,
    pub bold: bool,
}

/// Single logical console line preserving ANSI-colored spans alongside plain text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleLine {
    pub spans: Vec<ConsoleSpan>,
    pub plain_text: String,
}

impl ConsoleLine {
    fn empty() -> Self {
        Self {
            spans: Vec::new(),
            plain_text: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AnsiParseState {
    Ground,
    Escape,
    EscapeCharset,
    Csi { buf: String },
    Osc { esc_seen: bool, len: usize },
    StringSeq { esc_seen: bool, len: usize },
}

/// Bounded streaming ring buffer for live service & Docker logs with ANSI SGR parsing.
#[derive(Debug, Clone)]
pub struct ConsoleBuffer {
    lines: VecDeque<ConsoleLine>,
    max_lines: usize,
    has_open_line: bool,
    pending_cr: bool,
    utf8_pending: Vec<u8>,
    ansi_state: AnsiParseState,
    current_color: Option<Color32>,
    current_bold: bool,
}

impl Default for ConsoleBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_LINES)
    }
}

impl ConsoleBuffer {
    /// Create an empty console ring buffer keeping at most `max_lines` lines.
    pub fn new(max_lines: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(max_lines.min(256)),
            max_lines,
            has_open_line: false,
            pending_cr: false,
            utf8_pending: Vec::new(),
            ansi_state: AnsiParseState::Ground,
            current_color: None,
            current_bold: false,
        }
    }

    /// Append raw UTF-8/ANSI bytes from a streaming source, updating partial lines,
    /// handling `\r\n` and `\r` overwrites, parsing SGR colors, and evicting old lines.
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let combined: Vec<u8>;
        let mut remaining: &[u8] = if self.utf8_pending.is_empty() {
            bytes
        } else {
            let mut tmp = std::mem::take(&mut self.utf8_pending);
            tmp.extend_from_slice(bytes);
            combined = tmp;
            &combined
        };

        while !remaining.is_empty() {
            match std::str::from_utf8(remaining) {
                Ok(valid) => {
                    for ch in valid.chars() {
                        self.feed_char(ch);
                    }
                    break;
                }
                Err(err) => {
                    let valid_up_to = err.valid_up_to();
                    if valid_up_to > 0 {
                        if let Ok(valid) = std::str::from_utf8(&remaining[..valid_up_to]) {
                            for ch in valid.chars() {
                                self.feed_char(ch);
                            }
                        }
                    }
                    match err.error_len() {
                        Some(invalid_len) => {
                            self.feed_char('\u{FFFD}');
                            remaining = &remaining[valid_up_to + invalid_len..];
                        }
                        None => {
                            // Incomplete multi-byte UTF-8 sequence at chunk boundary.
                            self.utf8_pending.extend_from_slice(&remaining[valid_up_to..]);
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Push a local system notice (e.g. `[Connecté au flux live]` or `> say Bonjour`).
    pub fn push_system_line(&mut self, msg: &str, color: Option<Color32>) {
        if self.max_lines == 0 {
            return;
        }
        if self.has_open_line {
            self.has_open_line = false;
            self.pending_cr = false;
        }

        if msg.is_empty() {
            self.push_line_with_eviction(ConsoleLine::empty());
            return;
        }

        for part in msg.split('\n') {
            let clean = part.trim_end_matches('\r');
            let line = if clean.is_empty() {
                ConsoleLine::empty()
            } else {
                ConsoleLine {
                    spans: vec![ConsoleSpan {
                        text: clean.to_string(),
                        color,
                        bold: false,
                    }],
                    plain_text: clean.to_string(),
                }
            };
            self.push_line_with_eviction(line);
        }
    }

    /// Clear all buffered lines and reset parser state.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.has_open_line = false;
        self.pending_cr = false;
        self.utf8_pending.clear();
        self.ansi_state = AnsiParseState::Ground;
        self.current_color = None;
        self.current_bold = false;
    }

    /// Borrow all lines currently stored in the ring buffer (including any active partial line).
    pub fn lines(&self) -> &VecDeque<ConsoleLine> {
        &self.lines
    }

    /// Concatenate all plain-text lines separated by `\n` (for 1-click clipboard copy).
    pub fn plain_text_all(&self) -> String {
        let total_len: usize = self
            .lines
            .iter()
            .map(|l| l.plain_text.len() + 1)
            .sum();
        let mut out = String::with_capacity(total_len);
        for (idx, line) in self.lines.iter().enumerate() {
            if idx > 0 {
                out.push('\n');
            }
            out.push_str(&line.plain_text);
        }
        out
    }

    /// Return the total number of lines currently in the buffer.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Return whether the buffer contains no lines.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    fn push_line_with_eviction(&mut self, line: ConsoleLine) {
        if self.max_lines == 0 {
            return;
        }
        if self.lines.len() >= self.max_lines {
            let excess = self.lines.len() + 1 - self.max_lines;
            for _ in 0..excess {
                self.lines.pop_front();
            }
        }
        self.lines.push_back(line);
    }

    fn feed_char(&mut self, ch: char) {
        match &mut self.ansi_state {
            AnsiParseState::Ground => self.handle_ground_char(ch),
            AnsiParseState::Escape => match ch {
                '[' => {
                    self.ansi_state = AnsiParseState::Csi { buf: String::new() };
                }
                ']' => {
                    self.ansi_state = AnsiParseState::Osc {
                        esc_seen: false,
                        len: 0,
                    };
                }
                'P' | '_' | '^' | 'X' => {
                    self.ansi_state = AnsiParseState::StringSeq {
                        esc_seen: false,
                        len: 0,
                    };
                }
                '(' | ')' | '*' | '+' => {
                    self.ansi_state = AnsiParseState::EscapeCharset;
                }
                '\x1b' => {
                    // Stay in Escape if repeated ESC arrives.
                }
                _ => {
                    // 2-byte ESC sequence (e.g. ESC =, ESC >, ESC M, ESC 7, ESC 8) -> ignore.
                    self.ansi_state = AnsiParseState::Ground;
                }
            },
            AnsiParseState::EscapeCharset => {
                // Consume the single charset designator byte and return to Ground.
                self.ansi_state = AnsiParseState::Ground;
            }
            AnsiParseState::Csi { buf } => {
                let code = ch as u32;
                if (0x40..=0x7E).contains(&code) {
                    let params = std::mem::take(buf);
                    self.ansi_state = AnsiParseState::Ground;
                    self.handle_csi(&params, ch);
                } else if (0x20..=0x3F).contains(&code) {
                    if buf.len() < MAX_CSI_PARAMS {
                        buf.push(ch);
                    } else {
                        self.ansi_state = AnsiParseState::Ground;
                    }
                } else {
                    // Unexpected control/text char aborts malformed CSI cleanly.
                    self.ansi_state = AnsiParseState::Ground;
                    self.handle_ground_char(ch);
                }
            }
            AnsiParseState::Osc { esc_seen, len } | AnsiParseState::StringSeq { esc_seen, len } => {
                if ch == '\x07' || (*esc_seen && ch == '\\') {
                    self.ansi_state = AnsiParseState::Ground;
                } else if ch == '\x1b' {
                    *esc_seen = true;
                } else {
                    *esc_seen = false;
                    *len += 1;
                    if *len >= MAX_OSC_LEN {
                        self.ansi_state = AnsiParseState::Ground;
                    }
                }
            }
        }
    }

    fn handle_ground_char(&mut self, ch: char) {
        match ch {
            '\x1b' => {
                self.ansi_state = AnsiParseState::Escape;
            }
            '\r' => {
                self.pending_cr = true;
            }
            '\n' => {
                self.pending_cr = false;
                if self.has_open_line {
                    self.has_open_line = false;
                } else {
                    self.push_line_with_eviction(ConsoleLine::empty());
                }
            }
            '\t' => {
                self.append_printable_char('\t');
            }
            c if c.is_control() => {
                // Strip non-printable C0/DEL control bytes cleanly.
            }
            c => {
                self.append_printable_char(c);
            }
        }
    }

    fn append_printable_char(&mut self, ch: char) {
        if self.max_lines == 0 {
            return;
        }
        if self.pending_cr {
            if self.has_open_line {
                if let Some(line) = self.lines.back_mut() {
                    line.spans.clear();
                    line.plain_text.clear();
                }
            }
            self.pending_cr = false;
        }
        if !self.has_open_line {
            self.push_line_with_eviction(ConsoleLine::empty());
            self.has_open_line = true;
        }
        if let Some(line) = self.lines.back_mut() {
            if line.plain_text.len() >= MAX_LINE_BYTES {
                return;
            }
            line.plain_text.push(ch);
            let same_style = line
                .spans
                .last()
                .is_some_and(|s| s.color == self.current_color && s.bold == self.current_bold);
            if same_style {
                if let Some(last) = line.spans.last_mut() {
                    last.text.push(ch);
                }
            } else {
                line.spans.push(ConsoleSpan {
                    text: ch.to_string(),
                    color: self.current_color,
                    bold: self.current_bold,
                });
            }
        }
    }

    fn handle_csi(&mut self, params: &str, final_char: char) {
        match final_char {
            'm' if !params.starts_with(['?', '>', '<', '=']) => {
                // Ignore private-mode SGR prefixes (`?`, `>`, `<`, `=`).
                self.apply_sgr(params);
            }
            'K' if self.pending_cr && self.has_open_line => {
                // Erase in Line right after `\r` (common in Docker/progress spinners).
                if let Some(line) = self.lines.back_mut() {
                    line.spans.clear();
                    line.plain_text.clear();
                }
                self.pending_cr = false;
            }
            _ => {
                // Strip non-SGR CSI cursor/screen control sequences cleanly.
            }
        }
    }

    fn apply_sgr(&mut self, params: &str) {
        if params.is_empty() {
            self.current_color = None;
            self.current_bold = false;
            return;
        }

        let tokens: Vec<&str> = params.split(';').collect();
        let mut i = 0;
        while i < tokens.len() {
            let tok = tokens[i];
            if tok.contains(':') {
                self.apply_colon_sgr(tok);
                i += 1;
                continue;
            }

            let code: u16 = if tok.is_empty() {
                0
            } else if let Ok(v) = tok.parse::<u16>() {
                v
            } else {
                i += 1;
                continue;
            };

            match code {
                0 => {
                    self.current_color = None;
                    self.current_bold = false;
                }
                1 => {
                    self.current_bold = true;
                }
                22 => {
                    self.current_bold = false;
                }
                30..=37 => {
                    self.current_color = Some(ansi_idx_to_color((code - 30) as u8));
                }
                39 => {
                    self.current_color = None;
                }
                90..=97 => {
                    self.current_color = Some(ansi_idx_to_color((code - 90 + 8) as u8));
                }
                38 => {
                    // Extended foreground: 38;5;n or 38;2;r;g;b
                    if let Some(mode) = tokens.get(i + 1).and_then(|s| s.parse::<u16>().ok()) {
                        match mode {
                            5 => {
                                if let Some(idx) =
                                    tokens.get(i + 2).and_then(|s| s.parse::<u8>().ok())
                                {
                                    self.current_color = Some(ansi_idx_to_color(idx));
                                }
                                i += 2;
                            }
                            2 => {
                                if let (Some(r), Some(g), Some(b)) = (
                                    tokens.get(i + 2).and_then(|s| s.parse::<u8>().ok()),
                                    tokens.get(i + 3).and_then(|s| s.parse::<u8>().ok()),
                                    tokens.get(i + 4).and_then(|s| s.parse::<u8>().ok()),
                                ) {
                                    self.current_color = Some(Color32::from_rgb(r, g, b));
                                }
                                i += 4;
                            }
                            _ => {}
                        }
                    }
                }
                48 => {
                    // Consume extended background parameters so they aren't parsed as SGR codes.
                    if let Some(mode) = tokens.get(i + 1).and_then(|s| s.parse::<u16>().ok()) {
                        match mode {
                            5 => i += 2,
                            2 => i += 4,
                            _ => {}
                        }
                    }
                }
                _ => {
                    // Other SGR attributes (dim, italic, underline, bg 40..=49/100..=107) ignored.
                }
            }
            i += 1;
        }
    }

    fn apply_colon_sgr(&mut self, token: &str) {
        let parts: Vec<&str> = token.split(':').collect();
        if parts.first().copied() != Some("38") {
            return;
        }
        match parts.get(1).copied() {
            Some("5") => {
                if let Some(idx) = parts.get(2).and_then(|s| s.parse::<u8>().ok()) {
                    self.current_color = Some(ansi_idx_to_color(idx));
                }
            }
            Some("2") => {
                // Support both `38:2:r:g:b` and ITU T.416 `38:2:<colorspace>:r:g:b`.
                let rgb_offset = if parts.len() >= 6 { 3 } else { 2 };
                if let (Some(r), Some(g), Some(b)) = (
                    parts.get(rgb_offset).and_then(|s| s.parse::<u8>().ok()),
                    parts.get(rgb_offset + 1).and_then(|s| s.parse::<u8>().ok()),
                    parts.get(rgb_offset + 2).and_then(|s| s.parse::<u8>().ok()),
                ) {
                    self.current_color = Some(Color32::from_rgb(r, g, b));
                }
            }
            _ => {}
        }
    }
}

/// Interactive UI state for a live console widget instance.
#[derive(Debug, Clone)]
pub struct ConsoleWidgetState {
    pub input: String,
    pub search_query: String,
    pub auto_scroll: bool,
    pub command_history: Vec<String>,
    pub history_cursor: Option<usize>,
    pub copied_feedback_until: Option<Instant>,
}

impl Default for ConsoleWidgetState {
    fn default() -> Self {
        Self {
            input: String::new(),
            search_query: String::new(),
            auto_scroll: true,
            command_history: Vec::new(),
            history_cursor: None,
            copied_feedback_until: None,
        }
    }
}

impl ConsoleWidgetState {
    /// Push a submitted command to history (deduplicating consecutive identical commands,
    /// capped at 100 entries) and reset the history navigation cursor.
    pub fn record_command(&mut self, cmd: String) {
        if self.command_history.last().map(String::as_str) != Some(cmd.as_str()) {
            self.command_history.push(cmd);
            if self.command_history.len() > MAX_COMMAND_HISTORY {
                let excess = self.command_history.len() - MAX_COMMAND_HISTORY;
                self.command_history.drain(0..excess);
            }
        }
        self.history_cursor = None;
    }

    /// Navigate backward (older) in command history. Returns `true` if `self.input` changed.
    pub fn history_prev(&mut self) -> bool {
        if self.command_history.is_empty() {
            return false;
        }
        let next_idx = match self.history_cursor {
            None => self.command_history.len() - 1,
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_cursor = Some(next_idx);
        self.input = self.command_history[next_idx].clone();
        true
    }

    /// Navigate forward (newer) in command history. Returns `true` if `self.input` changed.
    pub fn history_next(&mut self) -> bool {
        let Some(cur) = self.history_cursor else {
            return false;
        };
        if cur + 1 < self.command_history.len() {
            let next_idx = cur + 1;
            self.history_cursor = Some(next_idx);
            self.input = self.command_history[next_idx].clone();
        } else {
            self.history_cursor = None;
            self.input.clear();
        }
        true
    }
}

/// Configuration parameters passed to [`render`] on each frame.
#[derive(Debug, Clone)]
pub struct ConsoleWidgetConfig<'a> {
    pub id_salt: &'a str,
    pub connected_label: &'a str,
    pub is_streaming: bool,
    pub can_send_input: bool,
    pub input_placeholder: &'a str,
    pub quick_commands: &'a [String],
    pub empty_hint: &'a str,
}

/// User actions emitted by [`render`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleWidgetAction {
    SendCommand(String),
    Reconnect,
    ClearBuffer,
}

/// Render a reusable Pterodactyl/Cockpit-style live console widget with toolbar,
/// virtualized ANSI output viewport, optional quick-command chips, and command bar.
pub fn render(
    ui: &mut Ui,
    p: &Palette,
    buffer: &ConsoleBuffer,
    state: &mut ConsoleWidgetState,
    cfg: &ConsoleWidgetConfig<'_>,
) -> Option<ConsoleWidgetAction> {
    let mut action: Option<ConsoleWidgetAction> = None;

    // Precompute filtered indices once for both toolbar counter and virtualized viewport.
    let query_trimmed = state.search_query.trim().to_string();
    let query_lower = query_trimmed.to_lowercase();
    let total_lines = buffer.line_count();
    let filtered_indices: Option<Vec<usize>> = if query_lower.is_empty() {
        None
    } else {
        Some(
            buffer
                .lines()
                .iter()
                .enumerate()
                .filter(|(_, line)| line.plain_text.to_lowercase().contains(&query_lower))
                .map(|(i, _)| i)
                .collect(),
        )
    };
    let visible_count = filtered_indices
        .as_ref()
        .map_or(total_lines, Vec::len);

    // 1. Top Console Mini-Toolbar (responsive — never right_to_left inside horizontal_wrapped!)
    theme::subtle_bar_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let avail_w = ui.available_width();

        if avail_w >= 760.0 {
            ui.horizontal(|ui| {
                render_status_and_counter(
                    ui,
                    p,
                    cfg,
                    total_lines,
                    visible_count,
                    !query_lower.is_empty(),
                );
                ui.add_space(4.0);
                let search_w = if avail_w >= 880.0 { 200.0 } else { 150.0 };
                render_search_box(ui, p, state, cfg.id_salt, search_w);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    render_toolbar_buttons(ui, p, buffer, state, cfg, true, &mut action);
                });
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                render_status_and_counter(
                    ui,
                    p,
                    cfg,
                    total_lines,
                    visible_count,
                    !query_lower.is_empty(),
                );
                render_toolbar_buttons(ui, p, buffer, state, cfg, false, &mut action);
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let clear_btn_w = if state.search_query.is_empty() {
                    0.0
                } else {
                    28.0
                };
                let search_w = (ui.available_width() - 26.0 - clear_btn_w).max(120.0);
                render_search_box(ui, p, state, cfg.id_salt, search_w);
            });
        }
    });

    ui.add_space(6.0);

    // 2. Dark Monospace Console Output Viewport (#0B0F14)
    let has_quick_cmds = !cfg.quick_commands.is_empty() && cfg.can_send_input;
    let bottom_reserve = if has_quick_cmds { 104.0 } else { 70.0 };
    let viewport_height = (ui.available_height() - bottom_reserve).max(120.0);
    let console_font = FontId::monospace(12.5);
    let row_height = ui.fonts(|f| f.row_height(&console_font)).max(15.0);

    egui::Frame::new()
        .fill(CONSOLE_BG)
        .stroke(Stroke::new(1.0_f32, p.border_strong))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            if total_lines == 0 {
                let hint = if cfg.empty_hint.is_empty() {
                    "Aucune sortie pour le moment."
                } else {
                    cfg.empty_hint
                };
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), viewport_height),
                    egui::Layout::centered_and_justified(egui::Direction::TopDown),
                    |ui| {
                        ui.label(
                            RichText::new(hint)
                                .monospace()
                                .size(12.5)
                                .color(CONSOLE_MUTED_FG),
                        );
                    },
                );
            } else if visible_count == 0 {
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), viewport_height),
                    egui::Layout::centered_and_justified(egui::Direction::TopDown),
                    |ui| {
                        ui.label(
                            RichText::new(format!(
                                "Aucune ligne ne correspond au filtre « {query_trimmed} »"
                            ))
                            .monospace()
                            .size(12.5)
                            .color(CONSOLE_MUTED_FG),
                        );
                    },
                );
            } else {
                ui.spacing_mut().item_spacing.y = 2.0;
                egui::ScrollArea::both()
                    .id_salt((cfg.id_salt, "console_viewport_scroll"))
                    .auto_shrink([false, false])
                    .max_height(viewport_height)
                    .min_scrolled_height(viewport_height)
                    .stick_to_bottom(state.auto_scroll)
                    .show_rows(ui, row_height, visible_count, |ui, row_range| {
                        let lines = buffer.lines();
                        for row_idx in row_range {
                            let line_opt = match &filtered_indices {
                                Some(indices) => indices.get(row_idx).and_then(|&i| lines.get(i)),
                                None => lines.get(row_idx),
                            };
                            let Some(line) = line_opt else {
                                continue;
                            };
                            let job = build_line_layout_job(
                                line,
                                &console_font,
                                if query_lower.is_empty() {
                                    None
                                } else {
                                    Some(&query_lower)
                                },
                            );
                            ui.add(
                                egui::Label::new(job)
                                    .wrap_mode(egui::TextWrapMode::Extend)
                                    .selectable(true),
                            );
                        }
                    });
            }
        });

    // 3. Optional Quick-Command Preset Chips
    if has_quick_cmds {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("⚡").color(p.text_muted).size(12.0));
            for cmd in cfg.quick_commands {
                let chip = egui::Button::new(
                    RichText::new(cmd)
                        .monospace()
                        .size(11.5)
                        .color(p.text_primary),
                )
                .fill(p.bg_subtle)
                .stroke(Stroke::new(1.0_f32, p.border))
                .corner_radius(CornerRadius::same(10));
                if ui
                    .add(chip)
                    .on_hover_text(format!("Envoyer la commande « {cmd} »"))
                    .clicked()
                {
                    action = Some(ConsoleWidgetAction::SendCommand(cmd.clone()));
                }
            }
        });
    }

    ui.add_space(6.0);

    // 4. Bottom Real-Time Command Input Bar
    render_command_bar(ui, p, state, cfg, &mut action);

    action
}

fn status_badge_colors(p: &Palette, cfg: &ConsoleWidgetConfig<'_>) -> (Color32, Color32, Color32) {
    if cfg.is_streaming {
        return (p.success_bg, p.success, p.success_border);
    }
    let lower = cfg.connected_label.to_lowercase();
    if lower.contains("erreur") || lower.contains("crash") || cfg.connected_label.contains('✖') {
        (p.danger_bg, p.danger, p.danger_border)
    } else if lower.contains("connexion")
        || lower.contains("démarrage")
        || cfg.connected_label.contains("...")
    {
        (p.accent_bg, p.accent, p.accent_border)
    } else {
        (p.bg_card, p.text_muted, p.border)
    }
}

fn render_status_and_counter(
    ui: &mut Ui,
    p: &Palette,
    cfg: &ConsoleWidgetConfig<'_>,
    total_lines: usize,
    visible_count: usize,
    is_filtered: bool,
) {
    if !cfg.connected_label.is_empty() {
        let (bg, fg, border) = status_badge_colors(p, cfg);
        theme::pill_badge(ui, cfg.connected_label, bg, fg, Some(border));
    }

    let count_text = if is_filtered {
        format!("{visible_count} / {total_lines}")
    } else if total_lines == 1 {
        "1 ligne".to_string()
    } else {
        format!("{total_lines} lignes")
    };
    let count_color = if is_filtered { p.accent } else { p.text_muted };
    ui.label(
        RichText::new(count_text)
            .monospace()
            .size(11.5)
            .color(count_color),
    );
}

fn render_search_box(
    ui: &mut Ui,
    p: &Palette,
    state: &mut ConsoleWidgetState,
    id_salt: &str,
    width: f32,
) {
    ui.label(RichText::new("🔍").size(12.5).color(p.text_muted));
    ui.add(
        egui::TextEdit::singleline(&mut state.search_query)
            .id_salt((id_salt, "console_search_input"))
            .hint_text("Filtrer les logs...")
            .desired_width(width),
    );
    if !state.search_query.is_empty()
        && ui
            .small_button("✖")
            .on_hover_text("Effacer le filtre")
            .clicked()
    {
        state.search_query.clear();
    }
}

#[allow(clippy::too_many_arguments)]
fn render_toolbar_buttons(
    ui: &mut Ui,
    p: &Palette,
    buffer: &ConsoleBuffer,
    state: &mut ConsoleWidgetState,
    cfg: &ConsoleWidgetConfig<'_>,
    rtl: bool,
    action: &mut Option<ConsoleWidgetAction>,
) {
    let now = Instant::now();
    let copied_active = match state.copied_feedback_until {
        Some(until) if now < until => {
            ui.ctx().request_repaint_after(
                until
                    .saturating_duration_since(now)
                    .min(Duration::from_millis(250)),
            );
            true
        }
        Some(_) => {
            state.copied_feedback_until = None;
            false
        }
        None => false,
    };

    let draw_reconnect = |ui: &mut Ui, action: &mut Option<ConsoleWidgetAction>| {
        if !cfg.is_streaming
            && theme::primary_button(ui, p, "🔄 Reconnecter")
                .on_hover_text("Reconnecter au flux live de la console")
                .clicked()
        {
            *action = Some(ConsoleWidgetAction::Reconnect);
        }
    };

    let draw_clear = |ui: &mut Ui, action: &mut Option<ConsoleWidgetAction>| {
        if ui
            .button("♻ Effacer")
            .on_hover_text("Vider l'affichage local de la console")
            .clicked()
        {
            *action = Some(ConsoleWidgetAction::ClearBuffer);
        }
    };

    let draw_copy = |ui: &mut Ui, state: &mut ConsoleWidgetState| {
        let btn = if copied_active {
            egui::Button::new(RichText::new("✔ Copié").color(p.success).strong())
                .fill(p.success_bg)
                .stroke(Stroke::new(1.0_f32, p.success_border))
                .corner_radius(CornerRadius::same(6))
        } else {
            egui::Button::new(RichText::new("📋 Copier").color(p.text_primary))
                .corner_radius(CornerRadius::same(6))
        };
        if ui
            .add(btn)
            .on_hover_text("Copier tout le contenu de la console")
            .clicked()
        {
            ui.ctx().copy_text(buffer.plain_text_all());
            state.copied_feedback_until = Some(Instant::now() + Duration::from_secs(2));
        }
    };

    let draw_autoscroll = |ui: &mut Ui, state: &mut ConsoleWidgetState| {
        let (fill, stroke, fg) = if state.auto_scroll {
            (
                p.accent_bg,
                Stroke::new(1.0_f32, p.accent_border),
                p.accent,
            )
        } else {
            (
                p.bg_card,
                Stroke::new(1.0_f32, p.border),
                p.text_secondary,
            )
        };
        let mut rich = RichText::new("⬇ Suivre").color(fg);
        if state.auto_scroll {
            rich = rich.strong();
        }
        let btn = egui::Button::new(rich)
            .fill(fill)
            .stroke(stroke)
            .corner_radius(CornerRadius::same(6));
        if ui
            .add(btn)
            .on_hover_text("Défilement automatique vers les derniers logs")
            .clicked()
        {
            state.auto_scroll = !state.auto_scroll;
        }
    };

    if rtl {
        draw_reconnect(ui, action);
        draw_clear(ui, action);
        draw_copy(ui, state);
        draw_autoscroll(ui, state);
    } else {
        draw_autoscroll(ui, state);
        draw_copy(ui, state);
        draw_clear(ui, action);
        draw_reconnect(ui, action);
    }
}

fn render_command_bar(
    ui: &mut Ui,
    p: &Palette,
    state: &mut ConsoleWidgetState,
    cfg: &ConsoleWidgetConfig<'_>,
    action: &mut Option<ConsoleWidgetAction>,
) {
    let input_id = ui.make_persistent_id((cfg.id_salt, "console_cmd_input"));

    // Handle ArrowUp / ArrowDown BEFORE rendering TextEdit so recalled history appears
    // on the same frame with the caret cleanly placed at the end of the line.
    if cfg.can_send_input && ui.memory(|m| m.has_focus(input_id)) {
        let arrow_up = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp));
        let arrow_down = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown));
        let navigated = if arrow_up {
            state.history_prev()
        } else if arrow_down {
            state.history_next()
        } else {
            false
        };
        if navigated {
            if let Some(mut te_state) = egui::TextEdit::load_state(ui.ctx(), input_id) {
                let end_pos = CCursor::new(state.input.chars().count());
                te_state
                    .cursor
                    .set_char_range(Some(CCursorRange::one(end_pos)));
                egui::TextEdit::store_state(ui.ctx(), input_id, te_state);
            }
        }
    }

    theme::subtle_bar_frame(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let send_btn_w = 104.0_f32;
        let prompt_w = 16.0_f32;
        let spacing = ui.spacing().item_spacing.x;
        let input_w = (ui.available_width() - prompt_w - send_btn_w - spacing * 2.0).max(80.0);

        let mut submit_requested = false;
        let mut keep_focus = false;

        ui.horizontal(|ui| {
            let prompt_color = if cfg.can_send_input {
                p.accent
            } else {
                p.text_muted
            };
            ui.label(
                RichText::new(">")
                    .monospace()
                    .size(14.0)
                    .strong()
                    .color(prompt_color),
            );

            let placeholder = if !cfg.input_placeholder.is_empty() {
                cfg.input_placeholder
            } else if cfg.can_send_input {
                "Envoyer une commande..."
            } else {
                "Entrée clavier indisponible (lecture seule ou arrêté)"
            };

            let mut text_resp = ui.add_enabled(
                cfg.can_send_input,
                egui::TextEdit::singleline(&mut state.input)
                    .id(input_id)
                    .font(egui::TextStyle::Monospace)
                    .hint_text(placeholder)
                    .desired_width(input_w),
            );
            if !cfg.can_send_input {
                text_resp = text_resp
                    .on_disabled_hover_text("Console en lecture seule ou service arrêté");
            }

            if text_resp.changed() {
                state.history_cursor = None;
            }

            if cfg.can_send_input
                && text_resp.lost_focus()
                && ui.input(|i| i.key_pressed(Key::Enter))
            {
                submit_requested = true;
                keep_focus = true;
            }

            let can_submit = cfg.can_send_input && !state.input.trim().is_empty();
            ui.add_enabled_ui(can_submit, |ui| {
                if theme::primary_button(ui, p, "▶ Envoyer")
                    .on_hover_text("Envoyer la commande (Entrée)")
                    .clicked()
                {
                    submit_requested = true;
                    keep_focus = true;
                }
            });
        });

        if submit_requested && cfg.can_send_input {
            let cmd = state.input.trim().to_string();
            if !cmd.is_empty() {
                state.record_command(cmd.clone());
                state.input.clear();
                *action = Some(ConsoleWidgetAction::SendCommand(cmd));
            }
            if keep_focus {
                ui.memory_mut(|m| m.request_focus(input_id));
            }
        }
    });
}

fn build_line_layout_job(
    line: &ConsoleLine,
    font_id: &FontId,
    query_lower: Option<&str>,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;

    if line.spans.is_empty() {
        // Reserve a single blank row so empty log lines keep uniform height in `show_rows`.
        job.append(
            " ",
            0.0,
            TextFormat {
                font_id: font_id.clone(),
                color: CONSOLE_DEFAULT_FG,
                ..Default::default()
            },
        );
        return job;
    }

    for span in &line.spans {
        let fg = resolve_span_color(span);
        match query_lower {
            Some(q) if !q.is_empty() => {
                append_highlighted_span(&mut job, &span.text, q, font_id, fg);
            }
            _ => {
                job.append(
                    &span.text,
                    0.0,
                    TextFormat {
                        font_id: font_id.clone(),
                        color: fg,
                        ..Default::default()
                    },
                );
            }
        }
    }

    job
}

fn resolve_span_color(span: &ConsoleSpan) -> Color32 {
    match (span.color, span.bold) {
        (Some(c), true) => brighten_color(c),
        (Some(c), false) => c,
        (None, true) => CONSOLE_BOLD_FG,
        (None, false) => CONSOLE_DEFAULT_FG,
    }
}

fn brighten_color(c: Color32) -> Color32 {
    let lift = |ch: u8| -> u8 { ch.saturating_add(((255 - ch) as f32 * 0.22) as u8) };
    Color32::from_rgb(lift(c.r()), lift(c.g()), lift(c.b()))
}

fn append_highlighted_span(
    job: &mut LayoutJob,
    text: &str,
    query_lower: &str,
    font_id: &FontId,
    fg: Color32,
) {
    let ranges = find_case_insensitive_ranges(text, query_lower);
    if ranges.is_empty() {
        job.append(
            text,
            0.0,
            TextFormat {
                font_id: font_id.clone(),
                color: fg,
                ..Default::default()
            },
        );
        return;
    }

    let mut cursor = 0usize;
    for (start, end) in ranges {
        if start > cursor {
            job.append(
                &text[cursor..start],
                0.0,
                TextFormat {
                    font_id: font_id.clone(),
                    color: fg,
                    ..Default::default()
                },
            );
        }
        job.append(
            &text[start..end],
            0.0,
            TextFormat {
                font_id: font_id.clone(),
                color: CONSOLE_SEARCH_HL_FG,
                background: CONSOLE_SEARCH_HL_BG,
                ..Default::default()
            },
        );
        cursor = end;
    }
    if cursor < text.len() {
        job.append(
            &text[cursor..],
            0.0,
            TextFormat {
                font_id: font_id.clone(),
                color: fg,
                ..Default::default()
            },
        );
    }
}

/// Find non-overlapping byte ranges in `text` matching `query_lower` case-insensitively,
/// safe across arbitrary multi-byte UTF-8 characters.
fn find_case_insensitive_ranges(text: &str, query_lower: &str) -> Vec<(usize, usize)> {
    if query_lower.is_empty() || text.is_empty() {
        return Vec::new();
    }

    // Fast path: pure ASCII keeps 1:1 byte alignment.
    if text.is_ascii() && query_lower.is_ascii() {
        let lowered = text.to_ascii_lowercase();
        let q_len = query_lower.len();
        let mut ranges = Vec::new();
        let mut pos = 0usize;
        while let Some(rel) = lowered[pos..].find(query_lower) {
            let start = pos + rel;
            let end = start + q_len;
            ranges.push((start, end));
            pos = end;
        }
        return ranges;
    }

    // General UTF-8 safe path: map each byte index in `lowered` back to its source char span.
    let mut lowered = String::with_capacity(text.len());
    let mut byte_to_orig_span: Vec<(usize, usize)> = Vec::with_capacity(text.len());
    for (orig_start, ch) in text.char_indices() {
        let orig_end = orig_start + ch.len_utf8();
        for lower_ch in ch.to_lowercase() {
            let start_len = lowered.len();
            lowered.push(lower_ch);
            for _ in start_len..lowered.len() {
                byte_to_orig_span.push((orig_start, orig_end));
            }
        }
    }

    let q_len = query_lower.len();
    let mut ranges = Vec::new();
    let mut pos = 0usize;
    while let Some(rel) = lowered[pos..].find(query_lower) {
        let l_start = pos + rel;
        let l_end = l_start + q_len;
        if l_end > 0 && l_end <= byte_to_orig_span.len() {
            let orig_start = byte_to_orig_span[l_start].0;
            let orig_end = byte_to_orig_span[l_end - 1].1;
            if orig_start < orig_end && text.is_char_boundary(orig_start) && text.is_char_boundary(orig_end) {
                ranges.push((orig_start, orig_end));
            }
        }
        pos = l_end;
    }
    ranges
}

/// ANSI 16/256-color palette aligned with `ntx-viewer/src/ui/terminal.rs`,
/// tuned for crisp readability on the `#0B0F14` dark console canvas.
fn ansi_idx_to_color(idx: u8) -> Color32 {
    match idx {
        0 => Color32::from_rgb(90, 100, 118),
        1 => Color32::from_rgb(240, 92, 92),
        2 => Color32::from_rgb(76, 209, 130),
        3 => Color32::from_rgb(245, 200, 75),
        4 => Color32::from_rgb(88, 155, 245),
        5 => Color32::from_rgb(200, 115, 235),
        6 => Color32::from_rgb(56, 210, 228),
        7 => Color32::from_rgb(215, 222, 230),
        8 => Color32::from_rgb(122, 134, 152),
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
    fn test_ansi_sgr_colors_bold_and_reset() {
        let mut buf = ConsoleBuffer::new(100);
        buf.push_bytes(
            b"\x1b[32m[INFO]\x1b[0m \x1b[1;31mErreur critique\x1b[22;39m normale\x1b[96m cyan\x1b[m fin\n",
        );

        assert_eq!(buf.line_count(), 1);
        let line = &buf.lines()[0];
        assert_eq!(line.plain_text, "[INFO] Erreur critique normale cyan fin");
        assert_eq!(line.spans.len(), 6);

        assert_eq!(line.spans[0].text, "[INFO]");
        assert_eq!(line.spans[0].color, Some(ansi_idx_to_color(2)));
        assert!(!line.spans[0].bold);

        assert_eq!(line.spans[1].text, " ");
        assert_eq!(line.spans[1].color, None);
        assert!(!line.spans[1].bold);

        assert_eq!(line.spans[2].text, "Erreur critique");
        assert_eq!(line.spans[2].color, Some(ansi_idx_to_color(1)));
        assert!(line.spans[2].bold);

        assert_eq!(line.spans[3].text, " normale");
        assert_eq!(line.spans[3].color, None);
        assert!(!line.spans[3].bold);

        assert_eq!(line.spans[4].text, " cyan");
        assert_eq!(line.spans[4].color, Some(ansi_idx_to_color(14)));
        assert!(!line.spans[4].bold);

        assert_eq!(line.spans[5].text, " fin");
        assert_eq!(line.spans[5].color, None);
        assert!(!line.spans[5].bold);
    }

    #[test]
    fn test_ansi_extended_256_and_truecolor_and_strip_non_sgr() {
        let mut buf = ConsoleBuffer::new(100);
        // Include OSC window title, cursor hide/clear CSI, charset ESC, 256-color, RGB, and bg SGR.
        buf.push_bytes(
            b"\x1b]0;PaperMC 1.21\x07\x1b[2J\x1b[H\x1b[?25l\x1b(B\x1b[38;5;196mA\x1b[48;5;21;38;2;10;20;30mB\x1b[0m\x1b]8;;https://example.com\x1b\\C\n",
        );

        assert_eq!(buf.line_count(), 1);
        let line = &buf.lines()[0];
        assert_eq!(line.plain_text, "ABC");
        assert_eq!(line.spans.len(), 3);
        assert_eq!(line.spans[0].color, Some(ansi_idx_to_color(196)));
        assert_eq!(line.spans[1].color, Some(Color32::from_rgb(10, 20, 30)));
        assert_eq!(line.spans[2].color, None);
    }

    #[test]
    fn test_streaming_chunks_utf8_and_split_ansi() {
        let mut buf = ConsoleBuffer::new(100);

        // Split multi-byte UTF-8 ("é" = 0xC3 0xA9) and ANSI CSI across chunks.
        buf.push_bytes(b"\x1b[1;");
        buf.push_bytes(b"32mConnect\xc3");
        buf.push_bytes(b"\xa9\x1b[0m\n");
        // Invalid lone byte should convert lossily to U+FFFD.
        buf.push_bytes(b"ok\xFFfin\n");

        assert_eq!(buf.line_count(), 2);
        assert_eq!(buf.lines()[0].plain_text, "Connecté");
        assert_eq!(buf.lines()[0].spans[0].color, Some(ansi_idx_to_color(2)));
        assert!(buf.lines()[0].spans[0].bold);
        assert_eq!(buf.lines()[1].plain_text, "ok\u{FFFD}fin");
    }

    #[test]
    fn test_crlf_and_cr_progress_overwrite() {
        let mut buf = ConsoleBuffer::new(100);

        // Standard CRLF, including split across chunks, plus blank CRLF line.
        buf.push_bytes(b"Ligne 1\r");
        buf.push_bytes(b"\n\r\nLigne 3\r\n");

        // Progress bar overwritten in-place with \r and \r\x1b[2K.
        buf.push_bytes(b"Downloading 10%\r");
        assert_eq!(buf.lines().back().unwrap().plain_text, "Downloading 10%");
        buf.push_bytes(b"Downloading 50%\r\x1b[2K");
        buf.push_bytes(b"\x1b[32mDownloading 100%\x1b[0m\n");

        assert_eq!(buf.line_count(), 4);
        assert_eq!(buf.lines()[0].plain_text, "Ligne 1");
        assert_eq!(buf.lines()[1].plain_text, "");
        assert_eq!(buf.lines()[2].plain_text, "Ligne 3");
        assert_eq!(buf.lines()[3].plain_text, "Downloading 100%");
        assert_eq!(buf.lines()[3].spans[0].color, Some(ansi_idx_to_color(2)));
    }

    #[test]
    fn test_ring_buffer_eviction_and_system_lines() {
        let mut buf = ConsoleBuffer::new(3);
        buf.push_bytes(b"L1\nL2\nL3\nL4\n");
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.plain_text_all(), "L2\nL3\nL4");

        // Partial line should be committed cleanly before a system line.
        buf.push_bytes(b"partial");
        buf.push_system_line("[Connecté au flux live]", Some(Color32::GREEN));
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.plain_text_all(), "L4\npartial\n[Connecté au flux live]");
        assert_eq!(buf.lines()[2].spans[0].color, Some(Color32::GREEN));

        // Zero-capacity buffer must never panic.
        let mut zero_buf = ConsoleBuffer::new(0);
        zero_buf.push_bytes(b"hello\n");
        zero_buf.push_system_line("sys", None);
        assert_eq!(zero_buf.line_count(), 0);
    }

    #[test]
    fn test_plain_text_all_and_clear() {
        let mut buf = ConsoleBuffer::new(50);
        assert_eq!(buf.plain_text_all(), "");
        assert!(buf.is_empty());

        buf.push_bytes(b"\x1b[33mwarn\x1b[0m\nsecond line");
        assert_eq!(buf.line_count(), 2);
        assert_eq!(buf.plain_text_all(), "warn\nsecond line");

        buf.clear();
        assert_eq!(buf.line_count(), 0);
        assert!(buf.is_empty());
        assert_eq!(buf.plain_text_all(), "");

        // Parser state must be reset after clear (no leftover green color).
        buf.push_bytes(b"fresh\n");
        assert_eq!(buf.line_count(), 1);
        assert_eq!(buf.lines()[0].spans[0].color, None);
        assert!(!buf.lines()[0].spans[0].bold);
    }

    #[test]
    fn test_command_history_navigation_and_cap() {
        let mut state = ConsoleWidgetState::default();
        assert!(state.auto_scroll);
        assert!(!state.history_prev());
        assert!(!state.history_next());

        state.record_command("list".to_string());
        state.record_command("list".to_string()); // consecutive duplicate ignored
        state.record_command("say Bonjour".to_string());
        assert_eq!(state.command_history, vec!["list", "say Bonjour"]);

        assert!(state.history_prev());
        assert_eq!(state.input, "say Bonjour");
        assert_eq!(state.history_cursor, Some(1));

        assert!(state.history_prev());
        assert_eq!(state.input, "list");
        assert_eq!(state.history_cursor, Some(0));

        // Clamps at oldest command.
        assert!(state.history_prev());
        assert_eq!(state.input, "list");
        assert_eq!(state.history_cursor, Some(0));

        assert!(state.history_next());
        assert_eq!(state.input, "say Bonjour");
        assert_eq!(state.history_cursor, Some(1));

        // Moving past newest clears input and resets cursor.
        assert!(state.history_next());
        assert_eq!(state.input, "");
        assert_eq!(state.history_cursor, None);

        // Verify 100-entry cap.
        for i in 0..120 {
            state.record_command(format!("cmd {i}"));
        }
        assert_eq!(state.command_history.len(), MAX_COMMAND_HISTORY);
        assert_eq!(state.command_history.first().unwrap(), "cmd 20");
        assert_eq!(state.command_history.last().unwrap(), "cmd 119");
    }

    #[test]
    fn test_case_insensitive_search_ranges_utf8() {
        assert_eq!(
            find_case_insensitive_ranges("INFO Error error", "error"),
            vec![(5, 10), (11, 16)]
        );

        let text = "Réseau : Échec critique (échec)";
        let ranges = find_case_insensitive_ranges(text, "échec");
        assert_eq!(ranges.len(), 2);
        assert_eq!(&text[ranges[0].0..ranges[0].1], "Échec");
        assert_eq!(&text[ranges[1].0..ranges[1].1], "échec");
    }

    #[test]
    fn test_render_smoke_wide_and_narrow() {
        let ctx = egui::Context::default();
        let mut buf = ConsoleBuffer::new(200);
        let mut state = ConsoleWidgetState::default();
        let quick = vec!["list".to_string(), "save-all".to_string()];

        // 1. Empty + disconnected state (light theme, narrow viewport)
        theme::apply_theme(&ctx, false);
        let raw_narrow = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(480.0, 520.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(raw_narrow, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let p = Palette::for_ui(ui);
                let cfg = ConsoleWidgetConfig {
                    id_salt: "test_console_empty",
                    connected_label: "○ Arrêté",
                    is_streaming: false,
                    can_send_input: false,
                    input_placeholder: "Service arrêté",
                    quick_commands: &quick,
                    empty_hint: "Démarrez le service pour voir la console.",
                };
                assert!(render(ui, &p, &buf, &mut state, &cfg).is_none());
            });
        });

        // 2. Populated ANSI logs + active search filter (dark theme, wide viewport)
        theme::apply_theme(&ctx, true);
        buf.push_bytes(b"\x1b[32m[INFO]\x1b[0m Serveur pr\xc3\xaat\n\x1b[1;31m[ERR]\x1b[0m \xc3\x89chec r\xc3\xa9seau\n");
        buf.push_system_line("[Connecté au flux live]", Some(Color32::from_rgb(52, 211, 153)));
        state.search_query = "échec".to_string();

        let raw_wide = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 640.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(raw_wide, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let p = Palette::for_ui(ui);
                let cfg = ConsoleWidgetConfig {
                    id_salt: "test_console_live",
                    connected_label: "● Flux live",
                    is_streaming: true,
                    can_send_input: true,
                    input_placeholder: "Envoyer une commande...",
                    quick_commands: &quick,
                    empty_hint: "En attente...",
                };
                assert!(render(ui, &p, &buf, &mut state, &cfg).is_none());
            });
        });
    }
}
