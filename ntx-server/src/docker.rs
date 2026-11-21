#![allow(clippy::result_large_err)]

use ntx_proto::docker_console_input;
use ntx_proto::{
    ConsoleChunk, DockerConsoleInput, DockerContainerItem, ExecDockerCommandResponse,
    ListDockerContainersResponse,
};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::process::{Command as StdCommand, Output, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command as TokioCommand;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, info, warn};

const DOCKER_PS_FORMAT: &str = "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.State}}\t{{.Status}}\t{{.Ports}}\t{{.CreatedAt}}\t{{.Label \"com.docker.compose.project\"}}";
const DOCKER_STATS_FORMAT: &str = "{{.ID}}\t{{.CPUPerc}}\t{{.MemUsage}}";

const DOCKER_VERSION_TIMEOUT: Duration = Duration::from_secs(8);
const DOCKER_LIST_TIMEOUT: Duration = Duration::from_secs(15);
const DOCKER_STATS_TIMEOUT: Duration = Duration::from_secs(15);
const DOCKER_CONTROL_TIMEOUT: Duration = Duration::from_secs(45);
const DOCKER_EXEC_TIMEOUT: Duration = Duration::from_secs(30);
const DOCKER_HISTORY_TIMEOUT: Duration = Duration::from_secs(10);

const CONSOLE_READ_CHUNK_SIZE: usize = 4096;
const HISTORY_CHUNK_SIZE: usize = 16 * 1024;

const VALID_CONTROL_ACTIONS: &[&str] = &[
    "start", "stop", "restart", "pause", "unpause", "kill", "remove",
];

enum PtyCommand {
    Write(Vec<u8>),
    Resize { cols: u16, rows: u16 },
}

enum BlockingDockerError {
    NotFound,
    Io(std::io::Error),
    Timeout,
    Join(String),
}

fn now_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn short_container_id(raw_id: &str) -> String {
    let trimmed = raw_id.trim();
    if trimmed.len() >= 12 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        trimmed[..12].to_string()
    } else {
        trimmed.to_string()
    }
}

fn container_state_rank(state: &str) -> u8 {
    match state.to_ascii_lowercase().as_str() {
        "running" => 0,
        "paused" | "restarting" => 1,
        _ => 2,
    }
}

fn sort_containers(containers: &mut [DockerContainerItem]) {
    containers.sort_by(|a, b| {
        container_state_rank(&a.state)
            .cmp(&container_state_rank(&b.state))
            .then_with(|| {
                a.name
                    .to_ascii_lowercase()
                    .cmp(&b.name.to_ascii_lowercase())
            })
            .then_with(|| a.id.cmp(&b.id))
    });
}

fn extract_cmd_error(output: &Output, fallback: &str) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr_trimmed = stderr.trim();
    if !stderr_trimmed.is_empty() {
        return stderr_trimmed.to_string();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout_trimmed = stdout.trim();
    if !stdout_trimmed.is_empty() {
        return stdout_trimmed.to_string();
    }
    fallback.to_string()
}

fn format_docker_daemon_error(detail: &str) -> String {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("permission denied") {
        format!(
            "Accès refusé au démon Docker (vérifiez l'appartenance au groupe 'docker') : {}",
            detail
        )
    } else if !detail.is_empty() {
        format!("Le démon Docker est inaccessible : {}", detail)
    } else {
        "Le démon Docker est arrêté ou inaccessible.".to_string()
    }
}

fn map_spawn_error(label: &str, err: std::io::Error) -> Status {
    if err.kind() == ErrorKind::NotFound {
        Status::unavailable(
            "Docker n'est pas installé sur cet hôte (binaire 'docker' introuvable).",
        )
    } else {
        Status::internal(format!("Impossible de démarrer '{}' : {}", label, err))
    }
}

async fn run_docker_blocking(
    args: Vec<String>,
    timeout_dur: Duration,
) -> Result<Output, BlockingDockerError> {
    let handle = tokio::task::spawn_blocking(move || {
        StdCommand::new("docker")
            .args(&args)
            .stdin(Stdio::null())
            .output()
    });

    match tokio::time::timeout(timeout_dur, handle).await {
        Ok(Ok(Ok(output))) => Ok(output),
        Ok(Ok(Err(err))) if err.kind() == ErrorKind::NotFound => Err(BlockingDockerError::NotFound),
        Ok(Ok(Err(err))) => Err(BlockingDockerError::Io(err)),
        Ok(Err(join_err)) => Err(BlockingDockerError::Join(join_err.to_string())),
        Err(_) => Err(BlockingDockerError::Timeout),
    }
}

/// Validate a Docker container ID or name to prevent flag injection or shell metacharacters.
pub fn validate_container_id(id: &str) -> Result<&str, String> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return Err("L'identifiant du conteneur ne peut pas être vide.".to_string());
    }
    if trimmed.chars().count() > 128 {
        return Err("L'identifiant du conteneur dépasse 128 caractères.".to_string());
    }
    if trimmed.starts_with('-') {
        return Err(format!(
            "Identifiant de conteneur invalide '{}' : ne peut pas commencer par '-'.",
            trimmed
        ));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return Err(format!(
            "Identifiant de conteneur invalide '{}' : caractères non autorisés.",
            trimmed
        ));
    }
    Ok(trimmed)
}

