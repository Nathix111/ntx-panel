use mdns_sd::{ServiceDaemon, ServiceEvent};
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
}

#[derive(Debug)]
pub enum DiscoveryEvent {
    Added(DiscoveredDevice),
    Removed(String),
}

pub struct DiscoveryHandle {
    _daemon: Option<ServiceDaemon>,
    is_running: Arc<AtomicBool>,
}

impl DiscoveryHandle {
    pub fn stop(&self) {
        self.is_running.store(false, Ordering::Relaxed);
    }
}

/// Start browsing mDNS for `_ntx._tcp.local.` services.
/// Emits `DiscoveryEvent` through the provided mpsc channel.
pub fn start_discovery(tx: mpsc::Sender<DiscoveryEvent>) -> anyhow::Result<DiscoveryHandle> {
    let daemon = ServiceDaemon::new().map_err(|e| anyhow::anyhow!("Failed to init mDNS: {e}"))?;
    let receiver = daemon
        .browse(NTX_SERVICE_TYPE)
        .map_err(|e| anyhow::anyhow!("Failed to browse mDNS: {e}"))?;

    let is_running = Arc::new(AtomicBool::new(true));
    let running_flag = Arc::clone(&is_running);

    tokio::spawn(async move {
        info!("mDNS discovery started browsing for {}", NTX_SERVICE_TYPE);
        while running_flag.load(Ordering::Relaxed) {
            match receiver.recv_async().await {
                Ok(event) => match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let fullname = info.get_fullname().to_string();
                        let hostname = info.get_hostname().trim_end_matches('.').to_string();

                        let addresses: Vec<String> = info
                            .get_addresses()
                            .iter()
                            .map(|ip| ip.to_string())
                            .collect();

                        let port = info.get_port();
                        let version = info
                            .get_property_val_str("version")
                            .unwrap_or("0.1.0")
                            .to_string();

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
                        };

                        if tx.send(DiscoveryEvent::Added(discovered)).await.is_err() {
                            break;
                        }
                    }
                    ServiceEvent::ServiceRemoved(_service_type, fullname) => {
                        info!("mDNS service removed: {}", fullname);
                        if tx.send(DiscoveryEvent::Removed(fullname)).await.is_err() {
                            break;
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
        _daemon: Some(daemon),
        is_running,
    })
}
