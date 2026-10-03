use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, Extension};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(debug_assertions)]
use subtle::ConstantTimeEq;
use tracing::{debug, error, info, warn};
use validator::Validate;

use crate::api::handlers::AuthUiStaticFiles;
use crate::auth::permissions::PermissionValidator;
use crate::auth::token::Claims;
use crate::auth::validation::{sanitize_identifier, sanitize_string, ValidatedJson};
use crate::server::AppState;
use crate::storage::models::Key;
use crate::AuthError;

/// Expiry leeway applied by the refresh endpoint's "access token must already
/// be expired" policy. Matches jsonwebtoken's default `Validation::leeway` so
/// the two notions of "expired" cannot disagree.
const JWT_EXPIRY_LEEWAY_SECS: u64 = 60;

/// Whether an access token and a refresh token were issued for the same key.
/// The key, not `sub`: every client key of one user shares its `sub`.
fn tokens_share_key(access: &Claims, refresh: &Claims) -> bool {
    access.key_id == refresh.key_id
}

// Common response type used by all helper functions
type ApiResponse = (StatusCode, HeaderMap, Json<serde_json::Value>);

pub fn success_response<T: Serialize>(data: T, headers: Option<HeaderMap>) -> ApiResponse {
    (
        StatusCode::OK,
        headers.unwrap_or_default(),
        Json(serde_json::json!({
            "data": data,
            "error": null
        })),
    )
}

pub fn error_response(
    status: StatusCode,
    error: impl Into<String>,
    headers: Option<HeaderMap>,
) -> ApiResponse {
    (
        status,
        headers.unwrap_or_default(),
        Json(serde_json::json!({
            "data": null,
            "error": error.into()
        })),
    )
}

/// Login request handler
///
/// This endpoint serves the login page.
pub async fn login_handler(state: Extension<Arc<AppState>>) -> impl IntoResponse {
    let enabled_providers = state.0.auth_service.providers();

    if !enabled_providers.is_empty() {
        info!("Loading authentication UI");

        if let Some(file) = AuthUiStaticFiles::get("index.html") {
            let html_content = String::from_utf8_lossy(&file.data);

            use axum::http::HeaderValue;
            let mut headers = HeaderMap::new();
            headers.insert("Content-Type", HeaderValue::from_static("text/html"));
            headers.insert(
                "Cache-Control",
                HeaderValue::from_static("no-cache, no-store, must-revalidate"),
            );
            headers.insert("Pragma", HeaderValue::from_static("no-cache"));
            headers.insert("Expires", HeaderValue::from_static("0"));

            return (
                StatusCode::OK,
                headers,
                html_content.into_owned().into_bytes(),
            )
                .into_response();
        }

        error!("Failed to load authentication UI - index.html not found");
    }

    warn!("No authentication providers available");
    let html = "<html><body><h1>No authentication provider is available</h1></body></html>";
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("text/html"));
    headers.insert(
        "Cache-Control",
        HeaderValue::from_static("no-cache, no-store, must-revalidate"),
    );
    headers.insert("Pragma", HeaderValue::from_static("no-cache"));
    headers.insert("Expires", HeaderValue::from_static("0"));
    (StatusCode::OK, headers, html.as_bytes().to_vec()).into_response()
}

/// Base token request with common fields
#[derive(Debug, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct BaseTokenRequest {
    /// Authentication method
    #[validate(length(min = 1, message = "Authentication method is required"))]
    pub auth_method: String,

    /// Public key
    #[validate(length(min = 1, message = "Public key is required"))]
    pub public_key: String,

    /// Client name
    #[validate(length(min = 1, message = "Client name is required"))]
    pub client_name: String,

    /// Permissions requested
    pub permissions: Option<Vec<String>>,

    /// Timestamp
    pub timestamp: u64,

    /// Provider-specific data as raw JSON
    pub provider_data: Value,
}

/// Token request that includes provider-specific data
pub type TokenRequest = BaseTokenRequest;

/// Token response
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    /// Access token
    access_token: String,
    /// Refresh token
    refresh_token: String,
    /// Error message
    error: Option<String>,
}

impl TokenResponse {
    /// Create a new success token response
    pub fn new(access_token: String, refresh_token: String) -> Self {
        Self {
            access_token,
            refresh_token,
            error: None,
        }
    }
}

/// Counts a rejected credential against the caller's own limit and the account's ceiling.
fn record_failure(state: &Extension<Arc<AppState>>, source_key: &str, account_key: &str) {
    state.0.login_rate_limiter.record_failure(source_key);
    state.0.account_rate_limiter.record_failure(account_key);
}

