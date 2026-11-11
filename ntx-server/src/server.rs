#![allow(clippy::result_large_err)]

use crate::auth::AuthManager;
use crate::config::{get_hostname, Config};
use crate::monitoring::{self, collect_system_info};
use crate::terminal;
use ntx_proto::ntx_agent_server::NtxAgent;
use ntx_proto::{
    AuthRequest, AuthResponse, Empty, MetricsRequest, PingResponse, SystemInfo, SystemMetrics,
    TerminalInput, TerminalOutput,
};
use std::time::Instant;
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
}

#[tonic::async_trait]
impl NtxAgent for NtxAgentService {
    async fn authenticate(
        &self,
        request: Request<AuthRequest>,
    ) -> Result<Response<AuthResponse>, Status> {
        let req = request.into_inner();
        let psk_hash = req.psk_hash.trim();

        if self.auth.verify_psk_hash(psk_hash) {
            let session_token = self.auth.create_session();
            info!(
                viewer_id = %req.viewer_id,
                "Authentication successful, created new session"
            );
            Ok(Response::new(AuthResponse {
                success: true,
                session_token,
                error_message: String::new(),
            }))
        } else {
            warn!(
                viewer_id = %req.viewer_id,
                "Authentication failed: invalid PSK hash"
            );
            Ok(Response::new(AuthResponse {
                success: false,
                session_token: String::new(),
                error_message: "Invalid PSK hash".to_string(),
            }))
        }
    }

    async fn get_system_info(
        &self,
        request: Request<Empty>,
    ) -> Result<Response<SystemInfo>, Status> {
        self.auth.verify_request(&request)?;
        let info = collect_system_info(&self.config);
        Ok(Response::new(info))
    }

    type StreamMetricsStream = ReceiverStream<Result<SystemMetrics, Status>>;

    async fn stream_metrics(
        &self,
        request: Request<MetricsRequest>,
    ) -> Result<Response<Self::StreamMetricsStream>, Status> {
        self.auth.verify_request(&request)?;

        let interval_ms = request.into_inner().interval_ms.max(250);
        let (tx, rx) = mpsc::channel(32);

        tokio::spawn(async move {
            let mut sys = System::new_all();
            let mut disks = Disks::new_with_refreshed_list();
            let mut networks = Networks::new_with_refreshed_list();

            loop {
                let metrics = monitoring::collect_metrics(&mut sys, &mut disks, &mut networks);
                if tx.send(Ok(metrics)).await.is_err() {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(interval_ms as u64)).await;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::hash_psk;
    use std::str::FromStr;
    use tonic::metadata::MetadataValue;

    #[tokio::test]
    async fn test_ping() {
        let config = Config::default();
        let auth = AuthManager::new(&config.server.psk);
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
    async fn test_authenticate_success_and_failure() {
        let psk = "super_secret_test_key";
        let mut config = Config::default();
        config.server.psk = psk.to_string();

        let auth = AuthManager::new(psk);
        let service = NtxAgentService::new(config, auth);

        // Success
        let valid_hash = hash_psk(psk);
        let auth_req = Request::new(AuthRequest {
            psk_hash: valid_hash,
            viewer_id: "viewer-123".to_string(),
        });
        let auth_res = service.authenticate(auth_req).await.unwrap().into_inner();
        assert!(auth_res.success);
        assert!(!auth_res.session_token.is_empty());

        // Failure
        let bad_req = Request::new(AuthRequest {
            psk_hash: "wrong_hash".to_string(),
            viewer_id: "viewer-123".to_string(),
        });
        let bad_res = service.authenticate(bad_req).await.unwrap().into_inner();
        assert!(!bad_res.success);
        assert!(bad_res.session_token.is_empty());
    }

    #[tokio::test]
    async fn test_get_system_info_auth() {
        let psk = "test_key_info";
        let mut config = Config::default();
        config.server.psk = psk.to_string();

        let auth = AuthManager::new(psk);
        let service = NtxAgentService::new(config, auth);

        // 1. Unauthenticated -> fails
        let unauth_req = Request::new(Empty {});
        assert!(service.get_system_info(unauth_req).await.is_err());

        // 2. Authenticate to get session token
        let valid_hash = hash_psk(psk);
        let auth_res = service
            .authenticate(Request::new(AuthRequest {
                psk_hash: valid_hash,
                viewer_id: "viewer-1".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();

        // 3. With session token -> succeeds
        let mut req_with_token = Request::new(Empty {});
        req_with_token.metadata_mut().insert(
            crate::auth::AUTH_HEADER,
            MetadataValue::from_str(&auth_res.session_token).unwrap(),
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

        let auth = AuthManager::new(psk);
        let service = NtxAgentService::new(config, auth);

        let mut req = Request::new(MetricsRequest { interval_ms: 250 });
        let psk_hash = hash_psk(psk);
        req.metadata_mut().insert(
            crate::auth::AUTH_HEADER,
            MetadataValue::from_str(&psk_hash).unwrap(),
        );

        let response = service.stream_metrics(req).await.unwrap();
        let mut stream = response.into_inner();

        let first_metric = tokio::time::timeout(tokio::time::Duration::from_secs(3), stream.next())
            .await
            .expect("metric timeout")
            .expect("metric stream open")
            .expect("valid metric");

        assert!(first_metric.total_memory_bytes > 0);
        assert!(first_metric.timestamp_ms > 0);
    }
}
