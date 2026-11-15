use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

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
            if c == '\x1b' {
                // Peek next char to distinguish CSI (\x1b[), OSC (\x1b]), etc.
                match chars.peek() {
                    Some(&'[') => {
                        // CSI sequence
                        chars.next(); // consume '['
                        if !cur_text.is_empty() {
                            let text = std::mem::take(&mut cur_text);
                            self.current_spans.push(TerminalSpan::new(
                                text,
                                self.current_color,
                                self.current_bold,
                            ));
                        }

                        let mut param_str = String::new();
                        while let Some(&next_c) = chars.peek() {
                            if next_c.is_ascii_alphabetic() || next_c == '@' || next_c == '`' {
                                let cmd = chars.next().unwrap();
                                self.handle_csi_sequence(&param_str, cmd);
                                break;
                            } else {
                                param_str.push(chars.next().unwrap());
                            }
                        }
                    }
                    Some(&']') => {
                        // OSC sequence (e.g. \x1b]0;Title\x07)
                        chars.next(); // consume ']'
                        while let Some(osc_c) = chars.next() {
                            if osc_c == '\x07' || (osc_c == '\x1b' && chars.peek() == Some(&'\\')) {
                                if osc_c == '\x1b' {
                                    chars.next(); // consume '\\'
                                }
                                break;
                            }
                        }
                    }
                    Some(&'(') | Some(&')') => {
                        // Character set selection (e.g. \x1b(B)
                        chars.next();
                        let _ = chars.next();
                    }
                    Some(&'=') | Some(&'>') => {
                        // Keypad mode
                        chars.next();
                    }
                    _ => {
                        // Other escape sequence or bare \x1b, ignore
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
                // If followed by \n, \n will handle newline.
                // If lone \r (carriage return), reset active line text.
                if chars.peek() == Some(&'\n') {
                    // Let '\n' handle line termination
                } else {
                    cur_text.clear();
                    self.current_spans.clear();
                }
            } else if c == '\x08' || c == '\x7f' {
                // Backspace: pop last character
                if !cur_text.is_empty() {
                    cur_text.pop();
                } else if let Some(last) = self.current_spans.last_mut() {
                    last.text.pop();
                }
            } else if c == '\t' {
                cur_text.push_str("    ");
            } else if c >= ' ' {
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
        match cmd {
            'm' => {
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
                        30 => self.current_color = [30, 30, 30],
                        31 => self.current_color = [230, 70, 70],
                        32 => self.current_color = [70, 200, 70],
                        33 => self.current_color = [230, 200, 50],
                        34 => self.current_color = [80, 140, 240],
                        35 => self.current_color = [200, 80, 200],
                        36 => self.current_color = [60, 200, 220],
                        37 => self.current_color = [220, 220, 220],
                        // High-intensity foreground colors
                        90 => self.current_color = [120, 120, 120],
                        91 => self.current_color = [255, 100, 100],
                        92 => self.current_color = [100, 255, 100],
                        93 => self.current_color = [255, 255, 100],
                        94 => self.current_color = [120, 170, 255],
                        95 => self.current_color = [255, 120, 255],
                        96 => self.current_color = [100, 240, 255],
                        97 => self.current_color = [255, 255, 255],
                        _ => {}
                    }
                }
            }
            'J' => {
                // 2J = Clear screen
                if params.contains('2') {
                    self.lines.clear();
                    self.current_spans.clear();
                }
            }
            'K' => {
                // Erase in line
                self.current_spans.clear();
            }
            _ => {}
        }
    }
}

/// Column used for sorting the process table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSortColumn {
    Pid,
    Name,
    User,
    Cpu,
    Memory,
    Status,
    CommandLine,
}

/// Column used for sorting the system services table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceSortColumn {
    Name,
    Description,
    ActiveState,
    SubState,
    LoadState,
}

/// Sorting direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

/// Subtab within the Process & Services manager view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProcessSubTab {
    #[default]
    Processes,
    Services,
}

