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
}