/// Token handler
///
/// This endpoint generates JWT tokens for authenticated clients.
///
/// # Arguments
///
/// * `state` - The application state
/// * `request` - The token request
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn token_handler(
    state: Extension<Arc<AppState>>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    ValidatedJson(mut token_request): ValidatedJson<TokenRequest>,
) -> impl IntoResponse {
    info!("token_handler");

    // Extract node URL from client_name for node-specific token generation
    let node_url = Some(token_request.client_name.clone());

    // Sanitize the method first so the throttle key and the provider lookup see
    // the same string; a method matching no provider runs no credential check.
    token_request.auth_method = sanitize_identifier(&token_request.auth_method);

    // Throttle on the provider's account identity, not a field the caller varies
    // per request. Raw values keep sanitization from merging buckets; parts are capped.
    const MAX_RL_KEY_FIELD: usize = 256;
    let cap_field = |s: &str| -> String { s.chars().take(MAX_RL_KEY_FIELD).collect() };
    let (rl_scope, rl_identity) = state
        .0
        .auth_service
        .throttle_identity(&token_request)
        .unwrap_or_else(|| (token_request.auth_method.clone(), String::new()));
    let rl_auth_method = cap_field(&rl_scope);
    let rl_public_key = cap_field(&rl_identity);
    // Length-prefix *both* fields so the key is unambiguous regardless of any
    // `|` characters in either component: `len|auth_method|len|public_key`. A
    // bare `|` separator would otherwise let an attacker who controls
    // `public_key` smuggle a separator and collide with a different identity's
    // bucket.
    let rl_key = format!(
        "{}|{}|{}|{}",
        rl_auth_method.len(),
        rl_auth_method,
        rl_public_key.len(),
        rl_public_key
    );

    // The tight limit is per caller and account, so one caller's bad guesses lock
    // out that caller and not the account's owner. Without a peer address the
    // key falls back to the account alone, the pre-existing behaviour.
    let source_key = format!(
        "src|{}|{rl_key}",
        peer.as_ref()
            .map_or_else(String::new, |Extension(ConnectInfo(addr))| addr
                .ip()
                .to_string())
    );

    // Sanitize string inputs to prevent injection attacks
    token_request.public_key = sanitize_string(&token_request.public_key);
    token_request.client_name = sanitize_string(&token_request.client_name);

    // Validate sanitized inputs are not empty
    if token_request.auth_method.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Authentication method must contain valid characters",
            None,
        );
    }

    if token_request.public_key.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Public key cannot be empty after sanitization",
            None,
        );
    }

    if token_request.client_name.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Client name cannot be empty after sanitization",
            None,
        );
    }

    // Brute-force throttle: if this caller has exceeded the failed-attempt
    // budget, reject with 429 + Retry-After before doing any credential work.
    // Either limit locks the caller out: its own, or the ceiling on the account
    // across all sources.
    let locked = [
        state.0.login_rate_limiter.check(&source_key),
        state.0.account_rate_limiter.check(&rl_key),
    ]
    .into_iter()
    .flatten()
    .max();
    if let Some(retry_after) = locked {
        // Log only the sanitized, low-cardinality auth method — never the raw
        // key (which holds the public key and could be a log-injection vector).
        warn!(
            auth_method = %token_request.auth_method,
            "Login rate limit exceeded"
        );
        let mut headers = HeaderMap::new();
        drop(
            headers.insert(
                "Retry-After",
                HeaderValue::from_str(&retry_after.to_string())
                    .unwrap_or_else(|_| HeaderValue::from_static("60")),
            ),
        );
        return error_response(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many failed login attempts; please try again later",
            Some(headers),
        );
    }

    // Authenticate directly using the token request with node context
    let auth_response = match state
        .0
        .auth_service
        .authenticate_token_request(&token_request, node_url.as_deref())
        .await
    {
        Ok(response) => response,
        Err(err) => {
            error!("Authentication failed: {}", err);
            // Only a rejected credential counts; a malformed request or an
            // unavailable service says nothing about the account.
            if !matches!(
                err,
                AuthError::InvalidRequest(_) | AuthError::ServiceUnavailable(_)
            ) {
                record_failure(&state, &source_key, &rl_key);
            }
            return error_response(
                StatusCode::UNAUTHORIZED,
                format!("Authentication failed: {err}"),
                None,
            );
        }
    };

    // Ensure authentication was successful
    if !auth_response.is_valid {
        record_failure(&state, &source_key, &rl_key);
        return error_response(
            StatusCode::UNAUTHORIZED,
            "Authentication failed: Invalid credentials",
            None,
        );
    }

    // Successful authentication clears the failed-attempt counter.
    state.0.login_rate_limiter.reset(&source_key);

    let key_id = auth_response.key_id;

    // Generate tokens using the validated permissions from auth_response and node_id
    match state
        .0
        .token_generator
        .generate_token_pair(
            key_id.clone(),
            auth_response.permissions,
            node_url,
            auth_response.device,
        )
        .await
    {
        Ok((access_token, refresh_token)) => {
            let response = TokenResponse::new(access_token, refresh_token);
            success_response(response, None)
        }
        Err(err) => {
            error!("Failed to generate tokens: {}", err);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to generate tokens",
                None,
            )
        }
    }
}

/// Refresh token request
#[derive(Debug, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct RefreshTokenRequest {
    /// Access token
    #[validate(length(min = 1, message = "Access token is required"))]
    access_token: String,
    /// Refresh token
    #[validate(length(min = 1, message = "Refresh token is required"))]
    refresh_token: String,
}

/// Refresh token handler
///
/// This endpoint refreshes an access token using a refresh token.
/// It supports both root and client tokens, handling them appropriately.
///
/// # Arguments
///
/// * `state` - The application state
/// * `request` - The refresh token request
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn refresh_token_handler(
    state: Extension<Arc<AppState>>,
    headers: HeaderMap,
    ValidatedJson(refresh_request): ValidatedJson<RefreshTokenRequest>,
) -> impl IntoResponse {
    // Decode the access token exactly once, skipping only expiry enforcement:
    // the refresh flow must read the claims of an already-expired token.
    // Signature validity and the Access token type are still enforced
    // (finding #1), so a refresh token in the access slot is rejected here.
    let access_claims = match state
        .0
        .token_generator
        .decode_access_claims_ignore_expiry(&refresh_request.access_token)
        .await
    {
        Ok(claims) => claims,
        Err(err) => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                format!("Invalid access token: {err}"),
                None,
            );
        }
    };

    // Refresh policy: only an expired access token may be exchanged. The leeway
    // mirrors jsonwebtoken's default `Validation::leeway`, so a token this
    // service would still accept as a bearer credential cannot simultaneously
    // be declared expired here.
    let now = u64::try_from(chrono::Utc::now().timestamp()).unwrap_or_default();
    if access_claims.exp.saturating_add(JWT_EXPIRY_LEEWAY_SECS) >= now {
        return error_response(StatusCode::UNAUTHORIZED, "Access token still valid", None);
    }

    // Verify the refresh token and extract claims (must be a refresh token).
    let refresh_claims = match state
        .0
        .token_generator
        .verify_refresh_token(&refresh_request.refresh_token)
        .await
    {
        Ok(claims) => claims,
        Err(err) => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                format!("Invalid refresh token: {err}"),
                None,
            );
        }
    };

    // Bind access <-> refresh to one key, or any refresh token could be paired
    // with an unrelated expired access token to mint a fresh pair.
    if !tokens_share_key(&access_claims, &refresh_claims) {
        warn!(
            "Refresh rejected: access/refresh key mismatch ({} != {})",
            access_claims.key_id, refresh_claims.key_id
        );
        return error_response(
            StatusCode::UNAUTHORIZED,
            "Access and refresh tokens do not belong to the same key",
            None,
        );
    }

    // Check node URL if token has node information
    if let Some(token_node_url) = &refresh_claims.node_url {
        if let Err(error_msg) = state
            .0
            .token_generator
            .validate_node_host(token_node_url, &headers)
        {
            let mut error_headers = HeaderMap::new();
            error_headers.insert("X-Auth-Error", "invalid_node".parse().unwrap());
            return error_response(StatusCode::FORBIDDEN, error_msg, Some(error_headers));
        }
    }

    // Use the refresh token to generate new tokens
    // Note: refresh_token_pair automatically preserves node_url from the refresh token
    match state
        .0
        .token_generator
        .refresh_token_pair(&refresh_request.refresh_token)
        .await
    {
        Ok((access_token, refresh_token)) => {
            let response = TokenResponse::new(access_token, refresh_token);
            success_response(response, None)
        }
        Err(AuthError::TokenReuse) => {
            // Replayed (already-consumed) refresh token: the family has been
            // revoked. Signal the terminal `token_reuse` contract so clients clear
            // their tokens and force re-auth instead of retrying (which would just
            // replay the consumed token again).
            warn!("Refresh token reuse detected; token family revoked");
            let mut reuse_headers = HeaderMap::new();
            reuse_headers.insert("X-Auth-Error", HeaderValue::from_static("token_reuse"));
            error_response(
                StatusCode::UNAUTHORIZED,
                "Refresh token reuse detected; re-authentication required",
                Some(reuse_headers),
            )
        }
        Err(err) => {
            error!("Failed to refresh token: {}", err);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to refresh token: {err}"),
                None,
            )
        }
    }
}

