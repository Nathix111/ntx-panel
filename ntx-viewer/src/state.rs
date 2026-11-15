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

/// Display mode for the process list (flat list or hierarchical tree).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProcessViewMode {
    #[default]
    Tree,
    Flat,
}

/// Category filter for processes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProcessFilterCategory {
    #[default]
    All,
    ActiveOnly,
    UserOnly,
    SystemOnly,
}

/// Category filter for services.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServiceFilterCategory {
    #[default]
    All,
    ActiveOnly,
    InactiveOnly,
    FailedOnly,
}

/// Smart ranking of process statuses: "Run" / "Running" processes come first.
pub fn process_status_rank(status: &str) -> u8 {
    let s = status.to_lowercase();
    if s.contains("run") {
        0
    } else if s.contains("sleep") || s.contains("idle") || s.contains("waking") || s.contains("parked") {
        1
    } else if s.contains("stop") || s.contains("tracing") {
        2
    } else if s.contains("zombie") || s.contains("dead") || s.contains("defunct") {
        3
    } else {
        4
    }
}

/// Smart ranking of systemd service states: active/running come first.
pub fn service_state_rank(state: &str) -> u8 {
    let s = state.to_lowercase();
    if s == "active" || s == "running" {
        0
    } else if s == "failed" {
        1
    } else if s == "inactive" || s == "dead" {
        2
    } else {
        3
    }
}

