use anyhow::Result;
use ntx_proto::terminal_input::Input;
use ntx_proto::{TerminalInput, TerminalOutput};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status, Streaming};

pub async fn handle_open_terminal(
    request: Request<Streaming<TerminalInput>>,
) -> Result<Response<ReceiverStream<Result<TerminalOutput, Status>>>, Status> {
    let mut in_stream = request.into_inner();

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| Status::internal(format!("Failed to open PTY: {}", e)))?;

    let cmd = if cfg!(windows) {
        CommandBuilder::new("cmd.exe")
    } else {
        let shell_path = if std::path::Path::new("/bin/bash").exists() {
            "/bin/bash"
        } else if std::path::Path::new("/usr/bin/bash").exists() {
            "/usr/bin/bash"
        } else {
            "/bin/sh"
        };
        let mut builder = CommandBuilder::new(shell_path);
        // Force interactive shell mode so bash loads prompt and handles input correctly
        builder.arg("-i");
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
        builder.env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| {
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string()
            }),
        );
        if let Ok(home) = std::env::var("HOME") {
            builder.env("HOME", home);
        } else {
            builder.env("HOME", "/root");
        }
        builder.env("SHELL", shell_path);
        builder
    };

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| Status::internal(format!("Failed to spawn shell process: {}", e)))?;

    // Close slave in the parent process on Unix so EOF and signals function properly
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| Status::internal(format!("Failed to clone PTY reader: {}", e)))?;

    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| Status::internal(format!("Failed to take PTY writer: {}", e)))?;

    let master_pty: Arc<Mutex<Box<dyn MasterPty + Send>>> = Arc::new(Mutex::new(pair.master));
    let master_clone = master_pty.clone();

    let (tx, rx) = mpsc::channel(64);

    // Task 1: Read from PTY reader and stream to gRPC output
    let tx_clone = tx.clone();
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let out = TerminalOutput {
                        data: buf[..n].to_vec(),
                    };
                    if tx_clone.blocking_send(Ok(out)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::debug!("PTY reader ended: {}", e);
                    break;
                }
            }
        }
    });

    // Task 2: Read from gRPC in_stream and write to PTY writer or handle resize
    tokio::spawn(async move {
        while let Some(msg_res) = in_stream.next().await {
            match msg_res {
                Ok(msg) => {
                    if let Some(input) = msg.input {
                        match input {
                            Input::Data(bytes) => {
                                if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                                    break;
                                }
                            }
                            Input::Resize(resize) => {
                                if let Ok(master) = master_clone.lock() {
                                    let _ = master.resize(PtySize {
                                        rows: resize.rows.max(1) as u16,
                                        cols: resize.cols.max(1) as u16,
                                        pixel_width: 0,
                                        pixel_height: 0,
                                    });
                                }
                            }
                        }
                    }
                }
                Err(_) => break,
            }
        }

        // Clean up child process if still running
        let _ = child.kill();
    });

    Ok(Response::new(ReceiverStream::new(rx)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pty_creation_and_shell() {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty should succeed");

        let cmd = if cfg!(windows) {
            CommandBuilder::new("cmd.exe")
        } else {
            CommandBuilder::new("/bin/sh")
        };

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .expect("spawn shell should succeed");
        let _ = child.kill();
    }
}
