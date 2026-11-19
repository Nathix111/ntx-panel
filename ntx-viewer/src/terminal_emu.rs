use std::fmt;

const DEFAULT_SCROLLBACK: usize = 2000;

pub struct TerminalEmulator {
    parser: vt100::Parser,
    rows: u16,
    cols: u16,
    scrollback_offset: usize,
}

impl fmt::Debug for TerminalEmulator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TerminalEmulator")
            .field("rows", &self.rows)
            .field("cols", &self.cols)
            .field("scrollback_offset", &self.scrollback_offset)
            .finish()
    }
}

impl Default for TerminalEmulator {
    fn default() -> Self {
        Self::new(24, 80)
    }
}

impl TerminalEmulator {
    pub fn new(rows: u16, cols: u16) -> Self {
        let rows = rows.clamp(5, 200);
        let cols = cols.clamp(20, 300);
        Self {
            parser: vt100::Parser::new(rows, cols, DEFAULT_SCROLLBACK),
            rows,
            cols,
            scrollback_offset: 0,
        }
    }

    pub fn process_bytes(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        if self.scrollback_offset > 0 {
            self.parser.set_scrollback(self.scrollback_offset);
            self.scrollback_offset = self.parser.screen().scrollback();
        }
    }

    pub fn write_local_notice(&mut self, text: &str) {
        self.process_bytes(text.as_bytes());
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> bool {
        let rows = rows.clamp(5, 200);
        let cols = cols.clamp(20, 300);
        if self.rows == rows && self.cols == cols {
            return false;
        }
        self.rows = rows;
        self.cols = cols;
        self.parser.set_size(rows, cols);
        true
    }

    pub fn clear(&mut self) {
        self.parser = vt100::Parser::new(self.rows, self.cols, DEFAULT_SCROLLBACK);
        self.scrollback_offset = 0;
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub fn scroll_by(&mut self, delta_lines: isize) {
        let current = self.parser.screen().scrollback() as isize;
        let next = (current + delta_lines).max(0) as usize;
        self.parser.set_scrollback(next);
        self.scrollback_offset = self.parser.screen().scrollback();
    }

    pub fn reset_scrollback(&mut self) {
        if self.scrollback_offset != 0 || self.parser.screen().scrollback() != 0 {
            self.parser.set_scrollback(0);
            self.scrollback_offset = 0;
        }
    }

    pub fn visible_plain_text(&self) -> String {
        self.parser
            .screen()
            .rows(0, self.cols)
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vt100::Color;

    #[test]
    fn test_utf8_and_ansi_split_across_chunks() {
        let mut emu = TerminalEmulator::new(24, 80);

        // Split UTF-8 'é' (0xC3, 0xA9) and ANSI red sequence across chunks
        emu.process_bytes(b"Caf");
        emu.process_bytes(&[0xC3]);
        emu.process_bytes(&[0xA9, b' ']);
        emu.process_bytes(b"\x1b[3");
        emu.process_bytes(b"1mRouge\x1b[0m");

        let screen = emu.screen();
        assert_eq!(screen.cell(0, 0).unwrap().contents(), "C");
        assert_eq!(screen.cell(0, 3).unwrap().contents(), "é");
        assert_eq!(screen.cell(0, 5).unwrap().contents(), "R");
        assert_eq!(screen.cell(0, 5).unwrap().fgcolor(), Color::Idx(1));
        assert_eq!(screen.cell(0, 10).unwrap().fgcolor(), Color::Default);
    }

    #[test]
    fn test_256_truecolor_carriage_return_and_alt_screen() {
        let mut emu = TerminalEmulator::new(10, 40);

        // 256-color and truecolor
        emu.process_bytes(b"\x1b[38;5;196mX\x1b[38;2;10;20;30mY\x1b[0m");
        assert_eq!(emu.screen().cell(0, 0).unwrap().fgcolor(), Color::Idx(196));
        assert_eq!(
            emu.screen().cell(0, 1).unwrap().fgcolor(),
            Color::Rgb(10, 20, 30)
        );

        // Carriage return overwrites same row
        emu.process_bytes(b"\rOK");
        assert_eq!(emu.screen().cell(0, 0).unwrap().contents(), "O");
        assert_eq!(emu.screen().cell(0, 1).unwrap().contents(), "K");

        // Alternate screen (used by vim/htop/tmux)
        emu.process_bytes(b"\x1b[?1049hALT_APP");
        assert!(emu.screen().alternate_screen());
        assert_eq!(emu.screen().cell(0, 0).unwrap().contents(), "A");

        emu.process_bytes(b"\x1b[?1049l");
        assert!(!emu.screen().alternate_screen());
        assert_eq!(emu.screen().cell(0, 0).unwrap().contents(), "O");
    }

    #[test]
    fn test_resize_and_clear() {
        let mut emu = TerminalEmulator::new(24, 80);
        assert!(!emu.resize(24, 80));
        assert!(emu.resize(40, 120));
        assert_eq!(emu.size(), (40, 120));

        emu.process_bytes(b"hello world");
        assert!(emu.visible_plain_text().contains("hello world"));
        emu.clear();
        assert!(!emu.visible_plain_text().contains("hello world"));
    }
}
