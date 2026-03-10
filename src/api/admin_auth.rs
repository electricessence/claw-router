//! Bearer-token authentication middleware for the admin API.
//!
//! When `admin_token_env` is configured in `[gateway]`, all admin routes
//! require an `Authorization: Bearer <token>` header. Requests with a missing
//! or incorrect token are rejected with `401 Unauthorized`.
//!
//! When `admin_token_env` is absent the middleware is a no-op — admin auth is
//! disabled. This is acceptable when the admin port is strictly firewalled to
//! trusted hosts only.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::router::RouterState;

/// Axum middleware: requires a valid `Authorization: Bearer <token>` header
/// on every admin route when `state.admin_token` is set.
pub async fn admin_auth_middleware(
    State(state): State<Arc<RouterState>>,
    req: Request,
    next: Next,
) -> Response {
    let Some(expected) = &state.admin_token else {
        // Auth disabled — pass through.
        return next.run(req).await;
    };

    let provided = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match provided {
        Some(token) if token == expected.as_str() => next.run(req).await,
        Some(_) => (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer realm=\"lm-gateway admin\"")],
            "Invalid admin token.",
        )
            .into_response(),
        None => (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer realm=\"lm-gateway admin\"")],
            "Admin API requires Authorization: Bearer <token>.",
        )
            .into_response(),
    }
}