/// State for caching processes, services, filter search queries, and sort parameters.
#[derive(Debug, Clone)]
pub struct ProcessState {
    pub processes: Vec<ntx_proto::ProcessItem>,
    pub services: Vec<ntx_proto::ServiceItem>,
    pub process_filter: String,
    pub service_filter: String,
    pub process_sort_column: ProcessSortColumn,
    pub process_sort_direction: SortDirection,
    pub service_sort_column: ServiceSortColumn,
    pub service_sort_direction: SortDirection,
    pub auto_refresh: bool,
    pub auto_refresh_interval_secs: u64,
    pub last_process_refresh: Option<Instant>,
    pub last_service_refresh: Option<Instant>,
    pub is_loading_processes: bool,
    pub is_loading_services: bool,
    pub active_subtab: ProcessSubTab,
    pub status_message: Option<(String, bool)>, // (message, is_error)
}

impl Default for ProcessState {
    fn default() -> Self {
        Self {
            processes: Vec::new(),
            services: Vec::new(),
            process_filter: String::new(),
            service_filter: String::new(),
            process_sort_column: ProcessSortColumn::Cpu,
            process_sort_direction: SortDirection::Descending,
            service_sort_column: ServiceSortColumn::Name,
            service_sort_direction: SortDirection::Ascending,
            auto_refresh: false,
            auto_refresh_interval_secs: 3,
            last_process_refresh: None,
            last_service_refresh: None,
            is_loading_processes: false,
            is_loading_services: false,
            active_subtab: ProcessSubTab::Processes,
            status_message: None,
        }
    }
}

