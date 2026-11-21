#![allow(clippy::result_large_err)]

use anyhow::Result;
use ntx_proto::ntx_agent_server::NtxAgentServer;
use ntx_server::auth::AuthManager;
use ntx_server::config::{
    config_dir_for_path, default_config_path, get_hostname, load_or_create_from_path,
};
use ntx_server::managed_service::ManagedServiceManager;
use ntx_server::server::NtxAgentService;
use ntx_server::{discovery, service, tls};
use std::net::SocketAddr;
use tonic::transport::Server;
use tonic::Request;
use tracing::info;

fn print_help() {
    println!(
        r#"NTX Panel Server Agent (v{})

Usage:
    ntx-server [command] [options]

Commands:
    run            Run the agent server in foreground (default)
    install        Install the agent as a Linux systemd service
    uninstall      Uninstall the agent Linux systemd service
                   Options:
                     --purge, -p    Remove binary and wipe all configuration directories
    status         Show the status of the agent systemd service
    config         Print the configuration path, TOML, and TLS fingerprint
    help, --help   Show this help message
"#,
        env!("CARGO_PKG_VERSION")
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let args: Vec<String> = std::env::args().collect();

    let is_help = args
        .iter()
        .skip(1)
        .any(|a| a == "--help" || a == "-h" || a == "help");
    if is_help {
        print_help();
        return Ok(());
    }

    let purge = args.iter().skip(1).any(|a| a == "--purge" || a == "-p");
    let non_flags: Vec<&str> = args
        .iter()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .map(|s| s.as_str())
        .collect();

    let command = non_flags.first().copied().unwrap_or("run");

    match command {
        "install" => {
            init_logging();
            service::install()?;
            Ok(())
        }
        "uninstall" => {
            init_logging();
            service::uninstall(purge)?;
            Ok(())
        }
        "status" => {
            init_logging();
            service::status()?;
            Ok(())
        }
        "config" => {
            let path = default_config_path();
            println!("Config path: {}", path.display());
            if path.exists() {
                let content = std::fs::read_to_string(&path)?;
                println!("\n{}", content);

                let config_dir = config_dir_for_path(&path);
                let cert_path = config_dir.join("server.crt");
                if cert_path.exists() {
                    if let Ok(cert_pem) = std::fs::read_to_string(&cert_path) {
                        if let Ok(fp) = tls::cert_fingerprint_from_pem(&cert_pem) {
                            println!("TLS Certificate  : {}", cert_path.display());
                            println!("TLS Fingerprint  : {}", tls::format_fingerprint(&fp));
                        }
                    }
                }
            } else {
                println!("(Config file does not exist yet. Run 'ntx-server run' to initialize.)");
            }
            Ok(())
        }
        "run" => run_server().await,
        other => {
            eprintln!("Unknown command: '{}'\n", other);
            print_help();
            std::process::exit(1);
        }
    }
}

fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Received SIGINT, terminating server...");
            }
            _ = async {
                if let Some(ref mut s) = sigterm {
                    s.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                info!("Received SIGTERM, terminating server...");
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        info!("Received shutdown signal, terminating server...");
    }
}

async fn run_server() -> Result<()> {
    init_logging();

    let path = default_config_path();
    let (config, is_new_psk) = load_or_create_from_path(&path)?;
    let hostname = get_hostname(&config);
    let config_dir = config_dir_for_path(&path);
    let tls_material = tls::load_or_generate_tls(&config_dir, &hostname)?;
    let formatted_fp = tls::format_fingerprint(&tls_material.fingerprint_sha256);

    if is_new_psk {
        println!();
        println!("==================================================================");
        println!("               NTX PANEL AGENT — FIRST-TIME SETUP                 ");
        println!("==================================================================");
        println!("  Config saved to : {}", path.display());
        println!("  Node Hostname   : {}", hostname);
        println!(
            "  gRPC Bind       : {}:{}",
            config.server.bind_address, config.server.port
        );
        println!("  Pre-Shared Key  : {}", config.server.psk);
        println!("  TLS Fingerprint : {}", formatted_fp);
        println!("------------------------------------------------------------------");
        println!("  IMPORTANT: Copy this Pre-Shared Key (PSK) into your ntx-viewer  ");
        println!("  client and verify the TLS Fingerprint on first connection.      ");
        println!("==================================================================");
        println!();
    } else {
        info!("Loaded configuration from {}", path.display());
    }

    info!(
        "Starting NTX Server on node '{}' ({}:{}, TLS SHA-256: {})",
        hostname, config.server.bind_address, config.server.port, formatted_fp
    );

    let mdns = match discovery::start_announcement(&config, &tls_material.fingerprint_sha256) {
        Ok(daemon) => daemon,
        Err(e) => {
            tracing::error!("Failed to start mDNS announcement: {:#}", e);
            None
        }
    };

    let managed_services_path = ManagedServiceManager::default_storage_path(&path);
    let managed_services = ManagedServiceManager::load_or_create(managed_services_path);
    managed_services.auto_start_configured().await;

    let auth_manager = AuthManager::new(&config.server.psk, config.server.session_ttl_secs);
    let agent_service = NtxAgentService::with_managed_services(
        config.clone(),
        auth_manager.clone(),
        managed_services.clone(),
    );

    let auth_interceptor = auth_manager.clone();
    let interceptor = move |req: Request<()>| auth_interceptor.intercept(req);

    let addr: SocketAddr = format!("{}:{}", config.server.bind_address, config.server.port).parse()?;
    let tls_config = tls::server_tls_config(&tls_material)?;
    info!("gRPC TLS server listening on https://{}", addr);

    let serve_result = Server::builder()
        .tls_config(tls_config)?
        .add_service(NtxAgentServer::with_interceptor(agent_service, interceptor))
        .serve_with_shutdown(addr, shutdown_signal())
        .await;

    info!("Stopping supervised managed services...");
    managed_services.shutdown_all().await;

    if let Some(daemon) = mdns {
        let _ = daemon.shutdown();
    }

    serve_result?;
    info!("NTX Server stopped cleanly.");
    Ok(())
}
