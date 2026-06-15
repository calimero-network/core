//! Resolve the effective requester identity for admin governance mutations.
//!
//! When a request is authenticated (the auth guard injected an
//! [`AuthenticatedKey`]), that identity is authoritative: a body-supplied
//! `requester` is accepted only if it matches, and a mismatch is rejected so a
//! caller cannot drive an operation as a different identity. When the request
//! is unauthenticated (Proxy auth mode, no embedded guard), the body
//! `requester` is used as-is, preserving the external-auth deployment model.

use axum::http::StatusCode;
use axum::Extension;
use calimero_primitives::identity::PublicKey;

use crate::admin::service::ApiError;
use crate::auth::AuthenticatedKey;

/// Resolve the effective requester from the authenticated identity (if any)
/// and the caller-supplied body value.
///
/// Returns `Err` (403) when an authenticated caller supplies a body
/// `requester` that disagrees with their token identity.
pub fn resolve_requester(
    auth_key: Option<Extension<AuthenticatedKey>>,
    body_requester: Option<PublicKey>,
) -> Result<Option<PublicKey>, ApiError> {
    match auth_key.map(|Extension(k)| k.0) {
        Some(authenticated) => match body_requester {
            Some(body) if body != authenticated => Err(ApiError {
                status_code: StatusCode::FORBIDDEN,
                message: "requester in request body does not match the authenticated identity"
                    .to_owned(),
            }),
            _ => Ok(Some(authenticated)),
        },
        None => Ok(body_requester),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(b: u8) -> PublicKey {
        PublicKey::from([b; 32])
    }

    #[test]
    fn authenticated_with_no_body_uses_authenticated() {
        let out = resolve_requester(Some(Extension(AuthenticatedKey(pk(1)))), None).unwrap();
        assert_eq!(out, Some(pk(1)));
    }

    #[test]
    fn authenticated_with_matching_body_uses_authenticated() {
        let out = resolve_requester(Some(Extension(AuthenticatedKey(pk(1)))), Some(pk(1))).unwrap();
        assert_eq!(out, Some(pk(1)));
    }

    #[test]
    fn authenticated_with_conflicting_body_is_rejected() {
        let err =
            resolve_requester(Some(Extension(AuthenticatedKey(pk(1)))), Some(pk(2))).unwrap_err();
        assert_eq!(err.status_code, StatusCode::FORBIDDEN);
    }

    #[test]
    fn unauthenticated_falls_back_to_body() {
        let out = resolve_requester(None, Some(pk(9))).unwrap();
        assert_eq!(out, Some(pk(9)));
    }

    #[test]
    fn unauthenticated_with_no_body_is_none() {
        let out = resolve_requester(None, None).unwrap();
        assert_eq!(out, None);
    }
}
