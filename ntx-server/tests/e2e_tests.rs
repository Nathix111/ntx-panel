#![allow(clippy::result_large_err)]

use ntx_proto::ntx_agent_client::NtxAgentClient;
use ntx_proto::ntx_agent_server::NtxAgentServer;
use ntx_proto::terminal_input::Input;
use ntx_proto::{
    AuthRequest, ControlServiceRequest, Empty, KillProcessRequest, ListProcessesRequest,
    ListServicesRequest, MetricsRequest, TerminalInput,
};
use ntx_server::auth::{hash_psk, AuthManager, AUTH_HEADER};
use ntx_server::config::Config;
use ntx_server::server::NtxAgentService;
use std::net::SocketAddr;
use std::str::FromStr;
use tokio::net::TcpListener;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tokio_stream::StreamExt;
use tonic::metadata::MetadataValue;
use tonic::transport::Server;
use tonic::Request;

async fn start_test_server(psk: &str) -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
    let mut config = Config::default();
    config.server.psk = psk.to_string();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let auth_manager = AuthManager::new(psk);
    let agent_service = NtxAgentService::new(config, auth_manager.clone());

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let auth_interceptor = auth_manager.clone();
    let interceptor = move |req: Request<()>| auth_interceptor.intercept(req);

    tokio::spawn(async move {
        Server::builder()
            .add_service(NtxAgentServer::with_interceptor(agent_service, interceptor))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });

    (addr, shutdown_tx)
}

