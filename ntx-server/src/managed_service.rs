#![allow(clippy::result_large_err)]

use crate::config::{config_dir_for_path, write_file_atomic};
use crate::service_manager::{control_system_service, validate_service_name};
use ntx_proto::{ConsoleChunk, ManagedServiceConfig, ManagedServiceStatus};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::future::Future;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tokio::sync::{broadcast, mpsc, watch, Mutex as AsyncMutex};
use tokio::task::{AbortHandle, JoinSet};
use tokio_stream::wrappers::ReceiverStream;
use tonic::Status;
use tracing::{debug, info, warn};

const CONSOLE_RING_BUFFER_MAX_BYTES: usize = 256 * 1024;
const CONSOLE_BROADCAST_CAPACITY: usize = 256;
const STOP_GRACE_TIMEOUT: Duration = Duration::from_secs(10);
const KILL_REAP_TIMEOUT: Duration = Duration::from_secs(5);
const AUTO_RESTART_BACKOFF: Duration = Duration::from_secs(2);
const CRASH_LOOP_WINDOW: Duration = Duration::from_secs(60);
const MAX_RAPID_CRASHES: usize = 5;
const MAX_QUICK_COMMANDS: usize = 24;
const MAX_SERVICE_NAME_CHARS: usize = 100;
const MAX_SERVICE_ID_LEN: usize = 64;
const MAX_INPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ManagedServicesFile {
    #[serde(default)]
    services: Vec<StoredServiceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct StoredServiceConfig {
    id: String,
    name: String,
    icon: String,
    description: String,
    command: String,
    working_dir: String,
    stop_command: String,
    auto_start: bool,
    auto_restart: bool,
    env_vars: Vec<String>,
    quick_commands: Vec<String>,
    systemd_unit: String,
    attach_command: String,
}

impl From<&ManagedServiceConfig> for StoredServiceConfig {
    fn from(cfg: &ManagedServiceConfig) -> Self {
        Self {
            id: cfg.id.clone(),
            name: cfg.name.clone(),
            icon: cfg.icon.clone(),
            description: cfg.description.clone(),
            command: cfg.command.clone(),
            working_dir: cfg.working_dir.clone(),
            stop_command: cfg.stop_command.clone(),
            auto_start: cfg.auto_start,
            auto_restart: cfg.auto_restart,
            env_vars: cfg.env_vars.clone(),
            quick_commands: cfg.quick_commands.clone(),
            systemd_unit: cfg.systemd_unit.clone(),
            attach_command: cfg.attach_command.clone(),
        }
    }
}

impl From<StoredServiceConfig> for ManagedServiceConfig {
    fn from(stored: StoredServiceConfig) -> Self {
        Self {
            id: stored.id,
            name: stored.name,
            icon: stored.icon,
            description: stored.description,
            command: stored.command,
            working_dir: stored.working_dir,
            stop_command: stored.stop_command,
            auto_start: stored.auto_start,
            auto_restart: stored.auto_restart,
            env_vars: stored.env_vars,
            quick_commands: stored.quick_commands,
            systemd_unit: stored.systemd_unit,
            attach_command: stored.attach_command,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceLifecycleState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Crashed,
}

impl ServiceLifecycleState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Crashed => "crashed",
        }
    }

    fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }
}

#[derive(Debug)]
struct ConsoleRingBuffer {
    chunks: VecDeque<ConsoleChunk>,
    total_bytes: usize,
    max_bytes: usize,
}

impl ConsoleRingBuffer {
    fn new(max_bytes: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            total_bytes: 0,
            max_bytes: max_bytes.max(1),
        }
    }

    fn push(&mut self, mut chunk: ConsoleChunk) {
        if chunk.data.is_empty() {
            return;
        }
        chunk.is_history = false;

        if chunk.data.len() >= self.max_bytes {
            self.chunks.clear();
            self.total_bytes = 0;
            let start = chunk.data.len() - self.max_bytes;
            chunk.data = chunk.data[start..].to_vec();
            self.total_bytes = chunk.data.len();
            self.chunks.push_back(chunk);
            return;
        }

        self.total_bytes += chunk.data.len();
        self.chunks.push_back(chunk);

        while self.total_bytes > self.max_bytes {
            if let Some(front) = self.chunks.front_mut() {
                let overflow = self.total_bytes - self.max_bytes;
                if front.data.len() <= overflow {
                    self.total_bytes -= front.data.len();
                    self.chunks.pop_front();
                } else {
                    front.data.drain(..overflow);
                    self.total_bytes -= overflow;
                }
            } else {
                self.total_bytes = 0;
                break;
            }
        }
    }

    fn snapshot_tail(&self, tail_lines: u32) -> Vec<ConsoleChunk> {
        if self.chunks.is_empty() {
            return Vec::new();
        }

        if tail_lines == 0 {
            return self
                .chunks
                .iter()
                .map(|c| ConsoleChunk {
                    data: c.data.clone(),
                    timestamp_ms: c.timestamp_ms,
                    is_history: true,
                })
                .collect();
        }

        // Locate the byte offset where the last `tail_lines` lines start across all buffered chunks.
        let total_len = self.total_bytes;
        if total_len == 0 {
            return Vec::new();
        }

        let last_byte = self
            .chunks
            .back()
            .and_then(|c| c.data.last().copied())
            .unwrap_or(0);
        // Ignore a single trailing '\n' so "a\nb\nc\n" with tail_lines=2 yields "b\nc\n".
        let scan_end = if last_byte == b'\n' {
            total_len.saturating_sub(1)
        } else {
            total_len
        };

        let mut remaining_to_skip_end = total_len - scan_end;
        let mut newlines_seen: u32 = 0;
        let mut start_offset: usize = 0;
        let mut cursor_from_end: usize = 0;

        'outer: for chunk in self.chunks.iter().rev() {
            for &b in chunk.data.iter().rev() {
                cursor_from_end += 1;
                if remaining_to_skip_end > 0 {
                    remaining_to_skip_end -= 1;
                    continue;
                }
                if b == b'\n' {
                    newlines_seen += 1;
                    if newlines_seen >= tail_lines {
                        // Start immediately after this newline byte.
                        let byte_index = total_len - cursor_from_end;
                        start_offset = byte_index + 1;
                        break 'outer;
                    }
                }
            }
        }

        let mut result = Vec::new();
        let mut consumed = 0usize;
        for chunk in &self.chunks {
            let chunk_len = chunk.data.len();
            let chunk_end = consumed + chunk_len;
            if chunk_end <= start_offset {
                consumed = chunk_end;
                continue;
            }
            let local_start = start_offset.saturating_sub(consumed);
            if local_start < chunk_len {
                result.push(ConsoleChunk {
                    data: chunk.data[local_start..].to_vec(),
                    timestamp_ms: chunk.timestamp_ms,
                    is_history: true,
                });
            }
            consumed = chunk_end;
        }

        result
    }
}

struct ActiveProcess {
    pid: Option<u32>,
    started_at: Instant,
    writer: Arc<StdMutex<Option<Box<dyn Write + Send>>>>,
    master: Arc<StdMutex<Option<Box<dyn MasterPty + Send>>>>,
    killer: Arc<StdMutex<Box<dyn ChildKiller + Send + Sync>>>,
    stop_requested: Arc<AtomicBool>,
    exit_done_rx: watch::Receiver<bool>,
}

struct ServiceRuntime {
    config: ManagedServiceConfig,
    lifecycle: ServiceLifecycleState,
    run_id: u64,
    active: Option<ActiveProcess>,
    last_exit_code: Option<i32>,
    restart_count: u32,
    last_error: String,
    recent_crashes: VecDeque<Instant>,
    auto_restart_task: Option<AbortHandle>,
    deleted: bool,
}

impl ServiceRuntime {
    fn new(config: ManagedServiceConfig) -> Self {
        Self {
            config,
            lifecycle: ServiceLifecycleState::Stopped,
            run_id: 0,
            active: None,
            last_exit_code: None,
            restart_count: 0,
            last_error: String::new(),
            recent_crashes: VecDeque::new(),
            auto_restart_task: None,
            deleted: false,
        }
    }

    fn cancel_auto_restart(&mut self) {
        if let Some(handle) = self.auto_restart_task.take() {
            handle.abort();
        }
    }
}

struct ServiceEntry {
    state: StdMutex<ServiceRuntime>,
    lifecycle_lock: AsyncMutex<()>,
    console_tx: broadcast::Sender<ConsoleChunk>,
    ring_buffer: StdMutex<ConsoleRingBuffer>,
}

impl ServiceEntry {
    fn new(config: ManagedServiceConfig) -> Self {
        let (console_tx, _) = broadcast::channel(CONSOLE_BROADCAST_CAPACITY);
        Self {
            state: StdMutex::new(ServiceRuntime::new(config)),
            lifecycle_lock: AsyncMutex::new(()),
            console_tx,
            ring_buffer: StdMutex::new(ConsoleRingBuffer::new(CONSOLE_RING_BUFFER_MAX_BYTES)),
        }
    }

