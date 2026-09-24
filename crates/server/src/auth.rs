use std::convert::Infallible;
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::proof_auth::{ProofPolicy, Refusal, MAX_PROVEN_BODY, PROOF_HEADER};
use axum::body::Body;
use axum::extract::OriginalUri;
use axum::http::{HeaderValue, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use calimero_governance_store::NamespaceRepository;
use calimero_store::Store;
use eyre::Result;
use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use mero_auth::auth::permissions::PermissionValidator;
use mero_auth::embedded::{build_app, default_config, EmbeddedAuthApp};
use mero_auth::{AuthError, AuthService};
use tower::{Layer, Service};
use tracing::{debug, info, warn};

use crate::config::ServerConfig;

/// Build the failure response for a rejected token, keeping the Bearer and
/// query-param paths in sync.
///
/// The status and `X-Auth-Error` hint are chosen by matching the typed
/// [`AuthError`] variant, not by inspecting message text:
/// - [`AuthError::TokenExpired`] → `401` with `token_expired`
/// - [`AuthError::TokenRevoked`] → `403` with `token_revoked`
/// - everything else → bare `401`
///
/// `403` for a revoked token is deliberate: `401` invites the client to
/// re-authenticate and retry, which is exactly wrong for a dead credential.
fn unauthorized_response(err: &AuthError) -> Response {
    match err {
        AuthError::TokenExpired => {
            let mut resp = StatusCode::UNAUTHORIZED.into_response();
            resp.headers_mut()
                .insert("X-Auth-Error", HeaderValue::from_static("token_expired"));
            resp
        }
        AuthError::TokenRevoked => {
            let mut resp = StatusCode::FORBIDDEN.into_response();
            resp.headers_mut()
                .insert("X-Auth-Error", HeaderValue::from_static("token_revoked"));
            resp
        }
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// The authenticated requester's public key, injected into request extensions
/// by [`AuthGuardService`] after token verification.
///
/// Handlers extract this via `Extension(AuthenticatedKey(pk))` and use it as
/// the effective requester instead of trusting the value from the request body.
#[derive(Clone, Debug)]
pub struct AuthenticatedKey(pub calimero_primitives::identity::PublicKey);

/// Marker injected by [`AuthGuardService`] when a request carries a valid token
/// but the auth method does not produce a cryptographic public key (e.g.
/// embedded username/password). The presence of this extension tells handlers
/// that the caller is the node owner, positively confirmed by the auth layer.
///
/// Using an explicit marker instead of relying on `Option<AuthenticatedKey>`
/// being `None` makes the bypass path auditable: `None` for both extensions
/// means the auth guard did not run (no-auth mode), not that a specific auth
/// method was used. Handlers can match on both extensions and reason about
/// exactly which auth path was taken.
#[derive(Clone, Debug)]
pub struct AuthenticatedNodeOwner;

/// The authenticated requester's **account**, injected by [`AuthGuardService`]
/// when the session is anchored to an account rather than to a key row in this
/// node's auth store — today, an `account_proof` login.
///
/// Separate from [`AuthenticatedNodeOwner`] because such a caller is precisely
/// not the node owner: they hold a device certified under their own account
/// root and may well be a tenant on a relay somebody else runs. Before this
/// existed they landed in the node-owner arm, because the store lookup
/// collapsed "no row" into the same answer as "a client key with no public
/// key".
///
/// Holding one of these says the auth layer authenticated *this account*. It
/// says nothing about what the account may do: membership and capabilities are
/// at-cut questions, answered per request against the target context's group,
/// never cached from the session.
#[derive(Clone, Debug)]
pub struct AuthenticatedAccount(pub calimero_account::AccountId);

/// The device whose key signed this request, when the caller proved it on the
/// request itself.
///
/// Separate from [`AuthenticatedAccount`] rather than a field on it, because
/// the two are known on different paths and collapsing them would hide that. A
/// request-carried proof names a device — the certificate says which one. A
/// session does not: `account_proof` mints a token whose subject is the
/// ACCOUNT, and the device that logged in is discarded at that point.
///
/// That asymmetry is load-bearing downstream. Device revocation is a
/// per-device, per-group governance row, so a caller whose device is unknown
/// cannot be filtered by it — which is a real gap for sessions, not a property
/// of the design. Making it a separate extension keeps the gap visible at every
/// call site instead of hiding an `Option` inside a struct everyone
/// destructures.
#[derive(Clone, Debug)]
pub struct AuthenticatedDevice(pub calimero_primitives::identity::DeviceId);

/// Wrapper around the embedded authentication application, keeping the router and shared state.
pub struct BundledAuth {
    app: EmbeddedAuthApp,
}

impl BundledAuth {
    #[must_use]
    pub fn auth_service(&self) -> AuthService {
        self.app.state.auth_service.clone()
    }

    pub fn into_router(self) -> Router {
        self.app.router
    }
}

/// Name this node in the auth config, so `account_proof` can tell a login
/// statement minted for it from one minted elsewhere.
///
/// `account_proof` refuses to start without `auth.account_proof.node_key`,
/// deliberately: a guessed value would accept statements addressed to a
/// different node. Nothing set it, so the provider could not start anywhere.
///
/// **Which key.** The node's *device signing key* — the one it signs ops with,
/// and the one its device certificate certifies. That is what the client
/// contract means by learning the node's identity "from a pinned certificate":
/// a libp2p identity key is a network address, certified by nothing, and
/// pinning it would pin something no certificate attests to.
///
/// Resolved here rather than in `merod run` because it lives in the datastore,
/// which `run` never opens — it hands the config to the node, which opens it
/// later. This is the first point that holds both the auth config and a store.
///
/// Read once at startup, so a device minted or rotated afterwards is picked up
/// on the next restart. That matches the field being configuration: a value
/// clients have pinned should not change under them mid-session.
///
/// Filled **only when unset**, so an operator who pinned a value keeps it, and
/// a node answering on several identities can name the one its clients pinned.
fn name_this_node(config: &mut mero_auth::config::AuthConfig, datastore: &Store) {
    if config.account_proof.node_key.is_some() {
        return;
    }

    match NamespaceRepository::new(datastore).node_identity() {
        Ok(Some(identity)) => {
            let key = identity.public_key.to_string();
            info!(
                node_key = %key,
                "embedded auth: naming this node for account-proof logins",
            );
            config.account_proof.node_key = Some(key);
        }
        // Not an error: a node mints its signing key the first time it takes
        // part in a namespace, so a fresh one legitimately has none yet. Leaving
        // the field unset keeps `account_proof` refusing to start on its own
        // terms rather than starting with a key it invented.
        Ok(None) => info!(
            "embedded auth: this node has no signing key yet, so account-proof logins stay \
             disabled; it mints one the first time it takes part in a namespace",
        ),
        // A failed read is not a missing row, and treating them alike would say
        // "not enrolled" when the truth is "could not look".
        Err(err) => warn!(
            %err,
            "embedded auth: could not read this node's signing key, so account-proof logins \
             stay disabled",
        ),
    }
}

/// Initialise the embedded authentication service according to the server configuration.
pub async fn initialise(server_config: &ServerConfig, datastore: &Store) -> Result<BundledAuth> {
    let mut auth_config = server_config
        .embedded_auth_config()
        .cloned()
        .unwrap_or_else(default_config);

    name_this_node(&mut auth_config, datastore);

    // Path resolution is handled by merod run.rs before passing config here
    let app = build_app(auth_config).await?;

    info!("Embedded authentication endpoints enabled at /auth and /admin");

    Ok(BundledAuth { app })
}

#[must_use]
/// Seconds since the Unix epoch, for the one rule on this path that needs a
/// clock: whether a caller's proof is inside its window.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Run the permission check both admission paths share.
///
/// Extracted so the token path and the proof path cannot drift: they authorize
/// against the same table, and a second copy of this is how one of them ends up
/// a route behind the other.
/// Returns the refusal when there is one, so the polarity is visible at every
/// call site: `Some` is a rejection, and there is no success value to discard.
fn authorization_refusal(
    method: &Method,
    full_uri: axum::http::Uri,
    permissions: &[String],
    subject: &str,
) -> Option<Response> {
    let perm_request = Request::builder()
        .method(method.clone())
        .uri(full_uri)
        .body(Body::empty())
        .expect("request built from an already-validated method and URI");

    let validator = PermissionValidator::new();
    let required = validator.determine_required_permissions(&perm_request);
    if validator.validate_permissions(permissions, &required) {
        return None;
    }

    warn!(
        %subject,
        ?required,
        granted = ?permissions,
        "permission denied: caller lacks the permissions this route requires",
    );
    let mut resp = StatusCode::FORBIDDEN.into_response();
    resp.headers_mut().insert(
        "X-Auth-Error",
        HeaderValue::from_static("permission_denied"),
    );
    Some(resp)
}

/// The permissions a verified proof confers.
///
/// Exactly what an `account_proof` SESSION confers, read from that provider's
/// own default rather than restated here. Both are the same claim — "this
/// caller is an account" — arrived at two ways, and giving them different
/// authority would mean the answer to "what does being an account get you"
/// depended on how you proved it.
fn proof_permissions() -> Vec<String> {
    mero_auth::config::AccountProofConfig::default().session_permissions
}

pub fn guard_layer(service: Arc<AuthService>, proof_policy: Option<ProofPolicy>) -> AuthGuardLayer {
    AuthGuardLayer::new(service, proof_policy)
}

#[derive(Clone)]
pub struct AuthGuardLayer {
    service: Arc<AuthService>,
    /// Absent when this node cannot serve proofs at all — it has minted no
    /// signing key yet, so there is no name for a session statement to be
    /// addressed to and nothing to compare against.
    proof_policy: Option<ProofPolicy>,
}

impl AuthGuardLayer {
    fn new(service: Arc<AuthService>, proof_policy: Option<ProofPolicy>) -> Self {
        Self {
            service,
            proof_policy,
        }
    }
}

impl<S> Layer<S> for AuthGuardLayer
where
    S: Service<Request<Body>, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Service = AuthGuardService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthGuardService {
            inner,
            service: Arc::clone(&self.service),
            proof_policy: self.proof_policy.clone(),
        }
    }
}

#[derive(Clone)]
pub struct AuthGuardService<S> {
    inner: S,
    service: Arc<AuthService>,
    proof_policy: Option<ProofPolicy>,
}

impl<S> Service<Request<Body>> for AuthGuardService<S>
where
    S: Service<Request<Body>, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Response, Infallible>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let mut inner = self.inner.clone();
        let service = Arc::clone(&self.service);
        let proof_policy = self.proof_policy.clone();
        let (mut parts, body) = req.into_parts();
        let method = parts.method.clone();
        let headers = parts.headers.clone();
        let uri = parts.uri.clone();

        async move {
            if method != Method::OPTIONS {
                let auth_response =
                    if headers.contains_key(axum::http::header::AUTHORIZATION) {
                        // Authorization header is present — validate it exclusively.
                        // Never fall through to the query param path: if the client
                        // explicitly sent a header (even an invalid/revoked one), honour
                        // that choice and reject rather than silently retrying with a
                        // query param token, which would bypass revocation.
                        match service.verify_token_from_headers(&headers).await {
                            Ok(resp) => resp,
                            Err(e) => {
                                debug!(error = ?e, "Bearer token validation failed");
                                return Ok(unauthorized_response(&e));
                            }
                        }
                    } else {
                        // No Authorization header — try the ?token= query parameter.
                        // Browser WebSocket and EventSource APIs cannot set custom
                        // headers, so the JS client passes the JWT as a query param.
                        let token = uri.query().and_then(|q| {
                            q.split('&').find_map(|pair| {
                                let (key, value) = pair.split_once('=')?;
                                (key == "token").then(|| value.to_owned())
                            })
                        });
                        match token {
                            Some(ref t) => {
                                match service.verify_token_string(t, Some(&headers)).await {
                                    Ok(resp) => resp,
                                    Err(e) => {
                                        debug!(error = ?e, "Query param token validation failed");
                                        return Ok(unauthorized_response(&e));
                                    }
                                }
                            }
                            None => {
                                // No token of any kind. Before refusing, the one
                                // other way a caller can say who it is: a proof
                                // carried on the request itself.
                                //
                                // Deliberately last. A request holding a token is
                                // decided by that token, so a caller cannot
                                // present a weak proof alongside a rejected token
                                // and have the proof answer instead.
                                let Some(header) = parts.headers.get(&PROOF_HEADER) else {
                                    debug!("no Authorization header, no ?token= and no proof");
                                    return Ok(StatusCode::UNAUTHORIZED.into_response());
                                };
                                let Some(policy) = proof_policy.as_ref() else {
                                    debug!("a proof was presented to a node that serves none");
                                    return Ok(StatusCode::UNAUTHORIZED.into_response());
                                };

                                // The body is read HERE and nowhere else on this
                                // path, because a proof commits to it. The token
                                // path never touches it, so a streaming upload
                                // authenticated by a token still streams.
                                let bytes = match axum::body::to_bytes(body, MAX_PROVEN_BODY).await
                                {
                                    Ok(bytes) => bytes,
                                    Err(_ignored) => {
                                        debug!("a proven request's body exceeded the cap");
                                        return Ok(
                                            StatusCode::PAYLOAD_TOO_LARGE.into_response()
                                        );
                                    }
                                };

                                let full_uri = parts
                                    .extensions
                                    .get::<OriginalUri>()
                                    .map_or_else(|| uri.clone(), |original| original.0.clone());

                                let (account, device) = match policy.admit(
                                    header.as_bytes(),
                                    method.as_str(),
                                    full_uri.path(),
                                    &bytes,
                                    now_secs(),
                                ) {
                                    Ok(admitted) => admitted,
                                    Err(refusal) => {
                                        debug!(?refusal, "proof refused");
                                        let mut resp = match refusal {
                                            // A caller this node was never asked
                                            // to serve is told so, rather than
                                            // being invited to re-authenticate
                                            // against a door that will not open
                                            // for it however it knocks.
                                            Refusal::NotServed => {
                                                StatusCode::FORBIDDEN.into_response()
                                            }
                                            Refusal::Malformed | Refusal::Unverified => {
                                                StatusCode::UNAUTHORIZED.into_response()
                                            }
                                        };
                                        resp.headers_mut().insert(
                                            "X-Auth-Error",
                                            HeaderValue::from_static("invalid_proof"),
                                        );
                                        return Ok(resp);
                                    }
                                };

                                if let Some(resp) = authorization_refusal(
                                    &method,
                                    full_uri,
                                    &proof_permissions(),
                                    &account.to_string(),
                                ) {
                                    return Ok(resp);
                                }

                                // An account, and only ever an account. A proof
                                // says a device speaks for an account root; it
                                // says nothing about who owns this node, so the
                                // node-owner marker is unreachable from here by
                                // construction rather than by a check.
                                parts.extensions.insert(AuthenticatedAccount(account));
                                // The device too, so the revocation check that
                                // runs where the group is known has something
                                // to check. A session cannot supply this.
                                parts.extensions.insert(AuthenticatedDevice(device));

                                let req = Request::from_parts(parts, Body::from(bytes));
                                return inner.call(req).await;
                            }
                        }
                    };

                // Authorisation. Authenticating the token is not enough: a valid
                // but under-privileged token must not reach privileged handlers.
                // Previously this guard stopped at verification, so any valid
                // token could hit every /admin-api/* and /jsonrpc endpoint. Run
                // the same determine + validate pass the auth crate's
                // `auth_middleware` uses, against the permissions carried by the
                // verified token, so the two enforcement paths stay in sync.
                //
                // `PermissionValidator` matches full request paths (e.g.
                // `/admin-api/contexts`, `/jsonrpc`). This guard runs inside a
                // nested router, so `parts.uri` has had the mount prefix stripped
                // (`/contexts`, `/`); recover the full path from `OriginalUri`,
                // which axum's `nest` inserts. Non-nested mounts (e.g. the `/ws`
                // route) have no `OriginalUri`, so fall back to the request URI,
                // which is already the full path there.
                let full_uri = parts
                    .extensions
                    .get::<OriginalUri>()
                    .map_or_else(|| uri.clone(), |original| original.0.clone());

                if let Some(resp) = authorization_refusal(
                    &method,
                    full_uri,
                    &auth_response.permissions,
                    &auth_response.key_id,
                ) {
                    return Ok(resp);
                }

                // Attempt to resolve the authenticated public key and inject it so
                // handlers can use it as the effective requester without trusting the
                // caller-supplied value.
                // Ask FIRST whether the subject is an account. An
                // `account_proof` login records its account on first use, so it
                // now has a row here AND that row carries the account in its
                // `public_key` field -- which parses as a `PublicKey` (that
                // `FromStr` is hex, while `Display` is bs58) and would otherwise
                // be injected as `AuthenticatedKey`, an identity no member holds.
                // The delegated read then refuses a perfectly good session for
                // want of an account.
                //
                // Neither inference below can answer this: presence says only
                // that something was provisioned, and a parseable `public_key`
                // says only that 32 bytes were stored. The record's own
                // `auth_method` says which provider minted it, so that is what
                // decides.
                match service.is_account_anchored_key(&auth_response.key_id).await {
                    Ok(true) => match auth_response.key_id.parse::<calimero_account::AccountId>() {
                        Ok(account) => {
                            debug!(%account, "account-anchored session: granting AuthenticatedAccount");
                            parts.extensions.insert(AuthenticatedAccount(account));

                            // And the device, when the token names one, so a
                            // session is filtered by revocation exactly as a
                            // request-carried proof is. A token minted before
                            // this claim existed names none and behaves as it
                            // always did — unfilterable, which is the gap being
                            // closed rather than one being opened.
                            //
                            // An unparseable value grants no device rather than
                            // a wrong one: the claim is advisory to this layer,
                            // and inventing a device id would make revocation
                            // consult a row about somebody else.
                            match auth_response
                                .device
                                .as_deref()
                                .map(str::parse::<calimero_primitives::identity::DeviceId>)
                            {
                                Some(Ok(device)) => {
                                    parts.extensions.insert(AuthenticatedDevice(device));
                                }
                                Some(Err(_)) => warn!(
                                    %account,
                                    "session names a device that does not parse; \
                                     granting no device",
                                ),
                                None => {}
                            }
                        }
                        Err(_) => {
                            // Minted by the account provider yet not a parseable
                            // account: grant nothing rather than fall through to
                            // an inference that would read it as the node owner.
                            warn!(key_id=%auth_response.key_id, "account-anchored record whose id is not an account; granting no identity");
                        }
                    },
                    Ok(false) => {
                        match service.get_key_public_key(&auth_response.key_id).await {
                    Ok(Some(pk_hex)) => {
                        use std::str::FromStr as _;
                        match calimero_primitives::identity::PublicKey::from_str(&pk_hex) {
                            Ok(pk) => {
                                parts.extensions.insert(AuthenticatedKey(pk));
                            }
                            Err(_) => {
                                // The stored value is not a valid Ed25519/hex public
                                // key. This is expected for username/password auth: the
                                // user_password provider stores the username in the
                                // `public_key` field as a human-readable identifier, not
                                // a real cryptographic key. Treat this path identically
                                // to Ok(None) — the auth layer confirmed a valid session;
                                // the caller is the node owner.
                                debug!(key_id=%auth_response.key_id, "non-key auth (parse failure): granting NodeOwner");
                                parts.extensions.insert(AuthenticatedNodeOwner);
                            }
                        }
                    }
                    Ok(None) => {
                        // No Ed25519 public key came back, which covers two callers
                        // the store lookup cannot tell apart on its own — it ends in
                        // `key.and_then(|k| k.public_key)`, so "no row" and "a row
                        // whose public_key is None" both arrive here. Ask which:
                        //
                        // * A ROW EXISTS → a client key (`KeyType::Client`), created
                        //   via `/auth/client-keys` by the node owner for their own
                        //   applications and provisioned with `public_key: None` by
                        //   design. Issued by and to the node owner, so NodeOwner
                        //   matches the intended access model. Unchanged.
                        //
                        // * NO ROW → a session anchored outside this store: an
                        //   `account_proof` login, whose account is its own
                        //   cryptographic anchor and which deliberately persists
                        //   nothing here. Emphatically NOT the node owner — they may
                        //   be one tenant among many on a relay. Before this split
                        //   they were granted NodeOwner, on a comment asserting that
                        //   no such key could exist; adding the provider made that
                        //   assertion false.
                        //
                        // Note: username/password root keys reach neither arm. That
                        // provider stores the username in `public_key`, so the lookup
                        // returns `Ok(Some(username))` and `PublicKey::from_str` fails
                        // → handled by the `Err(_)` arm above.
                        match service.key_row_exists(&auth_response.key_id).await {
                            Ok(true) => {
                                debug!(key_id=%auth_response.key_id, "client key (row present, no public key): granting NodeOwner");
                                parts.extensions.insert(AuthenticatedNodeOwner);
                            }
                            Ok(false) => {
                                match auth_response.key_id.parse::<calimero_account::AccountId>() {
                                    Ok(account) => {
                                        debug!(%account, "account-anchored session: granting AuthenticatedAccount");
                                        parts.extensions.insert(AuthenticatedAccount(account));
                                    }
                                    Err(_) => {
                                        // Authenticated, but the subject names
                                        // neither a stored key nor a parseable
                                        // account. Grant nothing rather than guess:
                                        // the permission check has already run, and
                                        // no handler should treat an unidentifiable
                                        // subject as anybody in particular.
                                        warn!(key_id=%auth_response.key_id, "authenticated subject is neither a stored key nor an account; granting no identity");
                                    }
                                }
                            }
                            Err(err) => {
                                // Same posture as the lookup error below: an
                                // infrastructure failure must not decide an identity.
                                warn!(key_id=%auth_response.key_id, %err, "failed to determine whether a key row exists; rejecting request");
                                return Ok(StatusCode::INTERNAL_SERVER_ERROR.into_response());
                            }
                        }
                    }
                    Err(err) => {
                        // A store or network error during key lookup is an
                        // infrastructure failure, not a known auth path. Fail
                        // closed rather than failing open: do NOT grant
                        // node-owner access on a transient error.
                        warn!(key_id=%auth_response.key_id, %err, "failed to look up public key for auth key_id; rejecting request");
                        return Ok(StatusCode::INTERNAL_SERVER_ERROR.into_response());
                    }
                }
                    }
                    Err(err) => {
                        // Whether the subject is an account decides WHICH
                        // identity a handler sees, so a failure here must not
                        // fall through to the inferences above: those would
                        // read an account-anchored session as the node owner,
                        // which on a relay is another tenant entirely.
                        warn!(key_id=%auth_response.key_id, %err, "failed to classify the authenticated subject; rejecting request");
                        return Ok(StatusCode::INTERNAL_SERVER_ERROR.into_response());
                    }
                }
            }

            let req = Request::from_parts(parts, body);
            let response = inner.call(req).await?;
            Ok(response)
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use axum::response::Response;
    use axum::routing::get;
    use axum::Router;
    use mero_auth::auth::permissions::PermissionValidator;
    use mero_auth::auth::token::TokenManager;
    use mero_auth::config::JwtConfig;
    use mero_auth::secrets::SecretManager;
    use mero_auth::storage::{Key, KeyManager, MemoryStorage, Storage};
    use mero_auth::AuthService;
    use tower::ServiceExt as _;

    /// Build the request the guard hands to the validator: only the method and
    /// the full path are read by `determine_required_permissions`.
    fn request(method: Method, path: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap()
    }

    /// The guard resolves the *full* request path (via `OriginalUri`) precisely
    /// because the validator's mappings are keyed on the server's mount paths.
    /// If these stopped yielding a required permission, enforcement would
    /// silently degrade to "any valid token", which is the bug being fixed.
    #[test]
    fn server_mount_paths_require_permissions() {
        let validator = PermissionValidator::new();

        for (method, path) in [
            (Method::GET, "/admin-api/contexts"),
            (Method::POST, "/admin-api/contexts"),
            (Method::GET, "/admin-api/applications"),
            (Method::POST, "/jsonrpc"),
        ] {
            let required = validator.determine_required_permissions(&request(method.clone(), path));
            assert!(
                !required.is_empty(),
                "{method} {path} must map to a required permission for the guard to enforce it",
            );
        }
    }

    /// End-to-end of the guard's authorisation decision: an authenticated token
    /// with no permissions is rejected for a privileged route, while a token
    /// holding `admin` is allowed.
    #[test]
    fn underprivileged_token_is_denied_admin_token_allowed() {
        let validator = PermissionValidator::new();
        let required =
            validator.determine_required_permissions(&request(Method::GET, "/admin-api/contexts"));

        let no_permissions: Vec<String> = Vec::new();
        assert!(
            !validator.validate_permissions(&no_permissions, &required),
            "a token with no permissions must not pass the contexts-list check",
        );

        let admin = vec!["admin".to_owned()];
        assert!(
            validator.validate_permissions(&admin, &required),
            "an admin token must pass every permission check",
        );
    }

    /// What happens to a key after its token has been minted.
    enum KeyFate {
        Revoked,
        Deleted,
    }

    /// Drive one request through the real guard over an in-memory auth service,
    /// so the assertions land on the response a client actually receives.
    async fn guarded_request(fate: KeyFate) -> Response {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let secrets = Arc::new(SecretManager::new(Arc::clone(&storage)));
        secrets.initialize().await.unwrap();
        let token_manager = TokenManager::new(
            JwtConfig {
                issuer: "test".to_owned(),
                access_token_expiry: 3600,
                refresh_token_expiry: 86400,
                node_host: None,
            },
            Arc::clone(&storage),
            secrets,
        );

        let key_manager = KeyManager::new(Arc::clone(&storage));
        let mut key = Key::new_root_key_with_permissions(
            "owner".to_owned(),
            "user_password".to_owned(),
            vec!["admin".to_owned()],
            None,
        );
        key_manager.set_key("k-1", &key).await.unwrap();

        let (access_token, _) = token_manager
            .generate_token_pair("k-1".to_owned(), vec!["admin".to_owned()], None, None)
            .await
            .unwrap();

        match fate {
            KeyFate::Revoked => {
                key.revoke();
                key_manager.set_key("k-1", &key).await.unwrap();
            }
            KeyFate::Deleted => key_manager.delete_key("k-1").await.unwrap(),
        }

        Router::new()
            .route("/admin-api/applications", get(|| async { "ok" }))
            .layer(super::guard_layer(
                Arc::new(AuthService::new(Vec::new(), token_manager)),
                // No proof policy: this test covers the token path, and giving
                // it one would let a failure there be masked by admission here.
                None,
            ))
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/admin-api/applications")
                    .header("Authorization", format!("Bearer {access_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    /// A client holding a revoked key must be told so, or a long-lived process
    /// cannot tell "re-read your credential" from "retry later".
    #[tokio::test]
    async fn guard_reports_revocation_to_the_client() {
        let resp = guarded_request(KeyFate::Revoked).await;

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            resp.headers().get("X-Auth-Error").unwrap(),
            "token_revoked",
            "a revoked key must carry the terminal signal the SDK keys on",
        );
    }

    /// The other direction: a key id that is genuinely gone must stay a generic
    /// rejection, or the revocation hint stops carrying information.
    #[tokio::test]
    async fn guard_gives_no_revocation_hint_for_an_absent_key() {
        let resp = guarded_request(KeyFate::Deleted).await;

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(
            resp.headers().get("X-Auth-Error").is_none(),
            "an absent key must not be reported as revoked",
        );
    }

    /// A revoked token must map to `403 Forbidden` with `X-Auth-Error:
    /// token_revoked`. Guards against a silent regression to the generic `401`
    /// if the typed variant is ever removed, renamed, or the arm dropped.
    #[test]
    fn revoked_token_maps_to_forbidden() {
        use axum::http::StatusCode;
        use mero_auth::AuthError;

        let resp = super::unauthorized_response(&AuthError::TokenRevoked);
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(resp.headers().get("X-Auth-Error").unwrap(), "token_revoked",);
    }

    /// An expired token maps to `401 Unauthorized` with `X-Auth-Error:
    /// token_expired`.
    #[test]
    fn expired_token_maps_to_unauthorized() {
        use axum::http::StatusCode;
        use mero_auth::AuthError;

        let resp = super::unauthorized_response(&AuthError::TokenExpired);
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(resp.headers().get("X-Auth-Error").unwrap(), "token_expired",);
    }

    /// Any other rejection falls back to a bare `401` with no error hint.
    #[test]
    fn other_errors_map_to_bare_unauthorized() {
        use axum::http::StatusCode;
        use mero_auth::AuthError;

        let resp = super::unauthorized_response(&AuthError::InvalidToken("nope".to_owned()));
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(resp.headers().get("X-Auth-Error").is_none());
    }

    /// Unmapped `/admin-api/*` routes (governance subpaths, unhandled methods)
    /// must fail closed: a valid but non-admin token is denied, an admin token
    /// is allowed. Without the default-deny these admitted any valid token —
    /// the audit's "unlisted /admin-api/... subpath → 403, currently 200".
    #[test]
    fn unmapped_admin_api_routes_deny_non_admin_tokens() {
        let validator = PermissionValidator::new();
        let scoped = vec!["context:execute".to_owned(), "context:list".to_owned()];
        let admin = vec!["admin".to_owned()];

        for (method, path) in [
            (Method::POST, "/admin-api/groups"),
            (Method::DELETE, "/admin-api/groups/g-1"),
            (Method::POST, "/admin-api/install-dev-application"),
            (Method::GET, "/admin-api/usage"),
        ] {
            let required = validator.determine_required_permissions(&request(method.clone(), path));
            assert!(
                !required.is_empty(),
                "{method} {path} must yield a requirement (admin) for the guard to enforce",
            );
            assert!(
                !validator.validate_permissions(&scoped, &required),
                "{method} {path}: a scoped non-admin token must be denied (403)",
            );
            assert!(
                validator.validate_permissions(&admin, &required),
                "{method} {path}: an admin token must be allowed",
            );
        }
    }
}
