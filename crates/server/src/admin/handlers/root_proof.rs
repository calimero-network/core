//! Shared parsing for the `rootProof` field every root-guarded owner op takes.

use axum::http::StatusCode;
use calimero_account::{AccountId, SignedOwnerOp};

use crate::admin::service::ApiError;

/// Decode an optional hex, borsh-encoded `SignedOwnerOp`.
///
/// Decoded here rather than in `validate`, which cannot see the op and so could
/// only confirm the string is hex. Whether the proof authorises the op is the
/// context manager's check, made before anything is published.
pub(crate) fn decode(raw: Option<&str>) -> Result<Option<SignedOwnerOp>, ApiError> {
    raw.map(calimero_context::root_guard::decode_root_proof)
        .transpose()
        .map_err(|message| ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message,
        })
}

/// Decode a hex `AccountId` that `validate` has already confirmed is 32 bytes.
pub(crate) fn account(field: &str, raw: &str) -> Result<AccountId, ApiError> {
    hex::decode(raw)
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .map(AccountId::from)
        .ok_or_else(|| ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("{field} must be 64 hex chars (32 bytes)"),
        })
}