    fn record_and_broadcast(&self, chunk: ConsoleChunk) {
        let mut ring = self.ring_buffer.lock().unwrap_or_else(|e| e.into_inner());
        ring.push(chunk.clone());
        let _ = self.console_tx.send(chunk);
    }
}

struct ProcessMonitor {
    system: System,
    last_refresh: Option<Instant>,
}

impl ProcessMonitor {
    fn new() -> Self {
        Self {
            system: System::new(),
            last_refresh: None,
        }
    }
}

struct Inner {
    storage_path: Option<PathBuf>,
    persist_lock: AsyncMutex<()>,
    services: StdRwLock<HashMap<String, Arc<ServiceEntry>>>,
    process_monitor: Arc<StdMutex<ProcessMonitor>>,
    auto_restart_backoff: Duration,
}

#[derive(Clone)]
pub struct ManagedServiceManager {
    inner: Arc<Inner>,
}

impl ManagedServiceManager {
    /// Returns the default `managed_services.toml` path in the same directory as `config_path`.
    pub fn default_storage_path(config_path: &Path) -> PathBuf {
        config_dir_for_path(config_path).join("managed_services.toml")
    }

    /// Loads saved managed services from `storage_path`, falling back cleanly to empty
    /// if the file does not exist or cannot be parsed.
    pub fn load_or_create(storage_path: PathBuf) -> Self {
        let mut map = HashMap::new();

        if storage_path.exists() {
            match fs::read_to_string(&storage_path) {
                Ok(content) => match toml::from_str::<ManagedServicesFile>(&content) {
                    Ok(parsed) => {
                        for stored in parsed.services {
                            let raw_cfg: ManagedServiceConfig = stored.into();
                            match validate_service_config(&raw_cfg) {
                                Ok(valid) => {
                                    map.insert(
                                        valid.id.clone(),
                                        Arc::new(ServiceEntry::new(valid)),
                                    );
                                }
                                Err(e) => {
                                    warn!(
                                        path = %storage_path.display(),
                                        service_id = %raw_cfg.id,
                                        error = %e,
                                        "Service géré ignoré lors du chargement (configuration invalide)"
                                    );
                                }
                            }
                        }
                        info!(
                            path = %storage_path.display(),
                            count = map.len(),
                            "Services gérés chargés"
                        );
                    }
                    Err(e) => {
                        warn!(
                            path = %storage_path.display(),
                            error = %e,
                            "Impossible de parser managed_services.toml, démarrage avec une liste vide"
                        );
                    }
                },
                Err(e) => {
                    warn!(
                        path = %storage_path.display(),
                        error = %e,
                        "Impossible de lire managed_services.toml, démarrage avec une liste vide"
                    );
                }
            }
        }

        Self::from_parts(Some(storage_path), map, AUTO_RESTART_BACKOFF)
    }

    /// Creates an in-memory manager with no disk persistence for unit/E2E tests.
    pub fn in_memory_for_tests() -> Self {
        Self::from_parts(None, HashMap::new(), AUTO_RESTART_BACKOFF)
    }

