use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::PathBuf;

/// Connection status of a discovered or added device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Discovered,
    Connecting,
    Connected,
    AuthFailed(String),
    Error(String),
    Disconnected,
}

impl ConnectionStatus {
    pub fn is_connected(&self) -> bool {
        matches!(self, ConnectionStatus::Connected)
    }

    pub fn display_text(&self) -> &str {
        match self {
            ConnectionStatus::Discovered => "Discovered",
            ConnectionStatus::Connecting => "Connecting...",
            ConnectionStatus::Connected => "Connected",
            ConnectionStatus::AuthFailed(_) => "Auth Failed",
            ConnectionStatus::Error(_) => "Error",
            ConnectionStatus::Disconnected => "Disconnected",
        }
    }
}

/// A span of terminal text with color and style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSpan {
    pub text: String,
    pub color: [u8; 3], // RGB
    pub bold: bool,
}

impl TerminalSpan {
    pub fn new(text: impl Into<String>, color: [u8; 3], bold: bool) -> Self {
        Self {
            text: text.into(),
            color,
            bold,
        }
    }
}

/// A line in the terminal view.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TerminalLine {
    pub spans: Vec<TerminalSpan>,
}

impl TerminalLine {
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty() || self.spans.iter().all(|s| s.text.is_empty())
    }

    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

/// Terminal state containing rendered lines, active command buffer, and input history.
#[derive(Debug, Clone)]
pub struct TerminalBuffer {
    pub lines: Vec<TerminalLine>,
    pub current_spans: Vec<TerminalSpan>,
    pub input_buffer: String,
    pub history: Vec<String>,
    pub history_idx: Option<usize>,
    pub current_color: [u8; 3],
    pub current_bold: bool,
    pub max_lines: usize,
    pub scroll_to_bottom: bool,
}

impl Default for TerminalBuffer {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            current_spans: Vec::new(),
            input_buffer: String::new(),
            history: Vec::new(),
            history_idx: None,
            current_color: [220, 220, 220], // standard terminal white
            current_bold: false,
            max_lines: 1000,
            scroll_to_bottom: true,
        }
    }
}

impl TerminalBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.current_spans.clear();
        self.input_buffer.clear();
        self.scroll_to_bottom = true;
    }

    /// Ingest raw bytes from remote shell (ANSI escape support)
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        self.push_str(&text);
    }

    /// Ingest text with basic ANSI escape code processing
    pub fn push_str(&mut self, s: &str) {
        let mut chars = s.chars().peekable();
        let mut cur_text = String::new();

        while let Some(c) = chars.next() {
            if c == '\x1b' && chars.peek() == Some(&'[') {
                // Flush pending text with current style
                if !cur_text.is_empty() {
                    let text = std::mem::take(&mut cur_text);
                    self.current_spans.push(TerminalSpan::new(
                        text,
                        self.current_color,
                        self.current_bold,
                    ));
                }

                // Consume '['
                chars.next();

                // Read parameters until terminating letter
                let mut param_str = String::new();
                while let Some(&next_c) = chars.peek() {
                    if next_c.is_ascii_alphabetic() {
                        let cmd = chars.next().unwrap();
                        self.handle_csi_sequence(&param_str, cmd);
                        break;
                    } else {
                        param_str.push(chars.next().unwrap());
                    }
                }
            } else if c == '\n' {
                if !cur_text.is_empty() {
                    let text = std::mem::take(&mut cur_text);
                    self.current_spans.push(TerminalSpan::new(
                        text,
                        self.current_color,
                        self.current_bold,
                    ));
                }
                let line = TerminalLine {
                    spans: std::mem::take(&mut self.current_spans),
                };
                self.lines.push(line);
                if self.lines.len() > self.max_lines {
                    self.lines.remove(0);
                }
            } else if c == '\r' {
                // Carriage return: ignore if followed by \n, or could reset line
                if chars.peek() == Some(&'\n') {
                    // let \n handle it
                } else {
                    // soft CR: flush text
                    if !cur_text.is_empty() {
                        let text = std::mem::take(&mut cur_text);
                        self.current_spans.push(TerminalSpan::new(
                            text,
                            self.current_color,
                            self.current_bold,
                        ));
                    }
                }
            } else if c == '\t' {
                cur_text.push_str("    ");
            } else {
                cur_text.push(c);
            }
        }

        if !cur_text.is_empty() {
            self.current_spans.push(TerminalSpan::new(
                cur_text,
                self.current_color,
                self.current_bold,
            ));
        }

        self.scroll_to_bottom = true;
    }

    fn handle_csi_sequence(&mut self, params: &str, cmd: char) {
        if cmd == 'm' {
            // SGR (Select Graphic Rendition)
            if params.is_empty() || params == "0" {
                self.current_color = [220, 220, 220];
                self.current_bold = false;
                return;
            }

            for part in params.split(';') {
                match part.parse::<u32>().unwrap_or(0) {
                    0 => {
                        self.current_color = [220, 220, 220];
                        self.current_bold = false;
                    }
                    1 => self.current_bold = true,
                    22 => self.current_bold = false,
                    // Standard foreground colors
                    30 => self.current_color = [30, 30, 30], // Black
                    31 => self.current_color = [230, 70, 70], // Red
                    32 => self.current_color = [70, 200, 70], // Green
                    33 => self.current_color = [230, 200, 50], // Yellow
                    34 => self.current_color = [80, 140, 240], // Blue
                    35 => self.current_color = [200, 80, 200], // Magenta
                    36 => self.current_color = [60, 200, 220], // Cyan
                    37 => self.current_color = [220, 220, 220], // White
                    // High-intensity foreground colors
                    90 => self.current_color = [120, 120, 120], // Bright Black (Gray)
                    91 => self.current_color = [255, 100, 100], // Bright Red
                    92 => self.current_color = [100, 255, 100], // Bright Green
                    93 => self.current_color = [255, 255, 100], // Bright Yellow
                    94 => self.current_color = [120, 170, 255], // Bright Blue
                    95 => self.current_color = [255, 120, 255], // Bright Magenta
                    96 => self.current_color = [100, 240, 255], // Bright Cyan
                    97 => self.current_color = [255, 255, 255], // Bright White
                    _ => {}
                }
            }
        }
    }
}