/// Parse Docker size strings (e.g. `"128MiB"`, `"1.5GiB"`, `"512KiB"`, `"42.3MB"`, `"0B"`, `"2.1kB"`, `"1TiB"`)
/// into bytes. Returns `0` for empty, placeholder (`"--"`), or malformed inputs.
pub fn parse_docker_size_bytes(raw: &str) -> u64 {
    let s = raw.trim();
    if s.is_empty() || s.starts_with('-') {
        return 0;
    }

    let split_idx = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    let num_part = &s[..split_idx];
    let unit_part = s[split_idx..].trim();

    if num_part.is_empty() {
        return 0;
    }

    let value: f64 = match num_part.parse::<f64>() {
        Ok(v) if v.is_finite() && v >= 0.0 => v,
        _ => return 0,
    };

    let multiplier: f64 = match unit_part.to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "kib" => 1024.0,
        "mib" => 1024.0_f64.powi(2),
        "gib" => 1024.0_f64.powi(3),
        "tib" => 1024.0_f64.powi(4),
        "pib" => 1024.0_f64.powi(5),
        "eib" => 1024.0_f64.powi(6),
        "kb" => 1_000.0,
        "mb" => 1_000.0_f64.powi(2),
        "gb" => 1_000.0_f64.powi(3),
        "tb" => 1_000.0_f64.powi(4),
        "pb" => 1_000.0_f64.powi(5),
        "eb" => 1_000.0_f64.powi(6),
        _ => return 0,
    };

    let bytes = value * multiplier;
    if !bytes.is_finite() || bytes <= 0.0 {
        0
    } else if bytes >= u64::MAX as f64 {
        u64::MAX
    } else {
        bytes.round() as u64
    }
}

/// Parse tab-separated output produced by:
/// `docker ps [-a] --no-trunc --format "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.State}}\t{{.Status}}\t{{.Ports}}\t{{.CreatedAt}}\t{{.Label \"com.docker.compose.project\"}}"`
pub fn parse_docker_ps_output(output: &str) -> Vec<DockerContainerItem> {
    let mut containers = Vec::new();

    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 4 {
            continue;
        }

        let raw_id = cols[0].trim();
        if raw_id.is_empty() {
            continue;
        }

        let id = short_container_id(raw_id);
        let name = cols[1].trim().trim_start_matches('/').to_string();
        let image = cols[2].trim().to_string();
        let state = cols[3].trim().to_ascii_lowercase();
        let status = cols.get(4).map(|s| s.trim()).unwrap_or("").to_string();
        let ports = cols.get(5).map(|s| s.trim()).unwrap_or("").to_string();
        let created = cols.get(6).map(|s| s.trim()).unwrap_or("").to_string();
        let raw_compose = cols.get(7).map(|s| s.trim()).unwrap_or("");
        let compose_project = if raw_compose == "<no value>" {
            String::new()
        } else {
            raw_compose.to_string()
        };

        containers.push(DockerContainerItem {
            id,
            name,
            image,
            state,
            status,
            ports,
            created,
            cpu_percent: 0.0,
            memory_used_bytes: 0,
            memory_limit_bytes: 0,
            compose_project,
        });
    }

    containers
}

/// Parse tab-separated output produced by:
/// `docker stats --no-stream --no-trunc --format "{{.ID}}\t{{.CPUPerc}}\t{{.MemUsage}}"`
///
/// Maps both the full container ID and its 12-character prefix to `(cpu_percent, mem_used_bytes, mem_limit_bytes)`.
pub fn parse_docker_stats_output(output: &str) -> HashMap<String, (f64, u64, u64)> {
    let mut map = HashMap::new();

    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 3 {
            continue;
        }

        let full_id = cols[0].trim();
        if full_id.is_empty() {
            continue;
        }

        let cpu_raw = cols[1].trim().trim_end_matches('%').trim();
        let cpu_percent = cpu_raw
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(0.0);

        let (mem_used_bytes, mem_limit_bytes) = match cols[2].split_once('/') {
            Some((used_raw, limit_raw)) => (
                parse_docker_size_bytes(used_raw),
                parse_docker_size_bytes(limit_raw),
            ),
            None => (0, 0),
        };

        let entry = (cpu_percent, mem_used_bytes, mem_limit_bytes);
        map.insert(full_id.to_string(), entry);

        if full_id.chars().count() > 12 {
            let short_prefix: String = full_id.chars().take(12).collect();
            map.insert(short_prefix, entry);
        }
    }

    map
}