    fn from_parts(
        storage_path: Option<PathBuf>,
        services: HashMap<String, Arc<ServiceEntry>>,
        auto_restart_backoff: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                storage_path,
                persist_lock: AsyncMutex::new(()),
                services: StdRwLock::new(services),
                process_monitor: Arc::new(StdMutex::new(ProcessMonitor::new())),
                auto_restart_backoff,
            }),
        }
    }

    /// Lists all saved services sorted by name, enriched with live runtime state and
    /// aggregated process-tree CPU/RAM usage.
    pub async fn list_services(&self) -> Vec<ManagedServiceStatus> {
        let entries: Vec<Arc<ServiceEntry>> = {
            let guard = self.inner.services.read().unwrap_or_else(|e| e.into_inner());
            guard.values().cloned().collect()
        };

        struct Snapshot {
            config: ManagedServiceConfig,
            lifecycle: ServiceLifecycleState,
            pid: u32,
            uptime_seconds: u64,
            last_exit_code: Option<i32>,
            restart_count: u32,
            last_error: String,
        }

        let mut snapshots = Vec::with_capacity(entries.len());
        let mut active_pids = Vec::new();
        let mut has_systemd_only = false;

        for entry in entries {
            let rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
            let (pid, uptime_seconds) = if let Some(ref active) = rt.active {
                let p = active.pid.unwrap_or(0);
                if p > 0 {
                    active_pids.push(p);
                }
                (p, active.started_at.elapsed().as_secs())
            } else {
                (0, 0)
            };

            if rt.config.command.is_empty() && !rt.config.systemd_unit.is_empty() {
                has_systemd_only = true;
            }

            snapshots.push(Snapshot {
                config: rt.config.clone(),
                lifecycle: rt.lifecycle,
                pid,
                uptime_seconds,
                last_exit_code: rt.last_exit_code,
                restart_count: rt.restart_count,
                last_error: rt.last_error.clone(),
            });
        }

        // Collect live CPU/RAM metrics for active PTY subtrees via sysinfo.
        let pid_metrics: HashMap<u32, (f32, u64)> = if active_pids.is_empty() {
            HashMap::new()
        } else {
            let monitor = Arc::clone(&self.inner.process_monitor);
            tokio::task::spawn_blocking(move || collect_subtree_metrics(&monitor, &active_pids))
                .await
                .unwrap_or_default()
        };

        // On Linux, enrich systemd-only services with their current active_state.
        let systemd_states = collect_systemd_states_if_needed(has_systemd_only).await;

        let mut statuses: Vec<ManagedServiceStatus> = snapshots
            .into_iter()
            .map(|snap| {
                let (cpu_usage, memory_bytes) = if snap.pid > 0 {
                    pid_metrics.get(&snap.pid).copied().unwrap_or((0.0, 0))
                } else {
                    (0.0, 0)
                };

                let state_str = if snap.config.command.is_empty()
                    && !snap.config.systemd_unit.is_empty()
                {
                    lookup_systemd_state(&systemd_states, &snap.config.systemd_unit)
                        .unwrap_or_else(|| snap.lifecycle.as_str().to_string())
                } else {
                    snap.lifecycle.as_str().to_string()
                };

                ManagedServiceStatus {
                    config: Some(snap.config),
                    state: state_str,
                    pid: snap.pid,
                    cpu_usage,
                    memory_bytes,
                    uptime_seconds: snap.uptime_seconds,
                    last_exit_code: snap.last_exit_code.unwrap_or(0),
                    has_exit_code: snap.last_exit_code.is_some(),
                    restart_count: snap.restart_count,
                    last_error: snap.last_error,
                }
            })
            .collect();

        statuses.sort_by(|a, b| {
            let a_cfg = a.config.as_ref();
            let b_cfg = b.config.as_ref();
            let a_name = a_cfg.map(|c| c.name.to_lowercase()).unwrap_or_default();
            let b_name = b_cfg.map(|c| c.name.to_lowercase()).unwrap_or_default();
            a_name.cmp(&b_name).then_with(|| {
                let a_id = a_cfg.map(|c| c.id.as_str()).unwrap_or("");
                let b_id = b_cfg.map(|c| c.id.as_str()).unwrap_or("");
                a_id.cmp(b_id)
            })
        });

        statuses
    }

    /// Validates and creates or updates a managed service. Preserves live runtime if already
    /// running (config edits take effect on next restart), and persists atomically to disk.
    pub async fn save_service(
        &self,
        config: ManagedServiceConfig,
    ) -> Result<ManagedServiceConfig, String> {
        let validated = validate_service_config(&config)?;

        let _persist_guard = self.inner.persist_lock.lock().await;

        let previous_config: Option<ManagedServiceConfig>;
        let inserted_new: bool;
        {
            let mut services = self.inner.services.write().unwrap_or_else(|e| e.into_inner());
            if let Some(existing) = services.get(&validated.id) {
                let mut rt = existing.state.lock().unwrap_or_else(|e| e.into_inner());
                previous_config = Some(rt.config.clone());
                rt.config = validated.clone();
                if !rt.config.auto_restart {
                    rt.cancel_auto_restart();
                }
                inserted_new = false;
            } else {
                previous_config = None;
                services.insert(
                    validated.id.clone(),
                    Arc::new(ServiceEntry::new(validated.clone())),
                );
                inserted_new = true;
            }
        }

        if let Err(err) = self.persist_locked().await {
            // Roll back in-memory mutation if atomic disk write failed.
            let mut services = self.inner.services.write().unwrap_or_else(|e| e.into_inner());
            if inserted_new {
                services.remove(&validated.id);
            } else if let (Some(entry), Some(prev)) =
                (services.get(&validated.id), previous_config)
            {
                let mut rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
                rt.config = prev;
            }
            return Err(err);
        }

        info!(
            service_id = %validated.id,
            service_name = %validated.name,
            "Service géré sauvegardé"
        );
        Ok(validated)
    }

    /// Stops the service if running, removes it from the registry, and persists atomically.
    pub async fn delete_service(&self, id: &str) -> Result<(), String> {
        let trimmed_id = id.trim();
        let entry = self.get_entry(trimmed_id)?;

        // Stop any running process and prevent any pending auto-restart from firing.
        {
            let _life_guard = entry.lifecycle_lock.lock().await;
            {
                let mut rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
                rt.deleted = true;
                rt.cancel_auto_restart();
            }
            let _ = self.stop_entry_locked(&entry, false).await;
        }

        let _persist_guard = self.inner.persist_lock.lock().await;
        let removed = {
            let mut services = self.inner.services.write().unwrap_or_else(|e| e.into_inner());
            services.remove(trimmed_id)
        };

        if let Err(err) = self.persist_locked().await {
            if let Some(entry_back) = removed {
                {
                    let mut rt = entry_back.state.lock().unwrap_or_else(|e| e.into_inner());
                    rt.deleted = false;
                }
                let mut services = self.inner.services.write().unwrap_or_else(|e| e.into_inner());
                services.insert(trimmed_id.to_string(), entry_back);
            }
            return Err(err);
        }

        info!(service_id = %trimmed_id, "Service géré supprimé");
        Ok(())
    }

    /// Controls a managed service with `"start"`, `"stop"`, `"restart"`, or `"kill"`.
    pub async fn control_service(&self, id: &str, action: &str) -> Result<(), String> {
        let trimmed_id = id.trim();
        let action_clean = action.trim().to_ascii_lowercase();
        if !matches!(
            action_clean.as_str(),
            "start" | "stop" | "restart" | "kill"
        ) {
            return Err(format!(
                "Action invalide '{}'. Actions supportées : start, stop, restart, kill.",
                action
            ));
        }

        let entry = self.get_entry(trimmed_id)?;
        let _life_guard = entry.lifecycle_lock.lock().await;

        let config = {
            let rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
            if rt.deleted {
                return Err(format!("Service géré '{}' introuvable.", trimmed_id));
            }
            rt.config.clone()
        };

        // Delegate to systemd if this service has no direct command and links a systemd unit.
        if config.command.is_empty() {
            if !config.systemd_unit.is_empty() {
                if action_clean == "kill" {
                    return Err(format!(
                        "L'action 'kill' n'est pas supportée pour l'unité systemd '{}' (utilisez 'stop').",
                        config.systemd_unit
                    ));
                }
                let unit = config.systemd_unit.clone();
                let act = action_clean.clone();
                let res =
                    tokio::task::spawn_blocking(move || control_system_service(&unit, &act))
                        .await
                        .map_err(|e| {
                            format!("Erreur interne lors du contrôle systemd : {}", e)
                        })?;
                let mut rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
                match &res {
                    Ok(()) => rt.last_error.clear(),
                    Err(e) => rt.last_error = e.clone(),
                }
                return res;
            }

            return Err(format!(
                "Le service '{}' ne possède ni commande de démarrage ni unité systemd configurée.",
                config.name
            ));
        }

        match action_clean.as_str() {
            "start" => {
                // Manual start resets crash-loop counter.
                {
                    let mut rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
                    rt.recent_crashes.clear();
                    rt.cancel_auto_restart();
                }
                self.start_entry_locked(&entry).await
            }
            "stop" => self.stop_entry_locked(&entry, false).await,
            "kill" => self.stop_entry_locked(&entry, true).await,
            "restart" => {
                self.stop_entry_locked(&entry, false).await?;
                {
                    let mut rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
                    rt.recent_crashes.clear();
                    rt.cancel_auto_restart();
                }
                self.start_entry_locked(&entry).await
            }
            _ => unreachable!(),
        }
    }

    /// Writes `input` to the running service's PTY stdin and flushes immediately.
    pub async fn send_input(
        &self,
        id: &str,
        input: &str,
        append_newline: bool,
    ) -> Result<(), String> {
        let trimmed_id = id.trim();
        if input.len() > MAX_INPUT_BYTES {
            return Err(format!(
                "Entrée trop volumineuse (maximum {} octets).",
                MAX_INPUT_BYTES
            ));
        }

        let entry = self.get_entry(trimmed_id)?;
        let writer_arc = {
            let rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref active) = rt.active {
                Arc::clone(&active.writer)
            } else if rt.config.command.is_empty() && !rt.config.systemd_unit.is_empty() {
                return Err(format!(
                    "Le service '{}' est géré via systemd et ne dispose pas d'entrée interactive.",
                    rt.config.name
                ));
            } else {
                return Err(format!(
                    "Le service '{}' n'est pas en cours d'exécution.",
                    rt.config.name
                ));
            }
        };

        let payload = build_stdin_payload(input, append_newline);
        write_to_pty_writer(writer_arc, payload).await
    }

    /// Streams historical ring-buffer output (tailed to `tail_lines` if > 0) followed by
    /// live console chunks. Stays open even while the service is stopped so subsequent starts
    /// stream automatically.
    pub async fn stream_console(
        &self,
        id: &str,
        tail_lines: u32,
    ) -> Result<ReceiverStream<Result<ConsoleChunk, Status>>, Status> {
        let trimmed_id = id.trim();
        let entry = {
            let services = self.inner.services.read().unwrap_or_else(|e| e.into_inner());
            services.get(trimmed_id).cloned()
        }
        .ok_or_else(|| Status::not_found(format!("Service géré '{}' introuvable.", trimmed_id)))?;

        // Subscribe to broadcast channel AND snapshot ring buffer under the same ring_buffer lock
        // so there is zero gap and zero duplication between replay and live chunks.
        let (mut rx, history) = {
            let ring = entry.ring_buffer.lock().unwrap_or_else(|e| e.into_inner());
            let rx = entry.console_tx.subscribe();
            let history = ring.snapshot_tail(tail_lines);
            (rx, history)
        };

        let (out_tx, out_rx) = mpsc::channel::<Result<ConsoleChunk, Status>>(256);
        let service_id = trimmed_id.to_string();

        tokio::spawn(async move {
            for chunk in history {
                if out_tx.send(Ok(chunk)).await.is_err() {
                    return;
                }
            }

            loop {
                tokio::select! {
                    _ = out_tx.closed() => break,
                    recv = rx.recv() => match recv {
                        Ok(chunk) => {
                            if out_tx.send(Ok(chunk)).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            debug!(
                                service_id = %service_id,
                                skipped,
                                "Abonné console en retard, reprise sur le flux en direct"
                            );
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        });

        Ok(ReceiverStream::new(out_rx))
    }

    /// Starts all configured services with `auto_start == true` and a non-empty `command`.
    pub async fn auto_start_configured(&self) {
        let to_start: Vec<(String, String)> = {
            let services = self.inner.services.read().unwrap_or_else(|e| e.into_inner());
            services
                .iter()
                .filter_map(|(id, entry)| {
                    let rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
                    if rt.config.auto_start && !rt.config.command.is_empty() {
                        Some((id.clone(), rt.config.name.clone()))
                    } else {
                        None
                    }
                })
                .collect()
        };

        for (id, name) in to_start {
            info!(service_id = %id, service_name = %name, "Démarrage automatique du service géré");
            if let Err(e) = self.control_service(&id, "start").await {
                warn!(
                    service_id = %id,
                    service_name = %name,
                    error = %e,
                    "Échec du démarrage automatique du service géré"
                );
            }
        }
    }

    /// Cleanly stops all running managed services and cancels any pending auto-restart timers.
    pub async fn shutdown_all(&self) {
        let entries: Vec<(String, Arc<ServiceEntry>)> = {
            let services = self.inner.services.read().unwrap_or_else(|e| e.into_inner());
            services
                .iter()
                .map(|(id, entry)| (id.clone(), Arc::clone(entry)))
                .collect()
        };

        let mut join_set = JoinSet::new();
        for (id, entry) in entries {
            let manager = self.clone();
            join_set.spawn(async move {
                let _life_guard = entry.lifecycle_lock.lock().await;
                {
                    let mut rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
                    rt.cancel_auto_restart();
                }
                if let Err(e) = manager.stop_entry_locked(&entry, false).await {
                    warn!(
                        service_id = %id,
                        error = %e,
                        "Erreur lors de l'arrêt propre du service géré"
                    );
                }
            });
        }

        while join_set.join_next().await.is_some() {}
    }

    fn get_entry(&self, id: &str) -> Result<Arc<ServiceEntry>, String> {
        let services = self.inner.services.read().unwrap_or_else(|e| e.into_inner());
        services
            .get(id)
            .cloned()
            .ok_or_else(|| format!("Service géré '{}' introuvable.", id))
    }

    async fn persist_locked(&self) -> Result<(), String> {
        let Some(path) = self.inner.storage_path.clone() else {
            return Ok(());
        };

        let mut stored_list: Vec<StoredServiceConfig> = {
            let services = self.inner.services.read().unwrap_or_else(|e| e.into_inner());
            services
                .values()
                .map(|entry| {
                    let rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
                    StoredServiceConfig::from(&rt.config)
                })
                .collect()
        };

        stored_list.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });

        let file_doc = ManagedServicesFile {
            services: stored_list,
        };
        let serialized = toml::to_string_pretty(&file_doc)
            .map_err(|e| format!("Échec de sérialisation des services gérés : {}", e))?;

        tokio::task::spawn_blocking(move || {
            write_file_atomic(&path, serialized.as_bytes(), 0o600)
        })
        .await
        .map_err(|e| format!("Tâche d'écriture interrompue : {}", e))?
        .map_err(|e| format!("Échec de sauvegarde atomique des services gérés : {}", e))
    }

    fn start_entry_locked<'a>(
        &'a self,
        entry: &'a Arc<ServiceEntry>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
        let (config, my_run_id) = {
            let mut rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
            if rt.deleted {
                return Err("Ce service géré a été supprimé.".to_string());
            }
            if rt.active.is_some() || rt.lifecycle.is_active() {
                return Err(format!(
                    "Le service '{}' est déjà en cours d'exécution.",
                    rt.config.name
                ));
            }
            if rt.config.command.is_empty() {
                return Err(format!(
                    "Le service '{}' n'a aucune commande de démarrage configurée.",
                    rt.config.name
                ));
            }

            rt.lifecycle = ServiceLifecycleState::Starting;
            rt.last_error.clear();
            rt.run_id += 1;
            (rt.config.clone(), rt.run_id)
        };

        // Guard against future cancellation while spawn_blocking is in flight.
        struct StartingGuard {
            entry: Arc<ServiceEntry>,
            run_id: u64,
            disarmed: bool,
        }
        impl Drop for StartingGuard {
            fn drop(&mut self) {
                if !self.disarmed {
                    let mut rt = self.entry.state.lock().unwrap_or_else(|e| e.into_inner());
                    if rt.run_id == self.run_id
                        && rt.lifecycle == ServiceLifecycleState::Starting
                        && rt.active.is_none()
                    {
                        rt.lifecycle = ServiceLifecycleState::Stopped;
                    }
                }
            }
        }
        let mut starting_guard = StartingGuard {
            entry: Arc::clone(entry),
            run_id: my_run_id,
            disarmed: false,
        };

        // Validate working_dir existence before spawning PTY.
        if !config.working_dir.is_empty() {
            let work_dir = Path::new(&config.working_dir);
            if !work_dir.exists() {
                let msg = format!(
                    "Le répertoire de travail '{}' n'existe pas.",
                    config.working_dir
                );
                let mut rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
                if rt.run_id == my_run_id {
                    rt.lifecycle = ServiceLifecycleState::Stopped;
                    rt.last_error = msg.clone();
                }
                return Err(msg);
            }
            if !work_dir.is_dir() {
                let msg = format!(
                    "Le chemin de travail '{}' n'est pas un répertoire.",
                    config.working_dir
                );
                let mut rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
                if rt.run_id == my_run_id {
                    rt.lifecycle = ServiceLifecycleState::Stopped;
                    rt.last_error = msg.clone();
                }
                return Err(msg);
            }
        }

        let spawn_cfg = config.clone();
        let spawn_res = tokio::task::spawn_blocking(move || spawn_pty_command(&spawn_cfg))
            .await
            .map_err(|e| format!("Erreur interne lors du lancement du PTY : {}", e))?;

        let (master, child, mut reader, writer, pid, killer) = match spawn_res {
            Ok(parts) => parts,
            Err(err_msg) => {
                let mut rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
                if rt.run_id == my_run_id {
                    rt.lifecycle = ServiceLifecycleState::Stopped;
                    rt.last_error = err_msg.clone();
                }
                return Err(err_msg);
            }
        };

        let writer_arc = Arc::new(StdMutex::new(Some(writer)));
        let master_arc = Arc::new(StdMutex::new(Some(master)));
        let killer_arc = Arc::new(StdMutex::new(killer));
        let stop_requested = Arc::new(AtomicBool::new(false));
        let (exit_done_tx, exit_done_rx) = watch::channel(false);

        {
            let mut rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
            rt.lifecycle = ServiceLifecycleState::Running;
            rt.active = Some(ActiveProcess {
                pid,
                started_at: Instant::now(),
                writer: Arc::clone(&writer_arc),
                master: Arc::clone(&master_arc),
                killer: Arc::clone(&killer_arc),
                stop_requested: Arc::clone(&stop_requested),
                exit_done_rx,
            });
            starting_guard.disarmed = true;
        }

        info!(
            service_id = %config.id,
            service_name = %config.name,
            pid = ?pid,
            "Service géré démarré dans un PTY persistant"
        );

        // Blocking Task A: Read PTY stdout/stderr -> ring buffer + live broadcast.
        let reader_entry = Arc::clone(entry);
        let reader_service_id = config.id.clone();
        tokio::task::spawn_blocking(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = ConsoleChunk {
                            data: buf[..n].to_vec(),
                            timestamp_ms: now_timestamp_ms(),
                            is_history: false,
                        };
                        reader_entry.record_and_broadcast(chunk);
                    }
                    Err(e) => {
                        debug!(
                            service_id = %reader_service_id,
                            error = %e,
                            "Fin de lecture PTY du service géré"
                        );
                        break;
                    }
                }
            }
        });

        // Blocking Task B: Reap child process, clean up PTY handles, update state, and schedule auto-restart.
        let waiter_manager = self.clone();
        let waiter_entry = Arc::clone(entry);
        let waiter_service_id = config.id.clone();
        let runtime_handle = tokio::runtime::Handle::current();

        tokio::task::spawn_blocking(move || {
            let mut child = child;
            let wait_res = child.wait();

            // Drop PTY writer & master outside any state lock on this blocking thread so
            // Windows ConPTY closes cleanly and flushes remaining buffered output to the reader.
            let dropped_writer = writer_arc
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            drop(dropped_writer);
            let dropped_master = master_arc
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            drop(dropped_master);

            let (exit_code, exit_success, exit_desc) = match wait_res {
                Ok(status) => (
                    status.exit_code() as i32,
                    status.success(),
                    status.to_string(),
                ),
                Err(e) => (
                    -1,
                    false,
                    format!("Erreur d'attente du processus : {}", e),
                ),
            };

            let was_stop_requested = stop_requested.load(Ordering::SeqCst);
            let mut schedule_restart = false;

            {
                let mut rt = waiter_entry.state.lock().unwrap_or_else(|e| e.into_inner());
                if rt.run_id == my_run_id {
                    rt.active = None;
                    rt.last_exit_code = Some(exit_code);

                    if was_stop_requested || exit_success {
                        rt.lifecycle = ServiceLifecycleState::Stopped;
                        rt.last_error.clear();
                        info!(
                            service_id = %waiter_service_id,
                            exit_code,
                            user_stopped = was_stop_requested,
                            "Service géré arrêté"
                        );
                    } else {
                        rt.lifecycle = ServiceLifecycleState::Crashed;
                        rt.last_error = format!(
                            "Arrêt inattendu du processus ({}).",
                            exit_desc
                        );
                        warn!(
                            service_id = %waiter_service_id,
                            exit_code,
                            detail = %exit_desc,
                            "Service géré terminé de manière inattendue"
                        );

                        if rt.config.auto_restart && !rt.deleted {
                            let now = Instant::now();
                            while let Some(&front) = rt.recent_crashes.front() {
                                if now.duration_since(front) > CRASH_LOOP_WINDOW {
                                    rt.recent_crashes.pop_front();
                                } else {
                                    break;
                                }
                            }

                            if rt.recent_crashes.len() < MAX_RAPID_CRASHES {
                                rt.recent_crashes.push_back(now);
                                rt.restart_count = rt.restart_count.saturating_add(1);
                                schedule_restart = true;
                            } else {
                                rt.last_error = format!(
                                    "Redémarrage automatique suspendu : trop de crashs répétés ({} en {}s).",
                                    MAX_RAPID_CRASHES,
                                    CRASH_LOOP_WINDOW.as_secs()
                                );
                                warn!(
                                    service_id = %waiter_service_id,
                                    "Boucle de crash détectée, redémarrage automatique suspendu"
                                );
                            }
                        }
                    }

                    if schedule_restart {
                        let backoff = waiter_manager.inner.auto_restart_backoff;
                        let mgr = waiter_manager.clone();
                        let restart_entry = Arc::clone(&waiter_entry);
                        let sid = waiter_service_id.clone();

                        let join_handle = runtime_handle.spawn(async move {
                            info!(
                                service_id = %sid,
                                delay_ms = backoff.as_millis() as u64,
                                "Planification du redémarrage automatique du service géré"
                            );
                            tokio::time::sleep(backoff).await;

                            let _life_guard = restart_entry.lifecycle_lock.lock().await;
                            let should_start = {
                                let mut rt = restart_entry
                                    .state
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                // Clear our own AbortHandle without aborting ourselves.
                                rt.auto_restart_task = None;
                                !rt.deleted
                                    && rt.run_id == my_run_id
                                    && rt.lifecycle == ServiceLifecycleState::Crashed
                                    && rt.active.is_none()
                                    && rt.config.auto_restart
                                    && !rt.config.command.is_empty()
                            };

                            if should_start {
                                if let Err(e) = mgr.start_entry_locked(&restart_entry).await {
                                    warn!(
                                        service_id = %sid,
                                        error = %e,
                                        "Échec du redémarrage automatique du service géré"
                                    );
                                }
                            }
                        });
                        rt.auto_restart_task = Some(join_handle.abort_handle());
                    }
                }
            }

            let _ = exit_done_tx.send(true);
        });

        Ok(())
        })
    }

    async fn stop_entry_locked(
        &self,
        entry: &Arc<ServiceEntry>,
        force_kill: bool,
    ) -> Result<(), String> {
        let stop_ctx = {
            let mut rt = entry.state.lock().unwrap_or_else(|e| e.into_inner());
            rt.cancel_auto_restart();

            let active_parts = rt.active.as_ref().map(|active| {
                active.stop_requested.store(true, Ordering::SeqCst);
                (
                    active.pid,
                    Arc::clone(&active.writer),
                    Arc::clone(&active.master),
                    Arc::clone(&active.killer),
                    active.exit_done_rx.clone(),
                )
            });

            let Some((pid, writer_arc, master_arc, killer_arc, exit_done_rx)) = active_parts
            else {
                if rt.lifecycle == ServiceLifecycleState::Crashed {
                    rt.lifecycle = ServiceLifecycleState::Stopped;
                }
                return Ok(());
            };

            rt.lifecycle = ServiceLifecycleState::Stopping;

            (
                rt.config.id.clone(),
                rt.config.stop_command.clone(),
                pid,
                writer_arc,
                master_arc,
                killer_arc,
                exit_done_rx,
            )
        };

        let (service_id, stop_command, pid, writer_arc, master_arc, killer_arc, mut exit_done_rx) =
            stop_ctx;

        if !force_kill {
            if !stop_command.is_empty() {
                let payload = build_stdin_payload(&stop_command, true);
                if let Err(e) = write_to_pty_writer(Arc::clone(&writer_arc), payload).await {
                    warn!(
                        service_id = %service_id,
                        error = %e,
                        "Impossible d'envoyer la commande d'arrêt, bascule vers l'arrêt forcé"
                    );
                } else if wait_for_exit(&mut exit_done_rx, STOP_GRACE_TIMEOUT).await {
                    return Ok(());
                } else {
                    warn!(
                        service_id = %service_id,
                        timeout_secs = STOP_GRACE_TIMEOUT.as_secs(),
                        "Délai d'arrêt gracieux dépassé, arrêt forcé du processus"
                    );
                }
            } else {
                #[cfg(unix)]
                {
                    if let Some(root_pid) = pid {
                        send_unix_group_signal(root_pid, libc::SIGTERM);
                        if wait_for_exit(&mut exit_done_rx, STOP_GRACE_TIMEOUT).await {
                            return Ok(());
                        }
                        warn!(
                            service_id = %service_id,
                            "SIGTERM sans effet après le délai imparti, envoi de SIGKILL"
                        );
                    }
                }
            }
        }

        // Force kill process tree + drop PTY writer/master on a blocking thread.
        let _ = tokio::task::spawn_blocking(move || {
            force_kill_process_tree(pid, &killer_arc);
            let dropped_writer = writer_arc
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            drop(dropped_writer);
            let dropped_master = master_arc
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            drop(dropped_master);
        })
        .await;

        let _ = wait_for_exit(&mut exit_done_rx, KILL_REAP_TIMEOUT).await;
        Ok(())
    }
}

