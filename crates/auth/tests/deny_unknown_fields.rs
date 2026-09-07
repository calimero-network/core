//! Every request body `mero-auth` deserializes is a closed set: a client
//! sending a field the service no longer knows gets a 400, never a silent
//! drop. A struct is listed here when it gains `deny_unknown_fields`; a new
//! request type belongs here too.

use mero_auth::api::handlers::auth::{BaseTokenRequest, RefreshTokenRequest, RevokeTokenRequest};
use mero_auth::api::handlers::client_keys::GenerateClientKeyRequest;
use mero_auth::api::handlers::permissions::UpdateKeyPermissionsRequest;
use mero_auth::api::handlers::root_keys::CreateKeyRequest;

macro_rules! rejects_unknown_fields {
    ($($ty:ty),* $(,)?) => {
        $({
            let err = serde_json::from_str::<$ty>(r#"{"bogus":1}"#)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(
                err.contains("unknown field `bogus`"),
                "{} accepts unknown fields: {err:?}",
                stringify!($ty)
            );
        })*
    };
}

#[test]
fn every_request_body_is_a_closed_set() {
    rejects_unknown_fields!(
        BaseTokenRequest,
        RefreshTokenRequest,
        RevokeTokenRequest,
        CreateKeyRequest,
        GenerateClientKeyRequest,
        UpdateKeyPermissionsRequest,
    );
}

// `/auth/mock-token` only exists in debug builds; its request type is
// compiled out of a release build the same way.
#[cfg(debug_assertions)]
#[test]
fn mock_token_request_is_a_closed_set() {
    use mero_auth::api::handlers::auth::MockTokenRequest;

    rejects_unknown_fields!(MockTokenRequest);
}
