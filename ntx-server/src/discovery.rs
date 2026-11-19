use crate::config::{get_hostname, Config};
use anyhow::{Context, Result};
use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};
use std::collections::HashMap;
use std::net::IpAddr;
use tracing::info;

/// Sanitize a system hostname into a single valid mDNS host label (without `.local.`).
pub fn sanitize_mdns_label(hostname: &str) -> String {
    let trimmed = hostname.trim().trim_end_matches('.');
    let without_local = trimmed
        .strip_suffix(".local")
        .or_else(|| trimmed.strip_suffix(".LOCAL"))
        .unwrap_or(trimmed);

    let first_label = without_local.split('.').next().unwrap_or("").trim();
    let cleaned: String = first_label
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let cleaned = cleaned.trim_matches('-');

    if cleaned.is_empty() {
        "ntx-device".to_string()
    } else {
        cleaned.to_string()
    }
}

pub fn start_announcement(config: &Config, cert_fingerprint: &str) -> Result<Option<ServiceDaemon>> {
    if !config.discovery.enabled {
        info!("mDNS discovery announcement is disabled in config");
        return Ok(None);
    }

    let raw_hostname = get_hostname(config);
    let label = sanitize_mdns_label(&raw_hostname);
    let mdns = ServiceDaemon::new().context("Failed to create mDNS daemon")?;
    if matches!(config.server.bind_address.trim().parse::<IpAddr>(), Ok(IpAddr::V4(_))) {
        let _ = mdns.disable_interface(IfKind::IPv6);
    }

    let host_target = format!("{}.local.", label);
    let mut properties = HashMap::new();
    properties.insert("version".to_string(), env!("CARGO_PKG_VERSION").to_string());
    properties.insert("hostname".to_string(), raw_hostname.clone());
    if !cert_fingerprint.trim().is_empty() {
        properties.insert("fingerprint".to_string(), cert_fingerprint.trim().to_string());
    }

    let service_info = ServiceInfo::new(
        &config.discovery.service_name,
        &label,
        &host_target,
        "",
        config.server.port,
        properties,
    )
    .context("Failed to construct ServiceInfo for mDNS")?
    .enable_addr_auto();

    mdns.register(service_info)
        .context("Failed to register mDNS service")?;

    info!(
        "mDNS: successfully announced as '{}' ({}) on port {}",
        raw_hostname, host_target, config.server.port
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
        let daemon = start_announcement(&config, "deadbeef").unwrap();
        assert!(daemon.is_none());
    }

    #[test]
    fn test_sanitize_mdns_label() {
        assert_eq!(sanitize_mdns_label("myhost.local"), "myhost");
        assert_eq!(sanitize_mdns_label("myhost.local."), "myhost");
        assert_eq!(sanitize_mdns_label("srv-01.home.lan"), "srv-01");
        assert_eq!(sanitize_mdns_label("  pi5  "), "pi5");
        assert_eq!(sanitize_mdns_label("..."), "ntx-device");
    }
}
