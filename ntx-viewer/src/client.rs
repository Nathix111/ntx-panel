use crate::auth::compute_challenge_hmac;
use crate::tls::{connect_tls_channel, TlsConnectError};
use ntx_proto::ntx_agent_client::NtxAgentClient;
use ntx_proto::*;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use tonic::transport::Channel;
use tonic::{Code, Request, Status};
use tracing::{debug, info};

const UNARY_RPC_TIMEOUT: Duration = Duration::from_secs(15);

/// Typed marker error returned when a remote `ntx-server` responds with gRPC `Code::Unimplemented`
/// (e.g. a legacy v0.2/v0.3 server that does not expose newer RPCs).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RpcUnimplementedError(pub String);

/// Returns `true` if `err` (or any error in its cause chain) represents a gRPC `Code::Unimplemented`
/// response from an unsupported legacy `ntx-server`.
pub fn is_unimplemented_error(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.downcast_ref::<RpcUnimplementedError>().is_some())
}

struct SharedInputStream<T> {
    rx: mpsc::Receiver<T>,
    recorded: VecDeque<T>,
    recording: bool,
    active_attempt: u8,
}

/// Request stream wrapper for bidirectional streaming RPCs that preserves early frames
/// (such as `DockerConsoleInit`) across a single transparent `Code::Unauthenticated` retry.
struct ReplayableInputStream<T> {
    shared: Arc<StdMutex<SharedInputStream<T>>>,
    attempt_id: u8,
}