/// A remote machine managed by NTX Panel.
#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub hostname: String,
    pub addresses: Vec<String>,
    pub port: u16,
    pub version: String,
    pub status: ConnectionStatus,
    pub psk: Option<String>,
    pub session_token: Option<String>,
    pub system_info: Option<ntx_proto::SystemInfo>,
    pub latest_metrics: Option<ntx_proto::SystemMetrics>,
    // History series for metrics plotting (stores last 60-120 data points)
    pub cpu_history: VecDeque<f64>,
    pub ram_history: VecDeque<f64>,
    pub rx_history: VecDeque<f64>,
    pub tx_history: VecDeque<f64>,
    // Remote terminal
    pub terminal: TerminalBuffer,
}

impl Device {
    pub fn new(
        id: String,
        hostname: String,
        addresses: Vec<String>,
        port: u16,
        version: String,
    ) -> Self {
        Self {
            id,
            hostname,
            addresses,
            port,
            version,
            status: ConnectionStatus::Discovered,
            psk: None,
            session_token: None,
            system_info: None,
            latest_metrics: None,
            cpu_history: VecDeque::with_capacity(120),
            ram_history: VecDeque::with_capacity(120),
            rx_history: VecDeque::with_capacity(120),
            tx_history: VecDeque::with_capacity(120),
            terminal: TerminalBuffer::new(),
        }
    }

    /// Best IP address to connect to.
    pub fn primary_address(&self) -> String {
        self.addresses
            .first()
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string())
    }

    /// Full gRPC endpoint: `http://<ip>:<port>`.
    pub fn grpc_endpoint(&self) -> String {
        format!("http://{}:{}", self.primary_address(), self.port)
    }

    /// Add a new metric reading and append to history series.
    pub fn record_metrics(&mut self, metrics: ntx_proto::SystemMetrics, capacity: usize) {
        // CPU percentage
        self.cpu_history.push_back(metrics.cpu_usage_percent);
        while self.cpu_history.len() > capacity {
            self.cpu_history.pop_front();
        }

        // RAM percentage
        let ram_pct = if metrics.total_memory_bytes > 0 {
            (metrics.used_memory_bytes as f64 / metrics.total_memory_bytes as f64) * 100.0
        } else {
            0.0
        };
        self.ram_history.push_back(ram_pct);
        while self.ram_history.len() > capacity {
            self.ram_history.pop_front();
        }

        // Network sum
        let total_rx: u64 = metrics
            .network_metrics
            .iter()
            .map(|n| n.rx_bytes_per_sec)
            .sum();
        let total_tx: u64 = metrics
            .network_metrics
            .iter()
            .map(|n| n.tx_bytes_per_sec)
            .sum();

        self.rx_history.push_back(total_rx as f64);
        while self.rx_history.len() > capacity {
            self.rx_history.pop_front();
        }

        self.tx_history.push_back(total_tx as f64);
        while self.tx_history.len() > capacity {
            self.tx_history.pop_front();
        }

        self.latest_metrics = Some(metrics);
    }
}

/// A known device cached across application restarts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnownDevice {
    pub id: String,
    pub hostname: String,
    pub addresses: Vec<String>,
    pub port: u16,
    pub version: String,
    #[serde(default)]
    pub psk: Option<String>,
}

/// A manually configured device.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManualDevice {
    pub hostname: String,
    pub address: String,
    pub port: u16,
    pub psk: Option<String>,
}

/// Application settings persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub default_psk: String,
    pub viewer_id: String,
    pub refresh_rate_ms: u32,
    pub history_capacity: usize,
    pub dark_mode: bool,
    pub device_psks: HashMap<String, String>,
    pub manual_devices: Vec<ManualDevice>,
    #[serde(default)]
    pub known_devices: Vec<KnownDevice>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            default_psk: String::new(),
            viewer_id: crate::auth::generate_viewer_id(),
            refresh_rate_ms: 1000,
            history_capacity: 60,
            dark_mode: true,
            device_psks: HashMap::new(),
            manual_devices: Vec::new(),
            known_devices: Vec::new(),
        }
    }
}

