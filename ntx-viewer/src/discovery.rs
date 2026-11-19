use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent};
use std::collections::{BTreeSet, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

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
    /// Re-trigger mDNS browsing on the existing daemon without dropping the UDP 5353 socket,
    /// replaying cached active services and issuing fresh PTR queries.
    pub fn refresh(
        &mut self,
        tx: mpsc::Sender<DiscoveryEvent>,
        handle: tokio::runtime::Handle,
        repaint_ctx: Option<egui::Context>,
    ) -> anyhow::Result<()> {
        self.is_running.store(false, Ordering::Relaxed);

        let daemon = match self.daemon.as_ref() {
            Some(d) => d,
            None => {
                let d = ServiceDaemon::new()
                    .map_err(|e| anyhow::anyhow!("Failed to init mDNS: {e}"))?;
                let _ = d.disable_interface(IfKind::IPv6);
                self.daemon = Some(d);
                self.daemon.as_ref().expect("daemon just set")
            }
        };

        let _ = daemon.stop_browse(NTX_SERVICE_TYPE);
        let receiver = daemon
            .browse(NTX_SERVICE_TYPE)
            .map_err(|e| anyhow::anyhow!("Failed to browse mDNS: {e}"))?;

        let is_running = Arc::new(AtomicBool::new(true));
        self.is_running = Arc::clone(&is_running);
        spawn_browse_loop(receiver, tx, handle, repaint_ctx, is_running);
        Ok(())
    }

    pub fn stop(&mut self) {
        self.is_running.store(false, Ordering::Relaxed);
        if let Some(daemon) = self.daemon.take() {
            let _ = daemon.stop_browse(NTX_SERVICE_TYPE);
            if let Ok(rx) = daemon.shutdown() {
                let _ = rx.recv_timeout(Duration::from_millis(300));
            }
        }
    }
}

pub fn is_ipv6_link_local(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
        IpAddr::V4(_) => false,
    }
}

/// Returns `true` if `ip` is routable/usable for remote gRPC connections.
/// Always excludes IPv6 link-local (`fe80::/10`), IPv4 link-local (`169.254.0.0/16`),
/// unspecified (`0.0.0.0`, `::`), and multicast addresses.
pub fn is_usable_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !v4.is_link_local() && !v4.is_unspecified() && !v4.is_multicast(),
        IpAddr::V6(v6) => {
            !is_ipv6_link_local(ip) && !v6.is_unspecified() && !v6.is_multicast()
        }
    }
}

/// Strip surrounding `[` / `]` brackets from an address string.
pub fn clean_address_str(addr: &str) -> &str {
    addr.trim().trim_start_matches('[').trim_end_matches(']')
}

/// Returns `true` if `addr` is a non-empty, usable IP or hostname for gRPC TLS.
pub fn is_usable_address(addr: &str) -> bool {
    let cleaned = clean_address_str(addr);
    if cleaned.is_empty() || cleaned.contains('%') {
        return false;
    }
    if let Ok(ip) = cleaned.parse::<IpAddr>() {
        return is_usable_ip(&ip);
    }
    true
}

/// Prioritize usable IP addresses in descending reliability order:
/// 1. `192.168.0.0/16` (typical home/office LAN)
/// 2. `10.0.0.0/8` & `172.16.0.0/12` (excluding default Docker bridge `172.17.0.0/16`)
/// 3. Other non-loopback IPv4 (including `172.17.0.0/16`, CGNAT, public IPv4)
/// 4. IPv4 loopback (`127.0.0.0/8`)
/// 5. Non-link-local, non-loopback IPv6 (Global / ULA)
/// 6. IPv6 loopback (`::1`)
///
/// Link-local addresses (`fe80::/10`, `169.254.0.0/16`) are strictly excluded.
pub fn prioritize_addresses<I>(raw_addrs: I) -> Vec<String>
where
    I: IntoIterator<Item = IpAddr>,
{
    let mut lan_192 = BTreeSet::<Ipv4Addr>::new();
    let mut lan_private = BTreeSet::<Ipv4Addr>::new();
    let mut ipv4_other = BTreeSet::<Ipv4Addr>::new();
    let mut ipv4_loopback = BTreeSet::<Ipv4Addr>::new();
    let mut ipv6_other = BTreeSet::<Ipv6Addr>::new();
    let mut ipv6_loopback = BTreeSet::<Ipv6Addr>::new();

    for ip in raw_addrs {
        if !is_usable_ip(&ip) {
            continue;
        }
        match ip {
            IpAddr::V4(v4) => {
                let o = v4.octets();
                if v4.is_loopback() {
                    ipv4_loopback.insert(v4);
                } else if o[0] == 192 && o[1] == 168 {
                    lan_192.insert(v4);
                } else if o[0] == 10 || (o[0] == 172 && (16..=31).contains(&o[1]) && o[1] != 17) {
                    lan_private.insert(v4);
                } else {
                    ipv4_other.insert(v4);
                }
            }
            IpAddr::V6(v6) => {
                if v6.is_loopback() {
                    ipv6_loopback.insert(v6);
                } else {
                    ipv6_other.insert(v6);
                }
            }
        }
    }

    let mut result = Vec::new();
    result.extend(lan_192.into_iter().map(|ip| ip.to_string()));
    result.extend(lan_private.into_iter().map(|ip| ip.to_string()));
    result.extend(ipv4_other.into_iter().map(|ip| ip.to_string()));
    result.extend(ipv4_loopback.into_iter().map(|ip| ip.to_string()));
    result.extend(ipv6_other.into_iter().map(|ip| ip.to_string()));
    result.extend(ipv6_loopback.into_iter().map(|ip| ip.to_string()));
    result
}

