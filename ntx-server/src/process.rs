use ntx_proto::ProcessItem;
use sysinfo::{Pid, ProcessesToUpdate, Signal, System, Users};

pub async fn list_processes() -> Vec<ProcessItem> {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let users = Users::new_with_refreshed_list();

    let mut items = Vec::new();
    for (pid, proc) in sys.processes() {
        let cmd = proc
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(" ");

        let user = proc
            .user_id()
            .and_then(|uid| users.get_user_by_id(uid))
            .map(|u| u.name().to_string())
            .or_else(|| proc.user_id().map(|u| u.to_string()))
            .unwrap_or_default();

        let parent_pid = proc.parent().map(|p| p.as_u32()).unwrap_or(0);

        items.push(ProcessItem {
            pid: pid.as_u32(),
            name: proc.name().to_string_lossy().to_string(),
            cpu_usage: proc.cpu_usage(),
            memory_bytes: proc.memory(),
            user,
            status: format!("{:?}", proc.status()),
            command_line: cmd,
            parent_pid,
        });
    }

    // Sort by CPU usage descending by default
    items.sort_by(|a, b| {
        b.cpu_usage
            .partial_cmp(&a.cpu_usage)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    items
}

pub fn kill_process(pid: u32, signal: i32) -> Result<(), String> {
    let mut sys = System::new();
    let sys_pid = Pid::from_u32(pid);
    sys.refresh_processes(ProcessesToUpdate::Some(&[sys_pid]), true);

    let proc = sys
        .process(sys_pid)
        .ok_or_else(|| format!("Process with PID {} not found", pid))?;

    let sig = match signal {
        9 => Some(Signal::Kill),
        15 => Some(Signal::Term),
        2 => Some(Signal::Interrupt),
        1 => Some(Signal::Hangup),
        _ => {
            if signal <= 0 {
                Some(Signal::Term)
            } else {
                None
            }
        }
    };

    let success = if let Some(s) = sig {
        proc.kill_with(s).unwrap_or_else(|| proc.kill())
    } else {
        proc.kill()
    };

    if success {
        return Ok(());
    }

    #[cfg(unix)]
    {
        let sig_num = if signal > 0 { signal } else { 15 };
        if let Ok(out) = std::process::Command::new("kill")
            .args([format!("-{}", sig_num), pid.to_string()])
            .output()
        {
            if out.status.success() {
                return Ok(());
            }
        }
    }

    #[cfg(windows)]
    {
        if let Ok(out) = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output()
        {
            if out.status.success() {
                return Ok(());
            }
        }
    }

    Err(format!("Failed to kill process with PID {}", pid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_list_processes_not_empty() {
        let processes = list_processes().await;
        assert!(!processes.is_empty(), "Process list should not be empty");

        let current_pid = std::process::id();
        let found = processes.iter().any(|p| p.pid == current_pid);
        assert!(
            found,
            "Current process PID {} should be present in process list",
            current_pid
        );
    }

    #[test]
    fn test_kill_nonexistent_process() {
        // PID 99999999 is very unlikely to exist
        let result = kill_process(99999999, 15);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_list_processes_process_fields() {
        let processes = list_processes().await;
        let current_pid = std::process::id();
        let current = processes
            .iter()
            .find(|p| p.pid == current_pid)
            .expect("Current process must exist in list");

        assert!(!current.name.is_empty(), "Process name should not be empty");
        assert!(
            current.memory_bytes > 0,
            "Process memory should be greater than 0"
        );
        assert!(
            !current.status.is_empty(),
            "Process status should not be empty"
        );
    }

    #[tokio::test]
    async fn test_kill_process_real_child_lifecycle() {
        #[cfg(windows)]
        let mut child = std::process::Command::new("ping")
            .args(["127.0.0.1", "-n", "20"])
            .spawn()
            .expect("Failed to spawn ping test child");

        #[cfg(unix)]
        let mut child = std::process::Command::new("sleep")
            .arg("20")
            .spawn()
            .expect("Failed to spawn sleep test child");

        let child_pid = child.id();
        assert!(child_pid > 0);

        // Give OS a moment to register process
        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        let kill_result = kill_process(child_pid, 15);
        assert!(
            kill_result.is_ok(),
            "Killing child process should succeed: {:?}",
            kill_result
        );

        let exit_status = child.wait().expect("Child process wait failed");
        assert!(!exit_status.success());
    }

    #[test]
    fn test_kill_process_signals_graceful_error() {
        let signals = [9, 15, 2, 1, 0, -1, 999];
        for sig in signals {
            let res = kill_process(99999999, sig);
            assert!(
                res.is_err(),
                "Signal {} on nonexistent PID should fail gracefully",
                sig
            );
            assert!(res.unwrap_err().contains("not found"));
        }
    }
}
