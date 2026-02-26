use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Context;
use tokio::signal;
use tracing::info;

mod api;
mod backends;
mod config;
mod error;
mod router;
mod traffic;

pub use config::Config;
pub use error::AppError;
pub use traffic::TrafficLog;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialise tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lm_gateway=info,tower_http=warn".into()),
        )
        .init();

    // Load config
    let config_path = std::env::var("LMG_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/etc/lm-gateway/config.toml"));

    let config = Config::load(&config_path)
        .with_context(|| format!("Failed to load config from {}", config_path.display()))?;

    info!(
        client_port = config.gateway.client_port,
        admin_port = config.gateway.admin_port,
        "lm-gateway starting"
    );

    let traffic_log = Arc::new(TrafficLog::new(config.gateway.traffic_log_capacity));
    let config = Arc::new(config);

    // Build router state
    let state = Arc::new(router::RouterState::new(Arc::clone(&config), Arc::clone(&traffic_log)));

    // Bind client API (agent-facing)
    let client_addr: SocketAddr = format!("0.0.0.0:{}", config.gateway.client_port).parse()?;

    // Bind admin API
    let admin_addr: SocketAddr = format!("0.0.0.0:{}", config.gateway.admin_port).parse()?;

    info!(%client_addr, "client API listening");
    info!(%admin_addr, "admin API listening");

    let client_listener = tokio::net::TcpListener::bind(client_addr).await?;
    let admin_listener = tokio::net::TcpListener::bind(admin_addr).await?;

    // Attach request tracing middleware to both servers
    let trace_layer = || {
        tower_http::trace::TraceLayer::new_for_http()
            .make_span_with(tower_http::trace::DefaultMakeSpan::new().level(tracing::Level::INFO))
            .on_response(tower_http::trace::DefaultOnResponse::new().level(tracing::Level::INFO))
    };

    let client_app = api::client::router(Arc::clone(&state)).layer(trace_layer());
    let admin_app = api::admin::router(Arc::clone(&state)).layer(trace_layer());

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