/// Validates and normalizes a `ManagedServiceConfig`.
pub fn validate_service_config(
    cfg: &ManagedServiceConfig,
) -> Result<ManagedServiceConfig, String> {
    let name = cfg.name.trim().to_string();
    if name.is_empty() {
        return Err("Le nom du service ne peut pas être vide.".to_string());
    }
    if name.chars().count() > MAX_SERVICE_NAME_CHARS {
        return Err(format!(
            "Le nom du service ne peut pas dépasser {} caractères.",
            MAX_SERVICE_NAME_CHARS
        ));
    }

    let raw_id = cfg.id.trim();
    let id = if raw_id.is_empty() {
        format!("svc_{}", uuid::Uuid::new_v4().simple())
    } else {
        if raw_id.len() > MAX_SERVICE_ID_LEN {
            return Err(format!(
                "L'identifiant du service ne peut pas dépasser {} caractères.",
                MAX_SERVICE_ID_LEN
            ));
        }
        if !raw_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(format!(
                "Identifiant de service invalide '{}' : seuls les caractères alphanumériques ASCII, '_' et '-' sont autorisés.",
                raw_id
            ));
        }
        raw_id.to_string()
    };

    let icon = cfg.icon.trim().to_string();
    let description = cfg.description.trim().to_string();
    let command = cfg.command.trim().to_string();
    let working_dir = cfg.working_dir.trim().to_string();
    let stop_command = cfg.stop_command.trim().to_string();
    let attach_command = cfg.attach_command.trim().to_string();

    let raw_unit = cfg.systemd_unit.trim();
    let systemd_unit = if raw_unit.is_empty() {
        String::new()
    } else {
        validate_service_name(raw_unit)?.to_string()
    };

    if command.is_empty() && systemd_unit.is_empty() && attach_command.is_empty() {
        return Err(
            "Au moins une commande, une unité systemd ou une commande d'attachement doit être renseignée."
                .to_string(),
        );
    }

    let mut env_vars = Vec::with_capacity(cfg.env_vars.len());
    for raw_env in &cfg.env_vars {
        let entry = raw_env.trim();
        let Some((key, val)) = entry.split_once('=') else {
            return Err(format!(
                "Variable d'environnement invalide '{}' : le format attendu est CLE=VALEUR.",
                raw_env
            ));
        };
        if !is_valid_env_key(key) {
            return Err(format!(
                "Clé de variable d'environnement invalide '{}' : doit correspondre à [A-Za-z_][A-Za-z0-9_]*.",
                key
            ));
        }
        if val.contains('\0') {
            return Err(format!(
                "La valeur de la variable d'environnement '{}' contient un caractère nul invalide.",
                key
            ));
        }
        env_vars.push(format!("{}={}", key, val));
    }

    let quick_commands: Vec<String> = cfg
        .quick_commands
        .iter()
        .map(|cmd| cmd.trim())
        .filter(|cmd| !cmd.is_empty())
        .take(MAX_QUICK_COMMANDS)
        .map(ToString::to_string)
        .collect();

    Ok(ManagedServiceConfig {
        id,
        name,
        icon,
        description,
        command,
        working_dir,
        stop_command,
        auto_start: cfg.auto_start,
        auto_restart: cfg.auto_restart,
        env_vars,
        quick_commands,
        systemd_unit,
        attach_command,
    })
}

fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn now_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn build_stdin_payload(input: &str, append_newline: bool) -> Vec<u8> {
    let mut payload = input.as_bytes().to_vec();
    if append_newline && !payload.ends_with(b"\n") && !payload.ends_with(b"\r") {
        #[cfg(windows)]
        payload.extend_from_slice(b"\r\n");
        #[cfg(not(windows))]
        payload.push(b'\n');
    }
    payload
}

async fn write_to_pty_writer(
    writer_arc: Arc<StdMutex<Option<Box<dyn Write + Send>>>>,
    payload: Vec<u8>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let mut guard = writer_arc.lock().unwrap_or_else(|e| e.into_inner());
        let Some(writer) = guard.as_mut() else {
            return Err("L'entrée standard du service n'est plus disponible.".to_string());
        };
        writer
            .write_all(&payload)
            .map_err(|e| format!("Échec d'écriture sur la console du service : {}", e))?;
        writer
            .flush()
            .map_err(|e| format!("Échec du flush sur la console du service : {}", e))?;
        Ok(())
    })
    .await
    .map_err(|e| format!("Erreur interne lors de l'écriture sur la console : {}", e))?
}

async fn wait_for_exit(exit_done_rx: &mut watch::Receiver<bool>, timeout: Duration) -> bool {
    if *exit_done_rx.borrow() {
        return true;
    }
    matches!(
        tokio::time::timeout(timeout, async {
            while !*exit_done_rx.borrow() {
                if exit_done_rx.changed().await.is_err() {
                    break;
                }
            }
        })
        .await,
        Ok(())
    )
}

type SpawnedPty = (
    Box<dyn MasterPty + Send>,
    Box<dyn portable_pty::Child + Send + Sync>,
    Box<dyn Read + Send>,
    Box<dyn Write + Send>,
    Option<u32>,
    Box<dyn ChildKiller + Send + Sync>,
);

fn spawn_pty_command(config: &ManagedServiceConfig) -> Result<SpawnedPty, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("Impossible d'ouvrir un PTY pour le service : {}", e))?;

    let mut cmd = build_service_command(config);

    if !config.working_dir.is_empty() {
        cmd.cwd(&config.working_dir);
    }

    for entry in &config.env_vars {
        if let Some((key, val)) = entry.split_once('=') {
            cmd.env(key, val);
        }
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("Impossible de lancer la commande du service : {}", e))?;

    // Close slave in parent process so EOF/signals propagate cleanly on Unix.
    drop(pair.slave);

    let pid = child.process_id();
    let killer = child.clone_killer();

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("Impossible d'ouvrir la lecture PTY : {}", e))?;

    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("Impossible d'ouvrir l'écriture PTY : {}", e))?;

    Ok((pair.master, child, reader, writer, pid, killer))
}

