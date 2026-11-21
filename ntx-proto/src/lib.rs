#![allow(clippy::result_large_err)]

pub mod ntx {
    tonic::include_proto!("ntx");
}

pub use ntx::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proto_types() {
        let challenge_req = AuthChallengeRequest {
            viewer_id: "test-viewer".to_string(),
        };
        assert_eq!(challenge_req.viewer_id, "test-viewer");

        let challenge_resp = AuthChallengeResponse {
            challenge_id: "chal-123".to_string(),
            nonce: vec![1, 2, 3, 4],
            expires_in_seconds: 30,
        };
        assert_eq!(challenge_resp.challenge_id, "chal-123");
        assert_eq!(challenge_resp.nonce, vec![1, 2, 3, 4]);
        assert_eq!(challenge_resp.expires_in_seconds, 30);

        let auth_req = AuthRequest {
            psk_hash: String::new(),
            viewer_id: "test-viewer".to_string(),
            challenge_id: "chal-123".to_string(),
            response_hmac: "deadbeef".to_string(),
        };
        assert_eq!(auth_req.viewer_id, "test-viewer");
        assert_eq!(auth_req.challenge_id, "chal-123");
        assert_eq!(auth_req.response_hmac, "deadbeef");

        let auth_resp = AuthResponse {
            success: true,
            session_token: "tok".to_string(),
            error_message: String::new(),
            expires_in_seconds: 3600,
        };
        assert!(auth_resp.success);
        assert_eq!(auth_resp.expires_in_seconds, 3600);

        let ping = PingResponse {
            hostname: "test-host".to_string(),
            uptime_seconds: 42,
            version: "0.2.0".to_string(),
        };
        assert_eq!(ping.hostname, "test-host");
        assert_eq!(ping.uptime_seconds, 42);
        assert_eq!(ping.version, "0.2.0");

        let sys_info = SystemInfo {
            hostname: "test".to_string(),
            os_name: "Linux".to_string(),
            os_version: "6.8".to_string(),
            arch: "x86_64".to_string(),
            total_memory_bytes: 1024,
            cpu_count: 8,
            disks: vec![],
            networks: vec![],
        };
        assert_eq!(sys_info.cpu_count, 8);
    }

    #[test]
    fn test_managed_service_and_console_proto_types() {
        let chunk = ConsoleChunk {
            data: b"[INFO] Done!\n".to_vec(),
            timestamp_ms: 1_700_000_000_000,
            is_history: true,
        };
        assert_eq!(chunk.data, b"[INFO] Done!\n");
        assert_eq!(chunk.timestamp_ms, 1_700_000_000_000);
        assert!(chunk.is_history);

        let cfg = ManagedServiceConfig {
            id: "mc-survival".to_string(),
            name: "Minecraft Survival".to_string(),
            icon: "⛏".to_string(),
            description: "PaperMC 1.21".to_string(),
            command: "java -Xmx4G -jar paper.jar nogui".to_string(),
            working_dir: "/srv/minecraft".to_string(),
            stop_command: "stop".to_string(),
            auto_start: true,
            auto_restart: true,
            env_vars: vec!["JAVA_OPTS=-XX:+UseG1GC".to_string()],
            quick_commands: vec!["list".to_string(), "save-all".to_string()],
            systemd_unit: String::new(),
            attach_command: String::new(),
        };
        assert_eq!(cfg.id, "mc-survival");
        assert_eq!(cfg.quick_commands.len(), 2);

        let status = ManagedServiceStatus {
            config: Some(cfg.clone()),
            state: "running".to_string(),
            pid: 4242,
            cpu_usage: 12.5,
            memory_bytes: 2_147_483_648,
            uptime_seconds: 3600,
            last_exit_code: 0,
            has_exit_code: false,
            restart_count: 1,
            last_error: String::new(),
        };
        assert_eq!(status.state, "running");
        assert_eq!(status.pid, 4242);
        assert_eq!(status.config.as_ref().map(|c| c.name.as_str()), Some("Minecraft Survival"));

        let list_resp = ListManagedServicesResponse {
            services: vec![status],
        };
        assert_eq!(list_resp.services.len(), 1);

        let save_req = SaveManagedServiceRequest { config: Some(cfg.clone()) };
        assert!(save_req.config.is_some());
        let save_resp = SaveManagedServiceResponse {
            success: true,
            error_message: String::new(),
            service: Some(cfg),
        };
        assert!(save_resp.success);
        assert!(save_resp.service.is_some());

        let del_req = DeleteManagedServiceRequest {
            id: "mc-survival".to_string(),
        };
        assert_eq!(del_req.id, "mc-survival");
        let del_resp = DeleteManagedServiceResponse {
            success: true,
            error_message: String::new(),
        };
        assert!(del_resp.success);

        let ctrl_req = ControlManagedServiceRequest {
            id: "mc-survival".to_string(),
            action: "restart".to_string(),
        };
        assert_eq!(ctrl_req.action, "restart");
        let ctrl_resp = ControlManagedServiceResponse {
            success: true,
            error_message: String::new(),
        };
        assert!(ctrl_resp.success);

        let input_req = SendManagedServiceInputRequest {
            id: "mc-survival".to_string(),
            input: "say Hello".to_string(),
            append_newline: true,
        };
        assert!(input_req.append_newline);
        let input_resp = SendManagedServiceInputResponse {
            success: true,
            error_message: String::new(),
        };
        assert!(input_resp.success);

        let stream_req = StreamManagedServiceConsoleRequest {
            id: "mc-survival".to_string(),
            tail_lines: 200,
        };
        assert_eq!(stream_req.tail_lines, 200);
    }

    #[test]
    fn test_docker_proto_types() {
        let item = DockerContainerItem {
            id: "a1b2c3d4e5f6".to_string(),
            name: "nginx-proxy".to_string(),
            image: "nginx:alpine".to_string(),
            state: "running".to_string(),
            status: "Up 3 hours".to_string(),
            ports: "0.0.0.0:80->80/tcp".to_string(),
            created: "2026-01-01 00:00:00 +0000 UTC".to_string(),
            cpu_percent: 0.42,
            memory_used_bytes: 33_554_432,
            memory_limit_bytes: 536_870_912,
            compose_project: "webstack".to_string(),
        };
        assert_eq!(item.name, "nginx-proxy");
        assert_eq!(item.compose_project, "webstack");

        let list_req = ListDockerContainersRequest { include_all: true };
        assert!(list_req.include_all);
        let list_resp = ListDockerContainersResponse {
            docker_available: true,
            docker_version: "27.3.1".to_string(),
            error_message: String::new(),
            containers: vec![item],
        };
        assert!(list_resp.docker_available);
        assert_eq!(list_resp.containers.len(), 1);

        let ctrl_req = ControlDockerContainerRequest {
            container_id: "a1b2c3d4e5f6".to_string(),
            action: "restart".to_string(),
            force: false,
        };
        assert_eq!(ctrl_req.action, "restart");
        let ctrl_resp = ControlDockerContainerResponse {
            success: true,
            error_message: String::new(),
        };
        assert!(ctrl_resp.success);

        let exec_req = ExecDockerCommandRequest {
            container_id: "a1b2c3d4e5f6".to_string(),
            command: "nginx -t".to_string(),
        };
        assert_eq!(exec_req.command, "nginx -t");
        let exec_resp = ExecDockerCommandResponse {
            success: true,
            exit_code: 0,
            stdout: "syntax is ok\n".to_string(),
            stderr: String::new(),
            error_message: String::new(),
        };
        assert_eq!(exec_resp.exit_code, 0);

        let init_input = DockerConsoleInput {
            input: Some(docker_console_input::Input::Init(DockerConsoleInit {
                container_id: "a1b2c3d4e5f6".to_string(),
                mode: "attach".to_string(),
                tail_lines: 100,
            })),
        };
        assert!(matches!(
            init_input.input,
            Some(docker_console_input::Input::Init(ref i)) if i.mode == "attach" && i.tail_lines == 100
        ));

        let data_input = DockerConsoleInput {
            input: Some(docker_console_input::Input::Data(b"ls -la\n".to_vec())),
        };
        assert!(matches!(
            data_input.input,
            Some(docker_console_input::Input::Data(ref d)) if d == b"ls -la\n"
        ));

        let resize_input = DockerConsoleInput {
            input: Some(docker_console_input::Input::Resize(TerminalResize {
                cols: 120,
                rows: 40,
            })),
        };
        assert!(matches!(
            resize_input.input,
            Some(docker_console_input::Input::Resize(ref r)) if r.cols == 120 && r.rows == 40
        ));
    }
}
