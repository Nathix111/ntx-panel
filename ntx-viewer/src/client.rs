use anyhow::Context;
use ntx_proto::ntx_agent_client::NtxAgentClient;
use ntx_proto::*;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;
use tonic::Request;
use tracing::{debug, info};

pub struct NtxClient {
    client: NtxAgentClient<Channel>,
    auth_token: Option<String>,
    psk_hash: Option<String>,
}

impl NtxClient {
    /// Connect to the gRPC server at the given endpoint (e.g. `http://192.168.1.10:9850`).
    pub async fn connect(endpoint: &str) -> anyhow::Result<Self> {
        let endpoint_str = if !endpoint.starts_with("http://") && !endpoint.starts_with("https://")
        {
            format!("http://{}", endpoint)
        } else {
            endpoint.to_string()
        };

        info!("Connecting to gRPC endpoint: {}", endpoint_str);

        let channel = Channel::from_shared(endpoint_str.clone())
            .with_context(|| format!("Invalid URI: {}", endpoint_str))?
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .connect()
            .await
            .with_context(|| format!("Failed to connect to {}", endpoint_str))?;

        let client = NtxAgentClient::new(channel);

        Ok(Self {
            client,
            auth_token: None,
            psk_hash: None,
        })
    }

    /// Helper to attach auth metadata (session_token or psk_hash) to any request.
    fn make_request<T>(&self, message: T) -> Request<T> {
        let mut req = Request::new(message);
        let token = self.auth_token.as_ref().or(self.psk_hash.as_ref());
        if let Some(t) = token {
            if let Ok(header_val) = t.parse() {
                req.metadata_mut().insert("x-ntx-auth", header_val);
            }
        }
        req
    }

    /// Authenticate with the remote agent using a PSK.
    pub async fn authenticate(&mut self, psk: &str, viewer_id: &str) -> anyhow::Result<String> {
        let hash = crate::auth::hash_psk(psk);
        self.psk_hash = Some(hash.clone());

        let req = self.make_request(AuthRequest {
            psk_hash: hash.clone(),
            viewer_id: viewer_id.to_string(),
        });

        debug!("Sending Authenticate RPC with psk_hash={}", hash);
        let response = self
            .client
            .authenticate(req)
            .await
            .context("Authenticate RPC failed")?
            .into_inner();

        if !response.success {
            let msg = if response.error_message.is_empty() {
                "Authentication rejected by server (invalid PSK)".to_string()
            } else {
                response.error_message
            };
            anyhow::bail!(msg);
        }

        let token = if !response.session_token.is_empty() {
            response.session_token
        } else {
            hash
        };

        self.auth_token = Some(token.clone());
        info!("Authentication successful!");
        Ok(token)
    }

    /// Ping the agent to check health and status.
    pub async fn ping(&mut self) -> anyhow::Result<PingResponse> {
        let req = self.make_request(Empty {});
        let resp = self
            .client
            .ping(req)
            .await
            .context("Ping RPC failed")?
            .into_inner();
        Ok(resp)
    }

    /// Query static system information (OS, CPU count, memory, disks, network interfaces).
    pub async fn get_system_info(&mut self) -> anyhow::Result<SystemInfo> {
        let req = self.make_request(Empty {});
        let resp = self
            .client
            .get_system_info(req)
            .await
            .context("GetSystemInfo RPC failed")?
            .into_inner();
        Ok(resp)
    }

    /// Request a live streaming metrics feed from the agent.
    pub async fn stream_metrics(
        &mut self,
        interval_ms: u32,
    ) -> anyhow::Result<tonic::Streaming<SystemMetrics>> {
        let req = self.make_request(MetricsRequest { interval_ms });
        let resp = self
            .client
            .stream_metrics(req)
            .await
            .context("StreamMetrics RPC failed")?
            .into_inner();
        Ok(resp)
    }

    /// Open a bidirectional interactive terminal session.
    pub async fn open_terminal(
        &mut self,
        rx: mpsc::Receiver<TerminalInput>,
    ) -> anyhow::Result<tonic::Streaming<TerminalOutput>> {
        let stream = ReceiverStream::new(rx);
        let req = self.make_request(stream);
        let resp = self
            .client
            .open_terminal(req)
            .await
            .context("OpenTerminal RPC failed")?
            .into_inner();
        Ok(resp)
    }

    /// List running processes on the remote agent.
    pub async fn list_processes(&mut self) -> anyhow::Result<Vec<ProcessItem>> {
        let req = self.make_request(ListProcessesRequest {});
        let resp = self
            .client
            .list_processes(req)
            .await
            .context("ListProcesses RPC failed")?
            .into_inner();
        Ok(resp.processes)
    }

    /// Kill a process by PID with a given signal (e.g. 15 for SIGTERM, 9 for SIGKILL).
    pub async fn kill_process(
        &mut self,
        pid: u32,
        signal: i32,
    ) -> anyhow::Result<KillProcessResponse> {
        let req = self.make_request(KillProcessRequest { pid, signal });
        let resp = self
            .client
            .kill_process(req)
            .await
            .context("KillProcess RPC failed")?
            .into_inner();
        Ok(resp)
    }

