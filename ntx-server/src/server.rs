#![allow(clippy::result_large_err)]

use crate::auth::{AuthFailure, AuthManager};
use crate::config::{get_hostname, Config};
use crate::monitoring::{self, collect_system_info};
use crate::process;
use crate::service_manager;
use crate::terminal;
use ntx_proto::ntx_agent_server::NtxAgent;
use ntx_proto::{
    AuthChallengeRequest, AuthChallengeResponse, AuthRequest, AuthResponse, ControlServiceRequest,
    ControlServiceResponse, Empty, KillProcessRequest, KillProcessResponse, ListProcessesRequest,
    ListProcessesResponse, ListServicesRequest, ListServicesResponse, MetricsRequest, PingResponse,
    SystemInfo, SystemMetrics, TerminalInput, TerminalOutput,
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
    start_time: Instant,
}

impl NtxAgentService {
    pub fn new(config: Config, auth: AuthManager) -> Self {
        Self {
            config,
            auth,
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
}