fn build_service_command(config: &ManagedServiceConfig) -> CommandBuilder {
    #[cfg(unix)]
    {
        let shell = if Path::new("/bin/sh").exists() {
            "/bin/sh"
        } else {
            "sh"
        };
        let mut cmd = CommandBuilder::new(shell);
        cmd.arg("-c");
        cmd.arg(&config.command);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd
    }

    #[cfg(windows)]
    {
        let mut cmd = CommandBuilder::new("cmd.exe");
        cmd.arg("/C");
        cmd.arg(&config.command);
        cmd
    }
}

#[cfg(unix)]
fn send_unix_group_signal(root_pid: u32, sig: libc::c_int) {
    if root_pid <= 1 || root_pid == std::process::id() {
        return;
    }
    // portable-pty calls setsid() in the child, so root_pid is the process group leader.
    let pgid = -(root_pid as libc::pid_t);
    unsafe {
        let _ = libc::kill(pgid, sig);
    }
}

fn force_kill_process_tree(
    pid: Option<u32>,
    killer_arc: &Arc<StdMutex<Box<dyn ChildKiller + Send + Sync>>>,
) {
    #[cfg(unix)]
    if let Some(root_pid) = pid {
        send_unix_group_signal(root_pid, libc::SIGKILL);
    }

    #[cfg(windows)]
    if let Some(root_pid) = pid {
        kill_windows_descendants(root_pid);
    }

    if let Ok(mut killer) = killer_arc.lock() {
        let _ = killer.kill();
    }
}

#[cfg(windows)]
fn kill_windows_descendants(root_pid: u32) {
    if root_pid <= 1 || root_pid == std::process::id() {
        return;
    }

    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let children_map = build_children_map(&sys);
    let root = Pid::from_u32(root_pid);

    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    let mut descendants = Vec::new();
    visited.insert(root);
    queue.push_back(root);

    while let Some(pid) = queue.pop_front() {
        if let Some(kids) = children_map.get(&pid) {
            for &kid in kids {
                if visited.insert(kid) {
                    descendants.push(kid);
                    queue.push_back(kid);
                }
            }
        }
    }

    // Kill children deepest-first so shells don't respawn or race.
    for pid in descendants.into_iter().rev() {
        if pid.as_u32() != std::process::id() {
            if let Some(proc_) = sys.process(pid) {
                let _ = proc_.kill();
            }
        }
    }
}

fn build_children_map(sys: &System) -> HashMap<Pid, Vec<Pid>> {
    let mut children_map: HashMap<Pid, Vec<Pid>> = HashMap::new();
    for (&pid, proc_) in sys.processes() {
        if let Some(parent) = proc_.parent() {
            if parent != pid {
                children_map.entry(parent).or_default().push(pid);
            }
        }
    }
    children_map
}

fn aggregate_subtree_for_pid(
    sys: &System,
    children_map: &HashMap<Pid, Vec<Pid>>,
    root_pid: u32,
) -> (f32, u64) {
    let root = Pid::from_u32(root_pid);
    if sys.process(root).is_none() {
        return (0.0, 0);
    }

    let mut total_cpu = 0.0_f32;
    let mut total_mem = 0_u64;
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    visited.insert(root);
    queue.push_back(root);

    while let Some(pid) = queue.pop_front() {
        if let Some(proc_) = sys.process(pid) {
            total_cpu += proc_.cpu_usage();
            total_mem = total_mem.saturating_add(proc_.memory());
        }
        if let Some(kids) = children_map.get(&pid) {
            for &kid in kids {
                if visited.insert(kid) {
                    queue.push_back(kid);
                }
            }
        }
    }

    (total_cpu, total_mem)
}

fn collect_subtree_metrics(
    monitor: &Arc<StdMutex<ProcessMonitor>>,
    active_pids: &[u32],
) -> HashMap<u32, (f32, u64)> {
    let needs_warmup = {
        let mut guard = monitor.lock().unwrap_or_else(|e| e.into_inner());
        match guard.last_refresh {
            None => {
                guard
                    .system
                    .refresh_processes(ProcessesToUpdate::All, true);
                true
            }
            Some(prev) => {
                if prev.elapsed() >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL {
                    guard
                        .system
                        .refresh_processes(ProcessesToUpdate::All, true);
                    guard.last_refresh = Some(Instant::now());
                }
                false
            }
        }
    };

    if needs_warmup {
        std::thread::sleep(Duration::from_millis(120));
        let mut guard = monitor.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .system
            .refresh_processes(ProcessesToUpdate::All, true);
        guard.last_refresh = Some(Instant::now());
    }

    let guard = monitor.lock().unwrap_or_else(|e| e.into_inner());
    let children_map = build_children_map(&guard.system);

    let mut out = HashMap::with_capacity(active_pids.len());
    for &root_pid in active_pids {
        out.insert(
            root_pid,
            aggregate_subtree_for_pid(&guard.system, &children_map, root_pid),
        );
    }
    out
}

async fn collect_systemd_states_if_needed(has_systemd_only: bool) -> HashMap<String, String> {
    if !has_systemd_only {
        return HashMap::new();
    }

    #[cfg(target_os = "linux")]
    {
        tokio::task::spawn_blocking(|| {
            let Ok(items) = crate::service_manager::list_system_services() else {
                return HashMap::new();
            };
            let mut map = HashMap::with_capacity(items.len());
            for item in items {
                let mapped = match item.active_state.as_str() {
                    "active" => "running",
                    "activating" => "starting",
                    "deactivating" => "stopping",
                    "failed" => "crashed",
                    _ => "stopped",
                };
                map.insert(item.name, mapped.to_string());
            }
            map
        })
        .await
        .unwrap_or_default()
    }

    #[cfg(not(target_os = "linux"))]
    {
        HashMap::new()
    }
}

