use crate::auth::compute_challenge_hmac;
use crate::tls::{connect_tls_channel, TlsConnectError};
use ntx_proto::ntx_agent_client::NtxAgentClient;
use ntx_proto::*;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;
use tonic::{Code, Request, Status};
use tracing::{debug, info};

const UNARY_RPC_TIMEOUT: Duration = Duration::from_secs(15);

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
        Code::Unimplemented => anyhow::anyhow!(
            "La méthode {method} n'est pas supportée par cet agent distant (version incompatible)."
        ),
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
    }

    impl MockAgent {
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
    }

    async fn start_mock_tls_server(
        psk: &str,
        session_token: &str,
        reject_next_once: bool,
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
}
