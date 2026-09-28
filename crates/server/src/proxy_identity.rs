//! Taking the caller's identity from the reverse proxy that authenticated it.
//!
//! # Why this exists
//!
//! Under [`AuthMode::Proxy`](crate::config::AuthMode::Proxy) core installs no
//! auth guard, so no request carries [`AuthenticatedAccount`]. Every handler
//! that narrows by caller then reads it as "no identity" and answers for the
//! whole node: the listings in `admin/caller_scope.rs` return every tenant's
//! rows, and `POST /contexts/:id/query` refuses outright, because it answers
//! "what may THIS account see" and has no account to answer for.
//!
//! That was right while the only sessions a proxy could mint were the node
//! owner's. It stops being right once the proxy's auth service serves
//! `account_proof` logins: a fleet relay then lets several tenants through the
//! same door, and the node cannot tell them apart.
//!
//! The proxy already knows who they are. mero-auth's `/auth/validate` names an
//! account-anchored session's account and device in [`ACCOUNT_HEADER`] and
//! [`DEVICE_HEADER`], and the proxy copies them onto the request it forwards.
//! This layer turns those into the same extensions the embedded guard injects,
//! so every handler downstream behaves exactly as it does under embedded auth.
//!
//! # What makes the headers trustworthy
//!
//! Nothing here, and nothing can: a header is whatever the last hop wrote. It
//! is trustworthy only because the proxy **replaces** these headers on every
//! route it authenticates and **strips** them on every route it does not.
//! That is a deployment property, which is why this is off unless the operator
//! turns it on (`server.proxy_identity`), and why turning it on for a node
//! reachable other than through such a proxy would let any caller name any
//! account.
//!
//! Two things this layer does to keep that property from leaking further:
//!
//! * the headers are **removed** from the request once read, so nothing past
//!   this point can read them a second time and reach a different answer;
//! * a sealed request never carries them — [`crate::sealed`] drops both from
//!   the request it opens, since the proxy cannot see inside an envelope and so
//!   cannot have written them.
//!
//! # Failing closed
//!
//! A header that is present but does not parse is refused with `401`, not
//! ignored. Ignoring it would leave the request with no identity, which in this
//! mode is the node-wide answer — the widest one there is.

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use calimero_account::AccountId;
use calimero_primitives::identity::DeviceId;
use tracing::{debug, warn};

use crate::auth::{AuthenticatedAccount, AuthenticatedDevice};

/// The account an account-anchored session belongs to, as the proxy says.
pub(crate) static ACCOUNT_HEADER: HeaderName = HeaderName::from_static("x-auth-account");

/// The device that opened that session, as the proxy says.
pub(crate) static DEVICE_HEADER: HeaderName = HeaderName::from_static("x-auth-device");

/// What the proxy said about this caller.
#[derive(Debug, Eq, PartialEq)]
enum Identity {
    /// Nothing: not an account-anchored session. The request goes on as it
    /// always has in proxy mode.
    None,
    /// An account, and the device that opened its session when one was named
    /// and parsed.
    Account(AccountId, Option<DeviceId>),
}

/// Why the proxy's identity headers were refused.
#[derive(Debug, Eq, PartialEq)]
enum Malformed {
    /// More than one value for a header the proxy writes exactly once.
    Repeated,
    /// A value that is not an account id.
    Account,
    /// A device with no account beside it.
    DeviceWithoutAccount,
}

/// A header's single value, if it has one.
fn single<'h>(
    headers: &'h HeaderMap,
    name: &HeaderName,
) -> Result<Option<&'h HeaderValue>, Malformed> {
    let mut values = headers.get_all(name).iter();
    let first = values.next();
    if values.next().is_some() {
        return Err(Malformed::Repeated);
    }
    Ok(first)
}

fn read(headers: &HeaderMap) -> Result<Identity, Malformed> {
    let account = single(headers, &ACCOUNT_HEADER)?;
    let device = single(headers, &DEVICE_HEADER)?;

    let Some(account) = account else {
        // A device names a device OF an account. Alone it identifies nobody,
        // and a proxy that wrote one without the other is not the proxy this
        // mode assumes.
        return match device {
            Some(_) => Err(Malformed::DeviceWithoutAccount),
            None => Ok(Identity::None),
        };
    };

    let account = account
        .to_str()
        .ok()
        .and_then(|raw| raw.parse::<AccountId>().ok())
        .ok_or(Malformed::Account)?;

    // An unparseable device grants no device rather than refusing the request,
    // as the embedded guard does for the same claim: the device is what
    // revocation filters by, and the account is still known. What it must never
    // do is grant a DIFFERENT device.
    let device = device.and_then(|value| {
        let parsed = value
            .to_str()
            .ok()
            .and_then(|raw| raw.parse::<DeviceId>().ok());
        if parsed.is_none() {
            warn!(%account, "proxy named a device that does not parse; granting no device");
        }
        parsed
    });

    Ok(Identity::Account(account, device))
}

