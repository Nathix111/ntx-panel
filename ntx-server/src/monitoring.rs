use crate::config::{get_hostname, Config};
use ntx_proto::{
    DiskInfo, DiskMetrics, NetworkInterface, NetworkMetrics, SystemInfo, SystemMetrics,
};
use std::time::{SystemTime, UNIX_EPOCH};
use sysinfo::{Disks, Networks, System};

pub fn collect_system_info(config: &Config) -> SystemInfo {
    let mut sys = System::new_all();
    sys.refresh_all();

    let hostname = get_hostname(config);
    let os_name = System::name().unwrap_or_else(|| std::env::consts::OS.to_string());
    let os_version = System::os_version().unwrap_or_default();
    let arch = std::env::consts::ARCH.to_string();
    let total_memory_bytes = sys.total_memory();
    let cpu_count = sys.cpus().len() as u32;

    let disks_info = Disks::new_with_refreshed_list();
    let disks = disks_info
        .iter()
        .map(|disk| DiskInfo {
            name: disk.name().to_string_lossy().to_string(),
            mount_point: disk.mount_point().to_string_lossy().to_string(),
            total_bytes: disk.total_space(),
            available_bytes: disk.available_space(),
            filesystem: disk.file_system().to_string_lossy().to_string(),
        })
        .collect();

    let networks_info = Networks::new_with_refreshed_list();
    let networks = networks_info
        .iter()
        .map(|(name, data)| {
            let ip_addresses = data
                .ip_networks()
                .iter()
                .map(|ip_net| ip_net.addr.to_string())
                .collect();

            NetworkInterface {
                name: name.clone(),
                mac_address: data.mac_address().to_string(),
                ip_addresses,
            }
        })
        .collect();

    SystemInfo {
        hostname,
        os_name,
        os_version,
        arch,
        total_memory_bytes,
        cpu_count,
        disks,
        networks,
    }
}

pub fn scale_bytes_per_sec(delta_bytes: u64, elapsed_secs: f64) -> u64 {
    let dt = elapsed_secs.max(0.05);
    (delta_bytes as f64 / dt).round() as u64
}

pub fn collect_metrics(
    sys: &mut System,
    disks: &mut Disks,
    networks: &mut Networks,
    elapsed_secs: f64,
) -> SystemMetrics {
    sys.refresh_cpu_all();
    sys.refresh_memory();
    disks.refresh(true);
    networks.refresh(true);

    let cpu_usage_percent = sys.global_cpu_usage() as f64;
    let used_memory_bytes = sys.used_memory();
    let total_memory_bytes = sys.total_memory();

    let disk_metrics = disks
        .iter()
        .map(|disk| {
            let total = disk.total_space();
            let avail = disk.available_space();
            let used = total.saturating_sub(avail);
            let usage = disk.usage();
            DiskMetrics {
                mount_point: disk.mount_point().to_string_lossy().to_string(),
                used_bytes: used,
                total_bytes: total,
                read_bytes_per_sec: scale_bytes_per_sec(usage.read_bytes, elapsed_secs),
                write_bytes_per_sec: scale_bytes_per_sec(usage.written_bytes, elapsed_secs),
            }
        })
        .collect();

    let network_metrics = networks
        .iter()
        .map(|(name, data)| NetworkMetrics {
            interface_name: name.clone(),
            rx_bytes_per_sec: scale_bytes_per_sec(data.received(), elapsed_secs),
            tx_bytes_per_sec: scale_bytes_per_sec(data.transmitted(), elapsed_secs),
        })
        .collect();

    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    SystemMetrics {
        cpu_usage_percent,
        used_memory_bytes,
        total_memory_bytes,
        disk_metrics,
        network_metrics,
        timestamp_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_system_info() {
        let config = Config::default();
        let info = collect_system_info(&config);
        assert!(!info.hostname.is_empty());
        assert!(!info.os_name.is_empty());
        assert!(!info.arch.is_empty());
        assert!(info.cpu_count > 0);
        assert!(info.total_memory_bytes > 0);
    }

    #[test]
    fn test_collect_metrics() {
        let mut sys = System::new_all();
        let mut disks = Disks::new_with_refreshed_list();
        let mut networks = Networks::new_with_refreshed_list();

        let metrics = collect_metrics(&mut sys, &mut disks, &mut networks, 1.0);
        assert!(metrics.total_memory_bytes > 0);
        assert!(metrics.timestamp_ms > 0);
    }

    #[test]
    fn test_scale_bytes_per_sec() {
        assert_eq!(scale_bytes_per_sec(1000, 1.0), 1000);
        assert_eq!(scale_bytes_per_sec(1000, 0.25), 4000);
        assert_eq!(scale_bytes_per_sec(1000, 2.0), 500);
        // Guard against near-zero dt
        assert_eq!(scale_bytes_per_sec(100, 0.0), 2000);
    }
}
