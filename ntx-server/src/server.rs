#![allow(clippy::result_large_err)]

use crate::auth::{AuthFailure, AuthManager};
use crate::config::{get_hostname, Config};
use crate::docker;
use crate::managed_service::ManagedServiceManager;
use crate::monitoring::{self, collect_system_info};
use crate::process;
use crate::service_manager;
use crate::terminal;
use ntx_proto::ntx_agent_server::NtxAgent;
use ntx_proto::{
    AuthChallengeRequest, AuthChallengeResponse, AuthRequest, AuthResponse, ConsoleChunk,
    ControlDockerContainerRequest, ControlDockerContainerResponse, ControlManagedServiceRequest,
    ControlManagedServiceResponse, ControlServiceRequest, ControlServiceResponse,
    DeleteManagedServiceRequest, DeleteManagedServiceResponse, DockerConsoleInput, Empty,
    ExecDockerCommandRequest, ExecDockerCommandResponse, KillProcessRequest, KillProcessResponse,
    ListDockerContainersRequest, ListDockerContainersResponse, ListManagedServicesRequest,
    ListManagedServicesResponse, ListProcessesRequest, ListProcessesResponse, ListServicesRequest,
    ListServicesResponse, MetricsRequest, PingResponse, SaveManagedServiceRequest,
    SaveManagedServiceResponse, SendManagedServiceInputRequest, SendManagedServiceInputResponse,
    StreamManagedServiceConsoleRequest, SystemInfo, SystemMetrics, TerminalInput, TerminalOutput,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use sysinfo::{Disks, Networks, System};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{info, warn};

pub struct NtxAgentService {
    config: Config,
    auth: AuthManager,
    managed_services: ManagedServiceManager,
    start_time: Instant,
}

impl NtxAgentService {
    pub fn new(config: Config, auth: AuthManager) -> Self {
        Self::with_managed_services(
            config,
            auth,
            ManagedServiceManager::in_memory_for_tests(),
        )
    }

    pub fn with_managed_services(
        config: Config,
        auth: AuthManager,
        managed_services: ManagedServiceManager,
    ) -> Self {
        Self {
            config,
            auth,
            managed_services,
            start_time: Instant::now(),
        }
    }

    fn peer_key<T>(request: &Request<T>) -> String {
        request
            .remote_addr()
            .map(|a| a.ip().to_string())
            .unwrap_or_else(|| "local".to_string())
    }
}

#[tonic::async_trait]
impl NtxAgent for NtxAgentService {
    async fn get_auth_challenge(
        &self,
        request: Request<AuthChallengeRequest>,
    ) -> Result<Response<AuthChallengeResponse>, Status> {
        let peer_key = Self::peer_key(&request);
        let req = request.into_inner();
        let (challenge_id, nonce, expires_in_seconds) =
            self.auth.create_challenge(&peer_key, &req.viewer_id)?;

        Ok(Response::new(AuthChallengeResponse {
            challenge_id,
            nonce,
            expires_in_seconds,
        }))
    }

    async fn authenticate(
        &self,
        request: Request<AuthRequest>,
    ) -> Result<Response<AuthResponse>, Status> {
        let peer_key = Self::peer_key(&request);
        let req = request.into_inner();

        if req.challenge_id.trim().is_empty() || req.response_hmac.trim().is_empty() {
            self.auth.record_auth_failure(&peer_key);
            warn!(
                viewer_id = %req.viewer_id,
                peer = %peer_key,
                "Authentication rejected: challenge-response required"
            );
            return Ok(Response::new(AuthResponse {
                success: false,
                session_token: String::new(),
                error_message: "Challenge-response authentication required".to_string(),
                expires_in_seconds: 0,
            }));
        }

        match self.auth.authenticate_challenge(
            &peer_key,
            &req.challenge_id,
            &req.viewer_id,
            &req.response_hmac,
        ) {
            Ok((session_token, expires_in_seconds)) => {
                info!(
                    viewer_id = %req.viewer_id,
                    peer = %peer_key,
                    "Authentication successful, created new session"
                );
                Ok(Response::new(AuthResponse {
                    success: true,
                    session_token,
                    error_message: String::new(),
                    expires_in_seconds,
                }))
            }
            Err(AuthFailure::RateLimited(msg)) => {
                warn!(
                    viewer_id = %req.viewer_id,
                    peer = %peer_key,
                    "Authentication blocked by rate limiter"
                );
                Err(Status::resource_exhausted(msg))
            }
            Err(AuthFailure::Rejected(msg)) => {
                warn!(
                    viewer_id = %req.viewer_id,
                    peer = %peer_key,
                    "Authentication failed: invalid credentials"
                );
                Ok(Response::new(AuthResponse {
                    success: false,
                    session_token: String::new(),
                    error_message: msg,
                    expires_in_seconds: 0,
                }))
            }
        }
    }

    async fn get_system_info(
        &self,
        request: Request<Empty>,
    ) -> Result<Response<SystemInfo>, Status> {
        self.auth.verify_request(&request)?;
        let config = self.config.clone();
        let info = tokio::task::spawn_blocking(move || collect_system_info(&config))
            .await
            .map_err(|e| Status::internal(format!("SystemInfo task failed: {}", e)))?;
        Ok(Response::new(info))
    }

    type StreamMetricsStream = ReceiverStream<Result<SystemMetrics, Status>>;

    async fn stream_metrics(
        &self,
        request: Request<MetricsRequest>,
    ) -> Result<Response<Self::StreamMetricsStream>, Status> {
        self.auth.verify_request(&request)?;

        let interval_ms = request.into_inner().interval_ms.clamp(250, 60_000);
        let (tx, rx) = mpsc::channel(32);

        tokio::spawn(async move {
            let state = Arc::new(Mutex::new((
                System::new_all(),
                Disks::new_with_refreshed_list(),
                Networks::new_with_refreshed_list(),
            )));

            // Warm-up sleep so initial CPU/network/disk deltas are valid
            let state_warmup = Arc::clone(&state);
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(mut guard) = state_warmup.lock() {
                    let (sys, disks, networks) = &mut *guard;
                    sys.refresh_cpu_all();
                    disks.refresh(true);
                    networks.refresh(true);
                }
            })
            .await;

            let mut last_tick = Instant::now();
            tokio::select! {
                _ = tx.closed() => return,
                _ = tokio::time::sleep(Duration::from_millis(200)) => {}
            }

            loop {
                let elapsed_secs = last_tick.elapsed().as_secs_f64();
                last_tick = Instant::now();

                let state_clone = Arc::clone(&state);
                let metrics_res = tokio::task::spawn_blocking(move || {
                    let mut guard = state_clone
                        .lock()
                        .map_err(|_| Status::internal("Metrics lock poisoned"))?;
                    let (sys, disks, networks) = &mut *guard;
                    Ok::<_, Status>(monitoring::collect_metrics(
                        sys,
                        disks,
                        networks,
                        elapsed_secs,
                    ))
                })
                .await;

                match metrics_res {
                    Ok(Ok(metrics)) => {
                        if tx.send(Ok(metrics)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Err(status)) => {
                        let _ = tx.send(Err(status)).await;
                        break;
                    }
                    Err(_) => break,
                }

                tokio::select! {
                    _ = tx.closed() => break,
                    _ = tokio::time::sleep(Duration::from_millis(interval_ms as u64)) => {}
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    type OpenTerminalStream = ReceiverStream<Result<TerminalOutput, Status>>;

    async fn open_terminal(
        &self,
        request: Request<Streaming<TerminalInput>>,
    ) -> Result<Response<Self::OpenTerminalStream>, Status> {
        self.auth.verify_request(&request)?;
        terminal::handle_open_terminal(request).await
    }

    async fn ping(&self, _request: Request<Empty>) -> Result<Response<PingResponse>, Status> {
        let uptime_seconds = self.start_time.elapsed().as_secs();
        let hostname = get_hostname(&self.config);
        let version = env!("CARGO_PKG_VERSION").to_string();

        Ok(Response::new(PingResponse {
            hostname,
            uptime_seconds,
            version,
        }))
    }

    async fn list_processes(
        &self,
        request: Request<ListProcessesRequest>,
    ) -> Result<Response<ListProcessesResponse>, Status> {
        self.auth.verify_request(&request)?;
        let processes = process::list_processes().await;
        Ok(Response::new(ListProcessesResponse { processes }))
    }

    async fn kill_process(
        &self,
        request: Request<KillProcessRequest>,
    ) -> Result<Response<KillProcessResponse>, Status> {
        self.auth.verify_request(&request)?;
        let req = request.into_inner();
        let res = tokio::task::spawn_blocking(move || process::kill_process(req.pid, req.signal))
            .await
            .map_err(|e| Status::internal(format!("KillProcess task failed: {}", e)))?;

        let (success, error_message) = match res {
            Ok(()) => (true, String::new()),
            Err(e) => (false, e),
        };
        Ok(Response::new(KillProcessResponse {
            success,
            error_message,
        }))
    }

    async fn list_services(
        &self,
        request: Request<ListServicesRequest>,
    ) -> Result<Response<ListServicesResponse>, Status> {
        self.auth.verify_request(&request)?;
        let res = tokio::task::spawn_blocking(service_manager::list_system_services)
            .await
            .map_err(|e| Status::internal(format!("ListServices task failed: {}", e)))?;

        match res {
            Ok(services) => Ok(Response::new(ListServicesResponse { services })),
            Err(msg) => Err(Status::unavailable(msg)),
        }
    }

    async fn control_service(
        &self,
        request: Request<ControlServiceRequest>,
    ) -> Result<Response<ControlServiceResponse>, Status> {
        self.auth.verify_request(&request)?;
        let req = request.into_inner();
        let res = tokio::task::spawn_blocking(move || {
            service_manager::control_system_service(&req.service_name, &req.action)
        })
        .await
        .map_err(|e| Status::internal(format!("ControlService task failed: {}", e)))?;

        let (success, error_message) = match res {
            Ok(()) => (true, String::new()),
            Err(e) => (false, e),
        };
        Ok(Response::new(ControlServiceResponse {
            success,
            error_message,
        }))
    }

    async fn list_managed_services(
        &self,
        request: Request<ListManagedServicesRequest>,
    ) -> Result<Response<ListManagedServicesResponse>, Status> {
        self.auth.verify_request(&request)?;
        let services = self.managed_services.list_services().await;
        Ok(Response::new(ListManagedServicesResponse { services }))
    }

    async fn save_managed_service(
        &self,
        request: Request<SaveManagedServiceRequest>,
    ) -> Result<Response<SaveManagedServiceResponse>, Status> {
        self.auth.verify_request(&request)?;
        let Some(cfg) = request.into_inner().config else {
            return Ok(Response::new(SaveManagedServiceResponse {
                success: false,
                error_message: "Configuration du service géré manquante.".to_string(),
                service: None,
            }));
        };

        let resp = match self.managed_services.save_service(cfg).await {
            Ok(service) => SaveManagedServiceResponse {
                success: true,
                error_message: String::new(),
                service: Some(service),
            },
            Err(error_message) => SaveManagedServiceResponse {
                success: false,
                error_message,
                service: None,
            },
        };
        Ok(Response::new(resp))
    }

    async fn delete_managed_service(
        &self,
        request: Request<DeleteManagedServiceRequest>,
    ) -> Result<Response<DeleteManagedServiceResponse>, Status> {
        self.auth.verify_request(&request)?;
        let req = request.into_inner();
        let (success, error_message) = match self.managed_services.delete_service(&req.id).await {
            Ok(()) => (true, String::new()),
            Err(e) => (false, e),
        };
        Ok(Response::new(DeleteManagedServiceResponse {
            success,
            error_message,
        }))
    }

    async fn control_managed_service(
        &self,
        request: Request<ControlManagedServiceRequest>,
    ) -> Result<Response<ControlManagedServiceResponse>, Status> {
        self.auth.verify_request(&request)?;
        let req = request.into_inner();
        let (success, error_message) = match self
            .managed_services
            .control_service(&req.id, &req.action)
            .await
        {
            Ok(()) => (true, String::new()),
            Err(e) => (false, e),
        };
        Ok(Response::new(ControlManagedServiceResponse {
            success,
            error_message,
        }))
    }

    async fn send_managed_service_input(
        &self,
        request: Request<SendManagedServiceInputRequest>,
    ) -> Result<Response<SendManagedServiceInputResponse>, Status> {
        self.auth.verify_request(&request)?;
        let req = request.into_inner();
        let (success, error_message) = match self
            .managed_services
            .send_input(&req.id, &req.input, req.append_newline)
            .await
        {
            Ok(()) => (true, String::new()),
            Err(e) => (false, e),
        };
        Ok(Response::new(SendManagedServiceInputResponse {
            success,
            error_message,
        }))
    }

    type StreamManagedServiceConsoleStream = ReceiverStream<Result<ConsoleChunk, Status>>;

    async fn stream_managed_service_console(
        &self,
        request: Request<StreamManagedServiceConsoleRequest>,
    ) -> Result<Response<Self::StreamManagedServiceConsoleStream>, Status> {
        self.auth.verify_request(&request)?;
        let req = request.into_inner();
        let stream = self
            .managed_services
            .stream_console(&req.id, req.tail_lines)
            .await?;
        Ok(Response::new(stream))
    }

    async fn list_docker_containers(
        &self,
        request: Request<ListDockerContainersRequest>,
    ) -> Result<Response<ListDockerContainersResponse>, Status> {
        self.auth.verify_request(&request)?;
        let req = request.into_inner();
        let resp = docker::list_containers(req.include_all).await;
        Ok(Response::new(resp))
    }

    async fn control_docker_container(
        &self,
        request: Request<ControlDockerContainerRequest>,
    ) -> Result<Response<ControlDockerContainerResponse>, Status> {
        self.auth.verify_request(&request)?;
        let req = request.into_inner();
        let (success, error_message) =
            match docker::control_container(&req.container_id, &req.action, req.force).await {
                Ok(()) => (true, String::new()),
                Err(e) => (false, e),
            };
        Ok(Response::new(ControlDockerContainerResponse {
            success,
            error_message,
        }))
    }

    async fn exec_docker_command(
        &self,
        request: Request<ExecDockerCommandRequest>,
    ) -> Result<Response<ExecDockerCommandResponse>, Status> {
        self.auth.verify_request(&request)?;
        let req = request.into_inner();
        let resp = docker::exec_command(&req.container_id, &req.command).await;
        Ok(Response::new(resp))
    }

    type StreamDockerConsoleStream = ReceiverStream<Result<ConsoleChunk, Status>>;

    async fn stream_docker_console(
        &self,
        request: Request<Streaming<DockerConsoleInput>>,
    ) -> Result<Response<Self::StreamDockerConsoleStream>, Status> {
        self.auth.verify_request(&request)?;
        docker::handle_stream_docker_console(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{compute_challenge_hmac, derive_psk_key};
    use std::str::FromStr;
    use tonic::metadata::MetadataValue;

    async fn authenticate_service(
        service: &NtxAgentService,
        psk: &str,
        viewer_id: &str,
    ) -> String {
        let chal = service
            .get_auth_challenge(Request::new(AuthChallengeRequest {
                viewer_id: viewer_id.to_string(),
            }))
            .await
            .unwrap()
            .into_inner();

        let hmac = compute_challenge_hmac(psk, &chal.nonce, viewer_id);
        let auth_res = service
            .authenticate(Request::new(AuthRequest {
                psk_hash: String::new(),
                viewer_id: viewer_id.to_string(),
                challenge_id: chal.challenge_id,
                response_hmac: hmac,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(auth_res.success);
        auth_res.session_token
    }

    #[tokio::test]
    async fn test_ping() {
        let config = Config::default();
        let auth = AuthManager::new(&config.server.psk, 3600);
        let service = NtxAgentService::new(config, auth);

        let resp = service
            .ping(Request::new(Empty {}))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp.hostname.is_empty());
        assert_eq!(resp.version, env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn test_authenticate_challenge_success_and_failure() {
        let psk = "super_secret_test_key";
        let mut config = Config::default();
        config.server.psk = psk.to_string();

        let auth = AuthManager::new(psk, 3600);
        let service = NtxAgentService::new(config, auth);

        // Success via challenge-response
        let session_token = authenticate_service(&service, psk, "viewer-123").await;
        assert!(!session_token.is_empty());

        // Failure with bad HMAC
        let chal = service
            .get_auth_challenge(Request::new(AuthChallengeRequest {
                viewer_id: "viewer-123".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        let bad_res = service
            .authenticate(Request::new(AuthRequest {
                psk_hash: String::new(),
                viewer_id: "viewer-123".to_string(),
                challenge_id: chal.challenge_id,
                response_hmac: "00".repeat(32),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!bad_res.success);
        assert!(bad_res.session_token.is_empty());

        // Legacy raw psk_hash without challenge must be rejected
        let legacy_res = service
            .authenticate(Request::new(AuthRequest {
                psk_hash: hex::encode(derive_psk_key(psk)),
                viewer_id: "viewer-123".to_string(),
                challenge_id: String::new(),
                response_hmac: String::new(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!legacy_res.success);
    }

    #[tokio::test]
    async fn test_get_system_info_auth() {
        let psk = "test_key_info";
        let mut config = Config::default();
        config.server.psk = psk.to_string();

        let auth = AuthManager::new(psk, 3600);
        let service = NtxAgentService::new(config, auth);

        // 1. Unauthenticated -> fails
        assert!(service.get_system_info(Request::new(Empty {})).await.is_err());

        // 2. Authenticate to get session token
        let session_token = authenticate_service(&service, psk, "viewer-1").await;

        // 3. Raw psk_hash in header -> fails
        let mut req_with_raw_hash = Request::new(Empty {});
        req_with_raw_hash.metadata_mut().insert(
            crate::auth::AUTH_HEADER,
            MetadataValue::from_str(&hex::encode(derive_psk_key(psk))).unwrap(),
        );
        assert!(service.get_system_info(req_with_raw_hash).await.is_err());

        // 4. With valid session token -> succeeds
        let mut req_with_token = Request::new(Empty {});
        req_with_token.metadata_mut().insert(
            crate::auth::AUTH_HEADER,
            MetadataValue::from_str(&session_token).unwrap(),
        );
        let sys_info = service
            .get_system_info(req_with_token)
            .await
            .unwrap()
            .into_inner();
        assert!(!sys_info.hostname.is_empty());
        assert!(sys_info.cpu_count > 0);
    }

    #[tokio::test]
    async fn test_stream_metrics() {
        use tokio_stream::StreamExt;

        let psk = "test_metrics_key";
        let mut config = Config::default();
        config.server.psk = psk.to_string();

        let auth = AuthManager::new(psk, 3600);
        let service = NtxAgentService::new(config, auth);
        let session_token = authenticate_service(&service, psk, "viewer-m").await;

        let mut req = Request::new(MetricsRequest { interval_ms: 250 });
        req.metadata_mut().insert(
            crate::auth::AUTH_HEADER,
            MetadataValue::from_str(&session_token).unwrap(),
        );

        let response = service.stream_metrics(req).await.unwrap();
        let mut stream = response.into_inner();

        let first_metric = tokio::time::timeout(Duration::from_secs(3), stream.next())
            .await
            .expect("metric timeout")
            .expect("metric stream open")
            .expect("valid metric");

        assert!(first_metric.total_memory_bytes > 0);
        assert!(first_metric.timestamp_ms > 0);
    }

    #[tokio::test]
    async fn test_list_processes_and_kill_auth() {
        let psk = "test_key_proc";
        let mut config = Config::default();
        config.server.psk = psk.to_string();

        let auth = AuthManager::new(psk, 3600);
        let service = NtxAgentService::new(config, auth);
        let session_token = authenticate_service(&service, psk, "viewer-p").await;

        assert!(service
            .list_processes(Request::new(ListProcessesRequest {}))
            .await
            .is_err());

        let mut req = Request::new(ListProcessesRequest {});
        req.metadata_mut().insert(
            crate::auth::AUTH_HEADER,
            MetadataValue::from_str(&session_token).unwrap(),
        );
        let resp = service.list_processes(req).await.unwrap().into_inner();
        assert!(!resp.processes.is_empty());
        let current_pid = std::process::id();
        assert!(resp.processes.iter().any(|p| p.pid == current_pid));

        // Kill protected self PID -> failure response
        let mut kill_self = Request::new(KillProcessRequest {
            pid: current_pid,
            signal: 9,
        });
        kill_self.metadata_mut().insert(
            crate::auth::AUTH_HEADER,
            MetadataValue::from_str(&session_token).unwrap(),
        );
        let kill_resp = service.kill_process(kill_self).await.unwrap().into_inner();
        assert!(!kill_resp.success);
        assert!(!kill_resp.error_message.is_empty());
    }

    #[tokio::test]
    async fn test_list_and_control_services_auth() {
        let psk = "test_key_svc";
        let mut config = Config::default();
        config.server.psk = psk.to_string();

        let auth = AuthManager::new(psk, 3600);
        let service = NtxAgentService::new(config, auth);
        let session_token = authenticate_service(&service, psk, "viewer-s").await;

        assert!(service
            .list_services(Request::new(ListServicesRequest {}))
            .await
            .is_err());

        let mut req = Request::new(ListServicesRequest {});
        req.metadata_mut().insert(
            crate::auth::AUTH_HEADER,
            MetadataValue::from_str(&session_token).unwrap(),
        );
        let svc_res = service.list_services(req).await;
        #[cfg(not(target_os = "linux"))]
        assert_eq!(svc_res.unwrap_err().code(), tonic::Code::Unavailable);
        #[cfg(target_os = "linux")]
        let _ = svc_res;

        let mut ctrl_req = Request::new(ControlServiceRequest {
            service_name: "--bad".to_string(),
            action: "start".to_string(),
        });
        ctrl_req.metadata_mut().insert(
            crate::auth::AUTH_HEADER,
            MetadataValue::from_str(&session_token).unwrap(),
        );
        let ctrl_resp = service.control_service(ctrl_req).await.unwrap().into_inner();
        assert!(!ctrl_resp.success);
        assert!(!ctrl_resp.error_message.is_empty());
    }

    fn auth_req<T>(payload: T, session_token: &str) -> Request<T> {
        let mut req = Request::new(payload);
        req.metadata_mut().insert(
            crate::auth::AUTH_HEADER,
            MetadataValue::from_str(session_token).unwrap(),
        );
        req
    }

    #[tokio::test]
    async fn test_managed_services_rpcs_auth_and_lifecycle() {
        use ntx_proto::ManagedServiceConfig;
        use tokio_stream::StreamExt;

        let psk = "test_key_managed";
        let mut config = Config::default();
        config.server.psk = psk.to_string();

        let auth = AuthManager::new(psk, 3600);
        let service = NtxAgentService::new(config, auth);

        // 1. All 6 Managed Services RPCs reject unauthenticated requests.
        assert!(service
            .list_managed_services(Request::new(ListManagedServicesRequest {}))
            .await
            .is_err());
        assert!(service
            .save_managed_service(Request::new(SaveManagedServiceRequest { config: None }))
            .await
            .is_err());
        assert!(service
            .delete_managed_service(Request::new(DeleteManagedServiceRequest {
                id: "svc1".to_string(),
            }))
            .await
            .is_err());
        assert!(service
            .control_managed_service(Request::new(ControlManagedServiceRequest {
                id: "svc1".to_string(),
                action: "start".to_string(),
            }))
            .await
            .is_err());
        assert!(service
            .send_managed_service_input(Request::new(SendManagedServiceInputRequest {
                id: "svc1".to_string(),
                input: "list".to_string(),
                append_newline: true,
            }))
            .await
            .is_err());
        assert!(service
            .stream_managed_service_console(Request::new(StreamManagedServiceConsoleRequest {
                id: "svc1".to_string(),
                tail_lines: 50,
            }))
            .await
            .is_err());

        let token = authenticate_service(&service, psk, "viewer-ms").await;

        // Missing config -> graceful failure response.
        let missing_resp = service
            .save_managed_service(auth_req(SaveManagedServiceRequest { config: None }, &token))
            .await
            .unwrap()
            .into_inner();
        assert!(!missing_resp.success);
        assert!(!missing_resp.error_message.is_empty());

        let temp_dir = std::env::temp_dir().join(format!(
            "ntx_srv_test_ms_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        #[cfg(unix)]
        let cmd =
            r#"echo READY; while IFS= read -r line; do echo "ECHO:$line"; [ "$line" = "quit" ] && exit 0; done"#
                .to_string();
        #[cfg(windows)]
        let cmd = {
            let script = temp_dir.join("srv_echo.cmd");
            std::fs::write(
                &script,
                "@echo off\r\necho READY\r\n:loop\r\nset \"line=\"\r\nset /p \"line=\"\r\necho ECHO:%line%\r\nif \"%line%\"==\"quit\" exit /b 0\r\ngoto loop\r\n",
            )
            .unwrap();
            script.to_string_lossy().to_string()
        };

        let cfg = ManagedServiceConfig {
            id: "srv-echo".to_string(),
            name: "Server Echo".to_string(),
            icon: "⚙".to_string(),
            description: "Test RPC".to_string(),
            command: cmd,
            working_dir: temp_dir.to_string_lossy().to_string(),
            stop_command: "quit".to_string(),
            auto_start: false,
            auto_restart: false,
            env_vars: vec![],
            quick_commands: vec!["status".to_string()],
            systemd_unit: String::new(),
            attach_command: String::new(),
        };

        // 2. Save -> List -> Stream -> Start -> Input -> Stop -> Delete.
        let save_resp = service
            .save_managed_service(auth_req(
                SaveManagedServiceRequest { config: Some(cfg) },
                &token,
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(save_resp.success, "save failed: {}", save_resp.error_message);
        assert_eq!(save_resp.service.unwrap().id, "srv-echo");

        let list_resp = service
            .list_managed_services(auth_req(ListManagedServicesRequest {}, &token))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(list_resp.services.len(), 1);
        assert_eq!(list_resp.services[0].state, "stopped");

        let mut console_stream = service
            .stream_managed_service_console(auth_req(
                StreamManagedServiceConsoleRequest {
                    id: "srv-echo".to_string(),
                    tail_lines: 50,
                },
                &token,
            ))
            .await
            .unwrap()
            .into_inner();

        let start_resp = service
            .control_managed_service(auth_req(
                ControlManagedServiceRequest {
                    id: "srv-echo".to_string(),
                    action: "start".to_string(),
                },
                &token,
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(start_resp.success, "start failed: {}", start_resp.error_message);

        let mut output = String::new();
        let ready_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !output.contains("READY") {
            let rem = ready_deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!rem.is_zero(), "Timed out waiting for READY; got: {:?}", output);
            let chunk = tokio::time::timeout(rem, console_stream.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            output.push_str(&String::from_utf8_lossy(&chunk.data));
        }

        let input_resp = service
            .send_managed_service_input(auth_req(
                SendManagedServiceInputRequest {
                    id: "srv-echo".to_string(),
                    input: "ping_srv".to_string(),
                    append_newline: true,
                },
                &token,
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(input_resp.success, "send_input failed: {}", input_resp.error_message);

        let echo_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !output.contains("ECHO:ping_srv") {
            let rem = echo_deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(
                !rem.is_zero(),
                "Timed out waiting for ECHO:ping_srv; got: {:?}",
                output
            );
            let chunk = tokio::time::timeout(rem, console_stream.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            output.push_str(&String::from_utf8_lossy(&chunk.data));
        }

        let stop_resp = service
            .control_managed_service(auth_req(
                ControlManagedServiceRequest {
                    id: "srv-echo".to_string(),
                    action: "stop".to_string(),
                },
                &token,
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(stop_resp.success);

        let del_resp = service
            .delete_managed_service(auth_req(
                DeleteManagedServiceRequest {
                    id: "srv-echo".to_string(),
                },
                &token,
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(del_resp.success);

        let list_after = service
            .list_managed_services(auth_req(ListManagedServicesRequest {}, &token))
            .await
            .unwrap()
            .into_inner();
        assert!(list_after.services.is_empty());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_docker_rpcs_auth_and_validation() {
        let psk = "test_key_docker";
        let mut config = Config::default();
        config.server.psk = psk.to_string();

        let auth = AuthManager::new(psk, 3600);
        let service = NtxAgentService::new(config, auth);

        // Unauthenticated requests must be rejected.
        assert!(service
            .list_docker_containers(Request::new(ListDockerContainersRequest {
                include_all: true,
            }))
            .await
            .is_err());
        assert!(service
            .control_docker_container(Request::new(ControlDockerContainerRequest {
                container_id: "abc123def456".to_string(),
                action: "restart".to_string(),
                force: false,
            }))
            .await
            .is_err());
        assert!(service
            .exec_docker_command(Request::new(ExecDockerCommandRequest {
                container_id: "abc123def456".to_string(),
                command: "echo hi".to_string(),
            }))
            .await
            .is_err());

        let token = authenticate_service(&service, psk, "viewer-docker").await;

        // ListDockerContainers succeeds cleanly whether Docker is installed or not.
        let list_resp = service
            .list_docker_containers(auth_req(
                ListDockerContainersRequest { include_all: true },
                &token,
            ))
            .await
            .unwrap()
            .into_inner();
        if !list_resp.docker_available {
            assert!(!list_resp.error_message.is_empty());
        }

        // ControlDockerContainer rejects invalid container ID and disallowed actions.
        let bad_id_resp = service
            .control_docker_container(auth_req(
                ControlDockerContainerRequest {
                    container_id: "--privileged".to_string(),
                    action: "start".to_string(),
                    force: false,
                },
                &token,
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(!bad_id_resp.success);
        assert!(!bad_id_resp.error_message.is_empty());

        let bad_act_resp = service
            .control_docker_container(auth_req(
                ControlDockerContainerRequest {
                    container_id: "valid-container".to_string(),
                    action: "prune".to_string(),
                    force: false,
                },
                &token,
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(!bad_act_resp.success);
        assert!(!bad_act_resp.error_message.is_empty());

        // ExecDockerCommand rejects bad container ID and empty command.
        let bad_exec_id = service
            .exec_docker_command(auth_req(
                ExecDockerCommandRequest {
                    container_id: "bad;id".to_string(),
                    command: "ls".to_string(),
                },
                &token,
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(!bad_exec_id.success);
        assert!(!bad_exec_id.error_message.is_empty());

        let bad_exec_cmd = service
            .exec_docker_command(auth_req(
                ExecDockerCommandRequest {
                    container_id: "valid-container".to_string(),
                    command: "   ".to_string(),
                },
                &token,
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(!bad_exec_cmd.success);
        assert!(!bad_exec_cmd.error_message.is_empty());
    }
}