/// List Docker containers (optionally including stopped containers) enriched with live stats
/// for running containers. Never panics if Docker is absent or stopped.
pub async fn list_containers(include_all: bool) -> ListDockerContainersResponse {
    let version_res = run_docker_blocking(
        vec![
            "version".to_string(),
            "--format".to_string(),
            "{{.Server.Version}}".to_string(),
        ],
        DOCKER_VERSION_TIMEOUT,
    )
    .await;

    let docker_version = match version_res {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if version.is_empty() && !stderr.is_empty() {
                return ListDockerContainersResponse {
                    docker_available: false,
                    docker_version: String::new(),
                    error_message: format_docker_daemon_error(&stderr),
                    containers: Vec::new(),
                };
            }
            version
        }
        Ok(output) => {
            let detail = extract_cmd_error(&output, "");
            return ListDockerContainersResponse {
                docker_available: false,
                docker_version: String::new(),
                error_message: format_docker_daemon_error(&detail),
                containers: Vec::new(),
            };
        }
        Err(BlockingDockerError::NotFound) => {
            return ListDockerContainersResponse {
                docker_available: false,
                docker_version: String::new(),
                error_message:
                    "Docker n'est pas installé sur cet hôte (binaire 'docker' introuvable)."
                        .to_string(),
                containers: Vec::new(),
            };
        }
        Err(BlockingDockerError::Io(err)) => {
            return ListDockerContainersResponse {
                docker_available: false,
                docker_version: String::new(),
                error_message: format!("Impossible d'exécuter Docker : {}", err),
                containers: Vec::new(),
            };
        }
        Err(BlockingDockerError::Timeout) => {
            return ListDockerContainersResponse {
                docker_available: false,
                docker_version: String::new(),
                error_message: "Délai dépassé lors de la vérification du démon Docker.".to_string(),
                containers: Vec::new(),
            };
        }
        Err(BlockingDockerError::Join(err)) => {
            return ListDockerContainersResponse {
                docker_available: false,
                docker_version: String::new(),
                error_message: format!(
                    "Erreur interne lors de la vérification de Docker : {}",
                    err
                ),
                containers: Vec::new(),
            };
        }
    };

    let mut ps_args = vec!["ps".to_string()];
    if include_all {
        ps_args.push("-a".to_string());
    }
    ps_args.push("--no-trunc".to_string());
    ps_args.push("--format".to_string());
    ps_args.push(DOCKER_PS_FORMAT.to_string());

    let mut containers = match run_docker_blocking(ps_args, DOCKER_LIST_TIMEOUT).await {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            parse_docker_ps_output(&stdout)
        }
        Ok(output) => {
            let detail = extract_cmd_error(&output, "Erreur Docker inconnue");
            warn!(error = %detail, "docker ps failed");
            return ListDockerContainersResponse {
                docker_available: true,
                docker_version,
                error_message: format!(
                    "Impossible de lister les conteneurs Docker : {}",
                    detail
                ),
                containers: Vec::new(),
            };
        }
        Err(BlockingDockerError::NotFound) => {
            return ListDockerContainersResponse {
                docker_available: false,
                docker_version: String::new(),
                error_message:
                    "Docker n'est pas installé sur cet hôte (binaire 'docker' introuvable)."
                        .to_string(),
                containers: Vec::new(),
            };
        }
        Err(BlockingDockerError::Io(err)) => {
            return ListDockerContainersResponse {
                docker_available: true,
                docker_version,
                error_message: format!("Impossible d'exécuter 'docker ps' : {}", err),
                containers: Vec::new(),
            };
        }
        Err(BlockingDockerError::Timeout) => {
            return ListDockerContainersResponse {
                docker_available: true,
                docker_version,
                error_message: "Délai dépassé lors du listage des conteneurs Docker.".to_string(),
                containers: Vec::new(),
            };
        }
        Err(BlockingDockerError::Join(err)) => {
            return ListDockerContainersResponse {
                docker_available: true,
                docker_version,
                error_message: format!(
                    "Erreur interne lors du listage des conteneurs Docker : {}",
                    err
                ),
                containers: Vec::new(),
            };
        }
    };

    let has_running = containers
        .iter()
        .any(|c| c.state.eq_ignore_ascii_case("running"));

    if has_running {
        let stats_args = vec![
            "stats".to_string(),
            "--no-stream".to_string(),
            "--no-trunc".to_string(),
            "--format".to_string(),
            DOCKER_STATS_FORMAT.to_string(),
        ];

        match run_docker_blocking(stats_args, DOCKER_STATS_TIMEOUT).await {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stats_map = parse_docker_stats_output(&stdout);
                for container in &mut containers {
                    let matched = stats_map.get(&container.id).copied().or_else(|| {
                        let prefix: String = container.id.chars().take(12).collect();
                        stats_map.get(&prefix).copied()
                    });
                    if let Some((cpu_percent, mem_used, mem_limit)) = matched {
                        container.cpu_percent = cpu_percent;
                        container.memory_used_bytes = mem_used;
                        container.memory_limit_bytes = mem_limit;
                    }
                }
            }
            Ok(output) => {
                let detail = extract_cmd_error(&output, "Erreur inconnue");
                warn!(error = %detail, "docker stats failed; returning containers without live metrics");
            }
            Err(BlockingDockerError::Timeout) => {
                warn!("docker stats timed out; returning containers without live metrics");
            }
            Err(_) => {
                debug!("docker stats unavailable; returning containers without live metrics");
            }
        }
    }

    sort_containers(&mut containers);

    ListDockerContainersResponse {
        docker_available: true,
        docker_version,
        error_message: String::new(),
        containers,
    }
}