/// What this call to `/auth/validate` actually is.
///
/// The endpoint has two callers, and only one of them is a reverse proxy. The
/// other is a client checking whether its own session is still good — mero-js
/// does `HEAD /auth/validate` with nothing but an `Authorization` header, and
/// `mero-react` treats a non-200 as "logged out". Denying that because it
/// carries no `X-Forwarded-Uri` would put every app back in a login loop, which
/// is the outage this endpoint's contract test was written after.
///
/// So absence of the forwarded headers is not a failure to authorize. It is a
/// different question being asked.
enum Probe {
    /// No forwarded headers. A client asking about its own token, and token
    /// validity is the whole answer.
    SessionGate,
    /// A reverse proxy asking about a request it is holding, reconstructed so
    /// the permission decision runs against the same shape a direct request
    /// would have.
    Forwarded(Box<Request<Body>>),
    /// Forwarded headers present but unusable. Refused rather than treated as a
    /// session check: once a proxy is asking, falling back to "token is valid"
    /// answers a question nobody asked and answers it permissively.
    Malformed,
}

/// Classify a validate call from its headers.
///
/// `X-Forwarded-Uri` is the switch, because it is the header that says a
/// request other than this one is being decided. Traefik sets it, and the
/// fields it sets come from the real request rather than from anything the
/// client sent, which is what makes them safe to authorize on.
fn classify(headers: &HeaderMap) -> Probe {
    let Some(uri) = headers.get("X-Forwarded-Uri") else {
        return Probe::SessionGate;
    };
    let Ok(uri) = uri.to_str() else {
        return Probe::Malformed;
    };

    // Defaulting an absent method to GET would silently authorize a write under
    // a read permission, so the method is required once the URI is present.
    let Some(Ok(method)) = headers.get("X-Forwarded-Method").map(|m| m.to_str()) else {
        return Probe::Malformed;
    };

    // Rebuilt as a real request rather than parsed into (path, method) here, so
    // the decision goes through the SAME code a direct request does — including
    // the `GET | HEAD` normalization. A second parse that forgot HEAD would 403
    // every `HEAD /admin-api/blobs/:id`, which is a shipped client call, and it
    // would do so only behind a proxy.
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .map_or(Probe::Malformed, |req| Probe::Forwarded(Box::new(req)))
}

/// Name an account-anchored session's account and device, for a node that sits
/// behind this service rather than embedding it.
///
/// `X-Auth-User` cannot say this. It is the user id, which for an
/// `account_proof` session happens to be the account and for a
/// username/password one is an opaque id, so a node reading it would have to
/// guess which it holds — and a wrong guess reads a node owner as a tenant or a
/// tenant as the owner. The record's own `auth_method` says which provider
/// minted it, so that decides, as it does in the embedded guard.
///
/// A proxy forwards these only if it is told to, and must strip them from every
/// request it does not authenticate: see `calimero-server`'s `proxy_identity`.
fn name_account_session(key: &Key, claims: &Claims, headers: &mut HeaderMap) {
    if !key.is_root_key()
        || key.auth_method.as_deref() != Some(crate::providers::impls::account_proof::METHOD)
    {
        return;
    }
    let Ok(account) = HeaderValue::from_str(&claims.sub) else {
        warn!("account-anchored session whose subject is not a header value; naming no account");
        return;
    };
    let _ignored = headers.insert("X-Auth-Account", account);
    if let Some(device) = claims
        .device
        .as_deref()
        .and_then(|device| HeaderValue::from_str(device).ok())
    {
        let _ignored = headers.insert("X-Auth-Device", device);
    }
}

/// Forward authentication validation handler
///
/// This endpoint is designed for reverse proxies (nginx, Traefik, etc.) to validate
/// authentication before forwarding requests to backend services. It validates JWT tokens
/// and returns user information via response headers.
///
/// # Arguments
///
/// * `state` - The application state
/// * `headers` - The request headers
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn validate_handler(
    state: Extension<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Classified before any token work: a malformed proxy probe is refused for
    // the cost of two header reads, and the classification decides whether the
    // permission check below runs at all.
    let probe = classify(&headers);
    if matches!(probe, Probe::Malformed) {
        let mut error_headers = HeaderMap::new();
        let _ignored = error_headers.insert(
            "X-Auth-Error",
            HeaderValue::from_static("malformed_forwarded_request"),
        );
        return error_response(
            StatusCode::FORBIDDEN,
            "X-Forwarded-Uri was set without a usable X-Forwarded-Method",
            Some(error_headers),
        );
    }

    let token =
        extract_token_from_headers(&headers).or_else(|| extract_token_from_forwarded_uri(&headers));

    let token = match token {
        Some(token) => token.to_string(),
        None => {
            let mut error_headers = HeaderMap::new();
            error_headers.insert("X-Auth-Error", "missing_token".parse().unwrap());
            return error_response(
                StatusCode::UNAUTHORIZED,
                "No token provided",
                Some(error_headers),
            );
        }
    };

    // Validate the token
    match state.0.token_generator.verify_token(&token).await {
        Ok(claims) => {
            // Check node URL if token has node information
            if let Some(token_node_url) = &claims.node_url {
                if let Err(error_msg) = state
                    .0
                    .token_generator
                    .validate_node_host(token_node_url, &headers)
                {
                    let mut error_headers = HeaderMap::new();
                    error_headers.insert("X-Auth-Error", "invalid_node".parse().unwrap());
                    return error_response(StatusCode::FORBIDDEN, error_msg, Some(error_headers));
                }
            }

            // Verify the key exists and is valid
            let key = match state.0.key_manager.get_key(&claims.key_id).await {
                Ok(Some(key)) if key.is_valid() => key,
                Ok(Some(_)) => {
                    let mut error_headers = HeaderMap::new();
                    error_headers.insert("X-Auth-Error", "token_revoked".parse().unwrap());
                    return error_response(
                        StatusCode::FORBIDDEN,
                        "Key has been revoked",
                        Some(error_headers),
                    );
                }
                Ok(None) => {
                    let mut error_headers = HeaderMap::new();
                    error_headers.insert("X-Auth-Error", "invalid_token".parse().unwrap());
                    return error_response(
                        StatusCode::UNAUTHORIZED,
                        "Key not found",
                        Some(error_headers),
                    );
                }
                Err(_) => {
                    let mut error_headers = HeaderMap::new();
                    error_headers.insert("X-Auth-Error", "invalid_token".parse().unwrap());
                    return error_response(
                        StatusCode::UNAUTHORIZED,
                        "Failed to verify key",
                        Some(error_headers),
                    );
                }
            };

            // Authorize the request the proxy is holding — the step whose
            // absence made this endpoint authenticate without authorizing, so
            // any valid token reached any route behind the gate.
            //
            // Runs only for a forwarded probe. A session gate carries no request
            // to decide, and inventing one would either deny every client
            // session check or authorize a path nobody named.
            if let Probe::Forwarded(request) = &probe {
                let validator = PermissionValidator::new();
                let required = validator.determine_required_permissions(request);

                if !validator.validate_permissions(&claims.permissions, &required) {
                    warn!(
                        method = %request.method(),
                        path = %request.uri().path(),
                        ?required,
                        "forward-auth: permission denied",
                    );
                    let mut error_headers = HeaderMap::new();
                    let _ignored = error_headers.insert(
                        "X-Auth-Error",
                        HeaderValue::from_static("permission_denied"),
                    );
                    return error_response(
                        StatusCode::FORBIDDEN,
                        "Token does not carry the permissions this route requires",
                        Some(error_headers),
                    );
                }
            }

            // Create response headers
            let mut response_headers = HeaderMap::new();

            // Add user ID header
            response_headers.insert("X-Auth-User", claims.sub.parse().unwrap());

            // Add permissions as a comma-separated list
            if !claims.permissions.is_empty() {
                response_headers.insert(
                    "X-Auth-Permissions",
                    claims.permissions.join(",").parse().unwrap(),
                );
            }

            name_account_session(&key, &claims, &mut response_headers);

            success_response("", Some(response_headers))
        }
        Err(err) => {
            let mut error_headers = HeaderMap::new();
            // Add error type header for better client handling
            if matches!(err, AuthError::TokenExpired) {
                error_headers.insert("X-Auth-Error", "token_expired".parse().unwrap());
            } else if matches!(err, AuthError::TokenRevoked) {
                error_headers.insert("X-Auth-Error", "token_revoked".parse().unwrap());
            } else {
                error_headers.insert("X-Auth-Error", "invalid_token".parse().unwrap());
            }
            error_response(
                StatusCode::UNAUTHORIZED,
                format!("Invalid token: {err}"),
                Some(error_headers),
            )
        }
    }
}

