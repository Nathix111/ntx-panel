use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub discovery: DiscoveryConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "generate_psk")]
    pub psk: String,
    #[serde(default)]
    pub hostname_override: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveryConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_service_name")]
    pub service_name: String,
}

fn default_port() -> u16 {
    9850
}

fn default_true() -> bool {
    true
}

fn default_service_name() -> String {
    "_ntx._tcp.local.".to_string()
}

pub fn generate_psk() -> String {
    format!(
        "ntx_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            psk: generate_psk(),
            hostname_override: None,
        }
    }
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            service_name: default_service_name(),
        }
    }
}

pub fn default_config_path() -> PathBuf {
    if let Ok(env_path) = std::env::var("NTX_CONFIG_PATH") {
        return PathBuf::from(env_path);
    }
    if let Some(config_dir) = dirs::config_dir() {
        config_dir.join("ntx-server").join("config.toml")
    } else {
        PathBuf::from("ntx-config.toml")
    }
}

pub fn load_or_create() -> Result<(Config, bool)> {
    load_or_create_from_path(&default_config_path())
}

pub fn load_or_create_from_path(path: &Path) -> Result<(Config, bool)> {
    if path.exists() {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file at {:?}", path))?;
        let mut config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse TOML from {:?}", path))?;

        let mut modified = false;
        if config.server.psk.trim().is_empty() {
            config.server.psk = generate_psk();
            modified = true;
        }

        if modified {
            save_config(&config, path)?;
        }

        Ok((config, false))
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create config directory {:?}", parent))?;
        }
        let config = Config::default();
        save_config(&config, path)?;
        Ok((config, true))
    }
}

pub fn save_config(config: &Config, path: &Path) -> Result<()> {
    let serialized =
        toml::to_string_pretty(config).context("Failed to serialize config to TOML")?;
    fs::write(path, serialized)
        .with_context(|| format!("Failed to write config file to {:?}", path))?;
    Ok(())
}

pub fn get_hostname(config: &Config) -> String {
    if let Some(ref name) = config.server.hostname_override {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| "ntx-device".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_psk_generation() {
        let psk1 = generate_psk();
        let psk2 = generate_psk();
        assert_ne!(psk1, psk2);
        assert!(psk1.starts_with("ntx_"));
        assert!(psk1.len() > 10);
    }

    #[test]
    fn test_load_or_create_workflow() {
        let temp_dir =
            std::env::temp_dir().join(format!("ntx_test_config_{}", uuid::Uuid::new_v4().simple()));
        let config_file = temp_dir.join("config.toml");

        // 1. Initial creation
        let (config, is_new) = load_or_create_from_path(&config_file).unwrap();
        assert!(is_new);
        assert_eq!(config.server.port, 9850);
        assert!(!config.server.psk.is_empty());
        assert!(config_file.exists());

        // 2. Loading existing
        let (loaded, is_new_2) = load_or_create_from_path(&config_file).unwrap();
        assert!(!is_new_2);
        assert_eq!(config.server.psk, loaded.server.psk);
        assert_eq!(config.server.port, loaded.server.port);

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_get_hostname_override() {
        let mut config = Config::default();
        config.server.hostname_override = Some("my-custom-node".to_string());
        assert_eq!(get_hostname(&config), "my-custom-node");

        config.server.hostname_override = Some("   ".to_string());
        assert_ne!(get_hostname(&config), "   ");
        assert!(!get_hostname(&config).is_empty());
    }
}
