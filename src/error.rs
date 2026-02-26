//! Unified HTTP error type for axum request handlers.
//!
//! [`AppError`] wraps [`anyhow::Error`] and converts it into an appropriate
//! HTTP response automatically via [`IntoResponse`]. This means every handler
//! that can fail can return `Result<T, AppError>` and propagate errors with `?`
//! — no manual `map_err`, no boilerplate.
//!
//! # Example
//!
//! ```rust,ignore
//! async fn my_handler(
//!     State(state): State<Arc<RouterState>>,
//! ) -> Result<Json<Value>, AppError> {
//!     let result = state.some_fallible_operation().await?;
//!     Ok(Json(result))
//! }
//! ```

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Wraps [`anyhow::Error`] so it can be returned from axum handlers.
///
/// Any type that implements `Into<anyhow::Error>` (which includes `io::Error`,
/// `reqwest::Error`, and any `#[derive(thiserror::Error)]` type) can be
/// converted into an [`AppError`] via the blanket [`From`] implementation.
#[derive(Debug)]
pub struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        tracing::warn!(error = %self.0, "handler error");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": self.0.to_string() })),
        )
            .into_response()
    }
}

/// Convert any `Into<anyhow::Error>` into an [`AppError`].
///
/// This is the idiomatic axum pattern — see
/// <https://docs.rs/axum/latest/axum/error_handling/index.html>.
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(e: E) -> Self {
        Self(e.into())
    }
}