/// Extracts the token from the Authorization header.
fn extract_token_from_headers(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("Authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.trim())
}

/// Extracts the token from the X-Forwarded-Uri header.
fn extract_token_from_forwarded_uri(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("X-Forwarded-Uri")
        .and_then(|value| value.to_str().ok())
        .and_then(|uri_str| {
            uri_str.split('?').nth(1).and_then(|query| {
                query
                    .split('&')
                    .find(|param| param.starts_with("token="))
                    .map(|param| &param[6..])
            })
        })
}

/// Revoke token request
#[derive(Debug, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct RevokeTokenRequest {
    /// Client ID to revoke
    #[validate(length(min = 1, message = "Client ID cannot be empty"))]
    client_id: String,
}

/// Revoke token handler
///
/// This endpoint revokes a client's tokens.
///
/// # Arguments
///
/// * `state` - The application state
/// * `request` - The revoke token request
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn revoke_token_handler(
    state: Extension<Arc<AppState>>,
    ValidatedJson(mut request): ValidatedJson<RevokeTokenRequest>,
) -> impl IntoResponse {
    // Sanitize client ID to prevent injection attacks
    request.client_id = sanitize_identifier(&request.client_id);

    if request.client_id.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Client ID must contain valid characters",
            None,
        );
    }
    match state
        .0
        .token_generator
        .revoke_client_tokens(&request.client_id)
        .await
    {
        Ok(_) => {
            debug!(
                "Successfully revoked tokens for client {}",
                request.client_id
            );

            success_response(
                serde_json::json!({
                        "success": true,
                        "message": "Tokens revoked successfully"
                }),
                None,
            )
        }
        Err(err) => {
            error!(
                "Failed to revoke tokens for client {}: {}",
                request.client_id, err
            );

            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to revoke tokens: {err}"),
                None,
            )
        }
    }
}

#[derive(Debug, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct LogoutRequest {
    #[validate(length(min = 1, message = "Refresh token is required"))]
    refresh_token: String,
}

pub async fn logout_handler(
    state: Extension<Arc<AppState>>,
    ValidatedJson(request): ValidatedJson<LogoutRequest>,
) -> impl IntoResponse {
    match state
        .0
        .token_generator
        .retire_refresh_token(&request.refresh_token)
        .await
    {
        Ok(()) => success_response(serde_json::json!({ "success": true }), None),
        Err(AuthError::StorageError(err)) => {
            error!("Failed to retire refresh token: {}", err);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to end the session",
                None,
            )
        }
        Err(err) => error_response(
            StatusCode::UNAUTHORIZED,
            format!("Invalid refresh token: {err}"),
            None,
        ),
    }
}

/// Mock token request for CI and testing
#[cfg(debug_assertions)]
#[derive(Debug, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct MockTokenRequest {
    /// Client name for identification
    #[validate(length(min = 1, message = "Client name is required"))]
    pub client_name: String,

    /// Permissions to grant (optional, defaults to admin)
    pub permissions: Option<Vec<String>>,

    /// Node URL this token should be valid for (optional)
    pub node_url: Option<String>,

    /// Token expiry override in seconds (optional, uses config defaults)
    pub access_token_expiry: Option<u64>,

    /// Refresh token expiry override in seconds (optional, uses config defaults)
    pub refresh_token_expiry: Option<u64>,
}

