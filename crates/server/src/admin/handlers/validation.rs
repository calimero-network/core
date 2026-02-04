//! Validation middleware and extractors for request validation.
//!
//! This module provides a `ValidatedJson` extractor that validates
//! request payloads before passing them to handlers.

use axum::extract::rejection::JsonRejection;
use axum::extract::FromRequest;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{async_trait, Json};
use calimero_server_primitives::validation::{Validate, ValidationError};
use serde::de::DeserializeOwned;

use crate::admin::service::ApiError;

/// A JSON extractor that validates the request payload.
///
/// This extractor first deserializes the JSON body, then runs validation
/// on the deserialized value. If validation fails, it returns a 400 Bad Request
/// response with details about the validation errors.
///
/// # Example
///
/// ```ignore
/// use crate::admin::handlers::validation::ValidatedJson;
///
/// pub async fn handler(
///     ValidatedJson(req): ValidatedJson<MyRequest>,
/// ) -> impl IntoResponse {
///     // req is guaranteed to have passed validation
/// }
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct ValidatedJson<T>(pub T);

/// Rejection type for `ValidatedJson` extractor
#[derive(Debug)]
pub enum ValidatedJsonRejection {
    /// JSON parsing failed
    JsonError(JsonRejection),
    /// Validation failed
    ValidationError(Vec<ValidationError>),
}

impl IntoResponse for ValidatedJsonRejection {
    fn into_response(self) -> Response {
        match self {
            Self::JsonError(rejection) => {
                let message = match &rejection {
                    JsonRejection::JsonDataError(e) => {
                        format!("Invalid JSON data: {}", e.body_text())
                    }
                    JsonRejection::JsonSyntaxError(e) => {
                        format!("JSON syntax error: {}", e.body_text())
                    }
                    JsonRejection::MissingJsonContentType(e) => e.body_text().to_string(),
                    JsonRejection::BytesRejection(e) => {
                        format!("Failed to read request body: {}", e.body_text())
                    }
                    _ => "Invalid JSON request".to_owned(),
                };
                ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message,
                }
                .into_response()
            }
            Self::ValidationError(errors) => {
                let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
                let message = if messages.len() == 1 {
                    messages.into_iter().next().unwrap_or_default()
                } else {
                    format!("Validation errors: {}", messages.join("; "))
                };
                ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message,
                }
                .into_response()
            }
        }
    }
}

#[async_trait]
impl<T, S> FromRequest<S> for ValidatedJson<T>
where
    T: DeserializeOwned + Validate,
    S: Send + Sync,
{
    type Rejection = ValidatedJsonRejection;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        // First, extract the JSON
        let Json(value) = Json::<T>::from_request(req, state)
            .await
            .map_err(ValidatedJsonRejection::JsonError)?;

        // Then validate it
        let errors = value.validate();
        if !errors.is_empty() {
            return Err(ValidatedJsonRejection::ValidationError(errors));
        }

        Ok(Self(value))
    }
}

/// Validates a request and returns an error response if validation fails.
///
/// This function can be used in handlers that receive `Json<T>` directly
/// and want to validate after extraction.
///
/// # Example
///
/// ```ignore
/// pub async fn handler(
///     Json(req): Json<MyRequest>,
/// ) -> impl IntoResponse {
///     if let Err(response) = validate_request(&req) {
///         return response;
///     }
///     // proceed with valid request
/// }
/// ```
pub fn validate_request<T: Validate>(req: &T) -> Result<(), Response> {
    let errors = req.validate();
    if errors.is_empty() {
        Ok(())
    } else {
        let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
        let message = if messages.len() == 1 {
            messages.into_iter().next().unwrap_or_default()
        } else {
            format!("Validation errors: {}", messages.join("; "))
        };
        Err(ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message,
        }
        .into_response())
    }
}
