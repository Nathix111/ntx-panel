use crate::terminal_emu::TerminalEmulator;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Connection status of a discovered or added device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Discovered,
    Connecting,
    Connected,
    AuthFailed(String),
    TlsMismatch { expected: String, actual: String },
    Error(String),
    Disconnected,
}

impl ConnectionStatus {
    pub fn is_connected(&self) -> bool {
        matches!(self, ConnectionStatus::Connected)
    }

    pub fn display_text(&self) -> &str {
        match self {
            ConnectionStatus::Discovered => "Découvert",
            ConnectionStatus::Connecting => "Connexion...",
            ConnectionStatus::Connected => "Connecté",
            ConnectionStatus::AuthFailed(_) => "Échec d'authentification",
            ConnectionStatus::TlsMismatch { .. } => "Certificat TLS modifié",
            ConnectionStatus::Error(_) => "Erreur",
            ConnectionStatus::Disconnected => "Déconnecté",
        }
    }
}

/// Lifecycle state of the interactive remote PTY terminal for a device.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TerminalSessionState {
    #[default]
    Idle,
    Connecting,
    Open,
    ClosedByUser,
    Error(String),
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

/// Confirmation prompt payload for destructive process/service actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingProcessConfirm {
    KillProcess {
        pid: u32,
        name: String,
        signal: i32,
    },
    ControlService {
        service_name: String,
        action: String,
    },
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
    pub collapsed_pids: HashSet<u32>,
    pub selected_pid: Option<u32>,
    pub is_paused: bool,
    pub auto_refresh: bool,
    pub auto_refresh_interval_secs: u64,
    pub last_process_refresh: Option<Instant>,
    pub last_service_refresh: Option<Instant>,
    pub is_loading_processes: bool,
    pub is_loading_services: bool,
    pub active_subtab: ProcessSubTab,
    pub status_message: Option<(String, bool)>,
    pub pending_confirm: Option<PendingProcessConfirm>,
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
            collapsed_pids: HashSet::new(),
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
            pending_confirm: None,
        }
    }
}