impl AppSettings {
    fn get_config_path() -> Option<PathBuf> {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let dir = PathBuf::from(appdata).join("ntx-viewer");
            let _ = fs::create_dir_all(&dir);
            return Some(dir.join("settings.toml"));
        }
        if let Ok(home) = std::env::var("HOME") {
            let dir = PathBuf::from(home).join(".config").join("ntx-viewer");
            let _ = fs::create_dir_all(&dir);
            return Some(dir.join("settings.toml"));
        }
        Some(PathBuf::from("ntx-viewer-settings.toml"))
    }

    pub fn load() -> Self {
        if let Some(path) = Self::get_config_path() {
            if path.exists() {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(settings) = toml::from_str::<AppSettings>(&content) {
                        return settings;
                    }
                }
            }
        }
        let settings = Self::default();
        let _ = settings.save();
        settings
    }

    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(path) = Self::get_config_path() {
            let content = toml::to_string_pretty(self)?;
            fs::write(path, content)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_ansi_parsing() {
        let mut term = TerminalBuffer::new();
        term.push_str("Hello \x1b[31mRed\x1b[0m Normal\n");
        assert_eq!(term.lines.len(), 1);
        let line = &term.lines[0];
        assert_eq!(line.spans.len(), 3);
        assert_eq!(line.spans[0].text, "Hello ");
        assert_eq!(line.spans[1].text, "Red");
        assert_eq!(line.spans[1].color, [230, 70, 70]);
        assert_eq!(line.spans[2].text, " Normal");
        assert_eq!(line.spans[2].color, [220, 220, 220]);
    }

    #[test]
    fn test_record_metrics() {
        let mut dev = Device::new(
            "dev1".to_string(),
            "my-pc".to_string(),
            vec!["192.168.1.10".to_string()],
            9850,
            "0.1.0".to_string(),
        );

        let metrics = ntx_proto::SystemMetrics {
            cpu_usage_percent: 25.5,
            used_memory_bytes: 4000,
            total_memory_bytes: 8000,
            disk_metrics: vec![],
            network_metrics: vec![ntx_proto::NetworkMetrics {
                interface_name: "eth0".to_string(),
                rx_bytes_per_sec: 1024,
                tx_bytes_per_sec: 512,
            }],
            timestamp_ms: 123456789,
        };

        dev.record_metrics(metrics, 5);
        assert_eq!(dev.cpu_history.len(), 1);
        assert_eq!(dev.cpu_history[0], 25.5);
        assert_eq!(dev.ram_history[0], 50.0);
        assert_eq!(dev.rx_history[0], 1024.0);
        assert_eq!(dev.tx_history[0], 512.0);
    }

    #[test]
    fn test_device_address_and_status() {
        let dev = Device::new(
            "test_id".to_string(),
            "test_host".to_string(),
            vec!["10.0.0.5".to_string(), "192.168.1.5".to_string()],
            9850,
            "1.0.0".to_string(),
        );

        assert_eq!(dev.primary_address(), "10.0.0.5");
        assert_eq!(dev.grpc_endpoint(), "http://10.0.0.5:9850");
        assert!(!dev.status.is_connected());
        assert_eq!(dev.status.display_text(), "Discovered");

        let connected = ConnectionStatus::Connected;
        assert!(connected.is_connected());
        assert_eq!(connected.display_text(), "Connected");
    }

    #[test]
    fn test_settings_serialization() {
        let mut settings = AppSettings::default();
        settings.default_psk = "my-secret-psk".to_string();
        settings.refresh_rate_ms = 500;
        settings
            .device_psks
            .insert("dev1".to_string(), "psk1".to_string());
        settings.manual_devices.push(ManualDevice {
            hostname: "srv".to_string(),
            address: "192.168.1.100".to_string(),
            port: 9850,
            psk: Some("psk100".to_string()),
        });

        let serialized = toml::to_string(&settings).expect("serialization failed");
        let deserialized: AppSettings =
            toml::from_str(&serialized).expect("deserialization failed");

        assert_eq!(deserialized.default_psk, "my-secret-psk");
        assert_eq!(deserialized.refresh_rate_ms, 500);
        assert_eq!(
            deserialized.device_psks.get("dev1"),
            Some(&"psk1".to_string())
        );
        assert_eq!(deserialized.manual_devices.len(), 1);
        assert_eq!(deserialized.manual_devices[0].hostname, "srv");
    }

    #[test]
    fn test_terminal_buffer_clear() {
        let mut term = TerminalBuffer::new();
        term.push_str("line 1\nline 2\n");
        assert_eq!(term.lines.len(), 2);
        term.clear();
        assert_eq!(term.lines.len(), 0);
        assert!(term.current_spans.is_empty());
    }
}
