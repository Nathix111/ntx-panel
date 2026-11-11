use crate::config::{get_hostname, Config};
use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::collections::HashMap;
use tracing::info;

pub fn start_announcement(config: &Config) -> Result<Option<ServiceDaemon>> {
    if !config.discovery.enabled {
        info!("mDNS discovery announcement is disabled in config");
        return Ok(None);
    }

    let hostname = get_hostname(config);
    let mdns = ServiceDaemon::new().context("Failed to create mDNS daemon")?;

    let host_target = format!("{}.local.", hostname);
    let mut properties = HashMap::new();
    properties.insert("version".to_string(), env!("CARGO_PKG_VERSION").to_string());
    properties.insert("hostname".to_string(), hostname.clone());

    let service_info = ServiceInfo::new(
        &config.discovery.service_name,
        &hostname,
        &host_target,
        "",
        config.server.port,
        properties,
    )
    .context("Failed to construct ServiceInfo for mDNS")?;

    mdns.register(service_info)
        .context("Failed to register mDNS service")?;

    info!(
        "mDNS: successfully announced as '{}' on port {}",
        hostname, config.server.port
    );

    Ok(Some(mdns))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_announcement_disabled() {
        let mut config = Config::default();
        config.discovery.enabled = false;
        let daemon = start_announcement(&config).unwrap();
        assert!(daemon.is_none());
    }
}