/// Mock token handler for CI and testing
///
/// This endpoint generates JWT tokens without authentication for testing purposes.
/// It should only be enabled in development/testing environments.
///
/// # Security Warning
/// This endpoint bypasses all authentication and should NEVER be enabled in production.
/// It creates temporary keys and generates valid JWT tokens for testing purposes.
///
/// # Arguments
///
/// * `state` - The application state
/// * `request` - The mock token request
///
/// # Returns
///
/// * `impl IntoResponse` - The response containing access and refresh tokens
#[cfg(debug_assertions)]
pub async fn mock_token_handler(
    state: Extension<Arc<AppState>>,
    headers: HeaderMap,
    ValidatedJson(mut request): ValidatedJson<MockTokenRequest>,
) -> impl IntoResponse {
    warn!("⚠️  MOCK TOKEN ENDPOINT ACCESSED - This should only be used for testing!");

    // Check if mock endpoints are enabled in config
    if !state.0.config.development.enable_mock_auth {
        warn!("Mock token endpoint is disabled in configuration");
        return error_response(StatusCode::NOT_FOUND, "Endpoint not found", None);
    }

    // Check authorization header if required
    if state.0.config.development.mock_auth_require_header {
        let auth_header = headers
            .get("Authorization")
            .and_then(|value| value.to_str().ok());

        if let Some(required_value) = &state.0.config.development.mock_auth_header_value {
            match auth_header {
                Some(value) if value.as_bytes().ct_eq(required_value.as_bytes()).into() => {
                    // Authorization header matches, continue
                }
                _ => {
                    warn!("Mock token endpoint accessed without proper authorization");
                    return error_response(
                        StatusCode::UNAUTHORIZED,
                        "Invalid or missing authorization for mock endpoint",
                        None,
                    );
                }
            }
        } else if auth_header.is_none() {
            warn!("Mock token endpoint requires authorization header but none provided");
            return error_response(
                StatusCode::UNAUTHORIZED,
                "Authorization header required for mock endpoint",
                None,
            );
        }
    }

    // Sanitize inputs
    request.client_name = sanitize_string(&request.client_name);

    if request.client_name.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Client name cannot be empty after sanitization",
            None,
        );
    }

    // Default permissions for mock tokens
    let permissions = request
        .permissions
        .unwrap_or_else(|| vec!["admin".to_string()]);

    // Generate a mock key ID for this client
    let timestamp = chrono::Utc::now().timestamp();
    let key_id = format!("mock_{}_{}", request.client_name, timestamp);

    // Create a temporary root key that can be validated
    // This allows the tokens to pass validation for e2e testing
    let mock_key = Key::new_root_key_with_permissions(
        format!("mock_public_key_{timestamp}"),
        "mock_auth".to_string(),
        permissions.clone(),
        request.node_url.clone(),
    );

    // Store the temporary key so tokens can be validated
    if let Err(err) = state.0.key_manager.set_key(&key_id, &mock_key).await {
        error!("Failed to store mock key: {}", err);
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create mock key: {err}"),
            None,
        );
    }

    info!(
        "Created temporary mock key: {} for client: {}",
        key_id, request.client_name
    );

    // Generate tokens using the stored mock key (will pass validation)
    match state
        .0
        .token_generator
        .generate_token_pair(key_id.clone(), permissions, request.node_url, None)
        .await
    {
        Ok((access_token, refresh_token)) => {
            info!(
                "Generated mock tokens for client '{}' with key_id '{}'",
                request.client_name, key_id
            );

            let response = TokenResponse::new(access_token, refresh_token);

            // Add warning headers
            let mut headers = HeaderMap::new();
            headers.insert("X-Mock-Token", "true".parse().unwrap());
            headers.insert("X-Key-Id", key_id.parse().unwrap());
            headers.insert(
                "X-Warning",
                "Mock token - for testing only".parse().unwrap(),
            );

            success_response(response, Some(headers))
        }
        Err(err) => {
            error!("Failed to generate mock tokens: {}", err);

            // Clean up the mock key on failure
            if let Err(cleanup_err) = state.0.key_manager.delete_key(&key_id).await {
                warn!("Failed to cleanup mock key {}: {}", key_id, cleanup_err);
            }

            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to generate mock tokens",
                None,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::token::TokenType;

    fn claims_for(sub: &str) -> Claims {
        Claims {
            device: None,
            sub: sub.to_string(),
            key_id: sub.to_string(),
            iss: "calimero-test".to_string(),
            aud: "calimero-test".to_string(),
            exp: 0,
            iat: 0,
            jti: "jti".to_string(),
            token_type: TokenType::Access,
            permissions: vec![],
            node_url: None,
        }
    }

    fn record(method: &str) -> Key {
        Key::new_root_key_with_permissions("subject".to_owned(), method.to_owned(), vec![], None)
    }

    fn named(key: &Key, claims: &Claims) -> (Option<String>, Option<String>) {
        let mut headers = HeaderMap::new();
        name_account_session(key, claims, &mut headers);
        let read = |name: &str| {
            headers
                .get(name)
                .map(|value| value.to_str().unwrap().to_owned())
        };
        (read("X-Auth-Account"), read("X-Auth-Device"))
    }

    /// An `account_proof` session names its account and the device that opened
    /// it, so a node behind a proxy can scope the caller as the embedded guard
    /// would.
    #[test]
    fn an_account_session_names_its_account_and_device() {
        let mut claims = claims_for("acc0unt");
        claims.device = Some("d3v1ce".to_owned());
        assert_eq!(
            named(
                &record(crate::providers::impls::account_proof::METHOD),
                &claims
            ),
            (Some("acc0unt".to_owned()), Some("d3v1ce".to_owned()))
        );
    }

    #[test]
    fn an_account_session_without_a_device_names_only_the_account() {
        assert_eq!(
            named(
                &record(crate::providers::impls::account_proof::METHOD),
                &claims_for("acc0unt")
            ),
            (Some("acc0unt".to_owned()), None)
        );
    }

    /// A username/password session's subject is no account. Naming it as one
    /// would read the node owner as a tenant.
    #[test]
    fn a_password_session_names_no_account() {
        let mut claims = claims_for("admin");
        claims.device = Some("d3v1ce".to_owned());
        assert_eq!(named(&record("user_password"), &claims), (None, None));
    }

    #[test]
    fn tokens_share_key_accepts_same_key() {
        assert!(tokens_share_key(&claims_for("key-a"), &claims_for("key-a")));
    }

    #[test]
    fn tokens_share_key_rejects_mismatched_key() {
        assert!(!tokens_share_key(
            &claims_for("key-a"),
            &claims_for("key-b")
        ));
    }

    #[test]
    fn tokens_share_key_rejects_two_keys_of_one_user() {
        let mut other_client = claims_for("client-b");
        other_client.sub = "user".to_owned();
        let mut client = claims_for("client-a");
        client.sub = "user".to_owned();
        assert!(!tokens_share_key(&client, &other_client));
    }
}

/// Response to `GET /auth/challenge`.
#[derive(Debug, Serialize)]
pub struct ChallengeResponse {
    /// Hex of the 32 bytes the client signs over.
    challenge: String,
    /// Unix seconds after which the challenge is refused.
    expires_at: u64,
}

/// Issue a login challenge for the account-proof provider.
///
/// Unauthenticated, and it has to be: a caller asking for one is by definition
/// not yet authenticated. That is safe because a challenge is inert on its own —
/// it authorizes nothing, is worthless without a device key certified under an
/// account, and is minted statelessly, so asking for one repeatedly writes
/// nothing and costs an HMAC.
///
/// The minter is built here over the same storage the provider uses, so both
/// read the same MAC key; there is nothing to plumb between them.
///
/// Refused when the provider is disabled, rather than handing out challenges no
/// login could ever spend.
pub async fn challenge_handler(state: Extension<Arc<AppState>>) -> impl IntoResponse {
    if !state
        .0
        .config
        .providers
        .get("account_proof")
        .copied()
        .unwrap_or(false)
    {
        return error_response(
            StatusCode::NOT_FOUND,
            "The account_proof provider is not enabled on this node",
            None,
        );
    }

    let minter = crate::auth::challenge::ChallengeMinter::new(
        Arc::clone(&state.0.storage),
        state.0.config.account_proof.challenge_ttl_secs,
    );

    match minter.issue().await {
        Ok(challenge) => success_response(
            ChallengeResponse {
                challenge: hex::encode(challenge.bytes),
                expires_at: challenge.expires_at,
            },
            None,
        ),
        Err(err) => {
            error!("Failed to issue a login challenge: {err}");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to issue a challenge",
                None,
            )
        }
    }
}