#[tokio::test]
async fn test_full_grpc_e2e_lifecycle() {
    let psk = "ntx_test_secret_key_12345";
    let (addr, _shutdown) = start_test_server(psk).await;

    let server_url = format!("http://{}", addr);

    // 1. Connect basic unauthenticated client
    let mut client = NtxAgentClient::connect(server_url.clone())
        .await
        .expect("Failed to connect client");

    // 2. Ping works unauthenticated
    let ping_resp = client
        .ping(Request::new(Empty {}))
        .await
        .expect("Ping should succeed unauthenticated")
        .into_inner();
    assert!(!ping_resp.hostname.is_empty());
    assert_eq!(ping_resp.version, env!("CARGO_PKG_VERSION"));

    // 3. GetSystemInfo without auth fails
    let sys_err = client.get_system_info(Request::new(Empty {})).await;
    assert!(sys_err.is_err(), "GetSystemInfo without auth should fail");

    // 4. Authenticate with bad PSK fails
    let bad_auth = client
        .authenticate(Request::new(AuthRequest {
            psk_hash: "invalid_hash_string".to_string(),
            viewer_id: "viewer-e2e".to_string(),
        }))
        .await
        .expect("Authenticate call should succeed")
        .into_inner();
    assert!(!bad_auth.success);
    assert!(bad_auth.session_token.is_empty());

    // 5. Authenticate with valid SHA-256 hash
    let psk_hash = hash_psk(psk);
    let auth_resp = client
        .authenticate(Request::new(AuthRequest {
            psk_hash: psk_hash.clone(),
            viewer_id: "viewer-e2e".to_string(),
        }))
        .await
        .expect("Authenticate should succeed")
        .into_inner();
    assert!(auth_resp.success);
    let session_token = auth_resp.session_token;
    assert!(!session_token.is_empty());

    // 6. GetSystemInfo with session token succeeds
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

    // 7. StreamMetrics with PSK hash succeeds
    let mut metrics_req = Request::new(MetricsRequest { interval_ms: 250 });
    metrics_req
        .metadata_mut()
        .insert(AUTH_HEADER, MetadataValue::from_str(&psk_hash).unwrap());
    let mut metrics_stream = client
        .stream_metrics(metrics_req)
        .await
        .expect("StreamMetrics should succeed")
        .into_inner();

    let first_metric =
        tokio::time::timeout(tokio::time::Duration::from_secs(3), metrics_stream.next())
            .await
            .expect("Timeout waiting for metrics")
            .expect("Metrics stream closed")
            .expect("Error in metrics item");

    assert!(first_metric.total_memory_bytes > 0);
    assert!(first_metric.timestamp_ms > 0);

    // 8. OpenTerminal with auth and bidirectional communication
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

    // Send echo command into the shell
    let echo_str = "echo E2E_SHELL_TEST_OK\r\n";
    in_tx
        .send(TerminalInput {
            input: Some(Input::Data(echo_str.as_bytes().to_vec())),
        })
        .await
        .unwrap();

    // Read output chunks looking for echo output
    let mut received_output = String::new();
    let start_time = tokio::time::Instant::now();
    while start_time.elapsed() < tokio::time::Duration::from_secs(5) {
        if let Ok(Some(Ok(chunk))) =
            tokio::time::timeout(tokio::time::Duration::from_millis(500), out_stream.next()).await
        {
            let text = String::from_utf8_lossy(&chunk.data);
            received_output.push_str(&text);
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

    // 9. ListProcesses with session token succeeds
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

    // 10. KillProcess with session token for invalid PID
    let mut kill_req = Request::new(KillProcessRequest {
        pid: 99999999,
        signal: 15,
    });
    kill_req
        .metadata_mut()
        .insert(AUTH_HEADER, MetadataValue::from_str(&session_token).unwrap());
    let kill_resp = client
        .kill_process(kill_req)
        .await
        .expect("KillProcess RPC should succeed")
        .into_inner();
    assert!(!kill_resp.success);
    assert!(!kill_resp.error_message.is_empty());

    // 11. ListServices with session token succeeds
    let mut svc_req = Request::new(ListServicesRequest {});
    svc_req
        .metadata_mut()
        .insert(AUTH_HEADER, MetadataValue::from_str(&session_token).unwrap());
    let svc_resp = client
        .list_services(svc_req)
        .await
        .expect("ListServices should succeed")
        .into_inner();
    assert!(!svc_resp.services.is_empty());

    // 12. ControlService with session token succeeds
    let mut ctrl_req = Request::new(ControlServiceRequest {
        service_name: "mock-service".to_string(),
        action: "start".to_string(),
    });
    ctrl_req
        .metadata_mut()
        .insert(AUTH_HEADER, MetadataValue::from_str(&session_token).unwrap());
    let ctrl_resp = client
        .control_service(ctrl_req)
        .await
        .expect("ControlService should succeed")
        .into_inner();
    assert!(ctrl_resp.success);
    assert!(ctrl_resp.error_message.is_empty());
}

#[tokio::test]
async fn test_process_manager_e2e_lifecycle() {
    let psk = "ntx_test_proc_e2e_key";
    let (addr, _shutdown) = start_test_server(psk).await;
    let server_url = format!("http://{}", addr);

    let mut client = NtxAgentClient::connect(server_url)
        .await
        .expect("Failed to connect client");

    // Authenticate
    let psk_hash = hash_psk(psk);
    let auth_resp = client
        .authenticate(Request::new(AuthRequest {
            psk_hash: psk_hash.clone(),
            viewer_id: "proc-verifier".to_string(),
        }))
        .await
        .expect("Auth call")
        .into_inner();
    assert!(auth_resp.success);
    let session_token = auth_resp.session_token;

    // Spawn a dummy child process to test listing and killing
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

    // Wait slightly for system process table to populate
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // 1. ListProcesses: child must be found
    let mut req = Request::new(ListProcessesRequest {});
    req.metadata_mut().insert(
        AUTH_HEADER,
        MetadataValue::from_str(&session_token).unwrap(),
    );
    let proc_resp = client.list_processes(req).await.unwrap().into_inner();
    assert!(!proc_resp.processes.is_empty());

    let found_child = proc_resp.processes.iter().find(|p| p.pid == child_pid);
    assert!(
        found_child.is_some(),
        "Spawned child PID {} should appear in process list",
        child_pid
    );

    // 2. KillProcess with SIGKILL (9)
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
    assert!(kill_resp.error_message.is_empty());

    // Verify child process exits
    let exit_status = child.wait().expect("Child process wait failed");
    assert!(!exit_status.success());

    // 3. KillProcess on already dead child PID -> returns failure
    let mut kill_dead_req = Request::new(KillProcessRequest {
        pid: child_pid,
        signal: 15,
    });
    kill_dead_req.metadata_mut().insert(
        AUTH_HEADER,
        MetadataValue::from_str(&session_token).unwrap(),
    );
    let kill_dead_resp = client
        .kill_process(kill_dead_req)
        .await
        .unwrap()
        .into_inner();
    assert!(!kill_dead_resp.success);
    assert!(!kill_dead_resp.error_message.is_empty());
}

#[tokio::test]
async fn test_service_manager_e2e_lifecycle() {
    let psk = "ntx_test_svc_e2e_key";
    let (addr, _shutdown) = start_test_server(psk).await;
    let server_url = format!("http://{}", addr);

    let mut client = NtxAgentClient::connect(server_url)
        .await
        .expect("Failed to connect client");

    // Authenticate
    let psk_hash = hash_psk(psk);
    let auth_resp = client
        .authenticate(Request::new(AuthRequest {
            psk_hash,
            viewer_id: "svc-verifier".to_string(),
        }))
        .await
        .expect("Auth call")
        .into_inner();
    let session_token = auth_resp.session_token;

    // 1. ListServices
    let mut svc_req = Request::new(ListServicesRequest {});
    svc_req.metadata_mut().insert(
        AUTH_HEADER,
        MetadataValue::from_str(&session_token).unwrap(),
    );
    let svc_resp = client.list_services(svc_req).await.unwrap().into_inner();
    assert!(!svc_resp.services.is_empty());

    // 2. ControlService valid actions on mock service
    for action in ["start", "stop", "restart"] {
        let mut ctrl_req = Request::new(ControlServiceRequest {
            service_name: "mock-service".to_string(),
            action: action.to_string(),
        });
        ctrl_req.metadata_mut().insert(
            AUTH_HEADER,
            MetadataValue::from_str(&session_token).unwrap(),
        );
        let resp = client
            .control_service(ctrl_req)
            .await
            .unwrap()
            .into_inner();
        assert!(
            resp.success,
            "Action '{}' on mock-service should succeed",
            action
        );
        assert!(resp.error_message.is_empty());
    }

    // 3. ControlService invalid action -> failure
    let mut ctrl_bad = Request::new(ControlServiceRequest {
        service_name: "mock-service".to_string(),
        action: "unsupported_action".to_string(),
    });
    ctrl_bad.metadata_mut().insert(
        AUTH_HEADER,
        MetadataValue::from_str(&session_token).unwrap(),
    );
    let bad_resp = client.control_service(ctrl_bad).await.unwrap().into_inner();
    assert!(!bad_resp.success);
    assert!(!bad_resp.error_message.is_empty());
}

#[tokio::test]
async fn test_process_and_service_auth_rejection() {
    let psk = "ntx_auth_test_key";
    let (addr, _shutdown) = start_test_server(psk).await;
    let server_url = format!("http://{}", addr);

    let mut client = NtxAgentClient::connect(server_url)
        .await
        .expect("Failed to connect client");

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
            service_name: "mock-service".to_string(),
            action: "start".to_string(),
        }))
        .await
        .is_err());

    // 2. Calls with bogus token must also return Err
    let bogus_token = "bogus_invalid_token_12345";

    let mut proc_req = Request::new(ListProcessesRequest {});
    proc_req
        .metadata_mut()
        .insert(AUTH_HEADER, MetadataValue::from_str(bogus_token).unwrap());
    assert!(client.list_processes(proc_req).await.is_err());

    let mut kill_req = Request::new(KillProcessRequest {
        pid: 1,
        signal: 15,
    });
    kill_req
        .metadata_mut()
        .insert(AUTH_HEADER, MetadataValue::from_str(bogus_token).unwrap());
    assert!(client.kill_process(kill_req).await.is_err());

    let mut svc_req = Request::new(ListServicesRequest {});
    svc_req
        .metadata_mut()
        .insert(AUTH_HEADER, MetadataValue::from_str(bogus_token).unwrap());
    assert!(client.list_services(svc_req).await.is_err());

    let mut ctrl_req = Request::new(ControlServiceRequest {
        service_name: "mock-service".to_string(),
        action: "start".to_string(),
    });
    ctrl_req
        .metadata_mut()
        .insert(AUTH_HEADER, MetadataValue::from_str(bogus_token).unwrap());
    assert!(client.control_service(ctrl_req).await.is_err());
}