/// A row rendered in the process tree view
#[derive(Debug, Clone)]
pub struct ProcessTreeRow<'a> {
    pub item: &'a ntx_proto::ProcessItem,
    pub depth: usize,
    pub has_children: bool,
    pub is_collapsed: bool,
    pub children_count: usize,
    pub total_cpu: f32,
    pub total_memory: u64,
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
    pub view_mode: ProcessViewMode,
    pub process_category: ProcessFilterCategory,
    pub service_category: ServiceFilterCategory,
    pub collapsed_pids: std::collections::HashSet<u32>,
    pub selected_pid: Option<u32>,
    pub is_paused: bool,
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
            view_mode: ProcessViewMode::Tree,
            process_category: ProcessFilterCategory::All,
            service_category: ServiceFilterCategory::All,
            collapsed_pids: std::collections::HashSet::new(),
            selected_pid: None,
            is_paused: false,
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

    /// Toggle collapsing of a process PID in tree view
    pub fn toggle_collapse(&mut self, pid: u32) {
        if self.collapsed_pids.contains(&pid) {
            self.collapsed_pids.remove(&pid);
        } else {
            self.collapsed_pids.insert(pid);
        }
    }

    /// Expand all process trees
    pub fn expand_all(&mut self) {
        self.collapsed_pids.clear();
    }

    /// Collapse all processes that have children
    pub fn collapse_all(&mut self) {
        let pids: Vec<u32> = self
            .build_tree_rows()
            .into_iter()
            .filter(|r| r.has_children)
            .map(|r| r.item.pid)
            .collect();
        for pid in pids {
            self.collapsed_pids.insert(pid);
        }
    }

    /// Process counts: (total, running, sleeping, zombies)
    pub fn process_counts(&self) -> (usize, usize, usize, usize) {
        let total = self.processes.len();
        let mut running = 0;
        let mut sleeping = 0;
        let mut zombies = 0;

        for p in &self.processes {
            let s = p.status.to_lowercase();
            if s.contains("run") {
                running += 1;
            } else if s.contains("sleep") || s.contains("idle") || s.contains("waking") {
                sleeping += 1;
            } else if s.contains("zombie") || s.contains("dead") || s.contains("defunct") {
                zombies += 1;
            }
        }

        (total, running, sleeping, zombies)
    }

    /// Top CPU consuming process
    pub fn top_cpu_process(&self) -> Option<(String, f32)> {
        self.processes
            .iter()
            .max_by(|a, b| a.cpu_usage.partial_cmp(&b.cpu_usage).unwrap_or(std::cmp::Ordering::Equal))
            .map(|p| (p.name.clone(), p.cpu_usage))
    }

    /// Top memory consuming process
    pub fn top_mem_process(&self) -> Option<(String, u64)> {
        self.processes
            .iter()
            .max_by_key(|p| p.memory_bytes)
            .map(|p| (p.name.clone(), p.memory_bytes))
    }

    /// Service counts: (total, active, inactive, failed)
    pub fn service_counts(&self) -> (usize, usize, usize, usize) {
        let total = self.services.len();
        let mut active = 0;
        let mut inactive = 0;
        let mut failed = 0;

        for s in &self.services {
            let st = s.active_state.to_lowercase();
            let sub = s.sub_state.to_lowercase();
            if st == "active" || sub == "running" {
                active += 1;
            } else if st == "failed" || sub == "failed" {
                failed += 1;
            } else if st == "inactive" || sub == "dead" {
                inactive += 1;
            }
        }

        (total, active, inactive, failed)
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
                // Category filter
                match self.process_category {
                    ProcessFilterCategory::All => {}
                    ProcessFilterCategory::ActiveOnly => {
                        if p.cpu_usage <= 0.05 {
                            return false;
                        }
                    }
                    ProcessFilterCategory::UserOnly => {
                        let u = p.user.to_lowercase();
                        if u == "root" || u == "system" || u.contains("daemon") {
                            return false;
                        }
                    }
                    ProcessFilterCategory::SystemOnly => {
                        let u = p.user.to_lowercase();
                        if u != "root" && u != "system" && !u.contains("daemon") {
                            return false;
                        }
                    }
                }

                // Text search query filter
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
                    let rank_a = process_status_rank(&a.status);
                    let rank_b = process_status_rank(&b.status);
                    rank_a.cmp(&rank_b).then_with(|| a.status.to_lowercase().cmp(&b.status.to_lowercase()))
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

    /// Build hierarchical rows for the process tree view
    pub fn build_tree_rows(&self) -> Vec<ProcessTreeRow<'_>> {
        let processes = self.filtered_processes();
        if self.view_mode == ProcessViewMode::Flat {
            return processes
                .into_iter()
                .map(|p| ProcessTreeRow {
                    item: p,
                    depth: 0,
                    has_children: false,
                    is_collapsed: false,
                    children_count: 0,
                    total_cpu: p.cpu_usage,
                    total_memory: p.memory_bytes,
                })
                .collect();
        }

        use std::collections::{HashMap, HashSet};

        let has_any_ppid = processes.iter().any(|p| p.parent_pid > 0);

        if has_any_ppid {
            // Mode A: Hierarchical tree based on parent_pid
            let mut children_map: HashMap<u32, Vec<&ntx_proto::ProcessItem>> = HashMap::new();
            let mut pid_set: HashSet<u32> = HashSet::new();

            for p in &processes {
                pid_set.insert(p.pid);
            }

            for p in &processes {
                if p.parent_pid > 0 && pid_set.contains(&p.parent_pid) && p.parent_pid != p.pid {
                    children_map.entry(p.parent_pid).or_default().push(p);
                }
            }

            fn calc_tree_totals(
                p: &ntx_proto::ProcessItem,
                children_map: &HashMap<u32, Vec<&ntx_proto::ProcessItem>>,
            ) -> (f32, u64, usize) {
                let mut cpu = p.cpu_usage;
                let mut mem = p.memory_bytes;
                let mut count = 0;
                if let Some(children) = children_map.get(&p.pid) {
                    count += children.len();
                    for child in children {
                        let (c_cpu, c_mem, c_count) = calc_tree_totals(child, children_map);
                        cpu += c_cpu;
                        mem += c_mem;
                        count += c_count;
                    }
                }
                (cpu, mem, count)
            }

            fn push_tree_node<'a>(
                p: &'a ntx_proto::ProcessItem,
                depth: usize,
                collapsed_pids: &HashSet<u32>,
                children_map: &HashMap<u32, Vec<&'a ntx_proto::ProcessItem>>,
                out: &mut Vec<ProcessTreeRow<'a>>,
            ) {
                let (total_cpu, total_mem, total_children) = calc_tree_totals(p, children_map);
                let has_children = total_children > 0;
                let is_collapsed = collapsed_pids.contains(&p.pid);

                out.push(ProcessTreeRow {
                    item: p,
                    depth,
                    has_children,
                    is_collapsed,
                    children_count: total_children,
                    total_cpu,
                    total_memory: total_mem,
                });

                if has_children && !is_collapsed {
                    if let Some(children) = children_map.get(&p.pid) {
                        for child in children {
                            push_tree_node(child, depth + 1, collapsed_pids, children_map, out);
                        }
                    }
                }
            }

            let mut out = Vec::new();
            for p in &processes {
                let is_root = p.parent_pid == 0 || !pid_set.contains(&p.parent_pid) || p.parent_pid == p.pid;
                if is_root {
                    push_tree_node(p, 0, &self.collapsed_pids, &children_map, &mut out);
                }
            }

            out
        } else {
            // Mode B: Fallback clustering by process name (when remote server lacks PPID support)
            let mut name_counts: HashMap<String, usize> = HashMap::new();
            for p in &processes {
                *name_counts.entry(p.name.clone()).or_insert(0) += 1;
            }

            let mut seen_names: HashSet<String> = HashSet::new();
            let mut name_children: HashMap<String, Vec<&ntx_proto::ProcessItem>> = HashMap::new();
            let mut leaders: Vec<&ntx_proto::ProcessItem> = Vec::new();

            for p in &processes {
                let count = *name_counts.get(&p.name).unwrap_or(&1);
                if count > 1 {
                    if seen_names.insert(p.name.clone()) {
                        leaders.push(p);
                    } else {
                        name_children.entry(p.name.clone()).or_default().push(p);
                    }
                } else {
                    leaders.push(p);
                }
            }

            let mut out = Vec::new();
            for leader in leaders {
                if let Some(children) = name_children.get(&leader.name) {
                    let children_count = children.len();
                    let has_children = children_count > 0;
                    let is_collapsed = self.collapsed_pids.contains(&leader.pid);
                    let total_cpu = leader.cpu_usage + children.iter().map(|c| c.cpu_usage).sum::<f32>();
                    let total_memory = leader.memory_bytes + children.iter().map(|c| c.memory_bytes).sum::<u64>();

                    out.push(ProcessTreeRow {
                        item: leader,
                        depth: 0,
                        has_children,
                        is_collapsed,
                        children_count,
                        total_cpu,
                        total_memory,
                    });

                    if has_children && !is_collapsed {
                        for child in children {
                            out.push(ProcessTreeRow {
                                item: child,
                                depth: 1,
                                has_children: false,
                                is_collapsed: false,
                                children_count: 0,
                                total_cpu: child.cpu_usage,
                                total_memory: child.memory_bytes,
                            });
                        }
                    }
                } else {
                    out.push(ProcessTreeRow {
                        item: leader,
                        depth: 0,
                        has_children: false,
                        is_collapsed: false,
                        children_count: 0,
                        total_cpu: leader.cpu_usage,
                        total_memory: leader.memory_bytes,
                    });
                }
            }

            out
        }
    }

    /// Get sorted and filtered slice of services
    pub fn filtered_services(&self) -> Vec<&ntx_proto::ServiceItem> {
        let q = self.service_filter.trim().to_lowercase();
        let mut list: Vec<&ntx_proto::ServiceItem> = self
            .services
            .iter()
            .filter(|s| {
                // Category filter
                match self.service_category {
                    ServiceFilterCategory::All => {}
                    ServiceFilterCategory::ActiveOnly => {
                        let st = s.active_state.to_lowercase();
                        let sub = s.sub_state.to_lowercase();
                        if st != "active" && sub != "running" {
                            return false;
                        }
                    }
                    ServiceFilterCategory::InactiveOnly => {
                        let st = s.active_state.to_lowercase();
                        let sub = s.sub_state.to_lowercase();
                        if st != "inactive" && sub != "dead" {
                            return false;
                        }
                    }
                    ServiceFilterCategory::FailedOnly => {
                        let st = s.active_state.to_lowercase();
                        let sub = s.sub_state.to_lowercase();
                        if st != "failed" && sub != "failed" {
                            return false;
                        }
                    }
                }

                // Text search query filter
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
                    let rank_a = service_state_rank(&a.active_state);
                    let rank_b = service_state_rank(&b.active_state);
                    rank_a.cmp(&rank_b).then_with(|| a.active_state.to_lowercase().cmp(&b.active_state.to_lowercase()))
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

/// Type of favorite application or workload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FavoriteAppType {
    #[default]
    Process,
    Docker,
    Service,
    Custom,
}

impl FavoriteAppType {
    pub fn display_name(&self) -> &'static str {
        match self {
            FavoriteAppType::Process => "Process",
            FavoriteAppType::Docker => "Docker",
            FavoriteAppType::Service => "Service",
            FavoriteAppType::Custom => "Custom",
        }
    }
}

/// A favorite application or workload pinned for quick access and monitoring.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FavoriteApp {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub app_type: FavoriteAppType,
    pub process_filter: String,
    pub service_name: Option<String>,
    pub terminal_command: String,
    pub description: String,
    pub auto_start_command: Option<String>,
}

