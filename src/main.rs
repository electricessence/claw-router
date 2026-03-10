use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use anyhow::Context;
use tokio::signal;
use tracing::{info, warn};

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
    // When invoked as a Docker HEALTHCHECK, hit /healthz and exit immediately.
    // This avoids needing any external tool (curl/wget) in the container image.
    if std::env::args().nth(1).as_deref() == Some("--healthcheck") {
        return healthcheck().await;
    }

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
    let state = Arc::new(router::RouterState::new(
        Arc::clone(&config),
        config_path.clone(),
        Arc::clone(&traffic_log),
    ));

    // Spawn hot-reload watcher — polls the config file every 5 seconds
    tokio::spawn(config_watcher(Arc::clone(&state)));

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

    let client_app = api::client::router(Arc::clone(&state))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            api::client_auth::client_auth_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            api::rate_limit::rate_limit_middleware,
        ))
        .layer(axum::middleware::from_fn(api::request_id::request_id_middleware))
        .layer(trace_layer());
    let admin_app = api::admin::router(Arc::clone(&state))
        .layer(axum::middleware::from_fn(api::request_id::request_id_middleware))
        .layer(trace_layer());

    tokio::select! {
        result = axum::serve(client_listener, client_app.into_make_service_with_connect_info::<SocketAddr>()) => {
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

/// Lightweight healthcheck: GET /healthz and exit 0 on 200, 1 otherwise.
/// Invoked via `lm-gateway --healthcheck` from Docker HEALTHCHECK.
async fn healthcheck() -> anyhow::Result<()> {
    let port = std::env::var("LMG_CLIENT_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(8080);

    let url = format!("http://127.0.0.1:{port}/healthz");
    let resp = reqwest::get(&url).await?;

    if resp.status().is_success() {
        std::process::exit(0);
    } else {
        std::process::exit(1);
    }
}

/// Background task: polls the config file every 5 seconds and hot-reloads on change.
///
/// Uses filesystem `mtime` for change detection — no inotify/kqueue dependencies.
/// Parse failures are logged and ignored; the running config is unchanged.
async fn config_watcher(state: Arc<router::RouterState>) {
    let path = &state.config_path;

    let mut last_mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok();

    // Initial tick fires immediately; skip it so we don't reload on startup.
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.tick().await;

    loop {
        interval.tick().await;

        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if mtime == last_mtime {
            continue;
        }

        match Config::load(path) {
            Ok(new_cfg) => {
                state.replace_config(Arc::new(new_cfg));
                info!(path = %path.display(), "config hot-reloaded");
                last_mtime = mtime;
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e, "config reload failed — keeping previous config");
            }
        }
    }
}
