use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Context;
use tokio::signal;
use tracing::info;

mod api;
mod backends;
mod config;
mod router;
mod traffic;

pub use config::Config;
pub use traffic::TrafficLog;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialise tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "claw_router=info,tower_http=warn".into()),
        )
        .init();

    // Load config
    let config_path = std::env::var("CLAW_ROUTER_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/etc/claw-router/config.toml"));

    let config = Config::load(&config_path)
        .with_context(|| format!("Failed to load config from {}", config_path.display()))?;

    info!(
        client_port = config.gateway.client_port,
        admin_port = config.gateway.admin_port,
        "claw-router starting"
    );

    let traffic_log = Arc::new(TrafficLog::new(config.gateway.traffic_log_capacity));
    let config = Arc::new(config);

    // Build router state
    let state = Arc::new(router::RouterState::new(Arc::clone(&config), Arc::clone(&traffic_log)));

    // Bind client API (ZeroClaw-facing)
    let client_addr: SocketAddr = format!("0.0.0.0:{}", config.gateway.client_port).parse()?;
    let client_app = api::client::router(Arc::clone(&state));

    // Bind admin API
    let admin_addr: SocketAddr = format!("0.0.0.0:{}", config.gateway.admin_port).parse()?;
    let admin_app = api::admin::router(Arc::clone(&state));

    info!(%client_addr, "client API listening");
    info!(%admin_addr, "admin API listening");

    let client_listener = tokio::net::TcpListener::bind(client_addr).await?;
    let admin_listener = tokio::net::TcpListener::bind(admin_addr).await?;

    tokio::select! {
        result = axum::serve(client_listener, client_app) => {
            result.context("client API server error")?;
        }
        result = axum::serve(admin_listener, admin_app) => {
            result.context("admin API server error")?;
        }
        _ = shutdown_signal() => {
            info!("shutdown signal received");
        }
    }

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
