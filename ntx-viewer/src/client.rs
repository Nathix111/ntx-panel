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
}