#[cfg(test)]
mod forward_auth_tests {
    use axum::http::{HeaderMap, HeaderName, HeaderValue};

    use super::{classify, Probe};
    use crate::auth::permissions::{
        AdminPermission, BlobPermission, Permission, PermissionValidator,
    };

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            let name = HeaderName::from_bytes(k.as_bytes()).unwrap();
            let _ignored = h.insert(name, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    /// The session gate must survive this change.
    ///
    /// `mero-js` calls `HEAD /auth/validate` carrying only an `Authorization`
    /// header and treats anything but 200 as "logged out". Deciding that a call
    /// with no forwarded headers is an unauthorized request would put every app
    /// back in a login loop — the outage this endpoint's contract test exists
    /// for. Absence of the headers is a different question, not a failed answer.
    #[test]
    fn a_call_with_no_forwarded_headers_is_a_session_gate() {
        assert!(matches!(
            classify(&headers(&[("Authorization", "Bearer x")])),
            Probe::SessionGate
        ));
    }

    /// A proxy probe is decided against the request it names, not this one.
    #[test]
    fn a_forwarded_probe_carries_the_named_method_and_path() {
        let Probe::Forwarded(request) = classify(&headers(&[
            ("X-Forwarded-Uri", "/admin-api/contexts"),
            ("X-Forwarded-Method", "POST"),
        ])) else {
            panic!("expected a forwarded probe");
        };

        assert_eq!(request.method(), "POST");
        assert_eq!(request.uri().path(), "/admin-api/contexts");
    }

    /// A query string travels in this header — that is how a token arrives on
    /// routes that cannot set one — and must not become part of the path, or
    /// every such request falls to the admin-api default-deny.
    #[test]
    fn a_forwarded_query_string_is_not_part_of_the_path() {
        let Probe::Forwarded(request) = classify(&headers(&[
            ("X-Forwarded-Uri", "/admin-api/contexts?token=abc"),
            ("X-Forwarded-Method", "GET"),
        ])) else {
            panic!("expected a forwarded probe");
        };

        assert_eq!(request.uri().path(), "/admin-api/contexts");
    }

    /// A URI without a method is refused rather than assumed.
    ///
    /// Defaulting to GET would authorize a write under a read permission, and
    /// falling back to the session-gate answer would report "your token is
    /// valid" to a proxy that asked whether a request may proceed — a question
    /// that was never answered, answered permissively.
    #[test]
    fn a_forwarded_uri_without_a_method_is_refused() {
        assert!(matches!(
            classify(&headers(&[("X-Forwarded-Uri", "/admin-api/contexts")])),
            Probe::Malformed
        ));
    }

    /// `HEAD` reported by a proxy must decide as `GET`.
    ///
    /// This is the regression the rebuild-the-request approach exists to make
    /// impossible. `determine_required_permissions` normalizes `GET | HEAD`
    /// because a HEAD probe on a mapped GET route would otherwise fall to the
    /// admin-api default-deny — an incident that has already happened once, to
    /// `getBlobInfo`, which is still a shipped `HEAD /admin-api/blobs/:id`.
    ///
    /// A second parse of the header that forgot HEAD would reintroduce it, and
    /// only behind a proxy, where it is hardest to see. Routing the forwarded
    /// method through the same code a direct request takes is what rules that
    /// out structurally; this test pins the behaviour so the structure cannot
    /// be quietly undone.
    #[test]
    fn a_forwarded_head_decides_as_a_get() {
        let Probe::Forwarded(request) = classify(&headers(&[
            ("X-Forwarded-Uri", "/admin-api/blobs/blob-1"),
            ("X-Forwarded-Method", "HEAD"),
        ])) else {
            panic!("expected a forwarded probe");
        };

        let validator = PermissionValidator::new();
        let required = validator.determine_required_permissions(&request);

        assert!(
            matches!(
                required.as_slice(),
                [Permission::Blob(BlobPermission::GetOwn(_))]
            ),
            "a forwarded HEAD must require the GET permission, got {required:?}",
        );
        assert_ne!(
            required,
            vec![Permission::Admin(AdminPermission)],
            "a forwarded HEAD fell to the admin-api default-deny",
        );

        // And the scoped token a client actually holds satisfies it.
        assert!(validator.validate_permissions(&["blob:get[blob-1]".to_owned()], &required));
    }

    /// The whole point: a scoped token is decided against the route it named.
    #[test]
    fn a_scoped_token_is_judged_against_the_forwarded_route() {
        let validator = PermissionValidator::new();
        let scoped = vec!["context:list".to_owned()];

        let Probe::Forwarded(allowed) = classify(&headers(&[
            ("X-Forwarded-Uri", "/admin-api/contexts"),
            ("X-Forwarded-Method", "GET"),
        ])) else {
            panic!("expected a forwarded probe");
        };
        assert!(validator
            .validate_permissions(&scoped, &validator.determine_required_permissions(&allowed)));

        // An unmapped operator route falls to the default-deny, and the same
        // token must not reach it. Before this endpoint authorized anything,
        // it did.
        let Probe::Forwarded(denied) = classify(&headers(&[
            ("X-Forwarded-Uri", "/admin-api/install-dev-application"),
            ("X-Forwarded-Method", "POST"),
        ])) else {
            panic!("expected a forwarded probe");
        };
        assert!(!validator
            .validate_permissions(&scoped, &validator.determine_required_permissions(&denied)));
    }
}

#[cfg(test)]
mod login_throttle_tests {
    use super::*;
    use crate::auth::rate_limit::LoginRateLimiter;
    use crate::auth::token::TokenManager;
    use crate::config::UserPasswordConfig;
    use crate::embedded::default_config;
    use crate::providers::impls::user_password::UserPasswordProvider;
    use crate::providers::ProviderContext;
    use crate::secrets::SecretManager;
    use crate::storage::{KeyManager, MemoryStorage, Storage};
    use crate::utils::AuthMetrics;
    use crate::AuthService;

    async fn state(limiter: LoginRateLimiter) -> Arc<AppState> {
        state_with_ceiling(limiter, LoginRateLimiter::account_ceiling()).await
    }

