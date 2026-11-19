#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::Result;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::process::Command;
#[cfg(target_os = "linux")]
use tracing::info;

pub const SERVICE_NAME: &str = "ntx-server";
pub const SERVICE_DISPLAY_NAME: &str = "NTX Panel Agent";

#[cfg(target_os = "linux")]
fn run_systemctl(args: &[&str]) -> Result<String> {
    let output = Command::new("systemctl")
        .args(args)
        .output()
        .with_context(|| format!("Failed to execute systemctl {:?}", args))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !output.status.success() {
        anyhow::bail!(
            "systemctl {:?} failed (exit {:?}): {}",
            args,
            output.status.code(),
            if !stderr.is_empty() { &stderr } else { &stdout }
        );
    }

    Ok(stdout)
}

#[cfg(unix)]
fn lookup_user_home(username: &str) -> Option<PathBuf> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in passwd.lines() {
        let mut fields = line.split(':');
        let user = fields.next()?;
        if user == username {
            let home = fields.nth(4)?; // 6th field (index 5)
            if !home.trim().is_empty() {
                return Some(PathBuf::from(home.trim()));
            }
        }
    }
    None
}

pub fn install() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let exe_path = std::env::current_exe().context("Failed to get current executable path")?;
        let exe_str = exe_path
            .to_str()
            .context("Executable path is not valid UTF-8")?;

        let config_path = PathBuf::from("/etc/ntx-server/config.toml");
        let (config, _) = crate::config::load_or_create_from_path(&config_path)?;
        let hostname = crate::config::get_hostname(&config);
        let config_dir = crate::config::config_dir_for_path(&config_path);
        let tls_material = crate::tls::load_or_generate_tls(&config_dir, &hostname)?;

        info!("Creating systemd service for Linux...");
        let unit_content = format!(
            r#"[Unit]
Description={description}
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
Environment=NTX_CONFIG_PATH=/etc/ntx-server/config.toml
ExecStart="{exe}" run
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
"#,
            description = SERVICE_DISPLAY_NAME,
            exe = exe_str
        );

        let unit_path = std::path::Path::new("/etc/systemd/system/ntx-server.service");
        std::fs::write(unit_path, &unit_content).with_context(|| {
            format!(
                "Impossible d'écrire {:?} (privilèges root requis)",
                unit_path
            )
        })?;

        run_systemctl(&["daemon-reload"])?;
        run_systemctl(&["enable", "--now", "ntx-server.service"])?;
        run_systemctl(&["restart", "ntx-server.service"])?;

        println!("Service systemd 'ntx-server.service' installé et démarré !");
        println!("Config path          : {}", config_path.display());
        println!("Pre-Shared Key (PSK) : {}", config.server.psk);
        println!(
            "TLS Fingerprint      : {}",
            crate::tls::format_fingerprint(&tls_material.fingerprint_sha256)
        );

        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    {
        anyhow::bail!(
            "L'installation en tant que service système ntx-server est uniquement supportée sur Linux (systemd). Utilisez 'ntx-server run' pour un lancement local."
        )
    }
}

pub fn purge_config_directories() -> Vec<PathBuf> {
    let mut dirs_to_remove = vec![
        PathBuf::from("/etc/ntx-server"),
        PathBuf::from("/root/.config/ntx-server"),
    ];

    if let Some(config_dir) = dirs::config_dir() {
        let user_cfg = config_dir.join("ntx-server");
        if !dirs_to_remove.contains(&user_cfg) {
            dirs_to_remove.push(user_cfg);
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            let home_cfg = PathBuf::from(home).join(".config").join("ntx-server");
            if !dirs_to_remove.contains(&home_cfg) {
                dirs_to_remove.push(home_cfg);
            }
        }
    }

    if let Ok(sudo_user) = std::env::var("SUDO_USER") {
        let trimmed = sudo_user.trim();
        if !trimmed.is_empty() && trimmed != "root" {
            #[cfg(unix)]
            let sudo_home = lookup_user_home(trimmed)
                .unwrap_or_else(|| PathBuf::from(format!("/home/{}", trimmed)));
            #[cfg(not(unix))]
            let sudo_home = PathBuf::from(format!("/home/{}", trimmed));

            let sudo_cfg = sudo_home.join(".config").join("ntx-server");
            if !dirs_to_remove.contains(&sudo_cfg) {
                dirs_to_remove.push(sudo_cfg);
            }
        }
    }

    dirs_to_remove
}

pub fn uninstall(purge: bool) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        info!("Stopping and removing systemd service...");
        let _ = Command::new("systemctl")
            .args(["stop", "ntx-server.service"])
            .output();
        let _ = Command::new("systemctl")
            .args(["disable", "ntx-server.service"])
            .output();

        let unit_path = std::path::Path::new("/etc/systemd/system/ntx-server.service");
        if unit_path.exists() {
            std::fs::remove_file(unit_path)
                .with_context(|| format!("Failed to remove {:?}", unit_path))?;
        }
        let _ = Command::new("systemctl").arg("daemon-reload").output();

        println!("Service 'ntx-server.service' uninstalled.");

        if purge {
            info!("Purging binary and configurations...");
            let bin_path = std::path::Path::new("/usr/local/bin/ntx-server");
            if bin_path.exists() {
                if let Err(e) = std::fs::remove_file(bin_path) {
                    eprintln!("Warning: failed to delete {}: {}", bin_path.display(), e);
                } else {
                    println!("Deleted {}", bin_path.display());
                }
            }

            let helper_path = std::path::Path::new("/usr/local/bin/ntx-uninstall");
            if helper_path.exists() {
                let _ = std::fs::remove_file(helper_path);
                println!("Deleted {}", helper_path.display());
            }

            for dir in purge_config_directories() {
                if dir.exists() {
                    if let Err(e) = std::fs::remove_dir_all(&dir) {
                        eprintln!("Warning: failed to remove {}: {}", dir.display(), e);
                    } else {
                        println!("Deleted configuration directory {}", dir.display());
                    }
                }
            }
            println!("Purge complete.");
        }

        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = purge;
        anyhow::bail!(
            "La désinstallation du service système ntx-server est uniquement supportée sur Linux (systemd)."
        )
    }
}

pub fn status() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let output = Command::new("systemctl")
            .args(["status", "ntx-server.service"])
            .output()
            .context("Failed to execute systemctl status")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("{}", stdout);
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.trim().is_empty() {
                eprintln!("{}", stderr);
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    {
        anyhow::bail!(
            "Le statut du service système ntx-server est uniquement supporté sur Linux (systemd)."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants() {
        assert_eq!(SERVICE_NAME, "ntx-server");
        assert_eq!(SERVICE_DISPLAY_NAME, "NTX Panel Agent");
    }

    #[test]
    fn test_purge_config_directories() {
        let dirs = purge_config_directories();
        assert!(dirs.contains(&PathBuf::from("/etc/ntx-server")));
        assert!(dirs.contains(&PathBuf::from("/root/.config/ntx-server")));
        assert!(!dirs.contains(&PathBuf::from("/home/root/.config/ntx-server")));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_service_commands_reject_non_linux() {
        assert!(install().is_err());
        assert!(uninstall(false).is_err());
        assert!(status().is_err());
    }
}