fn lookup_systemd_state(
    states: &HashMap<String, String>,
    unit: &str,
) -> Option<String> {
    if let Some(s) = states.get(unit) {
        return Some(s.clone());
    }
    if !unit.contains('.') {
        let with_suffix = format!("{}.service", unit);
        if let Some(s) = states.get(&with_suffix) {
            return Some(s.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_stream::StreamExt;

    fn sample_config(id: &str, name: &str, command: &str) -> ManagedServiceConfig {
        ManagedServiceConfig {
            id: id.to_string(),
            name: name.to_string(),
            icon: "⚙".to_string(),
            description: "Service de test".to_string(),
            command: command.to_string(),
            working_dir: String::new(),
            stop_command: String::new(),
            auto_start: false,
            auto_restart: false,
            env_vars: vec![],
            quick_commands: vec![],
            systemd_unit: String::new(),
            attach_command: String::new(),
        }
    }

    #[test]
    fn test_default_storage_path() {
        let cfg = PathBuf::from("/etc/ntx-server/config.toml");
        assert_eq!(
            ManagedServiceManager::default_storage_path(&cfg),
            PathBuf::from("/etc/ntx-server/managed_services.toml")
        );

        let local = PathBuf::from("config.toml");
        assert_eq!(
            ManagedServiceManager::default_storage_path(&local),
            PathBuf::from("./managed_services.toml")
        );
    }

    #[test]
    fn test_validate_service_config() {
        // 1. Auto-generated ID when empty, trimmed fields, quick_commands capped at 24 & trimmed.
        let mut cfg = sample_config("  ", "  Minecraft Survival  ", "  java -jar paper.jar  ");
        cfg.env_vars = vec![
            "JAVA_OPTS=-Xms1G -Xmx4G".to_string(),
            "  _CUSTOM_1=ok=val ".to_string(),
        ];
        cfg.quick_commands = (0..30)
            .map(|i| {
                if i % 5 == 0 {
                    "   ".to_string()
                } else {
                    format!("  cmd-{}  ", i)
                }
            })
            .collect();

        let valid = validate_service_config(&cfg).expect("valid config should pass");
        assert!(valid.id.starts_with("svc_"));
        assert!(valid.id.len() <= MAX_SERVICE_ID_LEN);
        assert_eq!(valid.name, "Minecraft Survival");
        assert_eq!(valid.command, "java -jar paper.jar");
        assert_eq!(valid.env_vars, vec!["JAVA_OPTS=-Xms1G -Xmx4G", "_CUSTOM_1=ok=val"]);
        assert_eq!(valid.quick_commands.len(), 24);
        assert_eq!(valid.quick_commands[0], "cmd-1");

        // 2. Explicit valid ID.
        let explicit = sample_config("mc_prod-01", "Prod", "echo hi");
        let valid_explicit = validate_service_config(&explicit).expect("explicit ID valid");
        assert_eq!(valid_explicit.id, "mc_prod-01");

        // 3. Empty name rejected.
        let empty_name = sample_config("svc1", "   ", "echo hi");
        assert!(validate_service_config(&empty_name).is_err());

        // 4. Name > 100 chars rejected.
        let long_name = sample_config("svc1", &"é".repeat(101), "echo hi");
        assert!(validate_service_config(&long_name).is_err());

        // 5. Bad IDs rejected (spaces, slashes, >64 chars).
        for bad_id in ["bad id", "../evil", "svc;rm", &"a".repeat(65)] {
            let bad = sample_config(bad_id, "Valid", "echo hi");
            assert!(
                validate_service_config(&bad).is_err(),
                "ID '{}' should be rejected",
                bad_id
            );
        }

        // 6. Empty command + systemd_unit + attach_command rejected.
        let no_target = sample_config("empty_cmd", "No Target", "   ");
        assert!(validate_service_config(&no_target).is_err());

        // 7. Systemd-only or attach-only configs accepted.
        let mut systemd_only = sample_config("nginx", "Nginx", "");
        systemd_only.systemd_unit = "nginx.service".to_string();
        assert!(validate_service_config(&systemd_only).is_ok());

        let mut attach_only = sample_config("tmux_mc", "Minecraft Tmux", "");
        attach_only.attach_command = "tmux attach -t mc".to_string();
        assert!(validate_service_config(&attach_only).is_ok());

        // 8. Invalid systemd unit rejected via service_manager::validate_service_name.
        let mut bad_unit = sample_config("bad_unit", "Bad Unit", "");
        bad_unit.systemd_unit = "--evil;rm -rf /".to_string();
        assert!(validate_service_config(&bad_unit).is_err());

        // 9. Invalid env_vars rejected.
        for bad_env in [
            "NO_EQUALS",
            "=value",
            "1INVALID=value",
            "BAD-KEY=value",
            "BAD KEY=value",
            "",
        ] {
            let mut bad = sample_config("env_bad", "Bad Env", "echo hi");
            bad.env_vars = vec![bad_env.to_string()];
            assert!(
                validate_service_config(&bad).is_err(),
                "Env var '{}' should be rejected",
                bad_env
            );
        }
    }

    #[tokio::test]
    async fn test_persistence_roundtrip() {
        let temp_dir = std::env::temp_dir().join(format!(
            "ntx_test_managed_svc_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let storage_path = temp_dir.join("managed_services.toml");

        let mgr = ManagedServiceManager::load_or_create(storage_path.clone());
        assert!(mgr.list_services().await.is_empty());

        let mut cfg_b = sample_config("zeta", "Zeta Bot", "echo zeta");
        cfg_b.auto_restart = true;
        cfg_b.env_vars = vec!["TOKEN=abc123".to_string()];
        cfg_b.quick_commands = vec!["status".to_string(), "reload".to_string()];

        let cfg_a = sample_config("alpha", "Alpha Server", "echo alpha");

        mgr.save_service(cfg_b.clone()).await.expect("save zeta");
        mgr.save_service(cfg_a.clone()).await.expect("save alpha");
        assert!(storage_path.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&storage_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        // Reload from disk with a brand-new manager instance.
        let reloaded = ManagedServiceManager::load_or_create(storage_path.clone());
        let listed = reloaded.list_services().await;
        assert_eq!(listed.len(), 2);
        // Sorted by name: Alpha Server before Zeta Bot.
        assert_eq!(listed[0].config.as_ref().unwrap().id, "alpha");
        assert_eq!(listed[1].config.as_ref().unwrap().id, "zeta");
        assert_eq!(listed[1].config.as_ref().unwrap().env_vars, vec!["TOKEN=abc123"]);
        assert_eq!(
            listed[1].config.as_ref().unwrap().quick_commands,
            vec!["status", "reload"]
        );
        assert!(listed[1].config.as_ref().unwrap().auto_restart);
        assert_eq!(listed[0].state, "stopped");

        // Update existing service.
        let mut updated = cfg_a.clone();
        updated.description = "Description mise à jour".to_string();
        reloaded
            .save_service(updated)
            .await
            .expect("update alpha");

        let reloaded2 = ManagedServiceManager::load_or_create(storage_path.clone());
        let listed2 = reloaded2.list_services().await;
        assert_eq!(
            listed2[0].config.as_ref().unwrap().description,
            "Description mise à jour"
        );

        // Delete one service and reload again.
        reloaded2.delete_service("alpha").await.expect("delete alpha");
        let reloaded3 = ManagedServiceManager::load_or_create(storage_path.clone());
        let listed3 = reloaded3.list_services().await;
        assert_eq!(listed3.len(), 1);
        assert_eq!(listed3[0].config.as_ref().unwrap().id, "zeta");

        // Deleting non-existent ID returns error.
        assert!(reloaded3.delete_service("missing").await.is_err());

        // Corrupt TOML falls back cleanly to empty.
        write_file_atomic(&storage_path, b"not = [valid toml", 0o600).unwrap();
        let corrupt_mgr = ManagedServiceManager::load_or_create(storage_path.clone());
        assert!(corrupt_mgr.list_services().await.is_empty());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_ring_buffer_tail_and_cap() {
        // 1. Test multi-chunk line tailing across chunk boundaries.
        let mut ring = ConsoleRingBuffer::new(1024);
        ring.push(ConsoleChunk {
            data: b"line1\nline2\nli".to_vec(),
            timestamp_ms: 100,
            is_history: false,
        });
        ring.push(ConsoleChunk {
            data: b"ne3\nline4\n".to_vec(),
            timestamp_ms: 200,
            is_history: false,
        });

        let join_bytes = |chunks: Vec<ConsoleChunk>| -> String {
            assert!(chunks.iter().all(|c| c.is_history));
            let bytes: Vec<u8> = chunks.into_iter().flat_map(|c| c.data).collect();
            String::from_utf8(bytes).unwrap()
        };

        assert_eq!(join_bytes(ring.snapshot_tail(0)), "line1\nline2\nline3\nline4\n");
        assert_eq!(join_bytes(ring.snapshot_tail(1)), "line4\n");
        assert_eq!(join_bytes(ring.snapshot_tail(2)), "line3\nline4\n");
        assert_eq!(join_bytes(ring.snapshot_tail(3)), "line2\nline3\nline4\n");
        assert_eq!(join_bytes(ring.snapshot_tail(10)), "line1\nline2\nline3\nline4\n");

        // Trailing line without final newline.
        ring.push(ConsoleChunk {
            data: b"partial5".to_vec(),
            timestamp_ms: 300,
            is_history: false,
        });
        assert_eq!(join_bytes(ring.snapshot_tail(1)), "partial5");
        assert_eq!(join_bytes(ring.snapshot_tail(2)), "line4\npartial5");

        // 2. Test bounded byte cap eviction.
        let mut capped = ConsoleRingBuffer::new(16);
        for i in 0..10 {
            capped.push(ConsoleChunk {
                data: format!("L{:02}\n", i).into_bytes(), // 4 bytes each
                timestamp_ms: i as u64,
                is_history: false,
            });
        }
        assert!(capped.total_bytes <= 16);
        assert_eq!(join_bytes(capped.snapshot_tail(0)), "L06\nL07\nL08\nL09\n");

        // 3. Oversized single chunk trims to last max_bytes.
        let mut tiny = ConsoleRingBuffer::new(5);
        tiny.push(ConsoleChunk {
            data: b"0123456789".to_vec(),
            timestamp_ms: 42,
            is_history: false,
        });
        assert_eq!(tiny.total_bytes, 5);
        assert_eq!(join_bytes(tiny.snapshot_tail(0)), "56789");
    }

    #[tokio::test]
    async fn test_live_pty_service_start_send_input_stream_and_graceful_stop() {
        let mgr = ManagedServiceManager::in_memory_for_tests();

        let temp_dir = std::env::temp_dir().join(format!(
            "ntx_test_pty_svc_{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&temp_dir).expect("create temp dir");

        #[cfg(unix)]
        let command =
            r#"echo READY; while IFS= read -r line; do echo "ECHO:$line"; [ "$line" = "quit" ] && exit 0; done"#
                .to_string();

        #[cfg(windows)]
        let command = {
            let script_path = temp_dir.join("echo_svc.cmd");
            let script = concat!(
                "@echo off\r\n",
                "echo READY\r\n",
                ":loop\r\n",
                "set \"line=\"\r\n",
                "set /p \"line=\"\r\n",
                "echo ECHO:%line%\r\n",
                "if \"%line%\"==\"quit\" exit /b 0\r\n",
                "goto loop\r\n"
            );
            fs::write(&script_path, script).expect("write test cmd script");
            script_path.to_string_lossy().to_string()
        };

        let mut cfg = sample_config("echo-svc", "Echo Service", &command);
        cfg.working_dir = temp_dir.to_string_lossy().to_string();
        cfg.stop_command = "quit".to_string();

        mgr.save_service(cfg).await.expect("save echo-svc");

        // Subscribe to console BEFORE starting to verify streaming works while stopped and
        // seamlessly picks up live output on start.
        let mut stream = mgr
            .stream_console("echo-svc", 50)
            .await
            .expect("stream console should open while stopped");

        mgr.control_service("echo-svc", "start")
            .await
            .expect("start echo-svc");

        // Wait until we see READY in the live console stream.
        let mut collected = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !collected.contains("READY") {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "Timed out waiting for READY; collected: {:?}",
                collected
            );
            let next = tokio::time::timeout(remaining, stream.next())
                .await
                .expect("stream read timeout")
                .expect("stream unexpectedly closed")
                .expect("chunk ok");
            assert!(!next.is_history);
            collected.push_str(&String::from_utf8_lossy(&next.data));
        }

        // Verify list_services shows running status + valid PID.
        let statuses = mgr.list_services().await;
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].state, "running");
        assert!(statuses[0].pid > 0);

        // Send interactive input and confirm it is echoed back on the live stream.
        mgr.send_input("echo-svc", "bonjour_ntx", true)
            .await
            .expect("send_input should succeed");

        let echo_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !collected.contains("ECHO:bonjour_ntx") {
            let remaining = echo_deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "Timed out waiting for ECHO:bonjour_ntx; collected: {:?}",
                collected
            );
            let next = tokio::time::timeout(remaining, stream.next())
                .await
                .expect("stream echo timeout")
                .expect("stream unexpectedly closed")
                .expect("chunk ok");
            collected.push_str(&String::from_utf8_lossy(&next.data));
        }

        // Open a second console stream and confirm history replay includes READY & ECHO.
        let mut replay_stream = mgr
            .stream_console("echo-svc", 100)
            .await
            .expect("second stream should open");
        let mut replayed = String::new();
        let replay_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while !replayed.contains("ECHO:bonjour_ntx") {
            let remaining = replay_deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "Timed out waiting for replayed history; got: {:?}",
                replayed
            );
            let next = tokio::time::timeout(remaining, replay_stream.next())
                .await
                .expect("replay timeout")
                .expect("replay closed")
                .expect("replay chunk ok");
            if next.is_history {
                replayed.push_str(&String::from_utf8_lossy(&next.data));
            }
        }

        // Graceful stop using stop_command = "quit" -> exits cleanly with code 0.
        mgr.control_service("echo-svc", "stop")
            .await
            .expect("graceful stop should succeed");

        let statuses_after = mgr.list_services().await;
        assert_eq!(statuses_after[0].state, "stopped");
        assert_eq!(statuses_after[0].pid, 0);
        assert!(statuses_after[0].has_exit_code);
        assert_eq!(statuses_after[0].last_exit_code, 0);

        // Sending input while stopped returns a French error.
        let err = mgr
            .send_input("echo-svc", "test", true)
            .await
            .expect_err("send_input while stopped must fail");
        assert!(err.contains("n'est pas en cours d'exécution"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_auto_restart_on_unexpected_crash_and_shutdown_all() {
        // Use a short backoff for fast deterministic testing.
        let mgr = ManagedServiceManager::from_parts(
            None,
            HashMap::new(),
            Duration::from_millis(150),
        );

        #[cfg(unix)]
        let crash_cmd = "echo CRASH_TICK; exit 42";
        #[cfg(windows)]
        let crash_cmd = "echo CRASH_TICK & exit /b 42";

        let mut cfg = sample_config("crash-svc", "Crash Test", crash_cmd);
        cfg.auto_restart = true;

        mgr.save_service(cfg).await.expect("save crash-svc");
        mgr.control_service("crash-svc", "start")
            .await
            .expect("start crash-svc");

        // Wait until auto-restart has triggered at least once and eventually hits the rapid-crash cap.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
        let mut saw_restart = false;
        let mut saw_capped = false;
        while tokio::time::Instant::now() < deadline {
            let list = mgr.list_services().await;
            let st = &list[0];
            if st.restart_count >= 1 {
                saw_restart = true;
            }
            if st.restart_count as usize >= MAX_RAPID_CRASHES
                && st.state == "crashed"
                && st.last_error.contains("suspendu")
            {
                saw_capped = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        assert!(saw_restart, "Service should have auto-restarted after crash");
        assert!(
            saw_capped,
            "Rapid crash loop should cap after {} crashes",
            MAX_RAPID_CRASHES
        );

        mgr.shutdown_all().await;
    }

    #[tokio::test]
    async fn test_systemd_delegation_and_unknown_service_errors() {
        let mgr = ManagedServiceManager::in_memory_for_tests();

        // Unknown service -> NotFound for stream_console and Err for control/send_input.
        let stream_err = mgr.stream_console("nope", 20).await.unwrap_err();
        assert_eq!(stream_err.code(), tonic::Code::NotFound);
        assert!(mgr.control_service("nope", "start").await.is_err());
        assert!(mgr.send_input("nope", "hi", true).await.is_err());

        // Mock systemd unit delegates cleanly to service_manager::control_system_service.
        let mut sys_cfg = sample_config("mock-sys", "Mock Systemd", "");
        sys_cfg.systemd_unit = "mock-unit".to_string();
        mgr.save_service(sys_cfg).await.expect("save mock-sys");

        assert!(mgr.control_service("mock-sys", "start").await.is_ok());
        assert!(mgr.control_service("mock-sys", "restart").await.is_ok());
        assert!(mgr.control_service("mock-sys", "stop").await.is_ok());
        // kill is unsupported on systemd-only units.
        assert!(mgr.control_service("mock-sys", "kill").await.is_err());
        // send_input is unsupported on systemd-only units.
        assert!(mgr.send_input("mock-sys", "hello", true).await.is_err());
        // invalid action rejected.
        assert!(mgr.control_service("mock-sys", "explode").await.is_err());
    }

    #[tokio::test]
    async fn test_save_preserves_runtime_env_vars_kill_restart_and_delete_closes_stream() {
        let mgr = ManagedServiceManager::in_memory_for_tests();

        // Missing working_dir fails cleanly on start.
        let mut bad_dir = sample_config("bad-dir", "Bad Dir", "echo hi");
        bad_dir.working_dir = std::env::temp_dir()
            .join(format!("ntx_nonexistent_{}", uuid::Uuid::new_v4().simple()))
            .to_string_lossy()
            .to_string();
        mgr.save_service(bad_dir).await.expect("save bad-dir");
        let start_err = mgr
            .control_service("bad-dir", "start")
            .await
            .expect_err("nonexistent working_dir should fail on start");
        assert!(start_err.contains("n'existe pas"));

        // Interactive service that prints an env var and waits until killed.
        #[cfg(unix)]
        let wait_cmd = r#"echo "ENV=$NTX_TEST_TOKEN"; sleep 60"#;
        #[cfg(windows)]
        let wait_cmd = "echo ENV=%NTX_TEST_TOKEN% & ping 127.0.0.1 -n 60 > nul";

        let mut cfg = sample_config("live-svc", "Live Service", wait_cmd);
        cfg.auto_start = true;
        cfg.env_vars = vec!["NTX_TEST_TOKEN=secret42".to_string()];
        mgr.save_service(cfg.clone()).await.expect("save live-svc");

        let mut stream = mgr
            .stream_console("live-svc", 20)
            .await
            .expect("open console stream");

        // Test auto_start_configured starts auto_start == true services.
        mgr.auto_start_configured().await;

        let mut out = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !out.contains("ENV=secret42") {
            let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!rem.is_zero(), "Timed out waiting for ENV=secret42; got: {:?}", out);
            let chunk = tokio::time::timeout(rem, stream.next())
                .await
                .expect("stream timeout")
                .expect("stream open")
                .expect("chunk ok");
            out.push_str(&String::from_utf8_lossy(&chunk.data));
        }

        let initial_pid = mgr
            .list_services()
            .await
            .into_iter()
            .find(|s| s.config.as_ref().unwrap().id == "live-svc")
            .unwrap()
            .pid;
        assert!(initial_pid > 0);

        // Updating config via save_service preserves live runtime (same PID, still running).
        let mut updated = cfg.clone();
        updated.description = "Edited while running".to_string();
        mgr.save_service(updated).await.expect("update live-svc");

        let after_edit = mgr
            .list_services()
            .await
            .into_iter()
            .find(|s| s.config.as_ref().unwrap().id == "live-svc")
            .unwrap();
        assert_eq!(after_edit.state, "running");
        assert_eq!(after_edit.pid, initial_pid);
        assert_eq!(
            after_edit.config.as_ref().unwrap().description,
            "Edited while running"
        );

        // Kill immediately transitions to stopped.
        mgr.control_service("live-svc", "kill")
            .await
            .expect("kill live-svc");
        let after_kill = mgr
            .list_services()
            .await
            .into_iter()
            .find(|s| s.config.as_ref().unwrap().id == "live-svc")
            .unwrap();
        assert_eq!(after_kill.state, "stopped");
        assert_eq!(after_kill.pid, 0);

        // Restart brings it back up with a new running process.
        mgr.control_service("live-svc", "restart")
            .await
            .expect("restart live-svc");
        let after_restart = mgr
            .list_services()
            .await
            .into_iter()
            .find(|s| s.config.as_ref().unwrap().id == "live-svc")
            .unwrap();
        assert_eq!(after_restart.state, "running");
        assert!(after_restart.pid > 0);

        // Deleting a running service stops it cleanly and closes open console streams.
        mgr.delete_service("live-svc")
            .await
            .expect("delete live-svc");

        let close_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let rem = close_deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!rem.is_zero(), "Console stream should close after delete_service");
            match tokio::time::timeout(rem, stream.next()).await {
                Ok(None) => break,
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(e))) => panic!("Unexpected stream error: {}", e),
                Err(_) => panic!("Timed out waiting for console stream to close after delete"),
            }
        }
    }
}
