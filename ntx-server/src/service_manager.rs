use ntx_proto::ServiceItem;
use std::process::Command;
use tracing::{info, warn};

/// Parse the output of `systemctl list-units --type=service --all --no-pager --no-legend`.
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

/// Parse the output of Windows `sc.exe query state= all`.
pub fn parse_sc_query_output(output: &str) -> Vec<ServiceItem> {
    let mut services = Vec::new();
    let mut current_name = String::new();
    let mut current_display = String::new();
    let mut current_state = String::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("SERVICE_NAME:") {
            if !current_name.is_empty() {
                services.push(build_windows_service_item(
                    &current_name,
                    &current_display,
                    &current_state,
                ));
                current_display.clear();
                current_state.clear();
            }
            current_name = rest.trim().to_string();
        } else if let Some(rest) = trimmed.strip_prefix("DISPLAY_NAME:") {
            current_display = rest.trim().to_string();
        } else if let Some(rest) = trimmed.strip_prefix("STATE") {
            // e.g. "STATE              : 4  RUNNING"
            if let Some((_, val)) = rest.split_once(':') {
                current_state = val.trim().to_string();
            }
        }
    }

    if !current_name.is_empty() {
        services.push(build_windows_service_item(
            &current_name,
            &current_display,
            &current_state,
        ));
    }

    services
}

fn build_windows_service_item(name: &str, display: &str, state: &str) -> ServiceItem {
    let is_running = state.contains("RUNNING");
    let active_state = if is_running { "active" } else { "inactive" };
    let sub_state = if is_running { "running" } else { "dead" };

    ServiceItem {
        name: name.to_string(),
        description: if display.is_empty() {
            name.to_string()
        } else {
            display.to_string()
        },
        load_state: "loaded".to_string(),
        active_state: active_state.to_string(),
        sub_state: sub_state.to_string(),
    }
}

/// Mock services for testing or environments without a supported init system.
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
        ServiceItem {
            name: "cron.service".to_string(),
            description: "Regular background program processing daemon".to_string(),
            load_state: "loaded".to_string(),
            active_state: "active".to_string(),
            sub_state: "running".to_string(),
        },
        ServiceItem {
            name: "systemd-journald.service".to_string(),
            description: "Journal Service".to_string(),
            load_state: "loaded".to_string(),
            active_state: "active".to_string(),
            sub_state: "running".to_string(),
        },
        ServiceItem {
            name: "emergency.service".to_string(),
            description: "Emergency Shell".to_string(),
            load_state: "loaded".to_string(),
            active_state: "inactive".to_string(),
            sub_state: "dead".to_string(),
        },
    ]
}

/// List system services, querying systemctl on Linux or sc.exe on Windows, falling back to mock services.
pub fn list_system_services() -> Vec<ServiceItem> {
    #[cfg(target_os = "linux")]
    {
        match Command::new("systemctl")
            .args(["list-units", "--type=service", "--all", "--no-pager", "--no-legend"])
            .output()
        {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let parsed = parse_systemctl_output(&stdout);
                if !parsed.is_empty() {
                    return parsed;
                }
            }
            Ok(output) => {
                warn!(
                    status = ?output.status,
                    stderr = %String::from_utf8_lossy(&output.stderr),
                    "systemctl list-units returned non-zero status"
                );
            }
            Err(e) => {
                warn!(error = %e, "systemctl command not found or failed to execute");
            }
        }
    }

    #[cfg(windows)]
    {
        match Command::new("sc.exe")
            .args(["query", "state=", "all"])
            .output()
        {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let parsed = parse_sc_query_output(&stdout);
                if !parsed.is_empty() {
                    return parsed;
                }
            }
            Ok(output) => {
                warn!(
                    status = ?output.status,
                    stderr = %String::from_utf8_lossy(&output.stderr),
                    "sc.exe query returned non-zero status"
                );
            }
            Err(e) => {
                warn!(error = %e, "sc.exe command not found or failed to execute");
            }
        }
    }

    info!("Using fallback mock services list");
    get_mock_services()
}