impl Default for FavoriteApp {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            icon: "⭐".to_string(),
            app_type: FavoriteAppType::Process,
            process_filter: String::new(),
            service_name: None,
            terminal_command: String::new(),
            description: String::new(),
            auto_start_command: None,
        }
    }
}

impl FavoriteApp {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        icon: impl Into<String>,
        app_type: FavoriteAppType,
        process_filter: impl Into<String>,
        service_name: Option<String>,
        terminal_command: impl Into<String>,
        description: impl Into<String>,
        auto_start_command: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            icon: icon.into(),
            app_type,
            process_filter: process_filter.into(),
            service_name,
            terminal_command: terminal_command.into(),
            description: description.into(),
            auto_start_command,
        }
    }

    /// Check whether this favorite application matches a running process.
    /// Matches against process name, command line, or numeric PID if the filter parses as a number.
    pub fn matches_process(&self, p: &ntx_proto::ProcessItem) -> bool {
        let filter = self.process_filter.trim();
        let query = if !filter.is_empty() {
            filter
        } else {
            self.name.trim()
        };

        if query.is_empty() {
            return false;
        }

        // Numeric PID matching
        if let Ok(pid) = query.parse::<u32>() {
            if p.pid == pid {
                return true;
            }
        }

        let q_lower = query.to_lowercase();
        p.name.to_lowercase().contains(&q_lower)
            || p.command_line.to_lowercase().contains(&q_lower)
    }

    /// Check whether this favorite application matches a system service.
    /// Matches against service name (with or without .service suffix) or description.
    pub fn matches_service(&self, s: &ntx_proto::ServiceItem) -> bool {
        let query = self
            .service_name
            .as_deref()
            .map(|sn| sn.trim())
            .filter(|sn| !sn.is_empty())
            .unwrap_or_else(|| {
                if !self.process_filter.trim().is_empty() {
                    self.process_filter.trim()
                } else {
                    self.name.trim()
                }
            });

        if query.is_empty() {
            return false;
        }

        let q_lower = query.to_lowercase();
        let s_lower = s.name.to_lowercase();

        // Exact match
        if s_lower == q_lower {
            return true;
        }

        // Normalized matching without .service suffix
        let s_base = s_lower.strip_suffix(".service").unwrap_or(&s_lower);
        let q_base = q_lower.strip_suffix(".service").unwrap_or(&q_lower);
        if s_base == q_base {
            return true;
        }

        // Substring matching on service name or description
        s_lower.contains(&q_lower) || s.description.to_lowercase().contains(&q_lower)
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
    // Favorite applications configured for this device
    pub favorites: Vec<FavoriteApp>,
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
            favorites: Vec::new(),
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
    #[serde(default)]
    pub favorite_apps: HashMap<String, Vec<FavoriteApp>>,
}