impl ProcessState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a temporary notification or error message
    pub fn set_status(&mut self, msg: impl Into<String>, is_error: bool) {
        self.status_message = Some((msg.into(), is_error));
    }

    pub fn clear_status(&mut self) {
        self.status_message = None;
    }

    /// Toggle or change sorting for processes
    pub fn toggle_process_sort(&mut self, col: ProcessSortColumn) {
        if self.process_sort_column == col {
            self.process_sort_direction = match self.process_sort_direction {
                SortDirection::Ascending => SortDirection::Descending,
                SortDirection::Descending => SortDirection::Ascending,
            };
        } else {
            self.process_sort_column = col;
            self.process_sort_direction = match col {
                ProcessSortColumn::Cpu | ProcessSortColumn::Memory => SortDirection::Descending,
                _ => SortDirection::Ascending,
            };
        }
    }

    /// Toggle or change sorting for services
    pub fn toggle_service_sort(&mut self, col: ServiceSortColumn) {
        if self.service_sort_column == col {
            self.service_sort_direction = match self.service_sort_direction {
                SortDirection::Ascending => SortDirection::Descending,
                SortDirection::Descending => SortDirection::Ascending,
            };
        } else {
            self.service_sort_column = col;
            self.service_sort_direction = SortDirection::Ascending;
        }
    }

    /// Get sorted and filtered slice of processes
    pub fn filtered_processes(&self) -> Vec<&ntx_proto::ProcessItem> {
        let q = self.process_filter.trim().to_lowercase();
        let mut list: Vec<&ntx_proto::ProcessItem> = self
            .processes
            .iter()
            .filter(|p| {
                if q.is_empty() {
                    return true;
                }
                p.name.to_lowercase().contains(&q)
                    || p.pid.to_string().contains(&q)
                    || p.user.to_lowercase().contains(&q)
                    || p.status.to_lowercase().contains(&q)
                    || p.command_line.to_lowercase().contains(&q)
            })
            .collect();

        list.sort_by(|a, b| {
            let cmp = match self.process_sort_column {
                ProcessSortColumn::Pid => a.pid.cmp(&b.pid),
                ProcessSortColumn::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                ProcessSortColumn::User => a.user.to_lowercase().cmp(&b.user.to_lowercase()),
                ProcessSortColumn::Cpu => a
                    .cpu_usage
                    .partial_cmp(&b.cpu_usage)
                    .unwrap_or(std::cmp::Ordering::Equal),
                ProcessSortColumn::Memory => a.memory_bytes.cmp(&b.memory_bytes),
                ProcessSortColumn::Status => {
                    a.status.to_lowercase().cmp(&b.status.to_lowercase())
                }
                ProcessSortColumn::CommandLine => a
                    .command_line
                    .to_lowercase()
                    .cmp(&b.command_line.to_lowercase()),
            };
            match self.process_sort_direction {
                SortDirection::Ascending => cmp,
                SortDirection::Descending => cmp.reverse(),
            }
        });

        list
    }

    /// Get sorted and filtered slice of services
    pub fn filtered_services(&self) -> Vec<&ntx_proto::ServiceItem> {
        let q = self.service_filter.trim().to_lowercase();
        let mut list: Vec<&ntx_proto::ServiceItem> = self
            .services
            .iter()
            .filter(|s| {
                if q.is_empty() {
                    return true;
                }
                s.name.to_lowercase().contains(&q)
                    || s.description.to_lowercase().contains(&q)
                    || s.active_state.to_lowercase().contains(&q)
                    || s.sub_state.to_lowercase().contains(&q)
                    || s.load_state.to_lowercase().contains(&q)
            })
            .collect();

        list.sort_by(|a, b| {
            let cmp = match self.service_sort_column {
                ServiceSortColumn::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                ServiceSortColumn::Description => {
                    a.description.to_lowercase().cmp(&b.description.to_lowercase())
                }
                ServiceSortColumn::ActiveState => {
                    a.active_state.to_lowercase().cmp(&b.active_state.to_lowercase())
                }
                ServiceSortColumn::SubState => {
                    a.sub_state.to_lowercase().cmp(&b.sub_state.to_lowercase())
                }
                ServiceSortColumn::LoadState => {
                    a.load_state.to_lowercase().cmp(&b.load_state.to_lowercase())
                }
            };
            match self.service_sort_direction {
                SortDirection::Ascending => cmp,
                SortDirection::Descending => cmp.reverse(),
            }
        });

        list
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
    // Process & service manager
    pub process_state: ProcessState,
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
            process_state: ProcessState::default(),
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
#[allow(clippy::field_reassign_with_default)]
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

    #[test]
    fn test_process_state_filtering_and_sorting() {
        let mut state = ProcessState::default();
        state.processes = vec![
            ntx_proto::ProcessItem {
                pid: 100,
                name: "systemd".to_string(),
                cpu_usage: 0.5,
                memory_bytes: 10 * 1024 * 1024,
                user: "root".to_string(),
                status: "running".to_string(),
                command_line: "/sbin/init".to_string(),
            },
            ntx_proto::ProcessItem {
                pid: 200,
                name: "chrome".to_string(),
                cpu_usage: 15.2,
                memory_bytes: 500 * 1024 * 1024,
                user: "user".to_string(),
                status: "sleeping".to_string(),
                command_line: "/opt/google/chrome".to_string(),
            },
            ntx_proto::ProcessItem {
                pid: 300,
                name: "ntx-server".to_string(),
                cpu_usage: 2.1,
                memory_bytes: 25 * 1024 * 1024,
                user: "root".to_string(),
                status: "running".to_string(),
                command_line: "./ntx-server".to_string(),
            },
        ];

        // Default sort is CPU descending
        let sorted = state.filtered_processes();
        assert_eq!(sorted.len(), 3);
        assert_eq!(sorted[0].name, "chrome");
        assert_eq!(sorted[1].name, "ntx-server");
        assert_eq!(sorted[2].name, "systemd");

        // Filter by "ntx"
        state.process_filter = "ntx".to_string();
        let filtered = state.filtered_processes();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].pid, 300);

        // Clear filter and sort by PID ascending
        state.process_filter.clear();
        state.toggle_process_sort(ProcessSortColumn::Pid);
        assert_eq!(state.process_sort_column, ProcessSortColumn::Pid);
        assert_eq!(state.process_sort_direction, SortDirection::Ascending);
        let sorted_pid = state.filtered_processes();
        assert_eq!(sorted_pid[0].pid, 100);
        assert_eq!(sorted_pid[2].pid, 300);

        // Toggle sort on PID flips to descending
        state.toggle_process_sort(ProcessSortColumn::Pid);
        assert_eq!(state.process_sort_direction, SortDirection::Descending);
        let sorted_pid_desc = state.filtered_processes();
        assert_eq!(sorted_pid_desc[0].pid, 300);
        assert_eq!(sorted_pid_desc[2].pid, 100);
    }

    #[test]
    fn test_service_state_filtering_and_sorting() {
        let mut state = ProcessState::default();
        state.services = vec![
            ntx_proto::ServiceItem {
                name: "nginx.service".to_string(),
                description: "Nginx Web Server".to_string(),
                load_state: "loaded".to_string(),
                active_state: "active".to_string(),
                sub_state: "running".to_string(),
            },
            ntx_proto::ServiceItem {
                name: "apache2.service".to_string(),
                description: "Apache Web Server".to_string(),
                load_state: "loaded".to_string(),
                active_state: "inactive".to_string(),
                sub_state: "dead".to_string(),
            },
        ];

        // Default sort is Name ascending
        let list = state.filtered_services();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "apache2.service");
        assert_eq!(list[1].name, "nginx.service");

        // Filter by "nginx"
        state.service_filter = "nginx".to_string();
        let filtered = state.filtered_services();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "nginx.service");
    }

    #[test]
    fn test_process_state_all_sort_columns_and_status() {
        let mut state = ProcessState::default();
        state.set_status("Operation succeeded", false);
        assert_eq!(
            state.status_message,
            Some(("Operation succeeded".to_string(), false))
        );
        state.clear_status();
        assert_eq!(state.status_message, None);

        state.processes = vec![
            ntx_proto::ProcessItem {
                pid: 1,
                name: "b_proc".to_string(),
                cpu_usage: 10.0,
                memory_bytes: 200,
                user: "alice".to_string(),
                status: "Running".to_string(),
                command_line: "b_proc --arg1".to_string(),
            },
            ntx_proto::ProcessItem {
                pid: 2,
                name: "a_proc".to_string(),
                cpu_usage: 20.0,
                memory_bytes: 100,
                user: "bob".to_string(),
                status: "Sleeping".to_string(),
                command_line: "a_proc --arg2".to_string(),
            },
        ];

        // Sort by Memory (default descending)
        state.toggle_process_sort(ProcessSortColumn::Memory);
        let list = state.filtered_processes();
        assert_eq!(list[0].pid, 1); // 200 > 100

        // Sort by User
        state.toggle_process_sort(ProcessSortColumn::User);
        let list = state.filtered_processes();
        assert_eq!(list[0].user, "alice");

        // Sort by Status
        state.toggle_process_sort(ProcessSortColumn::Status);
        let list = state.filtered_processes();
        assert_eq!(list[0].status, "Running");

        // Sort by CommandLine
        state.toggle_process_sort(ProcessSortColumn::CommandLine);
        let list = state.filtered_processes();
        assert_eq!(list[0].name, "a_proc");
    }

    #[test]
    fn test_service_state_all_sort_columns() {
        let mut state = ProcessState::default();
        state.services = vec![
            ntx_proto::ServiceItem {
                name: "srv_b".to_string(),
                description: "Zeta Description".to_string(),
                load_state: "loaded".to_string(),
                active_state: "active".to_string(),
                sub_state: "running".to_string(),
            },
            ntx_proto::ServiceItem {
                name: "srv_a".to_string(),
                description: "Alpha Description".to_string(),
                load_state: "not-found".to_string(),
                active_state: "inactive".to_string(),
                sub_state: "dead".to_string(),
            },
        ];

        // Sort by Description
        state.toggle_service_sort(ServiceSortColumn::Description);
        let list = state.filtered_services();
        assert_eq!(list[0].name, "srv_a"); // Alpha < Zeta

        // Sort by ActiveState
        state.toggle_service_sort(ServiceSortColumn::ActiveState);
        let list = state.filtered_services();
        assert_eq!(list[0].active_state, "active"); // active < inactive

        // Sort by SubState
        state.toggle_service_sort(ServiceSortColumn::SubState);
        let list = state.filtered_services();
        assert_eq!(list[0].sub_state, "dead"); // dead < running

        // Sort by LoadState
        state.toggle_service_sort(ServiceSortColumn::LoadState);
        let list = state.filtered_services();
        assert_eq!(list[0].load_state, "loaded"); // loaded < not-found
    }
}