impl<T: Clone> Stream for ReplayableInputStream<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if shared.active_attempt != self.attempt_id {
            return Poll::Ready(None);
        }
        if self.attempt_id > 1 {
            if let Some(item) = shared.recorded.pop_front() {
                return Poll::Ready(Some(item));
            }
        }
        match shared.rx.poll_recv(cx) {
            Poll::Ready(Some(item)) => {
                if shared.recording {
                    shared.recorded.push_back(item.clone());
                }
                Poll::Ready(Some(item))
            }
            other => other,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientConnectError {
    #[error("Empreinte TLS différente (attendu: {expected}, reçu: {actual})")]
    TlsMismatch { expected: String, actual: String },
    #[error("Authentification refusée: {0}")]
    AuthFailed(String),
    #[error("Trop de tentatives d'authentification: {0}")]
    RateLimited(String),
    #[error("{0}")]
    Transport(String),
}

#[derive(Clone)]
pub struct NtxClient {
    client: NtxAgentClient<Channel>,
    session: Arc<RwLock<Option<(String, Instant)>>>,
    auth_lock: Arc<Mutex<()>>,
    psk: Arc<String>,
    viewer_id: Arc<String>,
    cert_fingerprint: String,
}

impl NtxClient {
    /// Connect over TLS (with TOFU or pinned SHA-256 fingerprint) and perform challenge-response auth.
    pub async fn connect_and_authenticate(
        endpoint: &str,
        psk: &str,
        viewer_id: &str,
        expected_fingerprint: Option<String>,
    ) -> Result<Self, ClientConnectError> {
        info!("Connexion gRPC TLS vers {}", endpoint);
        let (channel, cert_fingerprint) = connect_tls_channel(endpoint, expected_fingerprint)
            .await
            .map_err(|err| match err {
                TlsConnectError::FingerprintMismatch { expected, actual } => {
                    ClientConnectError::TlsMismatch { expected, actual }
                }
                TlsConnectError::Other(msg) => ClientConnectError::Transport(msg),
            })?;

        let mut instance = Self {
            client: NtxAgentClient::new(channel),
            session: Arc::new(RwLock::new(None)),
            auth_lock: Arc::new(Mutex::new(())),
            psk: Arc::new(psk.to_string()),
            viewer_id: Arc::new(viewer_id.to_string()),
            cert_fingerprint,
        };

        instance.authenticate_internal().await?;
        Ok(instance)
    }

    pub fn cert_fingerprint(&self) -> &str {
        &self.cert_fingerprint
    }

    async fn authenticate_internal(&mut self) -> Result<String, ClientConnectError> {
        let mut chal_req = Request::new(AuthChallengeRequest {
            viewer_id: (*self.viewer_id).clone(),
        });
        chal_req.set_timeout(UNARY_RPC_TIMEOUT);

        let chal = self
            .client
            .get_auth_challenge(chal_req)
            .await
            .map_err(map_status_to_connect_err)?
            .into_inner();

        let response_hmac = compute_challenge_hmac(&self.psk, &chal.nonce, &self.viewer_id);

        let mut auth_req = Request::new(AuthRequest {
            psk_hash: String::new(),
            viewer_id: (*self.viewer_id).clone(),
            challenge_id: chal.challenge_id,
            response_hmac,
        });
        auth_req.set_timeout(UNARY_RPC_TIMEOUT);

        debug!("Envoi de l'authentification challenge-response");
        let response = self
            .client
            .authenticate(auth_req)
            .await
            .map_err(map_status_to_connect_err)?
            .into_inner();

        if !response.success {
            let msg = if response.error_message.is_empty() {
                "Clé PSK refusée par le serveur".to_string()
            } else {
                response.error_message
            };
            return Err(ClientConnectError::AuthFailed(msg));
        }

        if response.session_token.is_empty() {
            return Err(ClientConnectError::AuthFailed(
                "Le serveur n'a renvoyé aucun jeton de session".to_string(),
            ));
        }

        let ttl = response.expires_in_seconds.max(60);
        let refresh_after = Duration::from_secs(ttl.saturating_sub(30).max(15));
        let expires_at = Instant::now() + refresh_after;

        if let Ok(mut guard) = self.session.write() {
            *guard = Some((response.session_token.clone(), expires_at));
        }

        info!("Authentification challenge-response réussie");
        Ok(response.session_token)
    }

    async fn ensure_authenticated(&mut self) -> anyhow::Result<String> {
        if let Ok(guard) = self.session.read() {
            if let Some((ref token, exp)) = *guard {
                if Instant::now() < exp {
                    return Ok(token.clone());
                }
            }
        }

        let auth_lock = Arc::clone(&self.auth_lock);
        let _lock = auth_lock.lock().await;
        if let Ok(guard) = self.session.read() {
            if let Some((ref token, exp)) = *guard {
                if Instant::now() < exp {
                    return Ok(token.clone());
                }
            }
        }

        self.authenticate_internal()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    async fn force_reauthenticate(&mut self) -> anyhow::Result<String> {
        let auth_lock = Arc::clone(&self.auth_lock);
        let _lock = auth_lock.lock().await;
        if let Ok(mut guard) = self.session.write() {
            *guard = None;
        }
        self.authenticate_internal()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    fn make_unary_request<T>(&self, token: &str, message: T) -> Request<T> {
        let mut req = Request::new(message);
        if let Ok(header_val) = token.parse() {
            req.metadata_mut().insert("x-ntx-auth", header_val);
        }
        req.set_timeout(UNARY_RPC_TIMEOUT);
        req
    }

    fn make_stream_request<T>(&self, token: &str, message: T) -> Request<T> {
        let mut req = Request::new(message);
        if let Ok(header_val) = token.parse() {
            req.metadata_mut().insert("x-ntx-auth", header_val);
        }
        req
    }

    /// Ping the agent to check health and status.
    pub async fn ping(&mut self) -> anyhow::Result<PingResponse> {
        let mut req = Request::new(Empty {});
        req.set_timeout(UNARY_RPC_TIMEOUT);
        let resp = self
            .client
            .ping(req)
            .await
            .map_err(|s| anyhow::anyhow!("Échec du Ping RPC: {}", s.message()))?
            .into_inner();
        Ok(resp)
    }

    /// Query static system information (OS, CPU count, memory, disks, network interfaces).
    pub async fn get_system_info(&mut self) -> anyhow::Result<SystemInfo> {
        let token = self.ensure_authenticated().await?;
        let req = self.make_unary_request(&token, Empty {});
        match self.client.get_system_info(req).await {
            Ok(resp) => Ok(resp.into_inner()),
            Err(status) if status.code() == Code::Unauthenticated => {
                let fresh = self.force_reauthenticate().await?;
                let retry_req = self.make_unary_request(&fresh, Empty {});
                let resp = self
                    .client
                    .get_system_info(retry_req)
                    .await
                    .map_err(|s| format_rpc_error("GetSystemInfo", s))?;
                Ok(resp.into_inner())
            }
            Err(status) => Err(format_rpc_error("GetSystemInfo", status)),
        }
    }

    /// Request a live streaming metrics feed from the agent (no total stream timeout).
    pub async fn stream_metrics(
        &mut self,
        interval_ms: u32,
    ) -> anyhow::Result<tonic::Streaming<SystemMetrics>> {
        let token = self.ensure_authenticated().await?;
        let req = self.make_stream_request(&token, MetricsRequest { interval_ms });
        match self.client.stream_metrics(req).await {
            Ok(resp) => Ok(resp.into_inner()),
            Err(status) if status.code() == Code::Unauthenticated => {
                let fresh = self.force_reauthenticate().await?;
                let retry_req = self.make_stream_request(&fresh, MetricsRequest { interval_ms });
                let resp = self
                    .client
                    .stream_metrics(retry_req)
                    .await
                    .map_err(|s| format_rpc_error("StreamMetrics", s))?;
                Ok(resp.into_inner())
            }
            Err(status) => Err(format_rpc_error("StreamMetrics", status)),
        }
    }

    /// Open a bidirectional interactive terminal session (no total stream timeout).
    pub async fn open_terminal(
        &mut self,
        rx: mpsc::Receiver<TerminalInput>,
    ) -> anyhow::Result<tonic::Streaming<TerminalOutput>> {
        let token = self.ensure_authenticated().await?;
        let stream = ReceiverStream::new(rx);
        let req = self.make_stream_request(&token, stream);
        let resp = self
            .client
            .open_terminal(req)
            .await
            .map_err(|s| format_rpc_error("OpenTerminal", s))?
            .into_inner();
        Ok(resp)
    }

    /// List running processes on the remote agent.
    pub async fn list_processes(&mut self) -> anyhow::Result<Vec<ProcessItem>> {
        let token = self.ensure_authenticated().await?;
        let req = self.make_unary_request(&token, ListProcessesRequest {});
        match self.client.list_processes(req).await {
            Ok(resp) => Ok(resp.into_inner().processes),
            Err(status) if status.code() == Code::Unauthenticated => {
                let fresh = self.force_reauthenticate().await?;
                let retry_req = self.make_unary_request(&fresh, ListProcessesRequest {});
                let resp = self
                    .client
                    .list_processes(retry_req)
                    .await
                    .map_err(|s| format_rpc_error("ListProcesses", s))?;
                Ok(resp.into_inner().processes)
            }
            Err(status) => Err(format_rpc_error("ListProcesses", status)),
        }
    }

    /// Kill a process by PID with a given signal (e.g. 15 for SIGTERM, 9 for SIGKILL).
    pub async fn kill_process(
        &mut self,
        pid: u32,
        signal: i32,
    ) -> anyhow::Result<KillProcessResponse> {
        let token = self.ensure_authenticated().await?;
        let req = self.make_unary_request(&token, KillProcessRequest { pid, signal });
        match self.client.kill_process(req).await {
            Ok(resp) => Ok(resp.into_inner()),
            Err(status) if status.code() == Code::Unauthenticated => {
                let fresh = self.force_reauthenticate().await?;
                let retry_req =
                    self.make_unary_request(&fresh, KillProcessRequest { pid, signal });
                let resp = self
                    .client
                    .kill_process(retry_req)
                    .await
                    .map_err(|s| format_rpc_error("KillProcess", s))?;
                Ok(resp.into_inner())
            }
            Err(status) => Err(format_rpc_error("KillProcess", status)),
        }
    }

    /// List system services on the remote agent.
    pub async fn list_services(&mut self) -> anyhow::Result<Vec<ServiceItem>> {
        let token = self.ensure_authenticated().await?;
        let req = self.make_unary_request(&token, ListServicesRequest {});
        match self.client.list_services(req).await {
            Ok(resp) => Ok(resp.into_inner().services),
            Err(status) if status.code() == Code::Unauthenticated => {
                let fresh = self.force_reauthenticate().await?;
                let retry_req = self.make_unary_request(&fresh, ListServicesRequest {});
                let resp = self
                    .client
                    .list_services(retry_req)
                    .await
                    .map_err(|s| format_rpc_error("ListServices", s))?;
                Ok(resp.into_inner().services)
            }
            Err(status) => Err(format_rpc_error("ListServices", status)),
        }
    }

    /// Control a system service (e.g. "start", "stop", "restart").
    pub async fn control_service(
        &mut self,
        service_name: &str,
        action: &str,
    ) -> anyhow::Result<ControlServiceResponse> {
        let token = self.ensure_authenticated().await?;
        let msg = ControlServiceRequest {
            service_name: service_name.to_string(),
            action: action.to_string(),
        };
        let req = self.make_unary_request(&token, msg);
        match self.client.control_service(req).await {
            Ok(resp) => Ok(resp.into_inner()),
            Err(status) if status.code() == Code::Unauthenticated => {
                let fresh = self.force_reauthenticate().await?;
                let retry_req = self.make_unary_request(
                    &fresh,
                    ControlServiceRequest {
                        service_name: service_name.to_string(),
                        action: action.to_string(),
                    },
                );
                let resp = self
                    .client
                    .control_service(retry_req)
                    .await
                    .map_err(|s| format_rpc_error("ControlService", s))?;
                Ok(resp.into_inner())
            }
            Err(status) => Err(format_rpc_error("ControlService", status)),
        }
    }

    /// List managed services supervised by the remote agent.
    pub async fn list_managed_services(&mut self) -> anyhow::Result<Vec<ManagedServiceStatus>> {
        let token = self.ensure_authenticated().await?;
        let req = self.make_unary_request(&token, ListManagedServicesRequest {});
        match self.client.list_managed_services(req).await {
            Ok(resp) => Ok(resp.into_inner().services),
            Err(status) if status.code() == Code::Unauthenticated => {
                let fresh = self.force_reauthenticate().await?;
                let retry_req = self.make_unary_request(&fresh, ListManagedServicesRequest {});
                let resp = self
                    .client
                    .list_managed_services(retry_req)
                    .await
                    .map_err(|s| format_rpc_error("ListManagedServices", s))?;
                Ok(resp.into_inner().services)
            }
            Err(status) => Err(format_rpc_error("ListManagedServices", status)),
        }
    }

    /// Create or update a managed service configuration on the remote agent.
    pub async fn save_managed_service(
        &mut self,
        config: ManagedServiceConfig,
    ) -> anyhow::Result<SaveManagedServiceResponse> {
        let token = self.ensure_authenticated().await?;
        let req = self.make_unary_request(
            &token,
            SaveManagedServiceRequest {
                config: Some(config.clone()),
            },
        );
        match self.client.save_managed_service(req).await {
            Ok(resp) => Ok(resp.into_inner()),
            Err(status) if status.code() == Code::Unauthenticated => {
                let fresh = self.force_reauthenticate().await?;
                let retry_req = self.make_unary_request(
                    &fresh,
                    SaveManagedServiceRequest {
                        config: Some(config),
                    },
                );
                let resp = self
                    .client
                    .save_managed_service(retry_req)
                    .await
                    .map_err(|s| format_rpc_error("SaveManagedService", s))?;
                Ok(resp.into_inner())
            }
            Err(status) => Err(format_rpc_error("SaveManagedService", status)),
        }
    }

    /// Delete a managed service configuration by ID on the remote agent.
    pub async fn delete_managed_service(
        &mut self,
        id: &str,
    ) -> anyhow::Result<DeleteManagedServiceResponse> {
        let token = self.ensure_authenticated().await?;
        let req = self.make_unary_request(
            &token,
            DeleteManagedServiceRequest { id: id.to_string() },
        );
        match self.client.delete_managed_service(req).await {
            Ok(resp) => Ok(resp.into_inner()),
            Err(status) if status.code() == Code::Unauthenticated => {
                let fresh = self.force_reauthenticate().await?;
                let retry_req = self.make_unary_request(
                    &fresh,
                    DeleteManagedServiceRequest { id: id.to_string() },
                );
                let resp = self
                    .client
                    .delete_managed_service(retry_req)
                    .await
                    .map_err(|s| format_rpc_error("DeleteManagedService", s))?;
                Ok(resp.into_inner())
            }
            Err(status) => Err(format_rpc_error("DeleteManagedService", status)),
        }
    }

    /// Control a managed service lifecycle ("start", "stop", "restart", "kill").
    pub async fn control_managed_service(
        &mut self,
        id: &str,
        action: &str,
    ) -> anyhow::Result<ControlManagedServiceResponse> {
        let token = self.ensure_authenticated().await?;
        let req = self.make_unary_request(
            &token,
            ControlManagedServiceRequest {
                id: id.to_string(),
                action: action.to_string(),
            },
        );
        match self.client.control_managed_service(req).await {
            Ok(resp) => Ok(resp.into_inner()),
            Err(status) if status.code() == Code::Unauthenticated => {
                let fresh = self.force_reauthenticate().await?;
                let retry_req = self.make_unary_request(
                    &fresh,
                    ControlManagedServiceRequest {
                        id: id.to_string(),
                        action: action.to_string(),
                    },
                );
                let resp = self
                    .client
                    .control_managed_service(retry_req)
                    .await
                    .map_err(|s| format_rpc_error("ControlManagedService", s))?;
                Ok(resp.into_inner())
            }
            Err(status) => Err(format_rpc_error("ControlManagedService", status)),
        }
    }

    /// Send interactive stdin input to a running managed service PTY.
    pub async fn send_managed_service_input(
        &mut self,
        id: &str,
        input: &str,
        append_newline: bool,
    ) -> anyhow::Result<SendManagedServiceInputResponse> {
        let token = self.ensure_authenticated().await?;
        let req = self.make_unary_request(
            &token,
            SendManagedServiceInputRequest {
                id: id.to_string(),
                input: input.to_string(),
                append_newline,
            },
        );
        match self.client.send_managed_service_input(req).await {
            Ok(resp) => Ok(resp.into_inner()),
            Err(status) if status.code() == Code::Unauthenticated => {
                let fresh = self.force_reauthenticate().await?;
                let retry_req = self.make_unary_request(
                    &fresh,
                    SendManagedServiceInputRequest {
                        id: id.to_string(),
                        input: input.to_string(),
                        append_newline,
                    },
                );
                let resp = self
                    .client
                    .send_managed_service_input(retry_req)
                    .await
                    .map_err(|s| format_rpc_error("SendManagedServiceInput", s))?;
                Ok(resp.into_inner())
            }
            Err(status) => Err(format_rpc_error("SendManagedServiceInput", status)),
        }
    }

    /// Stream live PTY console output (plus ring-buffer tail history) for a managed service
    /// (no total stream timeout).
    pub async fn stream_managed_service_console(
        &mut self,
        id: &str,
        tail_lines: u32,
    ) -> anyhow::Result<tonic::Streaming<ConsoleChunk>> {
        let token = self.ensure_authenticated().await?;
        let req = self.make_stream_request(
            &token,
            StreamManagedServiceConsoleRequest {
                id: id.to_string(),
                tail_lines,
            },
        );
        match self.client.stream_managed_service_console(req).await {
            Ok(resp) => Ok(resp.into_inner()),
            Err(status) if status.code() == Code::Unauthenticated => {
                let fresh = self.force_reauthenticate().await?;
                let retry_req = self.make_stream_request(
                    &fresh,
                    StreamManagedServiceConsoleRequest {
                        id: id.to_string(),
                        tail_lines,
                    },
                );
                let resp = self
                    .client
                    .stream_managed_service_console(retry_req)
                    .await
                    .map_err(|s| format_rpc_error("StreamManagedServiceConsole", s))?;
                Ok(resp.into_inner())
            }
            Err(status) => Err(format_rpc_error("StreamManagedServiceConsole", status)),
        }
    }

    /// List Docker containers (with live CPU/RAM stats when running) on the remote host.
    pub async fn list_docker_containers(
        &mut self,
        include_all: bool,
    ) -> anyhow::Result<ListDockerContainersResponse> {
        let token = self.ensure_authenticated().await?;
        let req = self.make_unary_request(&token, ListDockerContainersRequest { include_all });
        match self.client.list_docker_containers(req).await {
            Ok(resp) => Ok(resp.into_inner()),
            Err(status) if status.code() == Code::Unauthenticated => {
                let fresh = self.force_reauthenticate().await?;
                let retry_req =
                    self.make_unary_request(&fresh, ListDockerContainersRequest { include_all });
                let resp = self
                    .client
                    .list_docker_containers(retry_req)
                    .await
                    .map_err(|s| format_rpc_error("ListDockerContainers", s))?;
                Ok(resp.into_inner())
            }
            Err(status) => Err(format_rpc_error("ListDockerContainers", status)),
        }
    }

    /// Control a Docker container ("start", "stop", "restart", "pause", "unpause", "kill", "remove").
    pub async fn control_docker_container(
        &mut self,
        container_id: &str,
        action: &str,
        force: bool,
    ) -> anyhow::Result<ControlDockerContainerResponse> {
        let token = self.ensure_authenticated().await?;
        let req = self.make_unary_request(
            &token,
            ControlDockerContainerRequest {
                container_id: container_id.to_string(),
                action: action.to_string(),
                force,
            },
        );
        match self.client.control_docker_container(req).await {
            Ok(resp) => Ok(resp.into_inner()),
            Err(status) if status.code() == Code::Unauthenticated => {
                let fresh = self.force_reauthenticate().await?;
                let retry_req = self.make_unary_request(
                    &fresh,
                    ControlDockerContainerRequest {
                        container_id: container_id.to_string(),
                        action: action.to_string(),
                        force,
                    },
                );
                let resp = self
                    .client
                    .control_docker_container(retry_req)
                    .await
                    .map_err(|s| format_rpc_error("ControlDockerContainer", s))?;
                Ok(resp.into_inner())
            }
            Err(status) => Err(format_rpc_error("ControlDockerContainer", status)),
        }
    }

    /// Execute a one-shot command inside a running Docker container.
    pub async fn exec_docker_command(
        &mut self,
        container_id: &str,
        command: &str,
    ) -> anyhow::Result<ExecDockerCommandResponse> {
        let token = self.ensure_authenticated().await?;
        let req = self.make_unary_request(
            &token,
            ExecDockerCommandRequest {
                container_id: container_id.to_string(),
                command: command.to_string(),
            },
        );
        match self.client.exec_docker_command(req).await {
            Ok(resp) => Ok(resp.into_inner()),
            Err(status) if status.code() == Code::Unauthenticated => {
                let fresh = self.force_reauthenticate().await?;
                let retry_req = self.make_unary_request(
                    &fresh,
                    ExecDockerCommandRequest {
                        container_id: container_id.to_string(),
                        command: command.to_string(),
                    },
                );
                let resp = self
                    .client
                    .exec_docker_command(retry_req)
                    .await
                    .map_err(|s| format_rpc_error("ExecDockerCommand", s))?;
                Ok(resp.into_inner())
            }
            Err(status) => Err(format_rpc_error("ExecDockerCommand", status)),
        }
    }

    /// Open a bidirectional Docker live console stream ("logs", "attach", or "shell" mode;
    /// no total stream timeout, with single re-auth retry preserving early init/input frames).
    pub async fn stream_docker_console(
        &mut self,
        input_rx: mpsc::Receiver<DockerConsoleInput>,
    ) -> anyhow::Result<tonic::Streaming<ConsoleChunk>> {
        let token = self.ensure_authenticated().await?;
        let shared = Arc::new(StdMutex::new(SharedInputStream {
            rx: input_rx,
            recorded: VecDeque::new(),
            recording: true,
            active_attempt: 1,
        }));
        let stream = ReplayableInputStream {
            shared: Arc::clone(&shared),
            attempt_id: 1,
        };
        let req = self.make_stream_request(&token, stream);
        match self.client.stream_docker_console(req).await {
            Ok(resp) => {
                let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
                guard.recording = false;
                guard.recorded.clear();
                guard.recorded.shrink_to_fit();
                Ok(resp.into_inner())
            }
            Err(status) if status.code() == Code::Unauthenticated => {
                {
                    let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
                    guard.active_attempt = 2;
                    guard.recording = false;
                }
                let fresh = self.force_reauthenticate().await?;
                let retry_stream = ReplayableInputStream {
                    shared: Arc::clone(&shared),
                    attempt_id: 2,
                };
                let retry_req = self.make_stream_request(&fresh, retry_stream);
                let resp = self
                    .client
                    .stream_docker_console(retry_req)
                    .await
                    .map_err(|s| format_rpc_error("StreamDockerConsole", s))?;
                Ok(resp.into_inner())
            }
            Err(status) => {
                let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
                guard.active_attempt = 0;
                guard.recording = false;
                guard.recorded.clear();
                Err(format_rpc_error("StreamDockerConsole", status))
            }
        }
    }
}

fn map_status_to_connect_err(status: Status) -> ClientConnectError {
    match status.code() {
        Code::ResourceExhausted => ClientConnectError::RateLimited(status.message().to_string()),
        Code::Unauthenticated | Code::PermissionDenied => {
            ClientConnectError::AuthFailed(status.message().to_string())
        }
        _ => ClientConnectError::Transport(format!(
            "Erreur RPC ({:?}): {}",
            status.code(),
            status.message()
        )),
    }
}

fn format_rpc_error(method: &str, status: Status) -> anyhow::Error {
    match status.code() {
        Code::Unimplemented => anyhow::Error::new(RpcUnimplementedError(format!(
            "La méthode {method} n'est pas supportée par cet agent distant (version incompatible)."
        ))),
        Code::Unavailable => anyhow::anyhow!("{}", status.message()),
        _ => anyhow::anyhow!("Échec {method}: {}", status.message()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ntx_proto::ntx_agent_server::{NtxAgent, NtxAgentServer};
    use rcgen::{CertificateParams, DnType, KeyPair};
    use sha2::{Digest, Sha256};
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::transport::{Identity, Server, ServerTlsConfig};
    use tonic::{Request, Response, Status};

    struct MockAgent {
        psk: String,
        expected_nonce: Vec<u8>,
        expected_challenge_id: String,
        session_token: String,
        reject_next_token_once: AtomicBool,
        legacy_unimplemented: AtomicBool,
    }

    impl MockAgent {
        #[allow(clippy::result_large_err)]
        fn check_supported(&self, rpc_name: &str) -> Result<(), Status> {
            if self.legacy_unimplemented.load(Ordering::SeqCst) {
                return Err(Status::unimplemented(format!(
                    "RPC {rpc_name} not implemented on legacy mock agent"
                )));
            }
            Ok(())
        }

        #[allow(clippy::result_large_err)]
        fn check_auth<T>(&self, req: &Request<T>) -> Result<(), Status> {
            if self.reject_next_token_once.swap(false, Ordering::SeqCst) {
                return Err(Status::unauthenticated("Simulated expired session"));
            }
            let tok = req
                .metadata()
                .get("x-ntx-auth")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| Status::unauthenticated("Missing token"))?;
            if tok == self.session_token {
                Ok(())
            } else {
                Err(Status::unauthenticated("Bad token"))
            }
        }
    }

    #[tonic::async_trait]
    impl NtxAgent for MockAgent {
        async fn get_auth_challenge(
            &self,
            _request: Request<AuthChallengeRequest>,
        ) -> Result<Response<AuthChallengeResponse>, Status> {
            Ok(Response::new(AuthChallengeResponse {
                challenge_id: self.expected_challenge_id.clone(),
                nonce: self.expected_nonce.clone(),
                expires_in_seconds: 30,
            }))
        }

        async fn ping(&self, _request: Request<Empty>) -> Result<Response<PingResponse>, Status> {
            Ok(Response::new(PingResponse {
                hostname: "mock-host".to_string(),
                uptime_seconds: 42,
                version: "0.2.0".to_string(),
            }))
        }

        async fn authenticate(
            &self,
            request: Request<AuthRequest>,
        ) -> Result<Response<AuthResponse>, Status> {
            let req = request.into_inner();
            let expected_hmac =
                crate::auth::compute_challenge_hmac(&self.psk, &self.expected_nonce, &req.viewer_id);
            if req.challenge_id == self.expected_challenge_id && req.response_hmac == expected_hmac
            {
                Ok(Response::new(AuthResponse {
                    success: true,
                    session_token: self.session_token.clone(),
                    error_message: String::new(),
                    expires_in_seconds: 3600,
                }))
            } else {
                Ok(Response::new(AuthResponse {
                    success: false,
                    session_token: String::new(),
                    error_message: "Invalid PSK credentials".to_string(),
                    expires_in_seconds: 0,
                }))
            }
        }

        async fn get_system_info(
            &self,
            request: Request<Empty>,
        ) -> Result<Response<SystemInfo>, Status> {
            self.check_auth(&request)?;
            Ok(Response::new(SystemInfo {
                hostname: "mock-host".to_string(),
                os_name: "Linux".to_string(),
                os_version: "6.8".to_string(),
                arch: "x86_64".to_string(),
                total_memory_bytes: 8 * 1024 * 1024 * 1024,
                cpu_count: 4,
                disks: vec![],
                networks: vec![],
            }))
        }

        type StreamMetricsStream =
            tokio_stream::wrappers::ReceiverStream<Result<SystemMetrics, Status>>;

        async fn stream_metrics(
            &self,
            request: Request<MetricsRequest>,
        ) -> Result<Response<Self::StreamMetricsStream>, Status> {
            self.check_auth(&request)?;
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
        }

        type OpenTerminalStream =
            tokio_stream::wrappers::ReceiverStream<Result<TerminalOutput, Status>>;

        async fn open_terminal(
            &self,
            request: Request<tonic::Streaming<TerminalInput>>,
        ) -> Result<Response<Self::OpenTerminalStream>, Status> {
            self.check_auth(&request)?;
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
        }

        async fn list_processes(
            &self,
            request: Request<ListProcessesRequest>,
        ) -> Result<Response<ListProcessesResponse>, Status> {
            self.check_auth(&request)?;
            let processes = vec![
                ProcessItem {
                    pid: 101,
                    name: "daemon-service".to_string(),
                    cpu_usage: 15.5,
                    memory_bytes: 128 * 1024 * 1024,
                    user: "root".to_string(),
                    status: "Running".to_string(),
                    command_line: "/usr/bin/daemon-service --config /etc/daemon.conf".to_string(),
                    parent_pid: 0,
                },
                ProcessItem {
                    pid: 202,
                    name: "worker-task".to_string(),
                    cpu_usage: 3.2,
                    memory_bytes: 64 * 1024 * 1024,
                    user: "app".to_string(),
                    status: "Sleeping".to_string(),
                    command_line: "/opt/worker-task".to_string(),
                    parent_pid: 101,
                },
            ];
            Ok(Response::new(ListProcessesResponse { processes }))
        }

        async fn kill_process(
            &self,
            request: Request<KillProcessRequest>,
        ) -> Result<Response<KillProcessResponse>, Status> {
            self.check_auth(&request)?;
            let req = request.into_inner();
            if req.pid == 101 && (req.signal == 15 || req.signal == 9) {
                Ok(Response::new(KillProcessResponse {
                    success: true,
                    error_message: String::new(),
                }))
            } else {
                Ok(Response::new(KillProcessResponse {
                    success: false,
                    error_message: format!("Process {} not found or kill failed", req.pid),
                }))
            }
        }

        async fn list_services(
            &self,
            request: Request<ListServicesRequest>,
        ) -> Result<Response<ListServicesResponse>, Status> {
            self.check_auth(&request)?;
            let services = vec![
                ServiceItem {
                    name: "nginx.service".to_string(),
                    description: "Nginx Web Server".to_string(),
                    load_state: "loaded".to_string(),
                    active_state: "active".to_string(),
                    sub_state: "running".to_string(),
                },
                ServiceItem {
                    name: "postgresql.service".to_string(),
                    description: "PostgreSQL Database".to_string(),
                    load_state: "loaded".to_string(),
                    active_state: "inactive".to_string(),
                    sub_state: "dead".to_string(),
                },
            ];
            Ok(Response::new(ListServicesResponse { services }))
        }

        async fn control_service(
            &self,
            request: Request<ControlServiceRequest>,
        ) -> Result<Response<ControlServiceResponse>, Status> {
            self.check_auth(&request)?;
            let req = request.into_inner();
            if req.service_name == "nginx.service"
                && (req.action == "start" || req.action == "stop" || req.action == "restart")
            {
                Ok(Response::new(ControlServiceResponse {
                    success: true,
                    error_message: String::new(),
                }))
            } else {
                Ok(Response::new(ControlServiceResponse {
                    success: false,
                    error_message: format!(
                        "Action '{}' failed on service '{}'",
                        req.action, req.service_name
                    ),
                }))
            }
        }

        async fn list_managed_services(
            &self,
            request: Request<ListManagedServicesRequest>,
        ) -> Result<Response<ListManagedServicesResponse>, Status> {
            self.check_supported("ListManagedServices")?;
            self.check_auth(&request)?;
            let services = vec![ManagedServiceStatus {
                config: Some(ManagedServiceConfig {
                    id: "mc-survival".to_string(),
                    name: "Minecraft Survival".to_string(),
                    icon: "⛏".to_string(),
                    description: "Serveur PaperMC".to_string(),
                    command: "java -Xmx4G -jar server.jar nogui".to_string(),
                    working_dir: "/srv/minecraft".to_string(),
                    stop_command: "stop".to_string(),
                    auto_start: true,
                    auto_restart: true,
                    env_vars: vec!["JAVA_OPTS=-XX:+UseG1GC".to_string()],
                    quick_commands: vec!["list".to_string(), "save-all".to_string()],
                    systemd_unit: String::new(),
                    attach_command: String::new(),
                }),
                state: "running".to_string(),
                pid: 4242,
                cpu_usage: 24.5,
                memory_bytes: 2 * 1024 * 1024 * 1024,
                uptime_seconds: 3600,
                last_exit_code: 0,
                has_exit_code: false,
                restart_count: 1,
                last_error: String::new(),
            }];
            Ok(Response::new(ListManagedServicesResponse { services }))
        }

        async fn save_managed_service(
            &self,
            request: Request<SaveManagedServiceRequest>,
        ) -> Result<Response<SaveManagedServiceResponse>, Status> {
            self.check_supported("SaveManagedService")?;
            self.check_auth(&request)?;
            match request.into_inner().config {
                Some(mut cfg) if !cfg.name.trim().is_empty() => {
                    if cfg.id.is_empty() {
                        cfg.id = "svc-generated-1".to_string();
                    }
                    Ok(Response::new(SaveManagedServiceResponse {
                        success: true,
                        error_message: String::new(),
                        service: Some(cfg),
                    }))
                }
                _ => Ok(Response::new(SaveManagedServiceResponse {
                    success: false,
                    error_message: "Nom du service requis".to_string(),
                    service: None,
                })),
            }
        }

        async fn delete_managed_service(
            &self,
            request: Request<DeleteManagedServiceRequest>,
        ) -> Result<Response<DeleteManagedServiceResponse>, Status> {
            self.check_supported("DeleteManagedService")?;
            self.check_auth(&request)?;
            let req = request.into_inner();
            if req.id == "mc-survival" {
                Ok(Response::new(DeleteManagedServiceResponse {
                    success: true,
                    error_message: String::new(),
                }))
            } else {
                Ok(Response::new(DeleteManagedServiceResponse {
                    success: false,
                    error_message: format!("Service '{}' introuvable", req.id),
                }))
            }
        }

        async fn control_managed_service(
            &self,
            request: Request<ControlManagedServiceRequest>,
        ) -> Result<Response<ControlManagedServiceResponse>, Status> {
            self.check_supported("ControlManagedService")?;
            self.check_auth(&request)?;
            let req = request.into_inner();
            if req.id == "mc-survival"
                && matches!(req.action.as_str(), "start" | "stop" | "restart" | "kill")
            {
                Ok(Response::new(ControlManagedServiceResponse {
                    success: true,
                    error_message: String::new(),
                }))
            } else {
                Ok(Response::new(ControlManagedServiceResponse {
                    success: false,
                    error_message: format!("Action '{}' invalide sur '{}'", req.action, req.id),
                }))
            }
        }

        async fn send_managed_service_input(
            &self,
            request: Request<SendManagedServiceInputRequest>,
        ) -> Result<Response<SendManagedServiceInputResponse>, Status> {
            self.check_supported("SendManagedServiceInput")?;
            self.check_auth(&request)?;
            let req = request.into_inner();
            if req.id == "mc-survival" && !req.input.is_empty() {
                Ok(Response::new(SendManagedServiceInputResponse {
                    success: true,
                    error_message: String::new(),
                }))
            } else {
                Ok(Response::new(SendManagedServiceInputResponse {
                    success: false,
                    error_message: "Entrée refusée".to_string(),
                }))
            }
        }

        type StreamManagedServiceConsoleStream =
            tokio_stream::wrappers::ReceiverStream<Result<ConsoleChunk, Status>>;

        async fn stream_managed_service_console(
            &self,
            request: Request<StreamManagedServiceConsoleRequest>,
        ) -> Result<Response<Self::StreamManagedServiceConsoleStream>, Status> {
            self.check_supported("StreamManagedServiceConsole")?;
            self.check_auth(&request)?;
            let req = request.into_inner();
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tokio::spawn(async move {
                let _ = tx
                    .send(Ok(ConsoleChunk {
                        data: format!("[tail={}] {}\n", req.tail_lines, req.id).into_bytes(),
                        timestamp_ms: 1000,
                        is_history: true,
                    }))
                    .await;
                let _ = tx
                    .send(Ok(ConsoleChunk {
                        data: b"Done (2.4s)! For help, type \"help\"\n".to_vec(),
                        timestamp_ms: 2000,
                        is_history: false,
                    }))
                    .await;
            });
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
                rx,
            )))
        }

        async fn list_docker_containers(
            &self,
            request: Request<ListDockerContainersRequest>,
        ) -> Result<Response<ListDockerContainersResponse>, Status> {
            self.check_supported("ListDockerContainers")?;
            self.check_auth(&request)?;
            let include_all = request.into_inner().include_all;
            let mut containers = vec![DockerContainerItem {
                id: "abc123def456".to_string(),
                name: "mc-docker".to_string(),
                image: "itzg/minecraft-server:latest".to_string(),
                state: "running".to_string(),
                status: "Up 3 hours".to_string(),
                ports: "0.0.0.0:25565->25565/tcp".to_string(),
                created: "2026-11-20 18:00:00".to_string(),
                cpu_percent: 12.5,
                memory_used_bytes: 512 * 1024 * 1024,
                memory_limit_bytes: 4 * 1024 * 1024 * 1024,
                compose_project: "gaming".to_string(),
            }];
            if include_all {
                containers.push(DockerContainerItem {
                    id: "def789012345".to_string(),
                    name: "backup-job".to_string(),
                    image: "alpine:3.20".to_string(),
                    state: "exited".to_string(),
                    status: "Exited (0) 2 hours ago".to_string(),
                    ports: String::new(),
                    created: "2026-11-20 16:00:00".to_string(),
                    cpu_percent: 0.0,
                    memory_used_bytes: 0,
                    memory_limit_bytes: 0,
                    compose_project: "ops".to_string(),
                });
            }
            Ok(Response::new(ListDockerContainersResponse {
                docker_available: true,
                docker_version: "27.3.1".to_string(),
                error_message: String::new(),
                containers,
            }))
        }

        async fn control_docker_container(
            &self,
            request: Request<ControlDockerContainerRequest>,
        ) -> Result<Response<ControlDockerContainerResponse>, Status> {
            self.check_supported("ControlDockerContainer")?;
            self.check_auth(&request)?;
            let req = request.into_inner();
            if req.container_id == "abc123def456"
                && matches!(
                    req.action.as_str(),
                    "start" | "stop" | "restart" | "pause" | "unpause" | "kill" | "remove"
                )
            {
                Ok(Response::new(ControlDockerContainerResponse {
                    success: true,
                    error_message: String::new(),
                }))
            } else {
                Ok(Response::new(ControlDockerContainerResponse {
                    success: false,
                    error_message: format!(
                        "Action '{}' refusée sur '{}'",
                        req.action, req.container_id
                    ),
                }))
            }
        }

        async fn exec_docker_command(
            &self,
            request: Request<ExecDockerCommandRequest>,
        ) -> Result<Response<ExecDockerCommandResponse>, Status> {
            self.check_supported("ExecDockerCommand")?;
            self.check_auth(&request)?;
            let req = request.into_inner();
            if req.container_id == "abc123def456" && !req.command.trim().is_empty() {
                Ok(Response::new(ExecDockerCommandResponse {
                    success: true,
                    exit_code: 0,
                    stdout: format!("executed: {}\n", req.command),
                    stderr: String::new(),
                    error_message: String::new(),
                }))
            } else {
                Ok(Response::new(ExecDockerCommandResponse {
                    success: false,
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: "invalid exec request".to_string(),
                    error_message: "Commande refusée".to_string(),
                }))
            }
        }

        type StreamDockerConsoleStream =
            tokio_stream::wrappers::ReceiverStream<Result<ConsoleChunk, Status>>;

        async fn stream_docker_console(
            &self,
            request: Request<tonic::Streaming<DockerConsoleInput>>,
        ) -> Result<Response<Self::StreamDockerConsoleStream>, Status> {
            self.check_supported("StreamDockerConsole")?;
            self.check_auth(&request)?;
            let mut inbound = request.into_inner();
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            tokio::spawn(async move {
                while let Ok(Some(msg)) = inbound.message().await {
                    let chunk = match msg.input {
                        Some(docker_console_input::Input::Init(init)) => ConsoleChunk {
                            data: format!(
                                "init:{}:{}:{}",
                                init.container_id, init.mode, init.tail_lines
                            )
                            .into_bytes(),
                            timestamp_ms: 1000,
                            is_history: true,
                        },
                        Some(docker_console_input::Input::Data(bytes)) => ConsoleChunk {
                            data: [b"echo:".as_slice(), &bytes].concat(),
                            timestamp_ms: 2000,
                            is_history: false,
                        },
                        Some(docker_console_input::Input::Resize(r)) => ConsoleChunk {
                            data: format!("resize:{}x{}", r.cols, r.rows).into_bytes(),
                            timestamp_ms: 3000,
                            is_history: false,
                        },
                        None => continue,
                    };
                    if tx.send(Ok(chunk)).await.is_err() {
                        break;
                    }
                }
            });
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
                rx,
            )))
        }
    }

    async fn start_mock_tls_server(
        psk: &str,
        session_token: &str,
        reject_next_once: bool,
    ) -> (SocketAddr, String, tokio::sync::oneshot::Sender<()>) {
        start_mock_tls_server_with_options(psk, session_token, reject_next_once, false).await
    }

    async fn start_mock_tls_server_with_options(
        psk: &str,
        session_token: &str,
        reject_next_once: bool,
        legacy_unimplemented: bool,
    ) -> (SocketAddr, String, tokio::sync::oneshot::Sender<()>) {
        let _ = rustls::crypto::ring::default_provider().install_default();

        let kp = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "ntx-agent".to_string(),
        ])
        .unwrap();
        params.distinguished_name.push(DnType::CommonName, "ntx-agent");
        let cert = params.self_signed(&kp).unwrap();
        let fp = hex::encode(Sha256::digest(cert.der().as_ref()));
        let server_tls =
            ServerTlsConfig::new().identity(Identity::from_pem(cert.pem(), kp.serialize_pem()));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let mock_agent = MockAgent {
            psk: psk.to_string(),
            expected_nonce: vec![42; 32],
            expected_challenge_id: "chal-mock-1".to_string(),
            session_token: session_token.to_string(),
            reject_next_token_once: AtomicBool::new(reject_next_once),
            legacy_unimplemented: AtomicBool::new(legacy_unimplemented),
        };

        tokio::spawn(async move {
            Server::builder()
                .tls_config(server_tls)
                .unwrap()
                .add_service(NtxAgentServer::new(mock_agent))
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        (addr, fp, shutdown_tx)
    }

    #[tokio::test]
    async fn test_client_tls_tofu_and_pin_mismatch() {
        let (addr, fp, _shutdown) = start_mock_tls_server("secret", "tok123", false).await;
        let endpoint = format!("https://{}", addr);

        // Wrong PSK fails with AuthFailed
        let err = NtxClient::connect_and_authenticate(&endpoint, "wrong", "v1", None)
            .await
            .err()
            .unwrap();
        assert!(matches!(err, ClientConnectError::AuthFailed(_)));

        // TOFU succeeds and returns fingerprint
        let mut client = NtxClient::connect_and_authenticate(&endpoint, "secret", "v1", None)
            .await
            .unwrap();
        assert_eq!(client.cert_fingerprint(), fp);
        let ping = client.ping().await.unwrap();
        assert_eq!(ping.hostname, "mock-host");

        // Wrong pinned fingerprint triggers TlsMismatch
        let wrong_fp = "00".repeat(32);
        let mismatch = NtxClient::connect_and_authenticate(
            &endpoint,
            "secret",
            "v1",
            Some(wrong_fp.clone()),
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(
            mismatch,
            ClientConnectError::TlsMismatch { expected, actual } if expected == wrong_fp && actual == fp
        ));
    }

    #[tokio::test]
    async fn test_client_multiplexed_rpcs_and_reauth() {
        let (addr, fp, _shutdown) = start_mock_tls_server("my_psk", "session_abc", true).await;
        let endpoint = format!("https://{}", addr);

        let mut client =
            NtxClient::connect_and_authenticate(&endpoint, "my_psk", "viewer-1", Some(fp))
                .await
                .unwrap();

        // First call sees simulated Unauthenticated and transparently re-authenticates & retries!
        let info = client.get_system_info().await.unwrap();
        assert_eq!(info.hostname, "mock-host");

        // Clone shares underlying Channel and session token concurrently
        let mut c1 = client.clone();
        let mut c2 = client.clone();
        let (procs_res, svcs_res) = tokio::join!(c1.list_processes(), c2.list_services());

        let procs = procs_res.unwrap();
        assert_eq!(procs.len(), 2);
        assert_eq!(procs[0].pid, 101);

        let svcs = svcs_res.unwrap();
        assert_eq!(svcs.len(), 2);
        assert_eq!(svcs[0].name, "nginx.service");

        let kill_ok = client.kill_process(101, 15).await.unwrap();
        assert!(kill_ok.success);

        let ctrl_ok = client.control_service("nginx.service", "restart").await.unwrap();
        assert!(ctrl_ok.success);

        let ctrl_bad = client.control_service("nginx.service", "destroy").await.unwrap();
        assert!(!ctrl_bad.success);
    }

    #[tokio::test]
    async fn test_client_managed_services_rpcs() {
        let (addr, fp, _shutdown) = start_mock_tls_server("my_psk", "session_ms", true).await;
        let endpoint = format!("https://{}", addr);

        let mut client =
            NtxClient::connect_and_authenticate(&endpoint, "my_psk", "viewer-ms", Some(fp))
                .await
                .unwrap();

        // First call triggers simulated Unauthenticated -> transparent re-auth retry
        let services = client.list_managed_services().await.unwrap();
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].state, "running");
        assert_eq!(services[0].pid, 4242);
        assert_eq!(services[0].config.as_ref().unwrap().id, "mc-survival");

        // Save new managed service
        let new_cfg = ManagedServiceConfig {
            id: String::new(),
            name: "Discord Bot".to_string(),
            icon: "🤖".to_string(),
            description: "Bot de modération".to_string(),
            command: "node index.js".to_string(),
            working_dir: "/srv/bot".to_string(),
            stop_command: String::new(),
            auto_start: false,
            auto_restart: true,
            env_vars: vec![],
            quick_commands: vec![],
            systemd_unit: String::new(),
            attach_command: String::new(),
        };
        let saved = client.save_managed_service(new_cfg).await.unwrap();
        assert!(saved.success);
        assert_eq!(saved.service.as_ref().unwrap().id, "svc-generated-1");

        // Control managed service
        let ctrl_ok = client
            .control_managed_service("mc-survival", "restart")
            .await
            .unwrap();
        assert!(ctrl_ok.success);
        let ctrl_bad = client
            .control_managed_service("mc-survival", "nuke")
            .await
            .unwrap();
        assert!(!ctrl_bad.success);

        // Send stdin input to managed service
        let input_ok = client
            .send_managed_service_input("mc-survival", "say hello", true)
            .await
            .unwrap();
        assert!(input_ok.success);

        // Stream console output (history + live chunk)
        let mut stream = client
            .stream_managed_service_console("mc-survival", 50)
            .await
            .unwrap();
        let first = stream.message().await.unwrap().unwrap();
        assert!(first.is_history);
        assert_eq!(
            String::from_utf8(first.data).unwrap(),
            "[tail=50] mc-survival\n"
        );
        let second = stream.message().await.unwrap().unwrap();
        assert!(!second.is_history);
        assert!(String::from_utf8(second.data)
            .unwrap()
            .contains("Done (2.4s)!"));

        // Delete managed service
        let del_ok = client.delete_managed_service("mc-survival").await.unwrap();
        assert!(del_ok.success);
        let del_missing = client.delete_managed_service("unknown").await.unwrap();
        assert!(!del_missing.success);
    }

    #[tokio::test]
    async fn test_client_docker_rpcs_and_console_stream() {
        let (addr, fp, _shutdown) = start_mock_tls_server("docker_psk", "session_dk", false).await;
        let endpoint = format!("https://{}", addr);

        let mut client =
            NtxClient::connect_and_authenticate(&endpoint, "docker_psk", "viewer-dk", Some(fp))
                .await
                .unwrap();

        let running = client.list_docker_containers(false).await.unwrap();
        assert!(running.docker_available);
        assert_eq!(running.docker_version, "27.3.1");
        assert_eq!(running.containers.len(), 1);
        assert_eq!(running.containers[0].name, "mc-docker");

        let all = client.list_docker_containers(true).await.unwrap();
        assert_eq!(all.containers.len(), 2);
        assert_eq!(all.containers[1].state, "exited");

        let ctrl_ok = client
            .control_docker_container("abc123def456", "restart", false)
            .await
            .unwrap();
        assert!(ctrl_ok.success);
        let ctrl_bad = client
            .control_docker_container("abc123def456", "prune", true)
            .await
            .unwrap();
        assert!(!ctrl_bad.success);

        let exec_res = client
            .exec_docker_command("abc123def456", "uname -a")
            .await
            .unwrap();
        assert!(exec_res.success);
        assert_eq!(exec_res.exit_code, 0);
        assert_eq!(exec_res.stdout, "executed: uname -a\n");

        // Bidirectional Docker live console stream
        let (tx, rx) = mpsc::channel(8);
        tx.send(DockerConsoleInput {
            input: Some(docker_console_input::Input::Init(DockerConsoleInit {
                container_id: "abc123def456".to_string(),
                mode: "attach".to_string(),
                tail_lines: 100,
            })),
        })
        .await
        .unwrap();
        let mut stream = client.stream_docker_console(rx).await.unwrap();

        let init_chunk = stream.message().await.unwrap().unwrap();
        assert!(init_chunk.is_history);
        assert_eq!(
            String::from_utf8(init_chunk.data).unwrap(),
            "init:abc123def456:attach:100"
        );

        tx.send(DockerConsoleInput {
            input: Some(docker_console_input::Input::Data(b"list\n".to_vec())),
        })
        .await
        .unwrap();
        let echo_chunk = stream.message().await.unwrap().unwrap();
        assert!(!echo_chunk.is_history);
        assert_eq!(echo_chunk.data, b"echo:list\n");

        tx.send(DockerConsoleInput {
            input: Some(docker_console_input::Input::Resize(TerminalResize {
                cols: 120,
                rows: 40,
            })),
        })
        .await
        .unwrap();
        let resize_chunk = stream.message().await.unwrap().unwrap();
        assert_eq!(
            String::from_utf8(resize_chunk.data).unwrap(),
            "resize:120x40"
        );
    }

    #[tokio::test]
    async fn test_client_stream_reauth_retry() {
        // 1. Server-streaming managed service console retries once on Code::Unauthenticated
        let (addr1, fp1, _shutdown1) = start_mock_tls_server("psk1", "tok1", true).await;
        let mut client1 = NtxClient::connect_and_authenticate(
            &format!("https://{}", addr1),
            "psk1",
            "v-stream-1",
            Some(fp1),
        )
        .await
        .unwrap();
        let mut ms_stream = client1
            .stream_managed_service_console("mc-survival", 25)
            .await
            .unwrap();
        let chunk = ms_stream.message().await.unwrap().unwrap();
        assert_eq!(
            String::from_utf8(chunk.data).unwrap(),
            "[tail=25] mc-survival\n"
        );

        // 2. Bidirectional Docker console retries once on Code::Unauthenticated AND replays Init + queued Data
        let (addr2, fp2, _shutdown2) = start_mock_tls_server("psk2", "tok2", true).await;
        let mut client2 = NtxClient::connect_and_authenticate(
            &format!("https://{}", addr2),
            "psk2",
            "v-stream-2",
            Some(fp2),
        )
        .await
        .unwrap();
        let (tx, rx) = mpsc::channel(8);
        tx.send(DockerConsoleInput {
            input: Some(docker_console_input::Input::Init(DockerConsoleInit {
                container_id: "abc123def456".to_string(),
                mode: "shell".to_string(),
                tail_lines: 10,
            })),
        })
        .await
        .unwrap();
        tx.send(DockerConsoleInput {
            input: Some(docker_console_input::Input::Data(b"whoami\n".to_vec())),
        })
        .await
        .unwrap();

        let mut dk_stream = client2.stream_docker_console(rx).await.unwrap();
        let init_chunk = dk_stream.message().await.unwrap().unwrap();
        assert_eq!(
            String::from_utf8(init_chunk.data).unwrap(),
            "init:abc123def456:shell:10"
        );
        let data_chunk = dk_stream.message().await.unwrap().unwrap();
        assert_eq!(data_chunk.data, b"echo:whoami\n");

        // Subsequent live input after retry continues flowing from the same channel
        tx.send(DockerConsoleInput {
            input: Some(docker_console_input::Input::Data(b"pwd\n".to_vec())),
        })
        .await
        .unwrap();
        let live_chunk = dk_stream.message().await.unwrap().unwrap();
        assert_eq!(live_chunk.data, b"echo:pwd\n");
    }

    #[tokio::test]
    async fn test_client_unimplemented_error_detection() {
        let (addr, fp, _shutdown) =
            start_mock_tls_server_with_options("legacy_psk", "legacy_tok", false, true).await;
        let endpoint = format!("https://{}", addr);

        let mut client = NtxClient::connect_and_authenticate(
            &endpoint,
            "legacy_psk",
            "viewer-legacy",
            Some(fp),
        )
        .await
        .unwrap();

        let check_unimpl = |err: anyhow::Error| {
            assert!(
                is_unimplemented_error(&err),
                "expected is_unimplemented_error == true, got: {err}"
            );
            assert!(err.downcast_ref::<RpcUnimplementedError>().is_some());
            let wrapped = err.context("contexte appelant");
            assert!(is_unimplemented_error(&wrapped));
        };

        check_unimpl(client.list_managed_services().await.unwrap_err());
        check_unimpl(
            client
                .save_managed_service(ManagedServiceConfig::default())
                .await
                .unwrap_err(),
        );
        check_unimpl(client.delete_managed_service("mc").await.unwrap_err());
        check_unimpl(
            client
                .control_managed_service("mc", "start")
                .await
                .unwrap_err(),
        );
        check_unimpl(
            client
                .send_managed_service_input("mc", "help", true)
                .await
                .unwrap_err(),
        );
        check_unimpl(
            client
                .stream_managed_service_console("mc", 20)
                .await
                .unwrap_err(),
        );
        check_unimpl(client.list_docker_containers(true).await.unwrap_err());
        check_unimpl(
            client
                .control_docker_container("abc", "start", false)
                .await
                .unwrap_err(),
        );
        check_unimpl(
            client
                .exec_docker_command("abc", "ls")
                .await
                .unwrap_err(),
        );
        let (_tx, rx) = mpsc::channel(1);
        check_unimpl(client.stream_docker_console(rx).await.unwrap_err());

        // Non-Unimplemented errors must NOT be classified as Unimplemented
        let other_err = format_rpc_error("Test", Status::unavailable("serveur indisponible"));
        assert!(!is_unimplemented_error(&other_err));
        assert!(other_err.downcast_ref::<RpcUnimplementedError>().is_none());

        let internal_err = format_rpc_error("Test", Status::internal("erreur interne"));
        assert!(!is_unimplemented_error(&internal_err));
    }
}
