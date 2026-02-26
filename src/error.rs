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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    // -----------------------------------------------------------------------
    // IntoResponse
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn into_response_returns_500_with_json_error_body() {
        let err: AppError = anyhow::anyhow!("something went wrong").into();
        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "something went wrong");
    }

    #[tokio::test]
    async fn error_message_survives_io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let app_err: AppError = io_err.into();
        let response = app_err.into_response();

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json["error"].as_str().unwrap().contains("file missing"),
            "error text not propagated: {:?}",
            json
        );
    }

    // -----------------------------------------------------------------------
    // From conversions
    // -----------------------------------------------------------------------

    #[test]
    fn converts_from_anyhow_error() {
        let _: AppError = anyhow::anyhow!("plain anyhow").into();
    }

    #[test]
    fn converts_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let _: AppError = io_err.into();
    }

    // -----------------------------------------------------------------------
    // Debug formatting
    // -----------------------------------------------------------------------

    #[test]
    fn debug_format_includes_inner_error_message() {
        let err: AppError = anyhow::anyhow!("debug me").into();
        let s = format!("{err:?}");
        assert!(s.contains("debug me"), "debug output: {s}");
    }
}