    /// List system services on the remote agent.
    pub async fn list_services(&mut self) -> anyhow::Result<Vec<ServiceItem>> {
        let req = self.make_request(ListServicesRequest {});
        let resp = self
            .client
            .list_services(req)
            .await
            .context("ListServices RPC failed")?
            .into_inner();
        Ok(resp.services)
    }

    /// Control a system service (e.g. "start", "stop", "restart").
    pub async fn control_service(
        &mut self,
        service_name: &str,
        action: &str,
    ) -> anyhow::Result<ControlServiceResponse> {
        let req = self.make_request(ControlServiceRequest {
            service_name: service_name.to_string(),
            action: action.to_string(),
        });
        let resp = self
            .client
            .control_service(req)
            .await
            .context("ControlService RPC failed")?
            .into_inner();
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ntx_proto::ntx_agent_server::{NtxAgent, NtxAgentServer};
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::transport::Server;
    use tonic::{Request, Response, Status};

    struct MockAgent {
        expected_psk_hash: String,
        session_token: String,
    }

    #[tonic::async_trait]
    impl NtxAgent for MockAgent {
        async fn ping(&self, _request: Request<Empty>) -> Result<Response<PingResponse>, Status> {
            Ok(Response::new(PingResponse {
                hostname: "mock-host".to_string(),
                uptime_seconds: 42,
                version: "0.1.0".to_string(),
            }))
        }

        async fn authenticate(
            &self,
            request: Request<AuthRequest>,
        ) -> Result<Response<AuthResponse>, Status> {
            let req = request.into_inner();
            if req.psk_hash == self.expected_psk_hash {
                Ok(Response::new(AuthResponse {
                    success: true,
                    session_token: self.session_token.clone(),
                    error_message: String::new(),
                }))
            } else {
                Ok(Response::new(AuthResponse {
                    success: false,
                    session_token: String::new(),
                    error_message: "Invalid PSK hash".to_string(),
                }))
            }
        }

        async fn get_system_info(
            &self,
            _request: Request<Empty>,
        ) -> Result<Response<SystemInfo>, Status> {
            Ok(Response::new(SystemInfo {
                hostname: "mock-host".to_string(),
                os_name: "TestOS".to_string(),
                os_version: "1.0".to_string(),
                arch: "x86_64".to_string(),
                total_memory_bytes: 8 * 1024 * 1024 * 1024,
                cpu_count: 4,
                disks: vec![],
                networks: vec![],
            }))
        }

        type StreamMetricsStream = tokio_stream::wrappers::ReceiverStream<Result<SystemMetrics, Status>>;

        async fn stream_metrics(
            &self,
            _request: Request<MetricsRequest>,
        ) -> Result<Response<Self::StreamMetricsStream>, Status> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
        }

        type OpenTerminalStream = tokio_stream::wrappers::ReceiverStream<Result<TerminalOutput, Status>>;

        async fn open_terminal(
            &self,
            _request: Request<tonic::Streaming<TerminalInput>>,
        ) -> Result<Response<Self::OpenTerminalStream>, Status> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
        }

        async fn list_processes(
            &self,
            _request: Request<ListProcessesRequest>,
        ) -> Result<Response<ListProcessesResponse>, Status> {
            let processes = vec![
                ProcessItem {
                    pid: 101,
                    name: "daemon-service".to_string(),
                    cpu_usage: 15.5,
                    memory_bytes: 128 * 1024 * 1024,
                    user: "root".to_string(),
                    status: "Running".to_string(),
                    command_line: "/usr/bin/daemon-service --config /etc/daemon.conf".to_string(),
                },
                ProcessItem {
                    pid: 202,
                    name: "worker-task".to_string(),
                    cpu_usage: 3.2,
                    memory_bytes: 64 * 1024 * 1024,
                    user: "app".to_string(),
                    status: "Sleeping".to_string(),
                    command_line: "/opt/worker-task".to_string(),
                },
            ];
            Ok(Response::new(ListProcessesResponse { processes }))
        }