/// Inject the identity the proxy vouched for, and remove the headers it came in.
pub(crate) async fn inject(mut request: Request<Body>, next: Next) -> Response {
    let identity = read(request.headers());

    let headers = request.headers_mut();
    let _previous = headers.remove(&ACCOUNT_HEADER);
    let _previous = headers.remove(&DEVICE_HEADER);

    match identity {
        Ok(Identity::None) => {}
        Ok(Identity::Account(account, device)) => {
            debug!(%account, "proxy-vouched account session: granting AuthenticatedAccount");
            let _previous = request
                .extensions_mut()
                .insert(AuthenticatedAccount(account));
            if let Some(device) = device {
                let _previous = request.extensions_mut().insert(AuthenticatedDevice(device));
            }
        }
        Err(malformed) => {
            warn!(
                ?malformed,
                "refusing a request whose proxy identity headers do not parse"
            );
            let mut response = StatusCode::UNAUTHORIZED.into_response();
            let _previous = response
                .headers_mut()
                .insert("X-Auth-Error", HeaderValue::from_static("invalid_identity"));
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

    fn account() -> AccountId {
        AccountId::from([7; 32])
    }

    fn device() -> DeviceId {
        DeviceId::from([9; 32])
    }

    /// Echo what the handler was given: the account, the device, and whether
    /// either header survived past the layer.
    async fn echo(
        account: Option<Extension<AuthenticatedAccount>>,
        device: Option<Extension<AuthenticatedDevice>>,
        headers: HeaderMap,
    ) -> String {
        format!(
            "account={} device={} leaked={}",
            account.map_or_else(
                || "-".to_owned(),
                |Extension(AuthenticatedAccount(a))| a.to_string()
            ),
            device.map_or_else(
                || "-".to_owned(),
                |Extension(AuthenticatedDevice(d))| hex::encode(d.as_bytes())
            ),
            headers.contains_key(&ACCOUNT_HEADER) || headers.contains_key(&DEVICE_HEADER),
        )
    }

    async fn call(headers: &[(&HeaderName, &str)]) -> (StatusCode, String) {
        let app = Router::new()
            .route("/", get(echo))
            .layer(axum::middleware::from_fn(inject));
        let mut request = Request::builder().uri("/");
        for (name, value) in headers {
            request = request.header(*name, *value);
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

    #[tokio::test]
    async fn no_headers_leave_the_request_as_proxy_mode_always_had_it() {
        let (status, body) = call(&[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "account=- device=- leaked=false");
    }

    #[tokio::test]
    async fn an_account_and_its_device_are_granted_and_the_headers_removed() {
        let account = account().to_string();
        let device = hex::encode(device().as_bytes());
        let (status, body) = call(&[(&ACCOUNT_HEADER, &account), (&DEVICE_HEADER, &device)]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            format!("account={account} device={device} leaked=false")
        );
    }

    #[tokio::test]
    async fn an_account_without_a_device_is_still_an_account() {
        let account = account().to_string();
        let (status, body) = call(&[(&ACCOUNT_HEADER, &account)]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("account={account} device=- leaked=false"));
    }

    #[tokio::test]
    async fn an_unparseable_device_grants_no_device_but_keeps_the_account() {
        let account = account().to_string();
        let (status, body) = call(&[
            (&ACCOUNT_HEADER, &account),
            (&DEVICE_HEADER, "not-a-device"),
        ])
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("account={account} device=- leaked=false"));
    }

    /// The case failing closed is for: ignored, the request would carry no
    /// identity, and in proxy mode that is the node-wide answer.
    #[tokio::test]
    async fn an_unparseable_account_is_refused_rather_than_read_as_nobody() {
        let (status, _body) = call(&[(&ACCOUNT_HEADER, "not-an-account")]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_device_without_an_account_is_refused() {
        let device = hex::encode(device().as_bytes());
        let (status, _body) = call(&[(&DEVICE_HEADER, &device)]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_repeated_account_header_is_refused() {
        let first = account().to_string();
        let second = AccountId::from([8; 32]).to_string();
        let (status, _body) = call(&[(&ACCOUNT_HEADER, &first), (&ACCOUNT_HEADER, &second)]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
