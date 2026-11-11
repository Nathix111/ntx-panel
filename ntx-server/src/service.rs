use anyhow::{Context, Result};
use std::process::Command;
use tracing::info;

pub const SERVICE_NAME: &str = "ntx-server";
pub const SERVICE_DISPLAY_NAME: &str = "NTX Panel Agent";

pub fn install() -> Result<()> {
    let exe_path = std::env::current_exe().context("Failed to get current executable path")?;
    let exe_str = exe_path
        .to_str()
        .context("Executable path is not valid UTF-8")?;

    #[cfg(windows)]
    {
        info!("Registering Windows service '{}'...", SERVICE_NAME);
        let bin_path_arg = format!("binPath= \"\\\"{}\\\" run\"", exe_str);

        let output = Command::new("sc.exe")
            .args([
                "create",
                SERVICE_NAME,
                &bin_path_arg,
                "start=",
                "auto",
                "DisplayName=",
                SERVICE_DISPLAY_NAME,
            ])
            .output()
            .context("Failed to execute sc.exe create")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            anyhow::bail!(
                "sc.exe create failed:\nstdout: {}\nstderr: {}",
                stdout,
                stderr
            );
        }

        println!("Service '{}' successfully installed!", SERVICE_NAME);
        println!("{}", stdout);
    }

    #[cfg(not(windows))]
    {
        info!("Creating systemd service for Linux...");
        let unit_content = format!(
            r#"[Unit]
Description={description}
After=network.target

[Service]
Type=simple
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
        if let Err(e) = std::fs::write(unit_path, &unit_content) {
            println!(
                "Could not write to /etc/systemd/system/ (root required: {}).",
                e
            );
            println!("You can manually create the systemd unit with:\n");
            println!(
                "sudo tee /etc/systemd/system/ntx-server.service << 'EOF'\n{}\nEOF",
                unit_content
            );
        } else {
            let _ = Command::new("systemctl").arg("daemon-reload").output();
            let _ = Command::new("systemctl")
                .args(["enable", "--now", "ntx-server.service"])
                .output();
            println!("Systemd unit installed and enabled!");
        }

        if let Ok((config, _)) = crate::config::load_or_create() {
            println!("Pre-Shared Key (PSK): {}", config.server.psk);
        }
    }

    Ok(())
}

pub fn purge_config_directories() -> Vec<std::path::PathBuf> {
    let mut dirs_to_remove = vec![
        std::path::PathBuf::from("/etc/ntx-server"),
        std::path::PathBuf::from("/root/.config/ntx-server"),
    ];

    if let Some(config_dir) = dirs::config_dir() {
        let user_cfg = config_dir.join("ntx-server");
        if !dirs_to_remove.contains(&user_cfg) {
            dirs_to_remove.push(user_cfg);
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        let home_cfg = std::path::PathBuf::from(home).join(".config").join("ntx-server");
        if !dirs_to_remove.contains(&home_cfg) {
            dirs_to_remove.push(home_cfg);
        }
    }

    if let Ok(sudo_user) = std::env::var("SUDO_USER") {
        let sudo_cfg = std::path::PathBuf::from(format!("/home/{}/.config/ntx-server", sudo_user));
        if !dirs_to_remove.contains(&sudo_cfg) {
            dirs_to_remove.push(sudo_cfg);
        }
    }

    dirs_to_remove
}

pub fn uninstall(purge: bool) -> Result<()> {
    #[cfg(windows)]
    {
        info!("Stopping Windows service '{}'...", SERVICE_NAME);
        let _ = Command::new("sc.exe").args(["stop", SERVICE_NAME]).output();

        info!("Deleting Windows service '{}'...", SERVICE_NAME);
        let output = Command::new("sc.exe")
            .args(["delete", SERVICE_NAME])
            .output()
            .context("Failed to execute sc.exe delete")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            anyhow::bail!(
                "sc.exe delete failed:\nstdout: {}\nstderr: {}",
                stdout,
                stderr
            );
        }

        println!("Service '{}' successfully uninstalled!", SERVICE_NAME);
        println!("{}", stdout);

        if purge {
            info!("Purging configurations...");
            for dir in purge_config_directories() {
                if dir.exists() {
                    let _ = std::fs::remove_dir_all(&dir);
                    println!("Deleted {}", dir.display());
                }
            }
            println!("Purge complete.");
        }
    }

    #[cfg(not(windows))]
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
            let _ = std::fs::remove_file(unit_path);
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
    }

    Ok(())
}

pub fn status() -> Result<()> {
    #[cfg(windows)]
    {
        let output = Command::new("sc.exe")
            .args(["query", SERVICE_NAME])
            .output()
            .context("Failed to execute sc.exe query")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("{}", stdout);
    }

    #[cfg(not(windows))]
    {
        let output = Command::new("systemctl")
            .args(["status", "ntx-server.service"])
            .output()
            .context("Failed to execute systemctl status")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("{}", stdout);
    }

    Ok(())
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
        assert!(dirs.contains(&std::path::PathBuf::from("/etc/ntx-server")));
        assert!(dirs.contains(&std::path::PathBuf::from("/root/.config/ntx-server")));
    }
}
