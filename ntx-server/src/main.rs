#![allow(clippy::result_large_err)]

use anyhow::Result;
use ntx_proto::ntx_agent_server::NtxAgentServer;
use ntx_server::auth::AuthManager;
use ntx_server::config::{default_config_path, get_hostname, load_or_create};
use ntx_server::server::NtxAgentService;
use ntx_server::{discovery, service};
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
    install        Install the agent as a system service
    uninstall      Uninstall the agent system service
                   Options:
                     --purge, -p    Remove binary and wipe all configuration directories
    status         Show the status of the agent service
    config         Print the path and contents of the configuration file
    help, --help   Show this help message
"#,
        env!("CARGO_PKG_VERSION")
    );
}

#[tokio::main]
async fn main() -> Result<()> {
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

async fn run_server() -> Result<()> {
    init_logging();

    let (config, is_new_psk) = load_or_create()?;
    let path = default_config_path();
    let hostname = get_hostname(&config);

    if is_new_psk {
        println!();
        println!("==================================================================");
        println!("               NTX PANEL AGENT — FIRST-TIME SETUP                 ");
        println!("==================================================================");
        println!("  Config saved to : {}", path.display());
        println!("  Node Hostname   : {}", hostname);
        println!("  gRPC Port       : {}", config.server.port);
        println!("  Pre-Shared Key  : {}", config.server.psk);
        println!("------------------------------------------------------------------");
        println!("  IMPORTANT: Copy this Pre-Shared Key (PSK) into your ntx-viewer  ");
        println!("  client to connect securely to this agent.                       ");
        println!("==================================================================");
        println!();
    } else {
        info!("Loaded configuration from {}", path.display());
    }

    info!(
        "Starting NTX Server on node '{}' (port {})",
        hostname, config.server.port
    );

    // Start mDNS announcement if enabled
    let _mdns = match discovery::start_announcement(&config) {
        Ok(daemon) => daemon,
        Err(e) => {
            tracing::error!("Failed to start mDNS announcement: {:#}", e);
            None
        }
    };

    let auth_manager = AuthManager::new(&config.server.psk);
    let agent_service = NtxAgentService::new(config.clone(), auth_manager.clone());

    let auth_interceptor = auth_manager.clone();
    let interceptor = move |req: Request<()>| auth_interceptor.intercept(req);

    let addr: SocketAddr = format!("0.0.0.0:{}", config.server.port).parse()?;
    info!("gRPC server listening on {}", addr);

    Server::builder()
        .add_service(NtxAgentServer::with_interceptor(agent_service, interceptor))
        .serve_with_shutdown(addr, async {
            tokio::signal::ctrl_c().await.ok();
            info!("Received shutdown signal, terminating server...");
        })
        .await?;

    info!("NTX Server stopped cleanly.");
    Ok(())
}
