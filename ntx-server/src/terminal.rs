use anyhow::Result;
use ntx_proto::terminal_input::Input;
use ntx_proto::{TerminalInput, TerminalOutput};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status, Streaming};

enum PtyCommand {
    Write(Vec<u8>),
    Resize { cols: u16, rows: u16 },
}

fn build_shell_command() -> CommandBuilder {
    if cfg!(windows) {
        CommandBuilder::new("cmd.exe")
    } else {
        let env_shell = std::env::var("SHELL").ok().filter(|s| {
            let p = std::path::Path::new(s.trim());
            p.is_absolute() && p.exists()
        });

        let shell_path = env_shell.unwrap_or_else(|| {
            if std::path::Path::new("/bin/bash").exists() {
                "/bin/bash".to_string()
            } else if std::path::Path::new("/usr/bin/bash").exists() {
                "/usr/bin/bash".to_string()
            } else {
                "/bin/sh".to_string()
            }
        });

        let mut builder = CommandBuilder::new(&shell_path);
        builder.arg("-i");
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
        builder.env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| {
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string()
            }),
        );

        let home = std::env::var("HOME")
            .ok()
            .filter(|h| !h.trim().is_empty())
            .unwrap_or_else(|| "/root".to_string());
        builder.env("HOME", &home);
        if std::path::Path::new(&home).is_dir() {
            builder.cwd(&home);
        }
        builder.env("SHELL", &shell_path);
        builder
    }
}

pub async fn handle_open_terminal(
    request: Request<Streaming<TerminalInput>>,
) -> Result<Response<ReceiverStream<Result<TerminalOutput, Status>>>, Status> {
    let mut in_stream = request.into_inner();

    let (pair, child, mut reader, mut writer) = tokio::task::spawn_blocking(|| {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Status::internal(format!("Failed to open PTY: {}", e)))?;

        let cmd = build_shell_command();
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| Status::internal(format!("Failed to spawn shell process: {}", e)))?;

        // Close slave in parent process so EOF/signals propagate cleanly on Unix
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| Status::internal(format!("Failed to clone PTY reader: {}", e)))?;

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| Status::internal(format!("Failed to take PTY writer: {}", e)))?;

        Ok::<_, Status>((pair.master, child, reader, writer))
    })
    .await
    .map_err(|e| Status::internal(format!("Failed to initialize PTY task: {}", e)))??;

    let (out_tx, out_rx) = mpsc::channel::<Result<TerminalOutput, Status>>(64);
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<PtyCommand>(128);
    let (eof_tx, eof_rx) = oneshot::channel::<()>();

    // Blocking Task A: Read PTY stdout/stderr -> gRPC output stream
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let out = TerminalOutput {
                        data: buf[..n].to_vec(),
                    };
                    if out_tx.blocking_send(Ok(out)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::debug!("PTY reader ended: {}", e);
                    break;
                }
            }
        }
        let _ = eof_tx.send(());
    });

    // Blocking Task B: Write / Resize / Reap child process
    tokio::task::spawn_blocking(move || {
        let master = pair;
        let mut child = child;
        while let Some(cmd) = cmd_rx.blocking_recv() {
            match cmd {
                PtyCommand::Write(bytes) => {
                    if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                        break;
                    }
                }
                PtyCommand::Resize { cols, rows } => {
                    let _ = master.resize(PtySize {
                        rows: rows.clamp(1, 500),
                        cols: cols.clamp(1, 500),
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
            }
        }

        // Drop master & writer first to send SIGHUP/EOF and unblock reader
        drop(writer);
        drop(master);
        let _ = child.kill();
        let _ = child.wait();
    });

    // Async Coordinator: Bridge gRPC input stream and detect shell EOF
    tokio::spawn(async move {
        let mut eof_rx = eof_rx;
        loop {
            tokio::select! {
                _ = &mut eof_rx => {
                    break;
                }
                msg_res = in_stream.next() => {
                    match msg_res {
                        Some(Ok(msg)) => {
                            if let Some(input) = msg.input {
                                let cmd = match input {
                                    Input::Data(bytes) => PtyCommand::Write(bytes),
                                    Input::Resize(resize) => PtyCommand::Resize {
                                        cols: resize.cols.clamp(1, 500) as u16,
                                        rows: resize.rows.clamp(1, 500) as u16,
                                    },
                                };
                                if cmd_tx.send(cmd).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Some(Err(_)) | None => break,
                    }
                }
            }
        }
        drop(cmd_tx);
    });

    Ok(Response::new(ReceiverStream::new(out_rx)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pty_creation_and_reap() {
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
        drop(pair.slave);
        drop(pair.master);
        let _ = child.kill();
        let status = child.wait().expect("child should be reapable");
        let _ = status;
    }
}