    async fn state_with_ceiling(
        limiter: LoginRateLimiter,
        ceiling: LoginRateLimiter,
    ) -> Arc<AppState> {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let secret_manager = Arc::new(SecretManager::new(Arc::clone(&storage)));
        secret_manager.initialize().await.unwrap();

        let config = default_config();
        let token_manager =
            TokenManager::new(config.jwt.clone(), Arc::clone(&storage), secret_manager);
        let key_manager = KeyManager::new(Arc::clone(&storage));
        let provider = UserPasswordProvider::new(
            ProviderContext {
                storage: Arc::clone(&storage),
                key_manager: key_manager.clone(),
                token_manager: token_manager.clone(),
                config: Arc::new(config.clone()),
            },
            UserPasswordConfig::default(),
        );

        Arc::new(AppState {
            auth_service: AuthService::new(vec![Box::new(provider)], token_manager.clone()),
            storage,
            key_manager,
            token_generator: token_manager,
            config,
            metrics: AuthMetrics::new(),
            login_rate_limiter: Arc::new(limiter),
            account_rate_limiter: Arc::new(ceiling),
        })
    }

    // A window wide enough that slow debug-build key derivations cannot age
    // failures out mid-test.
    async fn state_with_wide_window() -> Arc<AppState> {
        state(LoginRateLimiter::new(5, 3_600_000)).await
    }

    const PASSWORD: &str = "correct horse battery staple";

    async fn respond_with(
        state: &Arc<AppState>,
        method: &str,
        public_key: &str,
        username: &str,
        password: &str,
    ) -> axum::response::Response {
        let request: TokenRequest = serde_json::from_value(serde_json::json!({
            "auth_method": method,
            "public_key": public_key,
            "client_name": "http://localhost:2428",
            "timestamp": 0,
            "provider_data": { "username": username, "password": password },
        }))
        .unwrap();

        token_handler(Extension(Arc::clone(state)), None, ValidatedJson(request))
            .await
            .into_response()
    }

    async fn respond_from(
        state: &Arc<AppState>,
        ip: [u8; 4],
        username: &str,
        password: &str,
    ) -> axum::response::Response {
        let request: TokenRequest = serde_json::from_value(serde_json::json!({
            "auth_method": "user_password",
            "public_key": "pk",
            "client_name": "http://localhost:2428",
            "timestamp": 0,
            "provider_data": { "username": username, "password": password },
        }))
        .unwrap();
        let peer = Extension(ConnectInfo(SocketAddr::from((ip, 40_000))));

        token_handler(
            Extension(Arc::clone(state)),
            Some(peer),
            ValidatedJson(request),
        )
        .await
        .into_response()
    }

    async fn respond(
        state: &Arc<AppState>,
        method: &str,
        public_key: &str,
        username: &str,
    ) -> axum::response::Response {
        respond_with(state, method, public_key, username, "not the password").await
    }

    async fn provision_admin(state: &Arc<AppState>) -> String {
        crate::provisioning::provision_admin_key(
            &state.storage,
            &UserPasswordConfig::default(),
            "admin",
            PASSWORD,
        )
        .await
        .unwrap()
    }

    async fn login(state: &Arc<AppState>) -> String {
        let response = respond_with(state, "user_password", "pk", "admin", PASSWORD).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        body["data"]["access_token"].as_str().unwrap().to_owned()
    }

    /// Asks the token and the forward-auth gate who `token` names.
    /// Returns (token subject, `X-Auth-User` header).
    async fn subject_and_header_of(state: &Arc<AppState>, token: &str) -> (String, String) {
        let sub = state.token_generator.verify_token(token).await.unwrap().sub;

        let mut headers = HeaderMap::new();
        headers.insert("Authorization", format!("Bearer {token}").parse().unwrap());
        headers.insert("Host", "localhost:2428".parse().unwrap());
        let gate = validate_handler(Extension(Arc::clone(state)), headers)
            .await
            .into_response();
        assert_eq!(gate.status(), StatusCode::OK);
        let header = gate.headers()["X-Auth-User"].to_str().unwrap().to_owned();
        (sub, header)
    }

    async fn subject_and_header(state: &Arc<AppState>) -> (String, String) {
        subject_and_header_of(state, &login(state).await).await
    }

    #[tokio::test]
    async fn a_client_sessions_subject_and_header_name_its_user_not_its_key() {
        let state = state_with_wide_window().await;
        let user = provision_admin(&state).await;
        let client = Key::new_client_key(user.clone(), "app".to_owned(), vec![], None);
        let _ = state
            .key_manager
            .set_key("client-1", &client)
            .await
            .unwrap();
        let (access, refresh) = state
            .token_generator
            .generate_token_pair("client-1".to_owned(), vec![], None, None)
            .await
            .unwrap();

        assert_eq!(
            subject_and_header_of(&state, &access).await,
            (user.clone(), user.clone())
        );

        // Refreshing rotates the client key; the user it names stays put.
        let (rotated, _) = state
            .token_generator
            .refresh_token_pair(&refresh)
            .await
            .unwrap();
        assert_eq!(
            subject_and_header_of(&state, &rotated).await,
            (user.clone(), user)
        );
    }