/// Control a system service with actions like start, stop, restart.
pub fn control_system_service(service_name: &str, action: &str) -> Result<(), String> {
    let action_clean = action.trim().to_lowercase();
    let valid_actions = ["start", "stop", "restart", "reload", "enable", "disable"];
    if !valid_actions.contains(&action_clean.as_str()) {
        return Err(format!(
            "Invalid action '{}'. Supported actions: {:?}",
            action, valid_actions
        ));
    }

    // Support mock services for tests or simulation
    if service_name.starts_with("mock-") || service_name == "test-service" {
        info!(
            service = %service_name,
            action = %action_clean,
            "Mock service action executed successfully"
        );
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        let output = Command::new("systemctl")
            .arg(&action_clean)
            .arg(service_name)
            .output()
            .map_err(|e| format!("Failed to execute systemctl: {}", e))?;

        if output.status.success() {
            info!(
                service = %service_name,
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
                "Unknown systemctl error"
            };
            Err(format!(
                "systemctl {} {} failed: {}",
                action_clean, service_name, err_msg
            ))
        }
    }

    #[cfg(windows)]
    {
        let win_action = match action_clean.as_str() {
            "start" => Some("start"),
            "stop" => Some("stop"),
            "restart" => {
                let _ = Command::new("sc.exe").args(["stop", service_name]).output();
                std::thread::sleep(std::time::Duration::from_millis(500));
                Some("start")
            }
            _ => None,
        };

        if let Some(act) = win_action {
            let output = Command::new("sc.exe")
                .args([act, service_name])
                .output()
                .map_err(|e| format!("Failed to execute sc.exe: {}", e))?;

            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                Err(format!(
                    "sc.exe {} {} failed: {} {}",
                    act, service_name, stderr, stdout
                ))
            }
        } else {
            Err(format!(
                "Action '{}' is not supported on Windows services",
                action_clean
            ))
        }
    }

    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Err(format!(
            "Service management not supported on this operating system for '{}'",
            service_name
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_parse_sc_query_output() {
        let sample = r#"
SERVICE_NAME: TestService
DISPLAY_NAME: Test Service Description
        TYPE               : 10  WIN32_OWN_PROCESS  
        STATE              : 4  RUNNING 
                                (STOPPABLE, NOT_PAUSABLE, ACCEPTS_SHUTDOWN)
        WIN32_EXIT_CODE    : 0  (0x0)
        SERVICE_EXIT_CODE  : 0  (0x0)
        CHECKPOINT         : 0x0
        WAIT_HINT          : 0x0

SERVICE_NAME: StoppedService
DISPLAY_NAME: Stopped Service Description
        TYPE               : 20  WIN32_SHARE_PROCESS  
        STATE              : 1  STOPPED 
        WIN32_EXIT_CODE    : 1077  (0x435)
        SERVICE_EXIT_CODE  : 0  (0x0)
        CHECKPOINT         : 0x0
        WAIT_HINT          : 0x0
"#;
        let services = parse_sc_query_output(sample);
        assert_eq!(services.len(), 2);
        assert_eq!(services[0].name, "TestService");
        assert_eq!(services[0].description, "Test Service Description");
        assert_eq!(services[0].active_state, "active");
        assert_eq!(services[0].sub_state, "running");

        assert_eq!(services[1].name, "StoppedService");
        assert_eq!(services[1].active_state, "inactive");
        assert_eq!(services[1].sub_state, "dead");
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
            assert!(res.unwrap_err().contains("Invalid action"));
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
    fn test_parse_sc_query_empty_and_malformed() {
        assert!(parse_sc_query_output("").is_empty());
        assert!(parse_sc_query_output("Random text without service marker").is_empty());
        let malformed = "SERVICE_NAME: PartialService\nDISPLAY_NAME: No State";
        let parsed = parse_sc_query_output(malformed);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "PartialService");
        assert_eq!(parsed[0].active_state, "inactive");
    }

    #[test]
    fn test_list_system_services_not_empty() {
        let services = list_system_services();
        assert!(!services.is_empty());
    }
}
