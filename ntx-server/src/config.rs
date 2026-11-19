use anyhow::{Context, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub discovery: DiscoveryConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ServerConfig {
    pub port: u16,
    pub bind_address: String,
    pub psk: String,
    pub hostname_override: Option<String>,
    pub session_ttl_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DiscoveryConfig {
    pub enabled: bool,
    pub service_name: String,
}

fn default_port() -> u16 {
    9850
}

fn default_bind_address() -> String {
    "0.0.0.0".to_string()
}

fn default_session_ttl_secs() -> u64 {
    3600
}

fn default_true() -> bool {
    true
}

fn default_service_name() -> String {
    "_ntx._tcp.local.".to_string()
}

pub fn generate_psk() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("ntx_{}", hex::encode(bytes))
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            bind_address: default_bind_address(),
            psk: generate_psk(),
            hostname_override: None,
            session_ttl_secs: default_session_ttl_secs(),
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

/// Write `content` atomically to `path` via a temporary file in the same directory.
/// On Unix, applies `unix_mode` (e.g. `0o600` for secrets, `0o644` for public certs).
pub fn write_file_atomic(path: &Path, content: &[u8], unix_mode: u32) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let parent_existed = parent.exists();
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create directory {:?}", parent))?;

    #[cfg(unix)]
    if !parent_existed {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = parent_existed;

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let mut rand_suffix = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut rand_suffix);
    let tmp_name = format!(
        ".{}.tmp.{}.{}",
        file_name,
        std::process::id(),
        hex::encode(rand_suffix)
    );
    let tmp_path = parent.join(tmp_name);

    let write_res: Result<()> = (|| {
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(unix_mode);
        }
        #[cfg(not(unix))]
        let _ = unix_mode;

        let mut file = opts
            .open(&tmp_path)
            .with_context(|| format!("Failed to create temp file {:?}", tmp_path))?;

        file.write_all(content)
            .with_context(|| format!("Failed to write temp file {:?}", tmp_path))?;
        file.sync_all()
            .with_context(|| format!("Failed to sync temp file {:?}", tmp_path))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp_path, fs::Permissions::from_mode(unix_mode))
                .with_context(|| format!("Failed to set permissions on {:?}", tmp_path))?;
        }

        Ok(())
    })();

    if let Err(e) = write_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    if let Err(e) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e).with_context(|| format!("Failed to rename {:?} to {:?}", tmp_path, path));
    }

    Ok(())
}

pub fn config_dir_for_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn default_config_path() -> PathBuf {
    if let Ok(env_path) = std::env::var("NTX_CONFIG_PATH") {
        let trimmed = env_path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    #[cfg(unix)]
    {
        let etc_path = PathBuf::from("/etc/ntx-server/config.toml");
        if unsafe { libc::geteuid() } == 0 {
            return etc_path;
        }
        if etc_path.exists() && fs::OpenOptions::new().read(true).open(&etc_path).is_ok() {
            return etc_path;
        }
    }

    #[cfg(not(unix))]
    {
        let etc_path = PathBuf::from("/etc/ntx-server/config.toml");
        if etc_path.exists() {
            return etc_path;
        }
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
        if config.server.bind_address.trim().is_empty() {
            config.server.bind_address = default_bind_address();
            modified = true;
        }
        if config.server.session_ttl_secs == 0 {
            config.server.session_ttl_secs = default_session_ttl_secs();
            modified = true;
        }

        if modified {
            save_config(&config, path)?;
        }

        Ok((config, false))
    } else {
        let config = Config::default();
        save_config(&config, path)?;
        Ok((config, true))
    }
}

pub fn save_config(config: &Config, path: &Path) -> Result<()> {
    let serialized =
        toml::to_string_pretty(config).context("Failed to serialize config to TOML")?;
    write_file_atomic(path, serialized.as_bytes(), 0o600)
        .with_context(|| format!("Failed to save config file to {:?}", path))
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
        assert_eq!(psk1.len(), 4 + 64);
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
        assert_eq!(config.server.bind_address, "0.0.0.0");
        assert_eq!(config.server.session_ttl_secs, 3600);
        assert!(!config.server.psk.is_empty());
        assert!(config_file.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&config_file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        // 2. Loading existing
        let (loaded, is_new_2) = load_or_create_from_path(&config_file).unwrap();
        assert!(!is_new_2);
        assert_eq!(config.server.psk, loaded.server.psk);
        assert_eq!(config.server.port, loaded.server.port);
        assert_eq!(config.server.bind_address, loaded.server.bind_address);
        assert_eq!(
            config.server.session_ttl_secs,
            loaded.server.session_ttl_secs
        );

        // 3. Partial TOML backwards compatibility
        let partial_toml = "[server]\nport = 9999\npsk = \"custom_psk\"\n";
        write_file_atomic(&config_file, partial_toml.as_bytes(), 0o600).unwrap();
        let (partial_loaded, is_new_3) = load_or_create_from_path(&config_file).unwrap();
        assert!(!is_new_3);
        assert_eq!(partial_loaded.server.port, 9999);
        assert_eq!(partial_loaded.server.psk, "custom_psk");
        assert_eq!(partial_loaded.server.bind_address, "0.0.0.0");
        assert_eq!(partial_loaded.server.session_ttl_secs, 3600);

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
