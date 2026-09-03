pub mod applications;
pub mod devices;
pub mod pair_complete;
pub mod pair_init;
pub mod relink;

use reqwest::StatusCode;

use crate::admin::service::ApiError;

/// The 404 `GET /admin-api/identity` answers when this node holds neither a
/// usable device nor an account root. The account-scoped read endpoints answer
/// the same way - there is no account to report on either.
pub(crate) fn no_account_error() -> ApiError {
    ApiError {
        status_code: StatusCode::NOT_FOUND,
        message: "this node holds neither a usable device nor an account root yet; \
                  both are minted the first time it takes part in a namespace"
            .to_owned(),
    }
}

/// Decode a 64-hex-char field into 32 bytes. Lengths are already validated; the
/// decode is still fallible because validation and parsing are separate layers.
pub(crate) fn decode32(value: &str, field: &str) -> Result<[u8; 32], ApiError> {
    hex::decode(value)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("{field} must be 64 hex chars (32 bytes)"),
        })
}

/// Same, for the 64-byte pairing statement.
pub(crate) fn decode64(value: &str, field: &str) -> Result<[u8; 64], ApiError> {
    hex::decode(value)
        .ok()
        .and_then(|b| <[u8; 64]>::try_from(b).ok())
        .ok_or_else(|| ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("{field} must be 128 hex chars (64 bytes)"),
        })
}
