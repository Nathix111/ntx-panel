use ntx_proto::ServiceItem;
#[cfg(target_os = "linux")]
use std::process::Command;
use tracing::info;
#[cfg(target_os = "linux")]
use tracing::warn;

/// Validate a systemd unit / service name to prevent argument injection or path traversal.
pub fn validate_service_name(name: &str) -> Result<&str, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Le nom du service ne peut pas être vide.".to_string());
    }
    if trimmed.len() > 256 {
        return Err("Le nom du service dépasse 256 caractères.".to_string());
    }
    if trimmed.starts_with('-') {
        return Err(format!(
            "Nom de service invalide '{}': ne peut pas commencer par '-'.",
            trimmed
        ));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | ':'))
    {
        return Err(format!(
            "Nom de service invalide '{}': caractères non autorisés.",
            trimmed
        ));
    }
    Ok(trimmed)
}

/// Parse the output of `systemctl list-units --type=service --all --no-pager --no-legend --plain`.
pub fn parse_systemctl_output(output: &str) -> Vec<ServiceItem> {
    let mut services = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Clean any leading status symbols (e.g. '●', '*', etc.)
        let cleaned = trimmed
            .trim_start_matches(|c: char| c == '●' || c == '*' || c.is_whitespace())
            .trim();

        if cleaned.is_empty() {
            continue;
        }

        let parts: Vec<&str> = cleaned.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }

        // Skip header lines if present
        if parts[0].eq_ignore_ascii_case("UNIT") || parts[0].eq_ignore_ascii_case("Legend:") {
            continue;
        }

        let name = parts[0].to_string();
        let load_state = parts[1].to_string();
        let active_state = parts[2].to_string();
        let sub_state = parts[3].to_string();
        let description = if parts.len() > 4 {
            parts[4..].join(" ")
        } else {
            String::new()
        };

        services.push(ServiceItem {
            name,
            description,
            load_state,
            active_state,
            sub_state,
        });
    }

    services
}