    #[tokio::test]
    async fn re_registering_a_revoked_user_does_not_revive_its_sessions() {
        let state = state_with_wide_window().await;
        let old = provision_admin(&state).await;
        let token = login(&state).await;
        let mut key = state.key_manager.get_key(&old).await.unwrap().unwrap();
        key.revoke();
        let _ = state.key_manager.set_key(&old, &key).await.unwrap();

        let new = provision_admin(&state).await;

        assert_ne!(new, old);
        assert!(state
            .token_generator
            .verify_token_string(&token, None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn a_users_subject_is_random_not_derived_from_the_credentials() {
        let mut subjects = Vec::new();
        for _ in 0..2 {
            let state = state_with_wide_window().await;
            let key_id = provision_admin(&state).await;

            let (sub, header) = subject_and_header(&state).await;
            let (again, _) = subject_and_header(&state).await;

            assert_eq!((&sub, &header), (&key_id, &key_id));
            assert_eq!(again, sub, "the subject must stay stable across logins");
            subjects.push(sub);
        }

        assert_ne!(
            subjects[0], subjects[1],
            "the same credentials must not give the same subject"
        );
    }

    async fn attempt(
        state: &Arc<AppState>,
        method: &str,
        public_key: &str,
        username: &str,
    ) -> StatusCode {
        respond(state, method, public_key, username).await.status()
    }

    async fn fail_five_times(state: &Arc<AppState>, username: &str) {
        for i in 0..5 {
            let status = attempt(state, "user_password", "shared-pk", username).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "failure {i}");
        }
    }

    #[tokio::test]
    async fn repeated_failures_for_one_account_are_locked_out() {
        let state = state_with_wide_window().await;
        for _ in 0..5 {
            let status = attempt(&state, "user_password", "pk", "admin").await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
        let status = attempt(&state, "user_password", "pk", "admin").await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn lockout_is_per_account_regardless_of_public_key() {
        let state = state_with_wide_window().await;
        fail_five_times(&state, "admin").await;

        let status = attempt(&state, "user_password", "a-fresh-public-key", "admin").await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn method_aliases_share_one_account_bucket() {
        let state = state_with_wide_window().await;
        fail_five_times(&state, "admin").await;

        let status = attempt(&state, "username_password", "pk", "admin").await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn a_locked_account_does_not_lock_other_accounts() {
        let state = state_with_wide_window().await;
        fail_five_times(&state, "admin").await;

        let status = attempt(&state, "user_password", "shared-pk", "someone-else").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn rejected_attempts_do_not_extend_the_lockout() {
        let window_secs = 6;
        let state = state(LoginRateLimiter::new(1, window_secs * 1000)).await;
        assert_eq!(
            attempt(&state, "user_password", "pk", "admin").await,
            StatusCode::UNAUTHORIZED
        );
        let locked_at = std::time::Instant::now();

        for _ in 0..2 {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            let elapsed = locked_at.elapsed().as_secs_f64();
            let response = respond(&state, "user_password", "pk", "admin").await;
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

            // The lockout ends one window after the last real failure.
            let retry_after: f64 = response.headers()["Retry-After"]
                .to_str()
                .unwrap()
                .parse()
                .unwrap();
            assert!(
                retry_after <= (window_secs as f64 - elapsed).ceil(),
                "retry-after {retry_after}s at {elapsed:.1}s into a {window_secs}s window"
            );
        }
    }

    #[tokio::test]
    async fn one_callers_failures_do_not_lock_the_owner_out() {
        let state = state_with_wide_window().await;
        let _key_id = provision_admin(&state).await;

        let attacker = [203, 0, 113, 9];
        for i in 0..5 {
            let status = respond_from(&state, attacker, "admin", "wrong")
                .await
                .status();
            assert_eq!(status, StatusCode::UNAUTHORIZED, "failure {i}");
        }
        assert_eq!(
            respond_from(&state, attacker, "admin", "wrong")
                .await
                .status(),
            StatusCode::TOO_MANY_REQUESTS,
            "the attacker is locked out"
        );
        assert_eq!(
            respond_from(&state, attacker, "admin", PASSWORD)
                .await
                .status(),
            StatusCode::TOO_MANY_REQUESTS,
            "the lockout holds for the attacker even with the right password"
        );

        let owner = respond_from(&state, [198, 51, 100, 7], "admin", PASSWORD).await;
        assert_eq!(owner.status(), StatusCode::OK, "the owner still gets in");
    }

    #[tokio::test]
    async fn failures_spread_over_many_sources_reach_the_account_ceiling() {
        let state = state_with_ceiling(
            LoginRateLimiter::new(5, 3_600_000),
            LoginRateLimiter::new(10, 3_600_000),
        )
        .await;
        let _key_id = provision_admin(&state).await;

        for host in 0..10_u8 {
            let status = respond_from(&state, [203, 0, 113, host], "admin", "wrong")
                .await
                .status();
            assert_eq!(status, StatusCode::UNAUTHORIZED, "failure from host {host}");
        }

        let owner = respond_from(&state, [198, 51, 100, 7], "admin", PASSWORD).await;
        assert_eq!(
            owner.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "the account is locked at the ceiling whatever the source"
        );
    }

    /// Spellings that sanitize into `user_password`.
    fn method_spellings() -> Vec<String> {
        vec![
            "user_password.".to_owned(),
            ".user_password".to_owned(),
            " user_password ".to_owned(),
            "user_password\n".to_owned(),
            "user_password\0".to_owned(),
            "user\0_password".to_owned(),
            "user_pass word".to_owned(),
            "user_password\u{200b}".to_owned(),
            "user_password!!!".to_owned(),
            format!("user_password{}", "!".repeat(100_000)),
            "username_password.".to_owned(),
        ]
    }

    #[tokio::test]
    async fn method_spellings_share_the_locked_accounts_bucket() {
        let state = state_with_wide_window().await;
        let _key_id = provision_admin(&state).await;
        fail_five_times(&state, "admin").await;

        for (i, method) in method_spellings().iter().enumerate() {
            let wrong = attempt(&state, method, &format!("rotating-{i}"), "admin").await;
            assert_eq!(wrong, StatusCode::TOO_MANY_REQUESTS, "{method:?}");

            let right = respond_with(&state, method, &format!("other-{i}"), "admin", PASSWORD)
                .await
                .status();
            assert_eq!(right, StatusCode::TOO_MANY_REQUESTS, "{method:?}");
        }
    }

    #[tokio::test]
    async fn method_spellings_of_no_provider_never_authenticate() {
        let state = state_with_wide_window().await;
        let _key_id = provision_admin(&state).await;

        // Case and lookalike letters survive sanitizing and match no provider.
        for method in ["USER_PASSWORD", "User_Password", "user_passw\u{43e}rd"] {
            let status = respond_with(&state, method, "pk", "admin", PASSWORD)
                .await
                .status();
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method:?}");
        }

        // Nothing left after sanitizing is a bad request, not a login attempt.
        for method in ["", "!!!", "\0"] {
            let status = respond_with(&state, method, "pk", "admin", PASSWORD)
                .await
                .status();
            assert_eq!(status, StatusCode::BAD_REQUEST, "{method:?}");
        }
    }

    #[tokio::test]
    async fn a_locked_account_is_locked_under_every_method_spelling_at_once() {
        // Failures spread over spellings count towards the same account.
        let state = state_with_wide_window().await;
        let _key_id = provision_admin(&state).await;
        for (i, method) in method_spellings().iter().take(5).enumerate() {
            let status = attempt(&state, method, &format!("pk-{i}"), "admin").await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method:?}");
        }

        let status = respond_with(&state, "user_password", "pk", "admin", PASSWORD)
            .await
            .status();
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn username_spellings_are_the_accounts_storage_sees() {
        let state = state_with_wide_window().await;
        let _key_id = provision_admin(&state).await;
        fail_five_times(&state, "admin").await;

        // Case and whitespace name other accounts, which do not exist.
        for username in ["Admin", "admin ", " admin", "ADMIN", "admin\u{200b}"] {
            let status = respond_with(&state, "user_password", "pk", username, PASSWORD)
                .await
                .status();
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{username:?}");
        }
        let status = respond_with(&state, "user_password", "pk", "admin", PASSWORD)
            .await
            .status();
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn requests_that_are_not_credential_guesses_do_not_lock_the_account() {
        let state = state_with_wide_window().await;
        let _key_id = provision_admin(&state).await;

        let too_long = "x".repeat(500);
        for i in 0..6 {
            let status = respond_with(&state, "user_password", "pk", "admin", &too_long)
                .await
                .status();
            assert_eq!(status, StatusCode::UNAUTHORIZED, "attempt {i}");
        }

        let status = respond_with(&state, "user_password", "pk", "admin", PASSWORD)
            .await
            .status();
        assert_eq!(status, StatusCode::OK);
    }
}
