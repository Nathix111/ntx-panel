pub mod ntx {
    tonic::include_proto!("ntx");
}

pub use ntx::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proto_types() {
        let auth_req = AuthRequest {
            psk_hash: "dummy_hash".to_string(),
            viewer_id: "test-viewer".to_string(),
        };
        assert_eq!(auth_req.psk_hash, "dummy_hash");
        assert_eq!(auth_req.viewer_id, "test-viewer");

        let ping = PingResponse {
            hostname: "test-host".to_string(),
            uptime_seconds: 42,
            version: "0.1.0".to_string(),
        };
        assert_eq!(ping.hostname, "test-host");
        assert_eq!(ping.uptime_seconds, 42);
        assert_eq!(ping.version, "0.1.0");

        let sys_info = SystemInfo {
            hostname: "test".to_string(),
            os_name: "Windows".to_string(),
            os_version: "11".to_string(),
            arch: "x86_64".to_string(),
            total_memory_bytes: 1024,
            cpu_count: 8,
            disks: vec![],
            networks: vec![],
        };
        assert_eq!(sys_info.cpu_count, 8);
    }
}