/// List Linux systemd services. Returns an explicit error on non-Linux hosts or if systemctl fails.
pub fn list_system_services() -> Result<Vec<ServiceItem>, String> {
    #[cfg(target_os = "linux")]
    {
        match Command::new("systemctl")
            .args([
                "list-units",
                "--type=service",
                "--all",
                "--no-pager",
                "--no-legend",
                "--plain",
            ])
            .output()
        {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                Ok(parse_systemctl_output(&stdout))
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                warn!(status = ?output.status, stderr = %stderr, "systemctl list-units failed");
                Err(format!(
                    "systemctl list-units a échoué (code {:?}): {}",
                    output.status.code(),
                    stderr
                ))
            }
            Err(e) => {
                warn!(error = %e, "systemctl command failed to execute");
                Err(format!("Impossible d'exécuter systemctl: {}", e))
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        Err("La gestion des services système est uniquement supportée sur Linux (systemd).".to_string())
    }
}

/// Control a Linux systemd service with actions like start, stop, restart, reload, enable, disable.
pub fn control_system_service(service_name: &str, action: &str) -> Result<(), String> {
    let action_clean = action.trim().to_lowercase();
    let valid_actions = ["start", "stop", "restart", "reload", "enable", "disable"];
    if !valid_actions.contains(&action_clean.as_str()) {
        return Err(format!(
            "Action invalide '{}'. Actions supportées: {:?}",
            action, valid_actions
        ));
    }

    let clean_name = validate_service_name(service_name)?;

    #[cfg(test)]
    if clean_name.starts_with("mock-") || clean_name == "test-service" {
        info!(
            service = %clean_name,
            action = %action_clean,
            "Test-only mock service action executed"
        );
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        let output = Command::new("systemctl")
            .args([action_clean.as_str(), "--", clean_name])
            .output()
            .map_err(|e| format!("Impossible d'exécuter systemctl: {}", e))?;

        if output.status.success() {
            info!(
                service = %clean_name,
                action = %action_clean,
                "systemctl command succeeded"
            );
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let err_msg = if !stderr.trim().is_empty() {
                stderr.trim()
            } else if !stdout.trim().is_empty() {
                stdout.trim()
            } else {
                "Erreur systemctl inconnue"
            };
            Err(format!(
                "systemctl {} {} a échoué: {}",
                action_clean, clean_name, err_msg
            ))
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (clean_name, info!("Service control requested on unsupported OS"));
        Err("La gestion des services système est uniquement supportée sur Linux (systemd).".to_string())
    }
}

/// Mock services strictly for unit tests.
#[cfg(test)]
pub fn get_mock_services() -> Vec<ServiceItem> {
    vec![
        ServiceItem {
            name: "ntx-server.service".to_string(),
            description: "NTX Panel Agent Daemon".to_string(),
            load_state: "loaded".to_string(),
            active_state: "active".to_string(),
            sub_state: "running".to_string(),
        },
        ServiceItem {
            name: "ssh.service".to_string(),
            description: "OpenBSD Secure Shell server".to_string(),
            load_state: "loaded".to_string(),
            active_state: "active".to_string(),
            sub_state: "running".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_service_name() {
        assert_eq!(validate_service_name("nginx.service").unwrap(), "nginx.service");
        assert_eq!(validate_service_name("  ssh@1.service ").unwrap(), "ssh@1.service");
        assert_eq!(validate_service_name("my_app-2:dev").unwrap(), "my_app-2:dev");

        assert!(validate_service_name("").is_err());
        assert!(validate_service_name("   ").is_err());
        assert!(validate_service_name("-H").is_err());
        assert!(validate_service_name("--root=/tmp").is_err());
        assert!(validate_service_name("../evil.service").is_err());
        assert!(validate_service_name("nginx; rm -rf /").is_err());
        assert!(validate_service_name(&"a".repeat(257)).is_err());
    }

    #[test]
    fn test_parse_systemctl_output() {
        let sample = r#"
  cron.service                      loaded active running Regular background program processing daemon
  dbus.service                      loaded active running D-Bus System Message Bus
● failed.service                    loaded failed failed  Failed Service Description
  emergency.service                 loaded inactive dead  Emergency Shell
"#;
        let services = parse_systemctl_output(sample);
        assert_eq!(services.len(), 4);

        assert_eq!(services[0].name, "cron.service");
        assert_eq!(services[0].load_state, "loaded");
        assert_eq!(services[0].active_state, "active");
        assert_eq!(services[0].sub_state, "running");
        assert_eq!(
            services[0].description,
            "Regular background program processing daemon"
        );

        assert_eq!(services[2].name, "failed.service");
        assert_eq!(services[2].active_state, "failed");
        assert_eq!(services[2].sub_state, "failed");

        assert_eq!(services[3].name, "emergency.service");
        assert_eq!(services[3].active_state, "inactive");
        assert_eq!(services[3].sub_state, "dead");
    }

    #[test]
    fn test_mock_services() {
        let services = get_mock_services();
        assert!(!services.is_empty());
        assert!(services.iter().any(|s| s.name == "ntx-server.service"));
    }

    #[test]
    fn test_control_service_validation() {
        assert!(control_system_service("mock-service", "invalid_action").is_err());
        assert!(control_system_service("mock-service", "start").is_ok());
        assert!(control_system_service("mock-service", "stop").is_ok());
        assert!(control_system_service("mock-service", "restart").is_ok());
        assert!(control_system_service("--bad-flag", "start").is_err());
    }

    #[test]
    fn test_control_service_all_valid_actions_and_casing() {
        let actions = [
            "start", "STOP", "  Restart  ", "reload", "Enable", "DISABLE",
        ];
        for action in actions {
            assert!(
                control_system_service("mock-service", action).is_ok(),
                "Action '{}' on mock-service should succeed",
                action
            );
            assert!(
                control_system_service("test-service", action).is_ok(),
                "Action '{}' on test-service should succeed",
                action
            );
        }

        let invalid_actions = ["", "   ", "destroy", "kill", "status", "pause"];
        for action in invalid_actions {
            let res = control_system_service("mock-service", action);
            assert!(
                res.is_err(),
                "Invalid action '{}' should fail",
                action
            );
        }
    }

    #[test]
    fn test_parse_systemctl_empty_and_malformed() {
        assert!(parse_systemctl_output("").is_empty());
        assert!(parse_systemctl_output("   \n\n  \t ").is_empty());
        assert!(parse_systemctl_output("not enough tokens").is_empty());
        assert!(parse_systemctl_output("UNIT LOAD ACTIVE SUB DESCRIPTION").is_empty());
    }

    #[test]
    fn test_list_system_services_platform_behavior() {
        let res = list_system_services();
        #[cfg(not(target_os = "linux"))]
        assert!(res.is_err());
        #[cfg(target_os = "linux")]
        let _ = res;
    }
}
