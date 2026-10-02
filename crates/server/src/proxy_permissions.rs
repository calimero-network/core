//! Taking the caller's permissions from the reverse proxy that authenticated it.
//!
//! Under [`AuthMode::Proxy`](crate::config::AuthMode::Proxy) core installs no
//! auth guard, so the route check is the proxy's: mero-auth's `/auth/validate`
//! decides whether a token may reach `/ws` or `/jsonrpc` at all. What it cannot
//! decide is what the request then asks for. `/ws` is admitted on
//! `context:subscribe` and then accepts `execute` frames; `/jsonrpc` names the
//! context and method in its body. Without the token's permissions the node
//! answered every such request as if the token could do anything.
//!
//! `/auth/validate` already says what the token carries, in
//! [`PERMISSIONS_HEADER`], and the proxy copies it onto the request it forwards.
//! This layer turns it into the same [`GrantedPermissions`] extension the
//! embedded guard injects.
//!
//! # Why this needs no opt-in
//!
//! Unlike [`crate::proxy_identity`], nothing here can widen what a caller may
//! do. A request that names no permissions is answered as proxy mode always
//! answered it; one that names some is held to them. A caller who writes the
//! header themselves can only narrow their own request, so the header needs to
//! be trustworthy only in the direction the proxy uses it.
//!
//! # Failing closed
//!
//! A header that is repeated or not text is refused with `401`. Dropping it
//! would answer the request with no permissions named, which in this mode is
//! the widest answer there is.

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::warn;

use crate::auth::GrantedPermissions;

/// The token's permissions, comma-joined, as mero-auth's `/auth/validate`
/// writes them.
pub(crate) static PERMISSIONS_HEADER: HeaderName = HeaderName::from_static("x-auth-permissions");

/// Why the proxy's permissions header was refused.
#[derive(Debug, Eq, PartialEq)]
enum Malformed {
    /// More than one value for a header the proxy writes exactly once.
    Repeated,
    /// A value that is not visible ASCII.
    NotText,
}

fn read(headers: &HeaderMap) -> Result<Option<Vec<String>>, Malformed> {
    let mut values = headers.get_all(&PERMISSIONS_HEADER).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(Malformed::Repeated);
    }
    let raw = value.to_str().map_err(|_| Malformed::NotText)?;
    Ok(Some(split(raw)))
}

/// Split mero-auth's comma-joined list back into permission strings.
///
/// The join is not reversible by a plain split: a permission's parameters are
/// themselves comma-separated (`context:execute[<ctx>,<identity>,<method>]`).
/// Only a comma outside brackets separates two permissions.
fn split(raw: &str) -> Vec<String> {
    let mut permissions = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (at, ch) in raw.char_indices() {
        match ch {
            '[' => depth = depth.saturating_add(1),
            ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                permissions.push(&raw[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    permissions.push(&raw[start..]);
    permissions
        .into_iter()
        .map(str::trim)
        .filter(|permission| !permission.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Inject the permissions the proxy named, and remove the header they came in.
pub(crate) async fn inject(mut request: Request<Body>, next: Next) -> Response {
    let permissions = read(request.headers());
    let _previous = request.headers_mut().remove(&PERMISSIONS_HEADER);

    match permissions {
        Ok(None) => {}
        Ok(Some(permissions)) => {
            let _previous = request
                .extensions_mut()
                .insert(GrantedPermissions(permissions.into()));
        }
        Err(malformed) => {
            warn!(
                ?malformed,
                "refusing a request whose proxy permissions header does not parse"
            );
            let mut response = StatusCode::UNAUTHORIZED.into_response();
            let _previous = response.headers_mut().insert(
                "X-Auth-Error",
                HeaderValue::from_static("invalid_permissions"),
            );
            return response;
        }
    }

    next.run(request).await
}

#[cfg(test)]
mod tests {
    use axum::routing::get;
    use axum::{Extension, Router};
    use tower::ServiceExt;

    use super::*;

    async fn echo(
        permissions: Option<Extension<GrantedPermissions>>,
        headers: HeaderMap,
    ) -> String {
        format!(
            "permissions={:?} leaked={}",
            permissions.map(|Extension(GrantedPermissions(held))| held.to_vec()),
            headers.contains_key(&PERMISSIONS_HEADER),
        )
    }

    async fn call(values: &[&str]) -> (StatusCode, String) {
        let app = Router::new()
            .route("/", get(echo))
            .layer(axum::middleware::from_fn(inject));
        let mut request = Request::builder().uri("/");
        for value in values {
            request = request.header(&PERMISSIONS_HEADER, *value);
        }
        let response = app
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    /// A scoped permission's own commas stay inside it, so the list mero-auth
    /// joined comes back as the permissions the token holds.
    #[test]
    fn a_joined_list_splits_back_into_the_permissions_that_were_joined() {
        assert_eq!(
            split("context:subscribe,context:execute[ctx-1,member-1,get],blob:add:stream"),
            [
                "context:subscribe",
                "context:execute[ctx-1,member-1,get]",
                "blob:add:stream"
            ],
        );
        assert_eq!(split("admin"), ["admin"]);
        assert!(split("").is_empty());
    }

    #[tokio::test]
    async fn the_proxy_named_permissions_reach_the_handler_and_the_header_does_not() {
        let (status, body) = call(&["context:subscribe,context:execute[ctx-1,,get]"]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            r#"permissions=Some(["context:subscribe", "context:execute[ctx-1,,get]"]) leaked=false"#
        );
    }

    #[tokio::test]
    async fn a_request_naming_none_goes_on_as_proxy_mode_always_answered_it() {
        let (status, body) = call(&[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "permissions=None leaked=false");
    }

    #[tokio::test]
    async fn a_repeated_header_is_refused_rather_than_picked_from() {
        let (status, _) = call(&["admin", "context:subscribe"]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