/// Control a Docker container lifecycle using a strict action allowlist and no shell interpolation.
pub async fn control_container(
    container_id: &str,
    action: &str,
    force: bool,
) -> Result<(), String> {
    let clean_id = validate_container_id(container_id)?.to_string();

    let action_clean = action.trim().to_ascii_lowercase();
    if !VALID_CONTROL_ACTIONS.contains(&action_clean.as_str()) {
        return Err(format!(
            "Action Docker invalide '{}'. Actions autorisées : {}.",
            action.trim(),
            VALID_CONTROL_ACTIONS.join(", ")
        ));
    }

    let args: Vec<String> = match action_clean.as_str() {
        "remove" if force => vec!["rm".to_string(), "-f".to_string(), clean_id.clone()],
        "remove" => vec!["rm".to_string(), clean_id.clone()],
        other => vec![other.to_string(), clean_id.clone()],
    };

    let mut cmd = TokioCommand::new("docker");
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output = match tokio::time::timeout(DOCKER_CONTROL_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) if err.kind() == ErrorKind::NotFound => {
            return Err(
                "Docker n'est pas installé sur cet hôte (binaire 'docker' introuvable).".to_string(),
            );
        }
        Ok(Err(err)) => {
            return Err(format!(
                "Impossible d'exécuter la commande Docker : {}",
                err
            ));
        }
        Err(_) => {
            return Err(format!(
                "Délai dépassé lors de l'action '{}' sur le conteneur '{}'.",
                action_clean, clean_id
            ));
        }
    };

    if output.status.success() {
        info!(
            container = %clean_id,
            action = %action_clean,
            force,
            "Docker container control action succeeded"
        );
        Ok(())
    } else {
        let detail = extract_cmd_error(&output, "Erreur Docker inconnue");
        Err(format!(
            "Échec de l'action '{}' sur le conteneur '{}' : {}",
            action_clean, clean_id, detail
        ))
    }
}

/// Execute a one-shot shell command inside a running Docker container with a 30-second timeout.
pub async fn exec_command(container_id: &str, command: &str) -> ExecDockerCommandResponse {
    let clean_id = match validate_container_id(container_id) {
        Ok(id) => id.to_string(),
        Err(msg) => {
            return ExecDockerCommandResponse {
                success: false,
                exit_code: -1,
                stdout: String::new(),
                stderr: String::new(),
                error_message: msg,
            };
        }
    };

    let clean_cmd = command.trim();
    if clean_cmd.is_empty() {
        return ExecDockerCommandResponse {
            success: false,
            exit_code: -1,
            stdout: String::new(),
            stderr: String::new(),
            error_message: "La commande à exécuter ne peut pas être vide.".to_string(),
        };
    }
    if clean_cmd.contains('\0') {
        return ExecDockerCommandResponse {
            success: false,
            exit_code: -1,
            stdout: String::new(),
            stderr: String::new(),
            error_message: "La commande contient un caractère nul interdit.".to_string(),
        };
    }

    let mut cmd = TokioCommand::new("docker");
    cmd.args(["exec", &clean_id, "sh", "-c", clean_cmd])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    match tokio::time::timeout(DOCKER_EXEC_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let exit_code = output.status.code().unwrap_or(-1);
            let success = output.status.success();
            let error_message = if success {
                String::new()
            } else if !stderr.trim().is_empty() {
                stderr.trim().to_string()
            } else {
                format!(
                    "La commande s'est terminée avec le code de sortie {}.",
                    exit_code
                )
            };

            ExecDockerCommandResponse {
                success,
                exit_code,
                stdout,
                stderr,
                error_message,
            }
        }
        Ok(Err(err)) if err.kind() == ErrorKind::NotFound => ExecDockerCommandResponse {
            success: false,
            exit_code: -1,
            stdout: String::new(),
            stderr: String::new(),
            error_message:
                "Docker n'est pas installé sur cet hôte (binaire 'docker' introuvable).".to_string(),
        },
        Ok(Err(err)) => ExecDockerCommandResponse {
            success: false,
            exit_code: -1,
            stdout: String::new(),
            stderr: String::new(),
            error_message: format!("Impossible d'exécuter 'docker exec' : {}", err),
        },
        Err(_) => ExecDockerCommandResponse {
            success: false,
            exit_code: -1,
            stdout: String::new(),
            stderr: String::new(),
            error_message: "Délai d'exécution dépassé (30 s) pour la commande dans le conteneur."
                .to_string(),
        },
    }
}

async fn forward_async_pipe<R>(mut reader: R, out_tx: mpsc::Sender<Result<ConsoleChunk, Status>>)
where
    R: AsyncRead + Unpin,
{
    let mut buf = [0u8; CONSOLE_READ_CHUNK_SIZE];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let chunk = ConsoleChunk {
                    data: buf[..n].to_vec(),
                    timestamp_ms: now_timestamp_ms(),
                    is_history: false,
                };
                if out_tx.send(Ok(chunk)).await.is_err() {
                    break;
                }
            }
            Err(err) => {
                debug!("Docker console pipe reader ended: {}", err);
                break;
            }
        }
    }
}

async fn send_history_bytes(
    data: &[u8],
    out_tx: &mpsc::Sender<Result<ConsoleChunk, Status>>,
) -> bool {
    if data.is_empty() {
        return true;
    }
    let timestamp_ms = now_timestamp_ms();
    for piece in data.chunks(HISTORY_CHUNK_SIZE) {
        let chunk = ConsoleChunk {
            data: piece.to_vec(),
            timestamp_ms,
            is_history: true,
        };
        if out_tx.send(Ok(chunk)).await.is_err() {
            return false;
        }
    }
    true
}

