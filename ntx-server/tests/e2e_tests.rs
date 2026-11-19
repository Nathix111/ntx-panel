#![allow(clippy::result_large_err)]

use ntx_proto::ntx_agent_client::NtxAgentClient;
use ntx_proto::ntx_agent_server::NtxAgentServer;
use ntx_proto::terminal_input::Input;
use ntx_proto::{
    AuthChallengeRequest, AuthRequest, ControlServiceRequest, Empty, KillProcessRequest,
    ListProcessesRequest, ListServicesRequest, MetricsRequest, TerminalInput, TerminalResize,
};
use ntx_server::auth::{compute_challenge_hmac, derive_psk_key, AuthManager, AUTH_HEADER};
use ntx_server::config::Config;
use ntx_server::server::NtxAgentService;
use ntx_server::tls::{self, TlsMaterial};
use std::net::SocketAddr;
use std::str::FromStr;
use tokio::net::TcpListener;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tokio_stream::StreamExt;
use tonic::metadata::MetadataValue;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Server};
use tonic::{Code, Request};

async fn start_test_server(
    psk: &str,
) -> (SocketAddr, TlsMaterial, tokio::sync::oneshot::Sender<()>) {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut config = Config::default();
    config.server.psk = psk.to_string();

    let temp_dir =
        std::env::temp_dir().join(format!("ntx_e2e_tls_{}", uuid::Uuid::new_v4().simple()));
    let tls_material = tls::load_or_generate_tls(&temp_dir, "localhost").unwrap();
    let server_tls = tls::server_tls_config(&tls_material).unwrap();
    let _ = std::fs::remove_dir_all(&temp_dir);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let auth_manager = AuthManager::new(psk, 3600);
    let agent_service = NtxAgentService::new(config, auth_manager.clone());

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let auth_interceptor = auth_manager.clone();
    let interceptor = move |req: Request<()>| auth_interceptor.intercept(req);

    tokio::spawn(async move {
        Server::builder()
            .tls_config(server_tls)
            .unwrap()
            .add_service(NtxAgentServer::with_interceptor(agent_service, interceptor))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });

    (addr, tls_material, shutdown_tx)
}

async fn connect_tls_client(addr: SocketAddr, tls_material: &TlsMaterial) -> NtxAgentClient<Channel> {
    let url = format!("https://{}", addr);
    let tls_cfg = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(&tls_material.cert_pem))
        .domain_name("localhost");

    let channel = Endpoint::from_shared(url)
        .unwrap()
        .tls_config(tls_cfg)
        .unwrap()
        .connect()
        .await
        .expect("Failed to connect TLS client");

    NtxAgentClient::new(channel)
}

async fn authenticate_client(
    client: &mut NtxAgentClient<Channel>,
    psk: &str,
    viewer_id: &str,
) -> String {
    let chal = client
        .get_auth_challenge(Request::new(AuthChallengeRequest {
            viewer_id: viewer_id.to_string(),
        }))
        .await
        .expect("GetAuthChallenge should succeed")
        .into_inner();
    assert!(!chal.challenge_id.is_empty());
    assert_eq!(chal.nonce.len(), 32);

    let hmac = compute_challenge_hmac(psk, &chal.nonce, viewer_id);
    let auth_resp = client
        .authenticate(Request::new(AuthRequest {
            psk_hash: String::new(),
            viewer_id: viewer_id.to_string(),
            challenge_id: chal.challenge_id,
            response_hmac: hmac,
        }))
        .await
        .expect("Authenticate should succeed")
        .into_inner();

    assert!(auth_resp.success);
    assert!(!auth_resp.session_token.is_empty());
    assert!(auth_resp.expires_in_seconds >= 60);
    auth_resp.session_token
}

