//! API errors: `{ "error": CODE, "message": ..., ... }` with an HTTP status.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ApiError {
    #[serde(default = "default_status")]
    pub status: u16,
    #[serde(rename = "error")]
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "is_null")]
    pub details: Box<Value>,
}

#[allow(clippy::borrowed_box, reason = "serde passes the field by reference")]
fn is_null(v: &Box<Value>) -> bool {
    v.is_null()
}

const fn default_status() -> u16 {
    500
}

impl ApiError {
    #[must_use]
    pub fn new(status: u16, code: &str, message: &str) -> Self {
        Self {
            status,
            code: code.to_owned(),
            message: message.to_owned(),
            details: Box::new(Value::Null),
        }
    }

    #[must_use]
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Box::new(details);
        self
    }

    #[must_use]
    pub fn bad_request(code: &str, message: &str) -> Self {
        Self::new(400, code, message)
    }

    #[must_use]
    pub fn unauthenticated(message: &str) -> Self {
        Self::new(401, "UNAUTHENTICATED", message)
    }

    #[must_use]
    pub fn forbidden(message: &str) -> Self {
        Self::new(403, "FORBIDDEN", message)
    }

    #[must_use]
    pub fn not_found(message: &str) -> Self {
        Self::new(404, "NOT_FOUND", message)
    }

    #[must_use]
    pub fn conflict(message: &str) -> Self {
        Self::new(409, "CONFLICT", message)
    }

    #[must_use]
    pub fn unavailable(message: &str) -> Self {
        Self::new(503, "UNAVAILABLE", message)
    }

    #[must_use]
    pub fn internal(message: &str) -> Self {
        Self::new(500, "INTERNAL", message)
    }

    /// `410 UNSUPPORTED_ENGINE`: the app is an SDK 0.12 (wasmer) app, which this
    /// node no longer runs. It can be listed and deleted, nothing else.
    #[must_use]
    pub fn unsupported_engine() -> Self {
        Self::new(410, "UNSUPPORTED_ENGINE", crate::app::WASMER_GONE)
    }

    /// `400 UNSUPPORTED_BUNDLE`: the bundle's WASM is a core module (SDK 0.12, for
    /// wasmer), not a `calimero:app` component, so no app can run it here.
    #[must_use]
    pub fn unsupported_bundle() -> Self {
        Self::bad_request("UNSUPPORTED_BUNDLE", UNSUPPORTED_BUNDLE)
    }
}

/// Why a core-module bundle is refused ([`ApiError::unsupported_bundle`]).
pub const UNSUPPORTED_BUNDLE: &str = "this bundle is a WASM core module (an SDK 0.12 app, for wasmer), which this node no longer runs; rebuild it with calimero-app-sdk into a calimero:app component";

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let code = crate::metrics::ErrorCode(self.code.clone());
        let mut response = (status, axum::Json(self)).into_response();
        let _prev = response.extensions_mut().insert(code);
        response
    }
}