/// Filter out unusable addresses (`fe80::`, `169.254.x.x`, empty), prioritize IPs,
/// and retain any deduplicated non-IP hostnames at the end.
pub fn sanitize_addresses(addrs: &[String]) -> Vec<String> {
    let mut parsed_ips = Vec::new();
    let mut hostnames = Vec::new();
    let mut seen_hosts = HashSet::new();

    for raw in addrs {
        if !is_usable_address(raw) {
            continue;
        }
        let cleaned = clean_address_str(raw);
        if let Ok(ip) = cleaned.parse::<IpAddr>() {
            parsed_ips.push(ip);
        } else {
            let host_str = cleaned.to_string();
            if seen_hosts.insert(host_str.to_ascii_lowercase()) {
                hostnames.push(host_str);
            }
        }
    }

    let mut result = prioritize_addresses(parsed_ips);
    result.extend(hostnames);
    result
}

/// Merge existing and newly discovered addresses without losing manual entries or
/// poisoning with link-local addresses. When `preserve_first_existing` is `true`
/// and `existing[0]` is usable, it stays pinned at index 0.
pub fn merge_and_prioritize_addresses(
    existing: &[String],
    incoming: &[String],
    preserve_first_existing: bool,
) -> Vec<String> {
    let pinned_first = if preserve_first_existing {
        existing
            .first()
            .filter(|a| is_usable_address(a))
            .map(|a| clean_address_str(a).to_string())
    } else {
        None
    };

    let mut combined = Vec::with_capacity(existing.len() + incoming.len());
    combined.extend(existing.iter().cloned());
    combined.extend(incoming.iter().cloned());
    let mut prioritized = sanitize_addresses(&combined);

    if let Some(first) = pinned_first {
        prioritized.retain(|a| !a.eq_ignore_ascii_case(&first));
        prioritized.insert(0, first);
    }

    prioritized
}