/// Type alias for `AppSettings`.
pub type Settings = AppSettings;

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
            favorite_apps: HashMap::new(),
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
                parent_pid: 0,
            },
            ntx_proto::ProcessItem {
                pid: 200,
                name: "chrome".to_string(),
                cpu_usage: 15.2,
                memory_bytes: 500 * 1024 * 1024,
                user: "user".to_string(),
                status: "sleeping".to_string(),
                command_line: "/opt/google/chrome".to_string(),
                parent_pid: 100,
            },
            ntx_proto::ProcessItem {
                pid: 300,
                name: "ntx-server".to_string(),
                cpu_usage: 2.1,
                memory_bytes: 25 * 1024 * 1024,
                user: "root".to_string(),
                status: "running".to_string(),
                command_line: "./ntx-server".to_string(),
                parent_pid: 100,
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
                parent_pid: 0,
            },
            ntx_proto::ProcessItem {
                pid: 2,
                name: "a_proc".to_string(),
                cpu_usage: 20.0,
                memory_bytes: 100,
                user: "bob".to_string(),
                status: "Sleeping".to_string(),
                command_line: "a_proc --arg2".to_string(),
                parent_pid: 0,
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

        // Sort by Status (Run prioritized over Sleeping)
        state.toggle_process_sort(ProcessSortColumn::Status);
        let list = state.filtered_processes();
        assert_eq!(list[0].status, "Running");

        // Sort by CommandLine
        state.toggle_process_sort(ProcessSortColumn::CommandLine);
        let list = state.filtered_processes();
        assert_eq!(list[0].name, "a_proc");
    }

    #[test]
    fn test_process_status_rank_smart_sorting() {
        assert_eq!(process_status_rank("Running"), 0);
        assert_eq!(process_status_rank("Run"), 0);
        assert_eq!(process_status_rank("Sleep"), 1);
        assert_eq!(process_status_rank("Idle"), 1);
        assert_eq!(process_status_rank("Stop"), 2);
        assert_eq!(process_status_rank("Zombie"), 3);

        let mut state = ProcessState::default();
        state.processes = vec![
            ntx_proto::ProcessItem {
                pid: 10,
                name: "zombie_proc".to_string(),
                cpu_usage: 0.0,
                memory_bytes: 0,
                user: "root".to_string(),
                status: "Zombie".to_string(),
                command_line: "zombie".to_string(),
                parent_pid: 0,
            },
            ntx_proto::ProcessItem {
                pid: 20,
                name: "running_proc".to_string(),
                cpu_usage: 5.0,
                memory_bytes: 1000,
                user: "root".to_string(),
                status: "Run".to_string(),
                command_line: "running".to_string(),
                parent_pid: 0,
            },
            ntx_proto::ProcessItem {
                pid: 30,
                name: "sleep_proc".to_string(),
                cpu_usage: 0.0,
                memory_bytes: 1000,
                user: "root".to_string(),
                status: "Sleep".to_string(),
                command_line: "sleeping".to_string(),
                parent_pid: 0,
            },
        ];

        state.process_sort_column = ProcessSortColumn::Status;
        state.process_sort_direction = SortDirection::Ascending;
        let sorted = state.filtered_processes();
        // Priority: Run (rank 0) -> Sleep (rank 1) -> Zombie (rank 3)
        assert_eq!(sorted[0].pid, 20);
        assert_eq!(sorted[1].pid, 30);
        assert_eq!(sorted[2].pid, 10);
    }

    #[test]
    fn test_tree_hierarchy_and_collapse() {
        let mut state = ProcessState::default();
        state.processes = vec![
            ntx_proto::ProcessItem {
                pid: 1,
                name: "init".to_string(),
                cpu_usage: 1.0,
                memory_bytes: 100,
                user: "root".to_string(),
                status: "Run".to_string(),
                command_line: "/sbin/init".to_string(),
                parent_pid: 0,
            },
            ntx_proto::ProcessItem {
                pid: 10,
                name: "worker".to_string(),
                cpu_usage: 2.0,
                memory_bytes: 200,
                user: "root".to_string(),
                status: "Run".to_string(),
                command_line: "/usr/bin/worker".to_string(),
                parent_pid: 1,
            },
            ntx_proto::ProcessItem {
                pid: 100,
                name: "subworker".to_string(),
                cpu_usage: 3.0,
                memory_bytes: 300,
                user: "root".to_string(),
                status: "Run".to_string(),
                command_line: "/usr/bin/subworker".to_string(),
                parent_pid: 10,
            },
        ];

        // Tree view: all expanded
        let rows = state.build_tree_rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].item.pid, 1);
        assert_eq!(rows[0].depth, 0);
        assert!(rows[0].has_children);
        assert_eq!(rows[0].children_count, 2);
        assert!((rows[0].total_cpu - 6.0).abs() < 0.001); // 1 + 2 + 3
        assert_eq!(rows[0].total_memory, 600); // 100 + 200 + 300

        assert_eq!(rows[1].item.pid, 10);
        assert_eq!(rows[1].depth, 1);
        assert!(rows[1].has_children);
        assert_eq!(rows[1].children_count, 1);

        assert_eq!(rows[2].item.pid, 100);
        assert_eq!(rows[2].depth, 2);
        assert!(!rows[2].has_children);

        // Collapse node 1
        state.toggle_collapse(1);
        let rows_collapsed = state.build_tree_rows();
        assert_eq!(rows_collapsed.len(), 1);
        assert_eq!(rows_collapsed[0].item.pid, 1);
        assert!(rows_collapsed[0].is_collapsed);

        // Expand all
        state.expand_all();
        let rows_expanded = state.build_tree_rows();
        assert_eq!(rows_expanded.len(), 3);

        // Flat view mode
        state.view_mode = ProcessViewMode::Flat;
        let flat_rows = state.build_tree_rows();
        assert_eq!(flat_rows.len(), 3);
        assert_eq!(flat_rows[0].depth, 0);
    }

    #[test]
    fn test_process_and_service_counts() {
        let mut state = ProcessState::default();
        state.processes = vec![
            ntx_proto::ProcessItem {
                pid: 1,
                name: "p1".to_string(),
                cpu_usage: 10.0,
                memory_bytes: 1024,
                user: "root".to_string(),
                status: "Run".to_string(),
                command_line: "p1".to_string(),
                parent_pid: 0,
            },
            ntx_proto::ProcessItem {
                pid: 2,
                name: "p2".to_string(),
                cpu_usage: 25.0,
                memory_bytes: 512,
                user: "alice".to_string(),
                status: "Sleeping".to_string(),
                command_line: "p2".to_string(),
                parent_pid: 0,
            },
        ];

        let (total, running, sleeping, zombies) = state.process_counts();
        assert_eq!(total, 2);
        assert_eq!(running, 1);
        assert_eq!(sleeping, 1);
        assert_eq!(zombies, 0);

        let top_cpu = state.top_cpu_process().unwrap();
        assert_eq!(top_cpu.0, "p2");
        assert_eq!(top_cpu.1, 25.0);

        let top_mem = state.top_mem_process().unwrap();
        assert_eq!(top_mem.0, "p1");
        assert_eq!(top_mem.1, 1024);
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

    #[test]
    fn test_tree_fallback_name_clustering() {
        let mut state = ProcessState::default();
        // All parent_pid = 0 (simulates older server without PPID)
        state.processes = vec![
            ntx_proto::ProcessItem {
                pid: 10,
                name: "python3".to_string(),
                cpu_usage: 5.0,
                memory_bytes: 100,
                user: "user".to_string(),
                status: "Run".to_string(),
                command_line: "python3 worker1.py".to_string(),
                parent_pid: 0,
            },
            ntx_proto::ProcessItem {
                pid: 11,
                name: "python3".to_string(),
                cpu_usage: 10.0,
                memory_bytes: 150,
                user: "user".to_string(),
                status: "Run".to_string(),
                command_line: "python3 worker2.py".to_string(),
                parent_pid: 0,
            },
            ntx_proto::ProcessItem {
                pid: 20,
                name: "standalone".to_string(),
                cpu_usage: 1.0,
                memory_bytes: 50,
                user: "root".to_string(),
                status: "Sleep".to_string(),
                command_line: "standalone".to_string(),
                parent_pid: 0,
            },
        ];

        let rows = state.build_tree_rows();
        assert_eq!(rows.len(), 3);
        // python3 leader
        assert_eq!(rows[0].item.pid, 11); // sorted by CPU default: 11 has 10.0, 10 has 5.0
        assert!(rows[0].has_children);
        assert_eq!(rows[0].children_count, 1);
        assert!((rows[0].total_cpu - 15.0).abs() < 0.001);
        assert_eq!(rows[0].total_memory, 250);

        // python3 child
        assert_eq!(rows[1].item.pid, 10);
        assert_eq!(rows[1].depth, 1);
        assert!(!rows[1].has_children);

        // standalone
        assert_eq!(rows[2].item.pid, 20);
        assert!(!rows[2].has_children);

        // Collapse python3
        state.toggle_collapse(rows[0].item.pid);
        let rows_collapsed = state.build_tree_rows();
        assert_eq!(rows_collapsed.len(), 2);
        assert!(rows_collapsed[0].is_collapsed);

        // Test collapse_all
        state.expand_all();
        assert_eq!(state.build_tree_rows().len(), 3);
        state.collapse_all();
        assert_eq!(state.build_tree_rows().len(), 2);
    }

    #[test]
    fn test_favorite_app_matches_process() {
        let app = FavoriteApp {
            id: "fav-1".to_string(),
            name: "Nginx Server".to_string(),
            icon: "🌐".to_string(),
            app_type: FavoriteAppType::Process,
            process_filter: "nginx".to_string(),
            service_name: None,
            terminal_command: "systemctl status nginx".to_string(),
            description: "Main reverse proxy".to_string(),
            auto_start_command: Some("systemctl start nginx".to_string()),
        };

        // Match by process name
        let p1 = ntx_proto::ProcessItem {
            pid: 1001,
            name: "nginx".to_string(),
            cpu_usage: 1.5,
            memory_bytes: 50 * 1024 * 1024,
            user: "www-data".to_string(),
            status: "running".to_string(),
            command_line: "nginx: master process".to_string(),
            parent_pid: 1,
        };
        assert!(app.matches_process(&p1));

        // Match by command line
        let p2 = ntx_proto::ProcessItem {
            pid: 1002,
            name: "worker".to_string(),
            cpu_usage: 0.8,
            memory_bytes: 20 * 1024 * 1024,
            user: "www-data".to_string(),
            status: "running".to_string(),
            command_line: "/usr/sbin/nginx -g daemon off;".to_string(),
            parent_pid: 1001,
        };
        assert!(app.matches_process(&p2));

        // Non-matching process
        let p3 = ntx_proto::ProcessItem {
            pid: 2001,
            name: "postgres".to_string(),
            cpu_usage: 5.0,
            memory_bytes: 200 * 1024 * 1024,
            user: "postgres".to_string(),
            status: "running".to_string(),
            command_line: "/usr/lib/postgresql/bin/postgres".to_string(),
            parent_pid: 1,
        };
        assert!(!app.matches_process(&p3));

        // Match by PID when process_filter is a number
        let app_pid = FavoriteApp {
            id: "fav-pid".to_string(),
            name: "Dedicated Worker".to_string(),
            icon: "⚡".to_string(),
            app_type: FavoriteAppType::Process,
            process_filter: "1001".to_string(),
            service_name: None,
            terminal_command: String::new(),
            description: String::new(),
            auto_start_command: None,
        };
        assert!(app_pid.matches_process(&p1));
        assert!(!app_pid.matches_process(&p2));

        // Fallback to name when process_filter is empty
        let app_empty_filter = FavoriteApp {
            id: "fav-name".to_string(),
            name: "postgres".to_string(),
            icon: "🐘".to_string(),
            app_type: FavoriteAppType::Process,
            process_filter: String::new(),
            service_name: None,
            terminal_command: String::new(),
            description: String::new(),
            auto_start_command: None,
        };
        assert!(app_empty_filter.matches_process(&p3));
        assert!(!app_empty_filter.matches_process(&p1));
    }

    #[test]
    fn test_favorite_app_matches_service() {
        let app = FavoriteApp {
            id: "fav-svc".to_string(),
            name: "Docker Engine".to_string(),
            icon: "🐳".to_string(),
            app_type: FavoriteAppType::Service,
            process_filter: "dockerd".to_string(),
            service_name: Some("docker.service".to_string()),
            terminal_command: "systemctl status docker".to_string(),
            description: "Docker daemon".to_string(),
            auto_start_command: Some("systemctl start docker".to_string()),
        };

        // Exact match with .service
        let s1 = ntx_proto::ServiceItem {
            name: "docker.service".to_string(),
            description: "Docker Application Container Engine".to_string(),
            load_state: "loaded".to_string(),
            active_state: "active".to_string(),
            sub_state: "running".to_string(),
        };
        assert!(app.matches_service(&s1));

        // Match without .service in target service item
        let s1_short = ntx_proto::ServiceItem {
            name: "docker".to_string(),
            description: "Docker daemon".to_string(),
            load_state: "loaded".to_string(),
            active_state: "active".to_string(),
            sub_state: "running".to_string(),
        };
        assert!(app.matches_service(&s1_short));

        // When favorite app has service_name without .service
        let app_short = FavoriteApp {
            id: "fav-nginx".to_string(),
            name: "Nginx".to_string(),
            icon: "🌐".to_string(),
            app_type: FavoriteAppType::Service,
            process_filter: String::new(),
            service_name: Some("nginx".to_string()),
            terminal_command: String::new(),
            description: String::new(),
            auto_start_command: None,
        };
        let s2 = ntx_proto::ServiceItem {
            name: "nginx.service".to_string(),
            description: "A high performance web server".to_string(),
            load_state: "loaded".to_string(),
            active_state: "active".to_string(),
            sub_state: "running".to_string(),
        };
        assert!(app_short.matches_service(&s2));

        // Match by description substring
        let s3 = ntx_proto::ServiceItem {
            name: "web-reverse-proxy.service".to_string(),
            description: "A high performance nginx web server and proxy".to_string(),
            load_state: "loaded".to_string(),
            active_state: "active".to_string(),
            sub_state: "running".to_string(),
        };
        assert!(app_short.matches_service(&s3));

        // Non-matching service
        let s4 = ntx_proto::ServiceItem {
            name: "ssh.service".to_string(),
            description: "OpenSSH Server Daemon".to_string(),
            load_state: "loaded".to_string(),
            active_state: "active".to_string(),
            sub_state: "running".to_string(),
        };
        assert!(!app.matches_service(&s4));
        assert!(!app_short.matches_service(&s4));

        // Fallback to process_filter / name when service_name is None
        let app_fallback = FavoriteApp {
            id: "fav-fb".to_string(),
            name: "ssh".to_string(),
            icon: "🔒".to_string(),
            app_type: FavoriteAppType::Service,
            process_filter: String::new(),
            service_name: None,
            terminal_command: String::new(),
            description: String::new(),
            auto_start_command: None,
        };
        assert!(app_fallback.matches_service(&s4));
    }

    #[test]
    fn test_favorite_app_serialization() {
        let app = FavoriteApp::new(
            "app-1",
            "PostgreSQL",
            "🐘",
            FavoriteAppType::Docker,
            "postgres",
            Some("postgres.service".to_string()),
            "docker logs postgres",
            "Main database instance",
            Some("docker start postgres".to_string()),
        );

        // JSON serialization
        let json_str = serde_json::to_string(&app).expect("JSON serialization failed");
        let from_json: FavoriteApp = serde_json::from_str(&json_str).expect("JSON deserialization failed");
        assert_eq!(app, from_json);

        // TOML serialization
        let toml_str = toml::to_string(&app).expect("TOML serialization failed");
        let from_toml: FavoriteApp = toml::from_str(&toml_str).expect("TOML deserialization failed");
        assert_eq!(app, from_toml);

        // Test all enum variants
        let types = vec![
            FavoriteAppType::Process,
            FavoriteAppType::Docker,
            FavoriteAppType::Service,
            FavoriteAppType::Custom,
        ];
        for t in types {
            let s = serde_json::to_string(&t).unwrap();
            let parsed: FavoriteAppType = serde_json::from_str(&s).unwrap();
            assert_eq!(t, parsed);
        }
    }

    #[test]
    fn test_settings_favorite_apps_persistence() {
        let mut settings = Settings::default();
        assert!(settings.favorite_apps.is_empty());

        let dev1_apps = vec![
            FavoriteApp::new(
                "app-1",
                "Nginx",
                "🌐",
                FavoriteAppType::Service,
                "nginx",
                Some("nginx.service".to_string()),
                "systemctl status nginx",
                "Web server",
                Some("systemctl start nginx".to_string()),
            ),
            FavoriteApp::new(
                "app-2",
                "Redis",
                "🔴",
                FavoriteAppType::Process,
                "redis-server",
                None,
                "redis-cli ping",
                "Cache",
                None,
            ),
        ];

        let dev2_apps = vec![
            FavoriteApp::new(
                "app-3",
                "Docker Compose Stack",
                "🐳",
                FavoriteAppType::Custom,
                "docker",
                None,
                "docker compose ps",
                "Microservices stack",
                Some("docker compose up -d".to_string()),
            ),
        ];

        settings.favorite_apps.insert("dev-1".to_string(), dev1_apps.clone());
        settings.favorite_apps.insert("dev-2".to_string(), dev2_apps.clone());

        // Serialize to TOML
        let toml_str = toml::to_string_pretty(&settings).expect("TOML settings serialization failed");
        let loaded: Settings = toml::from_str(&toml_str).expect("TOML settings deserialization failed");

        assert_eq!(loaded.favorite_apps.len(), 2);
        assert_eq!(loaded.favorite_apps.get("dev-1"), Some(&dev1_apps));
        assert_eq!(loaded.favorite_apps.get("dev-2"), Some(&dev2_apps));

        // Backward compatibility: TOML without favorite_apps deserializes cleanly
        let old_toml = r#"
            default_psk = "old_psk"
            viewer_id = "test_viewer"
            refresh_rate_ms = 1000
            history_capacity = 60
            dark_mode = true
            manual_devices = []
            known_devices = []
            [device_psks]
        "#;
        let backwards_compatible: Settings = toml::from_str(old_toml).expect("Backward compatibility deserialization failed");
        assert!(backwards_compatible.favorite_apps.is_empty());
        assert_eq!(backwards_compatible.default_psk, "old_psk");
    }

    #[test]
    fn test_device_initializes_with_empty_favorites() {
        let dev = Device::new(
            "dev_id".to_string(),
            "hostname".to_string(),
            vec!["127.0.0.1".to_string()],
            9850,
            "1.0.0".to_string(),
        );
        assert!(dev.favorites.is_empty());
    }
}