impl ProcessState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_status(&mut self, msg: impl Into<String>, is_error: bool) {
        self.status_message = Some((msg.into(), is_error));
    }

    pub fn clear_status(&mut self) {
        self.status_message = None;
    }

    pub fn toggle_collapse(&mut self, pid: u32) {
        if !self.collapsed_pids.remove(&pid) {
            self.collapsed_pids.insert(pid);
        }
    }

    pub fn expand_all(&mut self) {
        self.collapsed_pids.clear();
    }

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

    pub fn top_cpu_process(&self) -> Option<(String, f32)> {
        self.processes
            .iter()
            .max_by(|a, b| {
                a.cpu_usage
                    .partial_cmp(&b.cpu_usage)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|p| (p.name.clone(), p.cpu_usage))
    }

    pub fn top_mem_process(&self) -> Option<(String, u64)> {
        self.processes
            .iter()
            .max_by_key(|p| p.memory_bytes)
            .map(|p| (p.name.clone(), p.memory_bytes))
    }

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

    pub fn filtered_processes(&self) -> Vec<&ntx_proto::ProcessItem> {
        let q = self.process_filter.trim().to_lowercase();
        let mut list: Vec<&ntx_proto::ProcessItem> = self
            .processes
            .iter()
            .filter(|p| {
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
                    rank_a
                        .cmp(&rank_b)
                        .then_with(|| a.status.to_lowercase().cmp(&b.status.to_lowercase()))
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

    pub fn build_tree_rows(&self) -> Vec<ProcessTreeRow<'_>> {
        let processes = self.filtered_processes();
        self.build_tree_rows_from_filtered(&processes)
    }

    pub fn build_tree_rows_from_filtered<'a>(
        &self,
        processes: &[&'a ntx_proto::ProcessItem],
    ) -> Vec<ProcessTreeRow<'a>> {
        if self.view_mode == ProcessViewMode::Flat {
            return processes
                .iter()
                .map(|&p| ProcessTreeRow {
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

        let has_any_ppid = processes.iter().any(|p| p.parent_pid > 0);

        if has_any_ppid {
            let mut proc_map: HashMap<u32, &'a ntx_proto::ProcessItem> =
                HashMap::with_capacity(processes.len());
            let mut children_map: HashMap<u32, Vec<&'a ntx_proto::ProcessItem>> = HashMap::new();

            for &p in processes {
                proc_map.insert(p.pid, p);
            }

            for &p in processes {
                if p.parent_pid > 0 && proc_map.contains_key(&p.parent_pid) && p.parent_pid != p.pid
                {
                    children_map.entry(p.parent_pid).or_default().push(p);
                }
            }

            fn calc_totals_memo(
                pid: u32,
                proc_map: &HashMap<u32, &ntx_proto::ProcessItem>,
                children_map: &HashMap<u32, Vec<&ntx_proto::ProcessItem>>,
                memo: &mut HashMap<u32, (f32, u64, usize)>,
                visiting: &mut HashSet<u32>,
            ) -> (f32, u64, usize) {
                if let Some(&cached) = memo.get(&pid) {
                    return cached;
                }
                if !visiting.insert(pid) {
                    return (0.0, 0, 0);
                }

                let Some(p) = proc_map.get(&pid) else {
                    visiting.remove(&pid);
                    return (0.0, 0, 0);
                };

                let mut cpu = p.cpu_usage;
                let mut mem = p.memory_bytes;
                let mut count = 0;

                if let Some(children) = children_map.get(&pid) {
                    for child in children {
                        if visiting.contains(&child.pid) {
                            continue;
                        }
                        count += 1;
                        let (c_cpu, c_mem, c_count) =
                            calc_totals_memo(child.pid, proc_map, children_map, memo, visiting);
                        cpu += c_cpu;
                        mem += c_mem;
                        count += c_count;
                    }
                }

                visiting.remove(&pid);
                let result = (cpu, mem, count);
                memo.insert(pid, result);
                result
            }

            fn mark_subtree_visited(
                pid: u32,
                children_map: &HashMap<u32, Vec<&ntx_proto::ProcessItem>>,
                visited: &mut HashSet<u32>,
            ) {
                if let Some(children) = children_map.get(&pid) {
                    for child in children {
                        if visited.insert(child.pid) {
                            mark_subtree_visited(child.pid, children_map, visited);
                        }
                    }
                }
            }

            #[allow(clippy::too_many_arguments)]
            fn push_tree_node<'a>(
                p: &'a ntx_proto::ProcessItem,
                depth: usize,
                collapsed_pids: &HashSet<u32>,
                children_map: &HashMap<u32, Vec<&'a ntx_proto::ProcessItem>>,
                memo: &HashMap<u32, (f32, u64, usize)>,
                visited: &mut HashSet<u32>,
                out: &mut Vec<ProcessTreeRow<'a>>,
            ) {
                if !visited.insert(p.pid) {
                    return;
                }

                let (total_cpu, total_mem, total_children) = memo
                    .get(&p.pid)
                    .copied()
                    .unwrap_or((p.cpu_usage, p.memory_bytes, 0));
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

                if has_children {
                    if is_collapsed {
                        mark_subtree_visited(p.pid, children_map, visited);
                    } else if let Some(children) = children_map.get(&p.pid) {
                        for child in children {
                            push_tree_node(
                                child,
                                depth + 1,
                                collapsed_pids,
                                children_map,
                                memo,
                                visited,
                                out,
                            );
                        }
                    }
                }
            }

            let mut memo = HashMap::with_capacity(processes.len());
            let mut visiting = HashSet::new();
            for &p in processes {
                calc_totals_memo(p.pid, &proc_map, &children_map, &mut memo, &mut visiting);
            }

            let mut visited = HashSet::with_capacity(processes.len());
            let mut out = Vec::with_capacity(processes.len());

            // 1. Traverse all natural roots
            for &p in processes {
                let is_root =
                    p.parent_pid == 0 || !proc_map.contains_key(&p.parent_pid) || p.parent_pid == p.pid;
                if is_root {
                    push_tree_node(
                        p,
                        0,
                        &self.collapsed_pids,
                        &children_map,
                        &memo,
                        &mut visited,
                        &mut out,
                    );
                }
            }

            // 2. Any remaining unvisited processes belong to disconnected PPID cycles
            for &p in processes {
                if !visited.contains(&p.pid) {
                    push_tree_node(
                        p,
                        0,
                        &self.collapsed_pids,
                        &children_map,
                        &memo,
                        &mut visited,
                        &mut out,
                    );
                }
            }

            out
        } else {
            let mut name_counts: HashMap<&str, usize> = HashMap::new();
            for &p in processes {
                *name_counts.entry(p.name.as_str()).or_insert(0) += 1;
            }

            let mut seen_names: HashSet<&str> = HashSet::new();
            let mut name_children: HashMap<&str, Vec<&'a ntx_proto::ProcessItem>> = HashMap::new();
            let mut leaders: Vec<&'a ntx_proto::ProcessItem> = Vec::new();

            for &p in processes {
                let count = *name_counts.get(p.name.as_str()).unwrap_or(&1);
                if count > 1 {
                    if seen_names.insert(p.name.as_str()) {
                        leaders.push(p);
                    } else {
                        name_children.entry(p.name.as_str()).or_default().push(p);
                    }
                } else {
                    leaders.push(p);
                }
            }

            let mut out = Vec::with_capacity(processes.len());
            for leader in leaders {
                if let Some(children) = name_children.get(leader.name.as_str()) {
                    let children_count = children.len();
                    let has_children = children_count > 0;
                    let is_collapsed = self.collapsed_pids.contains(&leader.pid);
                    let total_cpu =
                        leader.cpu_usage + children.iter().map(|c| c.cpu_usage).sum::<f32>();
                    let total_memory =
                        leader.memory_bytes + children.iter().map(|c| c.memory_bytes).sum::<u64>();

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

    pub fn filtered_services(&self) -> Vec<&ntx_proto::ServiceItem> {
        let q = self.service_filter.trim().to_lowercase();
        let mut list: Vec<&ntx_proto::ServiceItem> = self
            .services
            .iter()
            .filter(|s| {
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
                    rank_a.cmp(&rank_b).then_with(|| {
                        a.active_state
                            .to_lowercase()
                            .cmp(&b.active_state.to_lowercase())
                    })
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
            FavoriteAppType::Process => "Processus",
            FavoriteAppType::Docker => "Docker",
            FavoriteAppType::Service => "Service",
            FavoriteAppType::Custom => "Personnalisé",
        }
    }
}

/// A favorite application or workload pinned for quick access and monitoring.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
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

        if let Ok(pid) = query.parse::<u32>() {
            if p.pid == pid {
                return true;
            }
        }

        let q_lower = query.to_lowercase();
        p.name.to_lowercase().contains(&q_lower)
            || p.command_line.to_lowercase().contains(&q_lower)
    }

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

        if s_lower == q_lower {
            return true;
        }

        let s_base = s_lower.strip_suffix(".service").unwrap_or(&s_lower);
        let q_base = q_lower.strip_suffix(".service").unwrap_or(&q_lower);
        if s_base == q_base {
            return true;
        }

        s_lower.contains(&q_lower) || s.description.to_lowercase().contains(&q_lower)
    }
}

/// A remote machine managed by NTX Panel.
#[derive(Debug)]
pub struct Device {
    pub id: String,
    pub hostname: String,
    pub addresses: Vec<String>,
    pub port: u16,
    pub version: String,
    pub status: ConnectionStatus,
    pub psk: Option<String>,
    pub cert_fingerprint: Option<String>,
    pub mdns_fingerprint: Option<String>,
    pub is_manual: bool,
    pub auto_connect: bool,
    pub last_ping_ms: Option<u32>,
    pub server_uptime_secs: Option<u64>,
    pub metrics_paused: bool,
    pub system_info: Option<ntx_proto::SystemInfo>,
    pub latest_metrics: Option<ntx_proto::SystemMetrics>,
    pub cpu_history: VecDeque<f64>,
    pub ram_history: VecDeque<f64>,
    pub rx_history: VecDeque<f64>,
    pub tx_history: VecDeque<f64>,
    pub terminal: TerminalEmulator,
    pub terminal_state: TerminalSessionState,
    pub pending_terminal_command: Option<String>,
    pub process_state: ProcessState,
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
            cert_fingerprint: None,
            mdns_fingerprint: None,
            is_manual: false,
            auto_connect: false,
            last_ping_ms: None,
            server_uptime_secs: None,
            metrics_paused: false,
            system_info: None,
            latest_metrics: None,
            cpu_history: VecDeque::with_capacity(120),
            ram_history: VecDeque::with_capacity(120),
            rx_history: VecDeque::with_capacity(120),
            tx_history: VecDeque::with_capacity(120),
            terminal: TerminalEmulator::default(),
            terminal_state: TerminalSessionState::Idle,
            pending_terminal_command: None,
            process_state: ProcessState::default(),
            favorites: Vec::new(),
        }
    }

    pub fn primary_address(&self) -> String {
        self.addresses
            .first()
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string())
    }

    /// Full TLS gRPC endpoint: `https://<ip>:<port>` or `https://[<ipv6>]:<port>`.
    pub fn grpc_endpoint(&self) -> String {
        Self::format_grpc_url(&self.primary_address(), self.port)
    }

    fn format_grpc_url(raw_addr: &str, port: u16) -> String {
        let trimmed = raw_addr.trim().trim_start_matches('[').trim_end_matches(']');
        if let Ok(IpAddr::V6(_)) = trimmed.parse::<IpAddr>() {
            format!("https://[{}]:{}", trimmed, port)
        } else {
            format!("https://{}:{}", trimmed, port)
        }
    }

    /// Deduplicated candidate `(address, https://...)` pairs from usable device addresses.
    pub fn grpc_endpoints(&self) -> Vec<(String, String)> {
        let mut seen = std::collections::HashSet::new();
        let mut endpoints = Vec::new();
        for addr in &self.addresses {
            if !crate::discovery::is_usable_address(addr) {
                continue;
            }
            let cleaned = crate::discovery::clean_address_str(addr).to_string();
            if seen.insert(cleaned.to_ascii_lowercase()) {
                let url = Self::format_grpc_url(&cleaned, self.port);
                endpoints.push((cleaned, url));
            }
        }
        endpoints
    }

    pub fn record_metrics(&mut self, metrics: ntx_proto::SystemMetrics, capacity: usize) {
        if self.metrics_paused {
            self.latest_metrics = Some(metrics);
            return;
        }

        self.cpu_history.push_back(metrics.cpu_usage_percent);
        while self.cpu_history.len() > capacity {
            self.cpu_history.pop_front();
        }

        let ram_pct = if metrics.total_memory_bytes > 0 {
            (metrics.used_memory_bytes as f64 / metrics.total_memory_bytes as f64) * 100.0
        } else {
            0.0
        };
        self.ram_history.push_back(ram_pct);
        while self.ram_history.len() > capacity {
            self.ram_history.pop_front();
        }

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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct KnownDevice {
    pub id: String,
    pub hostname: String,
    pub addresses: Vec<String>,
    pub port: u16,
    pub version: String,
    pub psk: Option<String>,
    pub cert_fingerprint: Option<String>,
    pub is_manual: bool,
    pub auto_connect: bool,
}

/// A manually configured device (input from modal & legacy settings migration).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct ManualDevice {
    pub hostname: String,
    pub address: String,
    pub port: u16,
    pub psk: Option<String>,
}

/// Application settings persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub default_psk: String,
    pub viewer_id: String,
    pub refresh_rate_ms: u32,
    pub history_capacity: usize,
    pub dark_mode: bool,
    pub terminal_font_size: f32,
    pub hide_virtual_disks: bool,
    pub hide_virtual_interfaces: bool,
    pub device_psks: HashMap<String, String>,
    pub manual_devices: Vec<ManualDevice>,
    pub known_devices: Vec<KnownDevice>,
    pub favorite_apps: HashMap<String, Vec<FavoriteApp>>,
}

pub type Settings = AppSettings;

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            default_psk: String::new(),
            viewer_id: crate::auth::generate_viewer_id(),
            refresh_rate_ms: 1000,
            history_capacity: 60,
            dark_mode: true,
            terminal_font_size: 13.0,
            hide_virtual_disks: true,
            hide_virtual_interfaces: true,
            device_psks: HashMap::new(),
            manual_devices: Vec::new(),
            known_devices: Vec::new(),
            favorite_apps: HashMap::new(),
        }
    }
}

