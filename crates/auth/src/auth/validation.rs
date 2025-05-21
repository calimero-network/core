use axum::{
    body::Body,
    extract::{FromRequest, Json},
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use thiserror::Error;
use validator::Validate;

/// Validation error types
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Invalid JSON: {0}")]
    JsonError(#[from] axum::extract::rejection::JsonRejection),

    #[error("Validation error: {0}")]
    ValidationError(#[from] validator::ValidationErrors),
}

impl IntoResponse for ValidationError {
    fn into_response(self) -> Response {
        let error_message = self.to_string();
        let status = match self {
            ValidationError::InvalidInput(_) => StatusCode::BAD_REQUEST,
            ValidationError::JsonError(_) => StatusCode::BAD_REQUEST,
            ValidationError::ValidationError(_) => StatusCode::UNPROCESSABLE_ENTITY,
        };

        let body = Json(serde_json::json!({
            "error": error_message,
            "status": status.as_u16()
        }));

        (status, body).into_response()
    }
}

/// Validated JSON extractor that performs input validation and sanitization
pub struct ValidatedJson<T>(pub T);

#[async_trait]
impl<T, S> FromRequest<S> for ValidatedJson<T>
where
    T: DeserializeOwned + Validate + Send + 'static,
    S: Send + Sync + 'static,
{
    type Rejection = ValidationError;

    async fn from_request(req: Request<Body>, state: &S) -> Result<Self, Self::Rejection> {
        // Extract JSON body
        let Json(data) = Json::<T>::from_request(req, state).await?;

        // Validate the data
        data.validate()?;

        Ok(ValidatedJson(data))
    }
}

/// Sanitize a string by removing potentially dangerous characters
pub fn sanitize_string(input: &str) -> String {
    // Remove control characters and non-UTF8 sequences
    input
        .chars()
        .filter(|&c| !c.is_control() && c.is_ascii())
        .collect()
}

/// Sanitize an identifier (more strict than general string sanitization)
pub fn sanitize_identifier(input: &str) -> String {
    input
        .chars()
        .filter(|&c| c.is_alphanumeric() || c == '-' || c == '_')
        .collect()
}

/// HTML escape a string to prevent XSS
pub fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use validator::Validate;

    #[derive(Debug, Deserialize, Validate)]
    struct TestInput {
        #[validate(length(min = 3, max = 50))]
        name: String,
        
        #[validate(range(min = 0, max = 150))]
        age: u32,
        
        #[validate(email)]
        email: String,
    }

    #[test]
    fn test_string_sanitization() {
        let input = "Hello\0World\n<script>alert('xss')</script>";
        let sanitized = sanitize_string(input);
        assert_eq!(sanitized, "HelloWorld<script>alert('xss')</script>");
    }

    #[test]
    fn test_identifier_sanitization() {
        let input = "user-name123!@#$%^&*()";
        let sanitized = sanitize_identifier(input);
        assert_eq!(sanitized, "user-name123");
    }

    #[test]
    fn test_html_escape() {
        let input = "<script>alert('xss')</script>";
        let escaped = escape_html(input);
        assert_eq!(escaped, "&lt;script&gt;alert(&#x27;xss&#x27;)&lt;/script&gt;");
    }
} 