async fn stream_logs_mode(
    cid: String,
    tail_arg: String,
    in_stream: Streaming<DockerConsoleInput>,
) -> Result<Response<ReceiverStream<Result<ConsoleChunk, Status>>>, Status> {
    let mut cmd = TokioCommand::new("docker");
    cmd.args(["logs", "-f", "--tail", &tail_arg, &cid])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| map_spawn_error("docker logs", e))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let (out_tx, out_rx) = mpsc::channel::<Result<ConsoleChunk, Status>>(128);

    let stdout_handle = {
        let tx = out_tx.clone();
        tokio::spawn(async move {
            if let Some(pipe) = stdout {
                forward_async_pipe(pipe, tx).await;
            }
        })
    };

    let stderr_handle = {
        let tx = out_tx.clone();
        tokio::spawn(async move {
            if let Some(pipe) = stderr {
                forward_async_pipe(pipe, tx).await;
            }
        })
    };

    tokio::spawn(async move {
        // Keep request stream alive for the duration of log streaming so HTTP/2 does not reset early.
        let _in_stream = in_stream;
        let mut stdout_handle = stdout_handle;
        let mut stderr_handle = stderr_handle;
        let mut stdout_done = false;
        let mut stderr_done = false;

        loop {
            if stdout_done && stderr_done {
                break;
            }
            tokio::select! {
                _ = out_tx.closed() => {
                    stdout_handle.abort();
                    stderr_handle.abort();
                    break;
                }
                _ = &mut stdout_handle, if !stdout_done => {
                    stdout_done = true;
                }
                _ = &mut stderr_handle, if !stderr_done => {
                    stderr_done = true;
                }
            }
        }

        let _ = child.kill().await;
        let _ = child.wait().await;
        drop(out_tx);
    });

    Ok(Response::new(ReceiverStream::new(out_rx)))
}

async fn stream_attach_mode(
    cid: String,
    tail_arg: String,
    mut in_stream: Streaming<DockerConsoleInput>,
) -> Result<Response<ReceiverStream<Result<ConsoleChunk, Status>>>, Status> {
    let (out_tx, out_rx) = mpsc::channel::<Result<ConsoleChunk, Status>>(128);

    tokio::spawn(async move {
        // 1. Replay recent log history as `is_history: true` chunks before attaching live I/O.
        let mut hist_cmd = TokioCommand::new("docker");
        hist_cmd
            .args(["logs", "--tail", &tail_arg, &cid])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        match tokio::time::timeout(DOCKER_HISTORY_TIMEOUT, hist_cmd.output()).await {
            Ok(Ok(output)) if output.status.success() => {
                if !send_history_bytes(&output.stdout, &out_tx).await {
                    return;
                }
                if !send_history_bytes(&output.stderr, &out_tx).await {
                    return;
                }
            }
            Ok(Ok(output)) => {
                let detail = extract_cmd_error(&output, "");
                debug!(
                    container = %cid,
                    error = %detail,
                    "docker logs history replay returned non-zero status; continuing to attach"
                );
            }
            Ok(Err(err)) if err.kind() == ErrorKind::NotFound => {
                let _ = out_tx
                    .send(Err(Status::unavailable(
                        "Docker n'est pas installé sur cet hôte (binaire 'docker' introuvable).",
                    )))
                    .await;
                return;
            }
            Ok(Err(err)) => {
                let _ = out_tx
                    .send(Err(Status::internal(format!(
                        "Impossible de lire l'historique Docker : {}",
                        err
                    ))))
                    .await;
                return;
            }
            Err(_) => {
                warn!(
                    container = %cid,
                    "docker logs history replay timed out; continuing to attach"
                );
            }
        }

        // 2. Attach live stdin/stdout/stderr with `--sig-proxy=false` so detaching never stops the container.
        let mut attach_cmd = TokioCommand::new("docker");
        attach_cmd
            .args(["attach", "--sig-proxy=false", &cid])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = match attach_cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                let _ = out_tx
                    .send(Err(map_spawn_error("docker attach", err)))
                    .await;
                return;
            }
        };

        let mut child_stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let mut stdout_handle = {
            let tx = out_tx.clone();
            tokio::spawn(async move {
                if let Some(pipe) = stdout {
                    forward_async_pipe(pipe, tx).await;
                }
            })
        };

        let mut stderr_handle = {
            let tx = out_tx.clone();
            tokio::spawn(async move {
                if let Some(pipe) = stderr {
                    forward_async_pipe(pipe, tx).await;
                }
            })
        };

        let mut stdout_done = false;
        let mut stderr_done = false;

        loop {
            if stdout_done && stderr_done {
                break;
            }
            tokio::select! {
                _ = out_tx.closed() => {
                    stdout_handle.abort();
                    stderr_handle.abort();
                    break;
                }
                _ = &mut stdout_handle, if !stdout_done => {
                    stdout_done = true;
                }
                _ = &mut stderr_handle, if !stderr_done => {
                    stderr_done = true;
                }
                msg_res = in_stream.next() => {
                    match msg_res {
                        Some(Ok(msg)) => {
                            if let Some(docker_console_input::Input::Data(bytes)) = msg.input {
                                if !bytes.is_empty() {
                                    if let Some(ref mut stdin_pipe) = child_stdin {
                                        if stdin_pipe.write_all(&bytes).await.is_err()
                                            || stdin_pipe.flush().await.is_err()
                                        {
                                            child_stdin = None;
                                        }
                                    }
                                }
                            }
                        }
                        Some(Err(_)) | None => {
                            stdout_handle.abort();
                            stderr_handle.abort();
                            break;
                        }
                    }
                }
            }
        }

        // Kill `docker attach` BEFORE dropping stdin so we detach cleanly without forwarding EOF.
        let _ = child.kill().await;
        drop(child_stdin);
        let _ = child.wait().await;
        drop(out_tx);
    });

    Ok(Response::new(ReceiverStream::new(out_rx)))
}