impl AppSettings {
    pub fn config_path_display() -> String {
        Self::get_config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "ntx-viewer-settings.toml".to_string())
    }

    fn get_config_path() -> Option<PathBuf> {
        if let Ok(appdata) = std::env::var("APPDATA") {
            if !appdata.trim().is_empty() {
                return Some(PathBuf::from(appdata).join("ntx-viewer").join("settings.toml"));
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            if !home.trim().is_empty() {
                return Some(
                    PathBuf::from(home)
                        .join(".config")
                        .join("ntx-viewer")
                        .join("settings.toml"),
                );
            }
        }
        Some(PathBuf::from("ntx-viewer-settings.toml"))
    }

    pub fn load() -> (Self, Option<String>) {
        match Self::get_config_path() {
            Some(path) => Self::load_from_path(&path),
            None => (Self::default(), None),
        }
    }

    pub fn load_from_path(path: &Path) -> (Self, Option<String>) {
        if path.exists() {
            match fs::read_to_string(path) {
                Ok(content) => match toml::from_str::<AppSettings>(&content) {
                    Ok(mut settings) => {
                        let mut migrated = false;
                        if !settings.manual_devices.is_empty() {
                            for m in settings.manual_devices.drain(..) {
                                let id = format!("{}:{}", m.address, m.port);
                                if !settings.known_devices.iter().any(|k| {
                                    k.id == id
                                        || (k.port == m.port && k.addresses.contains(&m.address))
                                }) {
                                    settings.known_devices.push(KnownDevice {
                                        id,
                                        hostname: m.hostname,
                                        addresses: vec![m.address],
                                        port: m.port,
                                        version: "manual".to_string(),
                                        psk: m.psk,
                                        cert_fingerprint: None,
                                        is_manual: true,
                                        auto_connect: false,
                                    });
                                }
                            }
                            migrated = true;
                        }
                        if migrated {
                            let _ = settings.save_to_path(path);
                        }
                        return (settings, None);
                    }
                    Err(e) => {
                        let ts = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        let backup_name = format!(
                            "{}.bak.{}",
                            path.file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("settings.toml"),
                            ts
                        );
                        let backup_path = path.with_file_name(backup_name);
                        let _ = fs::copy(path, &backup_path);
                        tracing::error!(
                            "Paramètres TOML corrompus ({}): {}. Sauvegarde vers {:?}",
                            path.display(),
                            e,
                            backup_path
                        );
                        return (
                            Self::default(),
                            Some(format!(
                                "Le fichier de paramètres était corrompu et a été sauvegardé dans {}. Des paramètres par défaut sont utilisés.",
                                backup_path.display()
                            )),
                        );
                    }
                },
                Err(e) => {
                    tracing::error!("Impossible de lire {:?}: {}", path, e);
                    return (
                        Self::default(),
                        Some(format!("Impossible de lire les paramètres: {e}")),
                    );
                }
            }
        }

        let settings = Self::default();
        let _ = settings.save_to_path(path);
        (settings, None)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(path) = Self::get_config_path() {
            self.save_to_path(&path)?;
        }
        Ok(())
    }

    pub fn save_to_path(&self, path: &Path) -> anyhow::Result<()> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;

        let content = toml::to_string_pretty(self)?;
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("settings.toml");
        let tmp_path = parent.join(format!(".{}.tmp.{}.{}", file_name, std::process::id(), ts));

        let write_res: anyhow::Result<()> = (|| {
            let mut opts = OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut file = opts.open(&tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600))?;
            }
            Ok(())
        })();

        if let Err(e) = write_res {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }

        if let Err(e) = fs::rename(&tmp_path, path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(e.into());
        }

        Ok(())
    }
}