#[tokio::test]
async fn test_full_grpc_tls_e2e_lifecycle() {
    let psk = "ntx_test_secret_key_12345";
    let (addr, tls_material, _shutdown) = start_test_server(psk).await;
    let mut client = connect_tls_client(addr, &tls_material).await;

    // 1. Ping works unauthenticated
    let ping_resp = client
        .ping(Request::new(Empty {}))
        .await
        .expect("Ping should succeed unauthenticated")
        .into_inner();
    assert!(!ping_resp.hostname.is_empty());
    assert_eq!(ping_resp.version, env!("CARGO_PKG_VERSION"));

    // 2. GetSystemInfo without auth fails
    let sys_err = client.get_system_info(Request::new(Empty {})).await;
    assert!(sys_err.is_err(), "GetSystemInfo without auth should fail");

    // 3. Challenge with bad HMAC fails
    let chal_bad = client
        .get_auth_challenge(Request::new(AuthChallengeRequest {
            viewer_id: "viewer-e2e".to_string(),
        }))
        .await
        .unwrap()
        .into_inner();
    let bad_auth = client
        .authenticate(Request::new(AuthRequest {
            psk_hash: String::new(),
            viewer_id: "viewer-e2e".to_string(),
            challenge_id: chal_bad.challenge_id.clone(),
            response_hmac: "00".repeat(32),
        }))
        .await
        .expect("Authenticate call should return response")
        .into_inner();
    assert!(!bad_auth.success);
    assert!(bad_auth.session_token.is_empty());

    // 4. Replaying consumed challenge even with valid HMAC fails
    let replay_hmac = compute_challenge_hmac(psk, &chal_bad.nonce, "viewer-e2e");
    let replay_auth = client
        .authenticate(Request::new(AuthRequest {
            psk_hash: String::new(),
            viewer_id: "viewer-e2e".to_string(),
            challenge_id: chal_bad.challenge_id,
            response_hmac: replay_hmac,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(!replay_auth.success);

    // 5. Authenticate properly via Challenge-Response
    let session_token = authenticate_client(&mut client, psk, "viewer-e2e").await;

    // 6. Raw psk_hash in metadata header must be rejected
    let raw_psk_hash = hex::encode(derive_psk_key(psk));
    let mut raw_hash_req = Request::new(Empty {});
    raw_hash_req.metadata_mut().insert(
        AUTH_HEADER,
        MetadataValue::from_str(&raw_psk_hash).unwrap(),
    );
    assert!(client.get_system_info(raw_hash_req).await.is_err());

    // 7. GetSystemInfo with session token succeeds
    let mut info_req = Request::new(Empty {});
    info_req.metadata_mut().insert(
        AUTH_HEADER,
        MetadataValue::from_str(&session_token).unwrap(),
    );
    let sys_info = client
        .get_system_info(info_req)
        .await
        .expect("GetSystemInfo with session token should succeed")
        .into_inner();
    assert!(!sys_info.hostname.is_empty());
    assert!(sys_info.cpu_count > 0);
    assert!(sys_info.total_memory_bytes > 0);

    // 8. StreamMetrics with session token yields >= 2 ticks
    let mut metrics_req = Request::new(MetricsRequest { interval_ms: 250 });
    metrics_req
        .metadata_mut()
        .insert(AUTH_HEADER, MetadataValue::from_str(&session_token).unwrap());
    let mut metrics_stream = client
        .stream_metrics(metrics_req)
        .await
        .expect("StreamMetrics should succeed")
        .into_inner();

    let first_metric =
        tokio::time::timeout(tokio::time::Duration::from_secs(3), metrics_stream.next())
            .await
            .expect("Timeout waiting for metric 1")
            .expect("Metrics stream closed")
            .expect("Error in metrics item 1");
    assert!(first_metric.total_memory_bytes > 0);
    assert!(first_metric.timestamp_ms > 0);

    let second_metric =
        tokio::time::timeout(tokio::time::Duration::from_secs(3), metrics_stream.next())
            .await
            .expect("Timeout waiting for metric 2")
            .expect("Metrics stream closed")
            .expect("Error in metrics item 2");
    assert!(second_metric.timestamp_ms >= first_metric.timestamp_ms);

    // 9. OpenTerminal with auth, resize, echo, and clean exit
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let in_stream = ReceiverStream::new(in_rx);

    let mut term_req = Request::new(in_stream);
    term_req.metadata_mut().insert(
        AUTH_HEADER,
        MetadataValue::from_str(&session_token).unwrap(),
    );

    let mut out_stream = client
        .open_terminal(term_req)
        .await
        .expect("OpenTerminal should succeed")
        .into_inner();

    in_tx
        .send(TerminalInput {
            input: Some(Input::Resize(TerminalResize {
                cols: 100,
                rows: 30,
            })),
        })
        .await
        .unwrap();

    in_tx
        .send(TerminalInput {
            input: Some(Input::Data(b"echo E2E_SHELL_TEST_OK\r\n".to_vec())),
        })
        .await
        .unwrap();

    let mut received_output = String::new();
    let start_time = tokio::time::Instant::now();
    while start_time.elapsed() < tokio::time::Duration::from_secs(5) {
        if let Ok(Some(Ok(chunk))) =
            tokio::time::timeout(tokio::time::Duration::from_millis(500), out_stream.next()).await
        {
            received_output.push_str(&String::from_utf8_lossy(&chunk.data));
            if received_output.contains("E2E_SHELL_TEST_OK") {
                break;
            }
        }
    }

    assert!(
        received_output.contains("E2E_SHELL_TEST_OK"),
        "Shell output did not contain echoed text. Received: {}",
        received_output
    );

    // Send exit to verify clean shell termination & stream EOF without zombie/hang
    let _ = in_tx
        .send(TerminalInput {
            input: Some(Input::Data(b"exit\r\n".to_vec())),
        })
        .await;
    drop(in_tx);

    let drain_res = tokio::time::timeout(tokio::time::Duration::from_secs(3), async {
        while let Some(Ok(_)) = out_stream.next().await {}
    })
    .await;
    assert!(drain_res.is_ok(), "Terminal stream should close cleanly after exit");

    // 10. ListProcesses with session token succeeds
    let mut proc_req = Request::new(ListProcessesRequest {});
    proc_req
        .metadata_mut()
        .insert(AUTH_HEADER, MetadataValue::from_str(&session_token).unwrap());
    let proc_resp = client
        .list_processes(proc_req)
        .await
        .expect("ListProcesses should succeed")
        .into_inner();
    assert!(!proc_resp.processes.is_empty());
    let current_pid = std::process::id();
    assert!(proc_resp.processes.iter().any(|p| p.pid == current_pid));

    // 11. KillProcess with protected self-PID fails gracefully
    let mut kill_self = Request::new(KillProcessRequest {
        pid: current_pid,
        signal: 9,
    });
    kill_self
        .metadata_mut()
        .insert(AUTH_HEADER, MetadataValue::from_str(&session_token).unwrap());
    let kill_self_resp = client.kill_process(kill_self).await.unwrap().into_inner();
    assert!(!kill_self_resp.success);
    assert!(!kill_self_resp.error_message.is_empty());

    // 12. ListServices behavior: Linux returns units, non-Linux returns Unavailable (no mocks)
    let mut svc_req = Request::new(ListServicesRequest {});
    svc_req
        .metadata_mut()
        .insert(AUTH_HEADER, MetadataValue::from_str(&session_token).unwrap());
    let svc_res = client.list_services(svc_req).await;
    #[cfg(not(target_os = "linux"))]
    assert_eq!(svc_res.unwrap_err().code(), Code::Unavailable);
    #[cfg(target_os = "linux")]
    let _ = svc_res;

    // 13. ControlService rejects invalid service names
    let mut ctrl_req = Request::new(ControlServiceRequest {
        service_name: "--evil".to_string(),
        action: "start".to_string(),
    });
    ctrl_req
        .metadata_mut()
        .insert(AUTH_HEADER, MetadataValue::from_str(&session_token).unwrap());
    let ctrl_resp = client
        .control_service(ctrl_req)
        .await
        .expect("ControlService RPC should return validation error")
        .into_inner();
    assert!(!ctrl_resp.success);
    assert!(!ctrl_resp.error_message.is_empty());
}

#[tokio::test]
async fn test_process_manager_e2e_lifecycle() {
    let psk = "ntx_test_proc_e2e_key";
    let (addr, tls_material, _shutdown) = start_test_server(psk).await;
    let mut client = connect_tls_client(addr, &tls_material).await;
    let session_token = authenticate_client(&mut client, psk, "proc-verifier").await;

    #[cfg(windows)]
    let mut child = std::process::Command::new("ping")
        .args(["127.0.0.1", "-n", "30"])
        .spawn()
        .expect("Failed to spawn ping child process");

    #[cfg(unix)]
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("Failed to spawn sleep child process");

    let child_pid = child.id();
    assert!(child_pid > 0);

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let mut req = Request::new(ListProcessesRequest {});
    req.metadata_mut().insert(
        AUTH_HEADER,
        MetadataValue::from_str(&session_token).unwrap(),
    );
    let proc_resp = client.list_processes(req).await.unwrap().into_inner();
    assert!(
        proc_resp.processes.iter().any(|p| p.pid == child_pid),
        "Spawned child PID {} should appear in process list",
        child_pid
    );

    let mut kill_req = Request::new(KillProcessRequest {
        pid: child_pid,
        signal: 9,
    });
    kill_req.metadata_mut().insert(
        AUTH_HEADER,
        MetadataValue::from_str(&session_token).unwrap(),
    );
    let kill_resp = client.kill_process(kill_req).await.unwrap().into_inner();
    assert!(kill_resp.success, "Killing child PID should succeed");

    let exit_status = child.wait().expect("Child process wait failed");
    assert!(!exit_status.success());

    let mut kill_dead_req = Request::new(KillProcessRequest {
        pid: child_pid,
        signal: 15,
    });
    kill_dead_req.metadata_mut().insert(
        AUTH_HEADER,
        MetadataValue::from_str(&session_token).unwrap(),
    );
    let kill_dead_resp = client.kill_process(kill_dead_req).await.unwrap().into_inner();
    assert!(!kill_dead_resp.success);
}

#[tokio::test]
async fn test_auth_rejection_and_rate_limiting_e2e() {
    let psk = "ntx_auth_test_key";
    let (addr, tls_material, _shutdown) = start_test_server(psk).await;
    let mut client = connect_tls_client(addr, &tls_material).await;

    // 1. Unauthenticated calls must return Err
    assert!(client
        .list_processes(Request::new(ListProcessesRequest {}))
        .await
        .is_err());
    assert!(client
        .kill_process(Request::new(KillProcessRequest {
            pid: 1,
            signal: 15,
        }))
        .await
        .is_err());
    assert!(client
        .list_services(Request::new(ListServicesRequest {}))
        .await
        .is_err());
    assert!(client
        .control_service(Request::new(ControlServiceRequest {
            service_name: "nginx.service".to_string(),
            action: "start".to_string(),
        }))
        .await
        .is_err());

    // 2. Calls with bogus token must return Err
    let bogus_token = "bogus_invalid_token_12345";
    let mut proc_req = Request::new(ListProcessesRequest {});
    proc_req
        .metadata_mut()
        .insert(AUTH_HEADER, MetadataValue::from_str(bogus_token).unwrap());
    assert!(client.list_processes(proc_req).await.is_err());

    // 3. 5 bad authentication attempts trigger ResourceExhausted rate limit
    for _ in 0..5 {
        let chal = client
            .get_auth_challenge(Request::new(AuthChallengeRequest {
                viewer_id: "attacker".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        let resp = client
            .authenticate(Request::new(AuthRequest {
                psk_hash: String::new(),
                viewer_id: "attacker".to_string(),
                challenge_id: chal.challenge_id,
                response_hmac: "ff".repeat(32),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp.success);
    }

    let rate_limited = client
        .get_auth_challenge(Request::new(AuthChallengeRequest {
            viewer_id: "attacker".to_string(),
        }))
        .await;
    assert_eq!(rate_limited.unwrap_err().code(), Code::ResourceExhausted);
}