fn spawn_browse_loop(
    receiver: mdns_sd::Receiver<ServiceEvent>,
    tx: mpsc::Sender<DiscoveryEvent>,
    handle: tokio::runtime::Handle,
    repaint_ctx: Option<egui::Context>,
    running_flag: Arc<AtomicBool>,
) {
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

                        let port = info.get_port();
                        let mut addresses =
                            prioritize_addresses(info.get_addresses().iter().copied());

                        if addresses.is_empty() {
                            let mdns_host = info.get_hostname().trim_end_matches('.').to_string();
                            if !mdns_host.is_empty() && !mdns_host.eq_ignore_ascii_case("localhost")
                            {
                                if let Ok(Ok(resolved)) = tokio::time::timeout(
                                    Duration::from_secs(2),
                                    tokio::net::lookup_host((mdns_host.as_str(), port)),
                                )
                                .await
                                {
                                    addresses = prioritize_addresses(resolved.map(|sa| sa.ip()));
                                }
                            }
                        }

                        if addresses.is_empty() {
                            debug!(
                                "Ignoring mDNS service '{}' ({}): no routable IPv4/IPv6 address resolved (raw: {:?})",
                                hostname,
                                fullname,
                                info.get_addresses()
                            );
                            continue;
                        }

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
                    ServiceEvent::SearchStopped(_) => {
                        debug!("mDNS browse stopped");
                        break;
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
}

/// Start browsing mDNS for `_ntx._tcp.local.` services over IPv4.
/// Emits `DiscoveryEvent` through the provided mpsc channel.
pub fn start_discovery(
    tx: mpsc::Sender<DiscoveryEvent>,
    handle: tokio::runtime::Handle,
    repaint_ctx: Option<egui::Context>,
) -> anyhow::Result<DiscoveryHandle> {
    let daemon = ServiceDaemon::new().map_err(|e| anyhow::anyhow!("Failed to init mDNS: {e}"))?;
    let _ = daemon.disable_interface(IfKind::IPv6);
    let receiver = daemon
        .browse(NTX_SERVICE_TYPE)
        .map_err(|e| anyhow::anyhow!("Failed to browse mDNS: {e}"))?;

    let is_running = Arc::new(AtomicBool::new(true));
    spawn_browse_loop(receiver, tx, handle, repaint_ctx, Arc::clone(&is_running));

    Ok(DiscoveryHandle {
        daemon: Some(daemon),
        is_running,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_start_discovery_and_refresh_outside_runtime_context() {
        let rt = tokio::runtime::Runtime::new().expect("create runtime");
        let handle = rt.handle().clone();

        let thread_handle = std::thread::spawn(move || {
            let (tx, _rx) = mpsc::channel(16);
            let res = start_discovery(tx, handle.clone(), None);
            assert!(
                res.is_ok(),
                "start_discovery should succeed without a tokio reactor on the current thread"
            );
            let mut disc = res.unwrap();

            let (tx2, _rx2) = mpsc::channel(16);
            assert!(
                disc.refresh(tx2, handle, None).is_ok(),
                "DiscoveryHandle::refresh should reuse the existing daemon cleanly"
            );
            disc.stop();
        });

        thread_handle.join().expect("thread join");
    }

    #[test]
    fn test_prioritize_addresses_excludes_link_local_and_orders_lan_first() {
        let addrs: Vec<IpAddr> = vec![
            "fe80::344b:2fff:fe9a:e777".parse().unwrap(),
            "169.254.10.20".parse().unwrap(),
            "127.0.0.1".parse().unwrap(),
            "172.17.0.1".parse().unwrap(),
            "10.0.0.5".parse().unwrap(),
            "192.168.1.42".parse().unwrap(),
            "fd00::5".parse().unwrap(),
            "::1".parse().unwrap(),
        ];
        let sorted = prioritize_addresses(addrs);
        assert_eq!(
            sorted,
            vec![
                "192.168.1.42",
                "10.0.0.5",
                "172.17.0.1",
                "127.0.0.1",
                "fd00::5",
                "::1"
            ]
        );

        // Link-local must NEVER be returned even when nothing else is present
        let only_ll: Vec<IpAddr> = vec![
            "fe80::344b:2fff:fe9a:e777".parse().unwrap(),
            "169.254.1.2".parse().unwrap(),
        ];
        assert!(prioritize_addresses(only_ll).is_empty());
    }

    #[test]
    fn test_merge_and_prioritize_addresses_preserves_manual_and_filters_poison() {
        // mDNS arriving with fe80:: must not wipe out manual IPv4
        let existing = vec!["192.168.1.99".to_string()];
        let incoming_poison = vec!["fe80::1".to_string()];
        let merged = merge_and_prioritize_addresses(&existing, &incoming_poison, true);
        assert_eq!(merged, vec!["192.168.1.99"]);

        // Manual non-192.168 address stays pinned first even when LAN 192.168 arrives
        let manual_vpn = vec!["10.8.0.2".to_string()];
        let discovered_lan = vec!["192.168.1.50".to_string(), "172.17.0.1".to_string()];
        let merged_manual = merge_and_prioritize_addresses(&manual_vpn, &discovered_lan, true);
        assert_eq!(
            merged_manual,
            vec!["10.8.0.2", "192.168.1.50", "172.17.0.1"]
        );

        // Non-manual prioritizes 192.168 over 10.8 and drops fe80/169.254
        let auto_merged = merge_and_prioritize_addresses(
            &["fe80::1".to_string(), "172.17.0.1".to_string()],
            &["192.168.1.50".to_string(), "169.254.2.3".to_string()],
            false,
        );
        assert_eq!(auto_merged, vec!["192.168.1.50", "172.17.0.1"]);
    }
}