/// Returns `true` if a disk mount / filesystem is a virtual or system-internal mount
/// typically hidden from user storage views (tmpfs, snap, loop, boot, dev/sys/proc/run).
pub fn is_virtual_disk(mount_point: &str, filesystem: &str) -> bool {
    let fs = filesystem.trim().to_ascii_lowercase();
    if matches!(
        fs.as_str(),
        "tmpfs" | "devtmpfs" | "squashfs" | "overlay" | "proc" | "sysfs" | "devpts" | "autofs"
    ) || fs.starts_with("cgroup")
        || fs.starts_with("fuse.")
    {
        return true;
    }

    let mp = mount_point.trim();
    mp == "/boot"
        || mp.starts_with("/boot/")
        || mp == "/run"
        || mp.starts_with("/run/")
        || mp == "/dev"
        || mp.starts_with("/dev/")
        || mp == "/sys"
        || mp.starts_with("/sys/")
        || mp == "/proc"
        || mp.starts_with("/proc/")
        || mp.starts_with("/snap/")
}

/// Returns `true` if a network interface is loopback or a local container/bridge virtual interface.
pub fn is_virtual_interface(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    lower == "lo"
        || lower == "lo0"
        || lower.starts_with("docker")
        || lower.starts_with("veth")
        || lower.starts_with("br-")
        || lower.starts_with("virbr")
        || lower.starts_with("cni")
        || lower.starts_with("flannel")
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn test_record_metrics() {
        let mut dev = Device::new(
            "dev1".to_string(),
            "my-pc".to_string(),
            vec!["192.168.1.10".to_string()],
            9850,
            "0.2.0".to_string(),
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
    fn test_device_address_ipv4_ipv6_and_status() {
        let dev = Device::new(
            "test_id".to_string(),
            "test_host".to_string(),
            vec!["10.0.0.5".to_string(), "192.168.1.5".to_string()],
            9850,
            "0.2.0".to_string(),
        );
        assert_eq!(dev.primary_address(), "10.0.0.5");
        assert_eq!(dev.grpc_endpoint(), "https://10.0.0.5:9850");
        assert!(!dev.status.is_connected());
        assert_eq!(dev.status.display_text(), "Découvert");

        let dev_v6 = Device::new(
            "v6_id".to_string(),
            "v6_host".to_string(),
            vec!["::1".to_string()],
            9850,
            "0.2.0".to_string(),
        );
        assert_eq!(dev_v6.grpc_endpoint(), "https://[::1]:9850");

        let dev_mixed = Device::new(
            "mix_id".to_string(),
            "mix_host".to_string(),
            vec![
                "fe80::344b:2fff:fe9a:e777".to_string(),
                "192.168.1.50".to_string(),
                "192.168.1.50".to_string(),
                "fd00::1".to_string(),
            ],
            9850,
            "0.2.0".to_string(),
        );
        assert_eq!(
            dev_mixed.grpc_endpoints(),
            vec![
                (
                    "192.168.1.50".to_string(),
                    "https://192.168.1.50:9850".to_string()
                ),
                ("fd00::1".to_string(), "https://[fd00::1]:9850".to_string()),
            ]
        );

        let connected = ConnectionStatus::Connected;
        assert!(connected.is_connected());
        assert_eq!(connected.display_text(), "Connecté");
    }

    #[test]
    fn test_settings_corrupt_backup_and_manual_migration() {
        let temp_dir =
            std::env::temp_dir().join(format!("ntx_settings_test_{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&temp_dir).unwrap();
        let cfg_path = temp_dir.join("settings.toml");

        // 1. Corrupt file should NOT be overwritten silently and must produce a .bak file + warning
        fs::write(&cfg_path, "this is not valid === toml [[[").unwrap();
        let (_loaded, warning) = AppSettings::load_from_path(&cfg_path);
        assert!(warning.is_some());
        let has_bak = fs::read_dir(&temp_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains(".bak."));
        assert!(has_bak);
        assert_eq!(
            fs::read_to_string(&cfg_path).unwrap(),
            "this is not valid === toml [[["
        );

        // 2. Legacy manual_devices migration
        let legacy_toml = r#"
            default_psk = "psk123"
            viewer_id = "v-1"
            refresh_rate_ms = 1000
            history_capacity = 60
            dark_mode = true
            [[manual_devices]]
            hostname = "legacy-srv"
            address = "192.168.1.99"
            port = 9850
            psk = "srv-psk"
            [device_psks]
        "#;
        fs::write(&cfg_path, legacy_toml).unwrap();
        let (migrated, warn2) = AppSettings::load_from_path(&cfg_path);
        assert!(warn2.is_none());
        assert!(migrated.manual_devices.is_empty());
        assert_eq!(migrated.known_devices.len(), 1);
        assert_eq!(migrated.known_devices[0].id, "192.168.1.99:9850");
        assert!(migrated.known_devices[0].is_manual);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_tree_hierarchy_cycle_safety() {
        let mut state = ProcessState::default();
        // Cycle: PID 10 -> PPID 20, PID 20 -> PPID 10, plus root PID 1 -> child PID 2
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
                pid: 2,
                name: "child".to_string(),
                cpu_usage: 2.0,
                memory_bytes: 200,
                user: "root".to_string(),
                status: "Run".to_string(),
                command_line: "child".to_string(),
                parent_pid: 1,
            },
            ntx_proto::ProcessItem {
                pid: 10,
                name: "cycle_a".to_string(),
                cpu_usage: 3.0,
                memory_bytes: 300,
                user: "user".to_string(),
                status: "Run".to_string(),
                command_line: "cycle_a".to_string(),
                parent_pid: 20,
            },
            ntx_proto::ProcessItem {
                pid: 20,
                name: "cycle_b".to_string(),
                cpu_usage: 4.0,
                memory_bytes: 400,
                user: "user".to_string(),
                status: "Run".to_string(),
                command_line: "cycle_b".to_string(),
                parent_pid: 10,
            },
        ];

        let rows = state.build_tree_rows();
        // All 4 processes must appear without stack overflow or dropping cycle members
        assert_eq!(rows.len(), 4);
        let pids: HashSet<u32> = rows.iter().map(|r| r.item.pid).collect();
        assert_eq!(pids, HashSet::from([1, 2, 10, 20]));
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

        let sorted = state.filtered_processes();
        assert_eq!(sorted.len(), 3);
        assert_eq!(sorted[0].name, "chrome");
        assert_eq!(sorted[1].name, "ntx-server");
        assert_eq!(sorted[2].name, "systemd");

        state.process_filter = "ntx".to_string();
        let filtered = state.filtered_processes();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].pid, 300);

        state.process_filter.clear();
        state.toggle_process_sort(ProcessSortColumn::Pid);
        let sorted_pid = state.filtered_processes();
        assert_eq!(sorted_pid[0].pid, 100);
        assert_eq!(sorted_pid[2].pid, 300);
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

        let rows = state.build_tree_rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].item.pid, 1);
        assert_eq!(rows[0].depth, 0);
        assert!(rows[0].has_children);
        assert_eq!(rows[0].children_count, 2);
        assert!((rows[0].total_cpu - 6.0).abs() < 0.001);
        assert_eq!(rows[0].total_memory, 600);

        assert_eq!(rows[1].item.pid, 10);
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].item.pid, 100);
        assert_eq!(rows[2].depth, 2);

        state.toggle_collapse(1);
        let rows_collapsed = state.build_tree_rows();
        assert_eq!(rows_collapsed.len(), 1);
        assert!(rows_collapsed[0].is_collapsed);

        state.expand_all();
        assert_eq!(state.build_tree_rows().len(), 3);
    }

    #[test]
    fn test_tree_fallback_name_clustering() {
        let mut state = ProcessState::default();
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
        assert_eq!(rows[0].item.pid, 11);
        assert!(rows[0].has_children);
        assert_eq!(rows[0].children_count, 1);
        assert!((rows[0].total_cpu - 15.0).abs() < 0.001);
        assert_eq!(rows[0].total_memory, 250);
    }

    #[test]
    fn test_favorite_app_matches_process_and_service() {
        let app = FavoriteApp {
            id: "fav-1".to_string(),
            name: "Nginx Server".to_string(),
            icon: "🌐".to_string(),
            app_type: FavoriteAppType::Service,
            process_filter: "nginx".to_string(),
            service_name: Some("nginx.service".to_string()),
            terminal_command: "systemctl status nginx".to_string(),
            description: "Main reverse proxy".to_string(),
            auto_start_command: Some("systemctl start nginx".to_string()),
        };

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

        let s1 = ntx_proto::ServiceItem {
            name: "nginx.service".to_string(),
            description: "Nginx web server".to_string(),
            load_state: "loaded".to_string(),
            active_state: "active".to_string(),
            sub_state: "running".to_string(),
        };
        assert!(app.matches_service(&s1));
    }

    #[test]
    fn test_settings_favorite_apps_persistence() {
        let mut settings = Settings::default();
        settings.favorite_apps.insert(
            "dev-1".to_string(),
            vec![FavoriteApp::new(
                "app-1",
                "Nginx",
                "🌐",
                FavoriteAppType::Service,
                "nginx",
                Some("nginx.service".to_string()),
                "systemctl status nginx",
                "Web server",
                Some("systemctl start nginx".to_string()),
            )],
        );

        let toml_str = toml::to_string_pretty(&settings).unwrap();
        let loaded: Settings = toml::from_str(&toml_str).unwrap();
        assert_eq!(loaded.favorite_apps.len(), 1);
        assert_eq!(loaded.favorite_apps.get("dev-1").unwrap()[0].name, "Nginx");
    }

    #[test]
    fn test_virtual_disk_and_interface_filters() {
        assert!(is_virtual_disk("/run/user/1000", "tmpfs"));
        assert!(is_virtual_disk("/boot/firmware", "vfat"));
        assert!(is_virtual_disk("/snap/core20/123", "squashfs"));
        assert!(is_virtual_disk("/var/lib/docker/overlay2/abc", "overlay"));
        assert!(!is_virtual_disk("/", "ext4"));
        assert!(!is_virtual_disk("/mnt/nas", "ext4"));
        assert!(!is_virtual_disk("C:\\", "NTFS"));

        assert!(is_virtual_interface("lo"));
        assert!(is_virtual_interface("docker0"));
        assert!(is_virtual_interface("veth8a91b2"));
        assert!(is_virtual_interface("br-123456"));
        assert!(!is_virtual_interface("eth0"));
        assert!(!is_virtual_interface("wlan0"));
        assert!(!is_virtual_interface("tailscale0"));
    }
}
