use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

pub const NTX_SERVICE_TYPE: &str = "_ntx._tcp.local.";

#[derive(Debug, Clone)]
pub struct DiscoveredDevice {
    pub fullname: String,
    pub hostname: String,
    pub addresses: Vec<String>,
    pub port: u16,
    pub version: String,
    pub cert_fingerprint: Option<String>,
}

#[derive(Debug)]
pub enum DiscoveryEvent {
    Added(DiscoveredDevice),
    Removed(String),
}

pub struct DiscoveryHandle {
    daemon: Option<ServiceDaemon>,
    is_running: Arc<AtomicBool>,
}

impl DiscoveryHandle {
    pub fn stop(&mut self) {
        self.is_running.store(false, Ordering::Relaxed);
        if let Some(daemon) = self.daemon.take() {
            let _ = daemon.stop_browse(NTX_SERVICE_TYPE);
            let _ = daemon.shutdown();
        }
    }
}

fn is_ipv6_link_local(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
        IpAddr::V4(_) => false,
    }
}

/// Prioritize routable IPv4 first, then IPv4 loopback, then non-link-local IPv6, dropping
/// link-local IPv6 (`fe80::`) unless no other address exists.
pub fn prioritize_addresses<I>(raw_addrs: I) -> Vec<String>
where
    I: IntoIterator<Item = IpAddr>,
{
    let mut ipv4_lan = Vec::new();
    let mut ipv4_loopback = Vec::new();
    let mut ipv6_other = Vec::new();
    let mut ipv6_link_local = Vec::new();

    for ip in raw_addrs {
        match ip {
            IpAddr::V4(v4) if v4.is_loopback() => ipv4_loopback.push(v4.to_string()),
            IpAddr::V4(v4) => ipv4_lan.push(v4.to_string()),
            IpAddr::V6(v6) if is_ipv6_link_local(&IpAddr::V6(v6)) => {
                ipv6_link_local.push(v6.to_string());
            }
            IpAddr::V6(v6) => ipv6_other.push(v6.to_string()),
        }
    }

    ipv4_lan.sort();
    ipv4_loopback.sort();
    ipv6_other.sort();
    ipv6_link_local.sort();

    let mut result = Vec::new();
    result.extend(ipv4_lan);
    result.extend(ipv4_loopback);
    result.extend(ipv6_other);
    if result.is_empty() {
        result.extend(ipv6_link_local);
    }
    result
}

/// Start browsing mDNS for `_ntx._tcp.local.` services.
/// Emits `DiscoveryEvent` through the provided mpsc channel.
pub fn start_discovery(
    tx: mpsc::Sender<DiscoveryEvent>,
    handle: tokio::runtime::Handle,
    repaint_ctx: Option<egui::Context>,
) -> anyhow::Result<DiscoveryHandle> {
    let daemon = ServiceDaemon::new().map_err(|e| anyhow::anyhow!("Failed to init mDNS: {e}"))?;
    let receiver = daemon
        .browse(NTX_SERVICE_TYPE)
        .map_err(|e| anyhow::anyhow!("Failed to browse mDNS: {e}"))?;

    let is_running = Arc::new(AtomicBool::new(true));
    let running_flag = Arc::clone(&is_running);

    handle.spawn(async move {
        info!("mDNS discovery started browsing for {}", NTX_SERVICE_TYPE);
        while running_flag.load(Ordering::Relaxed) {
            match receiver.recv_async().await {
                Ok(event) => match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let fullname = info.get_fullname().to_string();
                        let hostname = info
                            .get_property_val_str("hostname")
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| {
                                info.get_hostname().trim_end_matches('.').to_string()
                            });

                        let addresses = prioritize_addresses(info.get_addresses().iter().copied());
                        let port = info.get_port();
                        let version = info
                            .get_property_val_str("version")
                            .unwrap_or("0.2.0")
                            .to_string();
                        let cert_fingerprint = info
                            .get_property_val_str("fingerprint")
                            .map(crate::tls::normalize_fingerprint)
                            .filter(|s| !s.is_empty());

                        info!(
                            "mDNS discovered service: {} at {:?}:{} (v{})",
                            hostname, addresses, port, version
                        );

                        let discovered = DiscoveredDevice {
                            fullname,
                            hostname,
                            addresses,
                            port,
                            version,
                            cert_fingerprint,
                        };

                        if tx.send(DiscoveryEvent::Added(discovered)).await.is_err() {
                            break;
                        }
                        if let Some(ref ctx) = repaint_ctx {
                            ctx.request_repaint();
                        }
                    }
                    ServiceEvent::ServiceRemoved(_service_type, fullname) => {
                        info!("mDNS service removed: {}", fullname);
                        if tx.send(DiscoveryEvent::Removed(fullname)).await.is_err() {
                            break;
                        }
                        if let Some(ref ctx) = repaint_ctx {
                            ctx.request_repaint();
                        }
                    }
                    _ => {}
                },
                Err(e) => {
                    warn!("mDNS receiver error: {e}");
                    break;
                }
            }
        }
        info!("mDNS discovery background task finished");
    });

    Ok(DiscoveryHandle {
        daemon: Some(daemon),
        is_running,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_start_discovery_outside_runtime_context() {
        let rt = tokio::runtime::Runtime::new().expect("create runtime");
        let handle = rt.handle().clone();

        let thread_handle = std::thread::spawn(move || {
            let (tx, _rx) = mpsc::channel(16);
            let res = start_discovery(tx, handle, None);
            assert!(
                res.is_ok(),
                "start_discovery should succeed without a tokio reactor on the current thread"
            );
            let mut disc = res.unwrap();
            disc.stop();
        });

        thread_handle.join().expect("thread join");
    }

    #[test]
    fn test_prioritize_addresses() {
        let addrs: Vec<IpAddr> = vec![
            "fe80::1".parse().unwrap(),
            "127.0.0.1".parse().unwrap(),
            "192.168.1.42".parse().unwrap(),
            "fd00::5".parse().unwrap(),
        ];
        let sorted = prioritize_addresses(addrs);
        assert_eq!(sorted, vec!["192.168.1.42", "127.0.0.1", "fd00::5"]);

        // Link-local kept only if nothing else exists
        let only_ll: Vec<IpAddr> = vec!["fe80::1".parse().unwrap()];
        assert_eq!(prioritize_addresses(only_ll), vec!["fe80::1"]);
    }
}