async fn stream_shell_mode(
    cid: String,
    mut in_stream: Streaming<DockerConsoleInput>,
) -> Result<Response<ReceiverStream<Result<ConsoleChunk, Status>>>, Status> {
    let cid_for_pty = cid.clone();
    let (master, child, mut reader, mut writer) = tokio::task::spawn_blocking(move || {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Status::internal(format!("Impossible d'ouvrir le PTY Docker : {}", e)))?;

        let mut cmd = CommandBuilder::new("docker");
        cmd.args([
            "exec",
            "-it",
            &cid_for_pty,
            "sh",
            "-c",
            "command -v bash >/dev/null 2>&1 && exec bash || exec sh",
        ]);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");

        #[cfg(unix)]
        if std::env::var_os("PATH").is_none() {
            cmd.env(
                "PATH",
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            );
        }

        let child = pair.slave.spawn_command(cmd).map_err(|e| {
            let msg = e.to_string();
            let lower = msg.to_ascii_lowercase();
            if lower.contains("no such file")
                || lower.contains("not found")
                || lower.contains("cannot find the file")
            {
                Status::unavailable(
                    "Docker n'est pas installé sur cet hôte (binaire 'docker' introuvable).",
                )
            } else {
                Status::internal(format!("Impossible de lancer le shell Docker : {}", msg))
            }
        })?;

        // Close slave in parent so EOF/signals propagate cleanly on Unix.
        drop(pair.slave);

        let reader = pair.master.try_clone_reader().map_err(|e| {
            Status::internal(format!("Impossible de cloner le lecteur PTY Docker : {}", e))
        })?;

        let writer = pair.master.take_writer().map_err(|e| {
            Status::internal(format!(
                "Impossible d'obtenir l'écrivain PTY Docker : {}",
                e
            ))
        })?;

        Ok::<_, Status>((pair.master, child, reader, writer))
    })
    .await
    .map_err(|e| {
        Status::internal(format!(
            "Échec d'initialisation de la tâche PTY Docker : {}",
            e
        ))
    })??;

    let (out_tx, out_rx) = mpsc::channel::<Result<ConsoleChunk, Status>>(64);
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<PtyCommand>(128);
    let (eof_tx, eof_rx) = oneshot::channel::<()>();

    // Blocking Task A: Read PTY stdout/stderr -> gRPC output stream
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; CONSOLE_READ_CHUNK_SIZE];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = ConsoleChunk {
                        data: buf[..n].to_vec(),
                        timestamp_ms: now_timestamp_ms(),
                        is_history: false,
                    };
                    if out_tx.blocking_send(Ok(chunk)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    debug!("Docker PTY reader ended: {}", e);
                    break;
                }
            }
        }
        let _ = eof_tx.send(());
    });

    // Blocking Task B: Write / Resize / Reap child process
    tokio::task::spawn_blocking(move || {
        let mut child = child;
        while let Some(cmd) = cmd_rx.blocking_recv() {
            match cmd {
                PtyCommand::Write(bytes) => {
                    if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                        break;
                    }
                }
                PtyCommand::Resize { cols, rows } => {
                    let _ = master.resize(PtySize {
                        rows: rows.clamp(1, 500),
                        cols: cols.clamp(1, 500),
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
            }
        }

        // Drop writer & master first so SIGHUP/EOF unblocks the reader, then kill & reap child.
        drop(writer);
        drop(master);
        let _ = child.kill();
        let _ = child.wait();
    });

    // Async Coordinator: Bridge gRPC input stream and detect shell EOF
    tokio::spawn(async move {
        let mut eof_rx = eof_rx;
        loop {
            tokio::select! {
                _ = &mut eof_rx => {
                    break;
                }
                msg_res = in_stream.next() => {
                    match msg_res {
                        Some(Ok(msg)) => {
                            if let Some(input) = msg.input {
                                let cmd = match input {
                                    docker_console_input::Input::Data(bytes) => {
                                        Some(PtyCommand::Write(bytes))
                                    }
                                    docker_console_input::Input::Resize(resize) => {
                                        Some(PtyCommand::Resize {
                                            cols: resize.cols.clamp(1, 500) as u16,
                                            rows: resize.rows.clamp(1, 500) as u16,
                                        })
                                    }
                                    docker_console_input::Input::Init(_) => None,
                                };
                                if let Some(cmd) = cmd {
                                    if cmd_tx.send(cmd).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                        Some(Err(_)) | None => break,
                    }
                }
            }
        }
        drop(cmd_tx);
    });

    Ok(Response::new(ReceiverStream::new(out_rx)))
}

/// Bidirectional Docker console stream supporting `"logs"`, `"attach"`, and `"shell"` modes.
pub async fn handle_stream_docker_console(
    request: Request<Streaming<DockerConsoleInput>>,
) -> Result<Response<ReceiverStream<Result<ConsoleChunk, Status>>>, Status> {
    let mut in_stream = request.into_inner();

    let first_msg = match in_stream.next().await {
        Some(Ok(msg)) => msg,
        Some(Err(status)) => {
            return Err(Status::invalid_argument(format!(
                "Erreur lors de la réception du message d'initialisation Docker : {}",
                status.message()
            )));
        }
        None => {
            return Err(Status::invalid_argument(
                "Le flux d'entrée s'est fermé avant l'initialisation de la console Docker.",
            ));
        }
    };

    let init = match first_msg.input {
        Some(docker_console_input::Input::Init(init)) => init,
        _ => {
            return Err(Status::invalid_argument(
                "Le premier message doit être un message d'initialisation DockerConsoleInit.",
            ));
        }
    };

    let cid = validate_container_id(&init.container_id)
        .map_err(Status::invalid_argument)?
        .to_string();

    let mode = init.mode.trim().to_ascii_lowercase();
    let tail_lines = if init.tail_lines == 0 {
        200
    } else {
        init.tail_lines.clamp(1, 2000)
    };
    let tail_arg = tail_lines.to_string();

    match mode.as_str() {
        "logs" => stream_logs_mode(cid, tail_arg, in_stream).await,
        "attach" => stream_attach_mode(cid, tail_arg, in_stream).await,
        "shell" => stream_shell_mode(cid, in_stream).await,
        _ => Err(Status::invalid_argument(format!(
            "Mode de console Docker invalide '{}'. Modes autorisés : logs, attach, shell.",
            init.mode.trim()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_container_id() {
        // Valid container IDs and names
        assert_eq!(
            validate_container_id("a1b2c3d4e5f6").unwrap(),
            "a1b2c3d4e5f6"
        );
        assert_eq!(
            validate_container_id("  minecraft_server-1.prod  ").unwrap(),
            "minecraft_server-1.prod"
        );
        let max_len = "a".repeat(128);
        assert_eq!(validate_container_id(&max_len).unwrap(), max_len);

        // Invalid: empty / whitespace
        assert!(validate_container_id("").is_err());
        assert!(validate_container_id("   \t ").is_err());

        // Invalid: flag injection (leading '-')
        assert!(validate_container_id("-f").is_err());
        assert!(validate_container_id("--privileged").is_err());
        assert!(validate_container_id("  --force").is_err());

        // Invalid: spaces & shell metacharacters
        assert!(validate_container_id("my container").is_err());
        assert!(validate_container_id("web; rm -rf /").is_err());
        assert!(validate_container_id("web$(id)").is_err());
        assert!(validate_container_id("web|cat").is_err());
        assert!(validate_container_id("../etc/passwd").is_err());
        assert!(validate_container_id("foo/bar").is_err());

        // Invalid: exceeds 128 chars
        let too_long = "a".repeat(129);
        assert!(validate_container_id(&too_long).is_err());
    }

    #[test]
    fn test_parse_docker_size_bytes() {
        assert_eq!(parse_docker_size_bytes("0B"), 0);
        assert_eq!(parse_docker_size_bytes("512KiB"), 512 * 1024);
        assert_eq!(
            parse_docker_size_bytes("128.5MiB"),
            (128.5 * 1024.0 * 1024.0) as u64
        );
        assert_eq!(
            parse_docker_size_bytes("2.5GiB"),
            (2.5 * 1024.0 * 1024.0 * 1024.0) as u64
        );
        assert_eq!(
            parse_docker_size_bytes("1TiB"),
            1024_u64 * 1024 * 1024 * 1024
        );
        assert_eq!(parse_docker_size_bytes("100MB"), 100_000_000);
        assert_eq!(parse_docker_size_bytes("42.3MB"), 42_300_000);
        assert_eq!(parse_docker_size_bytes("2.1kB"), 2_100);
        assert_eq!(parse_docker_size_bytes(" 1.5GiB "), 1_610_612_736);

        // Invalid & placeholder inputs
        assert_eq!(parse_docker_size_bytes(""), 0);
        assert_eq!(parse_docker_size_bytes("--"), 0);
        assert_eq!(parse_docker_size_bytes("N/A"), 0);
        assert_eq!(parse_docker_size_bytes("invalid"), 0);
        assert_eq!(parse_docker_size_bytes("-5MiB"), 0);
        assert_eq!(parse_docker_size_bytes("12.34XB"), 0);
        assert_eq!(parse_docker_size_bytes("1.2.3MiB"), 0);
    }

    #[test]
    fn test_parse_docker_ps_output() {
        let sample = "\n\
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\t/mc-survival\titzg/minecraft-server:latest\trunning\tUp 3 hours (healthy)\t0.0.0.0:25565->25565/tcp\t2026-01-10 12:00:00 +0100 CET\tminecraft-stack\n\
   \n\
fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210\t/redis-cache\tredis:7-alpine\texited\tExited (0) 2 days ago\t\t2026-01-08 09:00:00 +0100 CET\t\n\
aabbccddeeff\tnginx-proxy\tnginx:stable\tPaused\tUp 1 day (Paused)\t80/tcp\t2026-01-09 08:30:00 +0100 CET\n\
malformed-line-without-tabs\n";

        let items = parse_docker_ps_output(sample);
        assert_eq!(items.len(), 3);

        // Running container with compose project and leading '/' stripped + 12-char short ID
        assert_eq!(items[0].id, "0123456789ab");
        assert_eq!(items[0].name, "mc-survival");
        assert_eq!(items[0].image, "itzg/minecraft-server:latest");
        assert_eq!(items[0].state, "running");
        assert_eq!(items[0].status, "Up 3 hours (healthy)");
        assert_eq!(items[0].ports, "0.0.0.0:25565->25565/tcp");
        assert_eq!(items[0].created, "2026-01-10 12:00:00 +0100 CET");
        assert_eq!(items[0].compose_project, "minecraft-stack");

        // Exited container with empty ports and empty compose project
        assert_eq!(items[1].id, "fedcba987654");
        assert_eq!(items[1].name, "redis-cache");
        assert_eq!(items[1].image, "redis:7-alpine");
        assert_eq!(items[1].state, "exited");
        assert_eq!(items[1].status, "Exited (0) 2 days ago");
        assert_eq!(items[1].ports, "");
        assert_eq!(items[1].created, "2026-01-08 09:00:00 +0100 CET");
        assert_eq!(items[1].compose_project, "");

        // Line with missing trailing compose project column and uppercase state normalized
        assert_eq!(items[2].id, "aabbccddeeff");
        assert_eq!(items[2].name, "nginx-proxy");
        assert_eq!(items[2].state, "paused");
        assert_eq!(items[2].compose_project, "");
    }

    #[test]
    fn test_parse_docker_stats_output() {
        let full_id = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let sample = format!(
            "\n{}\t12.34%\t128MiB / 7.77GiB\n\
             fedcba9876543210\t--\t-- / --\n\
             aabbccddeeff\t205.50%\t1.5GiB / 4GiB\n\
             bad-line\n",
            full_id
        );

        let stats = parse_docker_stats_output(&sample);

        // Both full ID and 12-char prefix are mapped
        let expected_mem_used = 128 * 1024 * 1024;
        let expected_mem_limit = (7.77 * 1024.0 * 1024.0 * 1024.0_f64).round() as u64;
        let full_entry = stats.get(full_id).expect("full ID should be present");
        let short_entry = stats
            .get("0123456789ab")
            .expect("12-char prefix should be present");
        assert_eq!(full_entry, short_entry);
        assert!((full_entry.0 - 12.34).abs() < 1e-6);
        assert_eq!(full_entry.1, expected_mem_used);
        assert_eq!(full_entry.2, expected_mem_limit);

        // Placeholder `-- / --`
        let placeholder = stats.get("fedcba987654").expect("short ID present");
        assert_eq!(*placeholder, (0.0, 0, 0));

        // Multi-core >100% CPU
        let multicore = stats.get("aabbccddeeff").expect("12-char ID present");
        assert!((multicore.0 - 205.50).abs() < 1e-6);
        assert_eq!(multicore.1, (1.5 * 1024.0 * 1024.0 * 1024.0) as u64);
        assert_eq!(multicore.2, 4 * 1024 * 1024 * 1024);
    }

    #[tokio::test]
    async fn test_control_container_validation() {
        // Reject invalid container IDs before invoking docker
        assert!(control_container("", "start", false).await.is_err());
        assert!(control_container("--privileged", "start", false)
            .await
            .is_err());
        assert!(control_container("web; rm -rf /", "stop", false)
            .await
            .is_err());
        assert!(control_container(&"x".repeat(129), "restart", false)
            .await
            .is_err());

        // Reject invalid actions before invoking docker
        for bad_action in ["", "   ", "exec", "prune", "rm", "run", "inspect"] {
            let err = control_container("valid-container", bad_action, false)
                .await
                .expect_err(&format!("action '{}' must be rejected", bad_action));
            assert!(
                err.contains("invalide"),
                "expected French error message, got: {}",
                err
            );
        }
    }

    #[tokio::test]
    async fn test_exec_command_validation() {
        let bad_id = exec_command("--bad", "echo ok").await;
        assert!(!bad_id.success);
        assert_eq!(bad_id.exit_code, -1);
        assert!(!bad_id.error_message.is_empty());

        let empty_cmd = exec_command("valid-container", "   ").await;
        assert!(!empty_cmd.success);
        assert_eq!(empty_cmd.exit_code, -1);
        assert!(!empty_cmd.error_message.is_empty());
    }

    #[test]
    fn test_sort_containers_order() {
        let mut items = vec![
            DockerContainerItem {
                id: "1".to_string(),
                name: "zeta".to_string(),
                state: "exited".to_string(),
                ..Default::default()
            },
            DockerContainerItem {
                id: "2".to_string(),
                name: "Beta".to_string(),
                state: "running".to_string(),
                ..Default::default()
            },
            DockerContainerItem {
                id: "3".to_string(),
                name: "alpha".to_string(),
                state: "paused".to_string(),
                ..Default::default()
            },
            DockerContainerItem {
                id: "4".to_string(),
                name: "aardvark".to_string(),
                state: "running".to_string(),
                ..Default::default()
            },
            DockerContainerItem {
                id: "5".to_string(),
                name: "core".to_string(),
                state: "restarting".to_string(),
                ..Default::default()
            },
            DockerContainerItem {
                id: "6".to_string(),
                name: "Alpha-dead".to_string(),
                state: "dead".to_string(),
                ..Default::default()
            },
        ];

        sort_containers(&mut items);

        let names: Vec<&str> = items.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["aardvark", "Beta", "alpha", "core", "Alpha-dead", "zeta"]
        );
    }
}