        async fn kill_process(
            &self,
            request: Request<KillProcessRequest>,
        ) -> Result<Response<KillProcessResponse>, Status> {
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
            _request: Request<ListServicesRequest>,
        ) -> Result<Response<ListServicesResponse>, Status> {
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

    async fn start_mock_server(
        psk: &str,
        session_token: &str,
    ) -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let mock_agent = MockAgent {
            expected_psk_hash: crate::auth::hash_psk(psk),
            session_token: session_token.to_string(),
        };

        tokio::spawn(async move {
            Server::builder()
                .add_service(NtxAgentServer::new(mock_agent))
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        (addr, shutdown_tx)
    }

    #[tokio::test]
    async fn test_client_connect_and_ping() {
        let (addr, _shutdown) = start_mock_server("secret", "tok123").await;
        let mut client = NtxClient::connect(&addr.to_string()).await.unwrap();

        let ping = client.ping().await.unwrap();
        assert_eq!(ping.hostname, "mock-host");
        assert_eq!(ping.uptime_seconds, 42);
        assert_eq!(ping.version, "0.1.0");
    }

    #[tokio::test]
    async fn test_client_authenticate_flow() {
        let (addr, _shutdown) = start_mock_server("my_psk", "session_abc").await;
        let mut client = NtxClient::connect(&addr.to_string()).await.unwrap();

        // 1. Wrong PSK fails
        let err = client.authenticate("wrong_psk", "viewer-1").await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("Invalid PSK hash"));

        // 2. Correct PSK succeeds
        let token = client.authenticate("my_psk", "viewer-1").await.unwrap();
        assert_eq!(token, "session_abc");
        assert_eq!(client.auth_token.as_deref(), Some("session_abc"));
    }

    #[tokio::test]
    async fn test_client_list_processes() {
        let (addr, _shutdown) = start_mock_server("my_psk", "session_abc").await;
        let mut client = NtxClient::connect(&addr.to_string()).await.unwrap();
        client.authenticate("my_psk", "viewer-1").await.unwrap();

        let procs = client.list_processes().await.unwrap();
        assert_eq!(procs.len(), 2);
        assert_eq!(procs[0].pid, 101);
        assert_eq!(procs[0].name, "daemon-service");
        assert_eq!(procs[0].cpu_usage, 15.5);
        assert_eq!(procs[0].memory_bytes, 128 * 1024 * 1024);
        assert_eq!(procs[0].user, "root");
        assert_eq!(procs[0].status, "Running");
        assert_eq!(
            procs[0].command_line,
            "/usr/bin/daemon-service --config /etc/daemon.conf"
        );

        assert_eq!(procs[1].pid, 202);
        assert_eq!(procs[1].name, "worker-task");
    }

    #[tokio::test]
    async fn test_client_kill_process() {
        let (addr, _shutdown) = start_mock_server("my_psk", "session_abc").await;
        let mut client = NtxClient::connect(&addr.to_string()).await.unwrap();
        client.authenticate("my_psk", "viewer-1").await.unwrap();

        // Successful kill (PID 101, SIGTERM 15)
        let resp_ok = client.kill_process(101, 15).await.unwrap();
        assert!(resp_ok.success);
        assert!(resp_ok.error_message.is_empty());

        // Successful kill (PID 101, SIGKILL 9)
        let resp_kill = client.kill_process(101, 9).await.unwrap();
        assert!(resp_kill.success);
        assert!(resp_kill.error_message.is_empty());

        // Failed kill (nonexistent PID 999)
        let resp_err = client.kill_process(999, 15).await.unwrap();
        assert!(!resp_err.success);
        assert!(resp_err.error_message.contains("not found"));
    }

    #[tokio::test]
    async fn test_client_list_services() {
        let (addr, _shutdown) = start_mock_server("my_psk", "session_abc").await;
        let mut client = NtxClient::connect(&addr.to_string()).await.unwrap();
        client.authenticate("my_psk", "viewer-1").await.unwrap();

        let svcs = client.list_services().await.unwrap();
        assert_eq!(svcs.len(), 2);

        assert_eq!(svcs[0].name, "nginx.service");
        assert_eq!(svcs[0].description, "Nginx Web Server");
        assert_eq!(svcs[0].load_state, "loaded");
        assert_eq!(svcs[0].active_state, "active");
        assert_eq!(svcs[0].sub_state, "running");

        assert_eq!(svcs[1].name, "postgresql.service");
        assert_eq!(svcs[1].active_state, "inactive");
        assert_eq!(svcs[1].sub_state, "dead");
    }

    #[tokio::test]
    async fn test_client_control_service() {
        let (addr, _shutdown) = start_mock_server("my_psk", "session_abc").await;
        let mut client = NtxClient::connect(&addr.to_string()).await.unwrap();
        client.authenticate("my_psk", "viewer-1").await.unwrap();

        // Successful control action: restart nginx.service
        let resp_restart = client
            .control_service("nginx.service", "restart")
            .await
            .unwrap();
        assert!(resp_restart.success);
        assert!(resp_restart.error_message.is_empty());

        // Successful control action: stop nginx.service
        let resp_stop = client
            .control_service("nginx.service", "stop")
            .await
            .unwrap();
        assert!(resp_stop.success);

        // Failed control action: unknown action
        let resp_invalid = client
            .control_service("nginx.service", "destroy")
            .await
            .unwrap();
        assert!(!resp_invalid.success);
        assert!(resp_invalid.error_message.contains("failed on service"));

        // Failed control action: unknown service
        let resp_unknown = client
            .control_service("unknown.service", "start")
            .await
            .unwrap();
        assert!(!resp_unknown.success);
        assert!(resp_unknown.error_message.contains("failed on service"));
    }
}
