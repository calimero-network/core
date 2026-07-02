//! Connection management for Calimero client
//!
//! This module provides the core connection functionality for making
//! authenticated API requests to Calimero services.

// Standard library
use std::sync::Arc;

// External crates
use eyre::{bail, eyre, Result};
use reqwest::{Client, Response};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::sync::Mutex;
use url::Url;

// Local crate
use crate::errors::ClientError;
use crate::storage::JwtToken;
use crate::traits::{ClientAuthenticator, ClientStorage};

/// Refresh a token this many seconds *before* its stated expiry, so a request
/// isn't sent with a token that lapses in flight (which would waste a 401
/// round-trip). Also the window within which a still-valid token is refreshed
/// proactively rather than reactively.
const TOKEN_REFRESH_SKEW_SECS: i64 = 30;

/// Maximum size of a control-plane JSON response body we will buffer. Admin-API
/// responses are small; a multi-gigabyte body from a hostile or buggy node must
/// not be read into memory unbounded.
pub(crate) const MAX_JSON_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Maximum size of an error response body we will buffer before extracting a
/// message. Error bodies should be tiny; cap them hard.
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// Maximum size of a binary (blob) response body. Larger than the JSON cap
/// because blobs are the data plane, but still bounded so a lying
/// `Content-Length` or an endless stream can't exhaust memory.
const MAX_BINARY_BODY_BYTES: usize = 512 * 1024 * 1024;

/// Authentication mode for a connection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// Authentication is required for this connection
    Required,
    /// No authentication is required for this connection
    None,
}

// Define RequestType enum locally since it's not available in client
#[derive(Debug, Clone, Copy)]
enum RequestType {
    Get,
    Post,
    Delete,
    Patch,
    Put,
    DeleteWithBody,
}

impl RequestType {
    /// Whether re-sending this request after a 401 is safe.
    ///
    /// Idempotent verbs (GET/HEAD/PUT/DELETE) can be replayed with no
    /// additional side effect. POST and PATCH are **not** replayed: a 401 from
    /// an intermediary that already forwarded the write to the origin is
    /// indistinguishable from one where the write never landed, so replaying a
    /// `create_context`/governance POST risks duplicating it.
    const fn is_idempotent(self) -> bool {
        match self {
            Self::Get | Self::Put | Self::Delete | Self::DeleteWithBody => true,
            Self::Post | Self::Patch => false,
        }
    }
}

/// Generic connection information that can work with any authenticator and storage implementation
#[derive(Clone, Debug)]
pub struct ConnectionInfo<A, S>
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    // Private: the HTTP transport (`client`), the `authenticator` (holds
    // credentials), and `client_storage` must not leak out of the connection,
    // and none of the fields may be swapped mid-flight by a caller. Access the
    // non-sensitive bits through the accessors below; everything else goes
    // through the request methods.
    api_url: Url,
    client: Client,
    node_name: Option<String>,
    authenticator: A,
    client_storage: S,
    // Serializes token refresh / interactive re-authentication so N concurrent
    // 401s (or N token-less requests) collapse into a single refresh instead of
    // each firing its own `/auth/refresh` (which would burn a one-time refresh
    // token) or opening its own browser prompt.
    auth_lock: Arc<Mutex<()>>,
}

impl<A, S> ConnectionInfo<A, S>
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    pub fn new(
        api_url: Url,
        node_name: Option<String>,
        authenticator: A,
        client_storage: S,
    ) -> Self {
        Self {
            api_url,
            client: Client::new(),
            node_name,
            authenticator,
            client_storage,
            auth_lock: Arc::new(Mutex::new(())),
        }
    }

    /// The base API URL this connection targets.
    #[must_use]
    pub fn api_url(&self) -> &Url {
        &self.api_url
    }

    /// The configured node name, if any.
    #[must_use]
    pub fn node_name(&self) -> Option<&str> {
        self.node_name.as_deref()
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request(RequestType::Get, path, None::<()>).await
    }

    /// Check if a path requires authentication
    fn path_requires_auth(&self, path: &str) -> bool {
        // Only admin-api/health is public, everything else requires authentication
        !path.starts_with("admin-api/health")
    }

    pub async fn post<I, O>(&self, path: &str, body: I) -> Result<O>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        self.request(RequestType::Post, path, Some(body)).await
    }

    pub async fn post_no_body<O: DeserializeOwned>(&self, path: &str) -> Result<O> {
        self.request(RequestType::Post, path, None::<()>).await
    }

    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request(RequestType::Delete, path, None::<()>).await
    }

    pub async fn delete_with_body<I, O>(&self, path: &str, body: I) -> Result<O>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        self.request(RequestType::DeleteWithBody, path, Some(body))
            .await
    }

    pub async fn patch<I, O>(&self, path: &str, body: I) -> Result<O>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        self.request(RequestType::Patch, path, Some(body)).await
    }

    pub async fn put_json<I, O>(&self, path: &str, body: I) -> Result<O>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        self.request(RequestType::Put, path, Some(body)).await
    }

    pub async fn put_binary(&self, path: &str, data: Vec<u8>) -> Result<reqwest::Response> {
        let url = resolve_path(&self.api_url, path)?;

        let requires_auth = self.path_requires_auth(path);

        // PUT is idempotent — safe to replay after a 401.
        self.execute_request_with_auth_retry(requires_auth, true, |auth_header| {
            let mut builder = self.client.put(url.clone()).body(data.clone());
            if let Some(h) = auth_header {
                builder = builder.header("Authorization", h);
            }
            builder.send()
        })
        .await
    }

    pub async fn get_binary(&self, path: &str) -> Result<Vec<u8>> {
        let url = resolve_path(&self.api_url, path)?;

        let requires_auth = self.path_requires_auth(path);

        let response = self
            .execute_request_with_auth_retry(requires_auth, true, |auth_header| {
                let mut builder = self.client.get(url.clone());
                if let Some(h) = auth_header {
                    builder = builder.header("Authorization", h);
                }
                builder.send()
            })
            .await?;

        read_body_capped(response, MAX_BINARY_BODY_BYTES).await
    }

    pub async fn head(&self, path: &str) -> Result<reqwest::header::HeaderMap> {
        let url = resolve_path(&self.api_url, path)?;

        let requires_auth = self.path_requires_auth(path);

        let response = self
            .execute_request_with_auth_retry(requires_auth, true, |auth_header| {
                let mut builder = self.client.head(url.clone());
                if let Some(h) = auth_header {
                    builder = builder.header("Authorization", h);
                }
                builder.send()
            })
            .await?;

        Ok(response.headers().clone())
    }

    async fn request<I, O>(&self, req_type: RequestType, path: &str, body: Option<I>) -> Result<O>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        let url = resolve_path(&self.api_url, path)?;

        let requires_auth = self.path_requires_auth(path);

        let response = self
            .execute_request_with_auth_retry(
                requires_auth,
                req_type.is_idempotent(),
                |auth_header| {
                    let mut builder = match req_type {
                        RequestType::Get => self.client.get(url.clone()),
                        RequestType::Post => self.client.post(url.clone()).json(&body),
                        RequestType::Delete => self.client.delete(url.clone()),
                        RequestType::Patch => self.client.patch(url.clone()).json(&body),
                        RequestType::Put => self.client.put(url.clone()).json(&body),
                        RequestType::DeleteWithBody => self.client.delete(url.clone()).json(&body),
                    };
                    if let Some(h) = auth_header {
                        builder = builder.header("Authorization", h);
                    }
                    builder.send()
                },
            )
            .await?;

        let body = read_body_capped(response, MAX_JSON_BODY_BYTES).await?;
        serde_json::from_slice::<O>(&body).map_err(Into::into)
    }

    /// Resolve the bearer auth header for this connection (always treating auth
    /// as required), if a node name and credentials are available.
    ///
    /// Public counterpart to [`Self::ensure_auth_header`] for callers outside
    /// the HTTP request helpers — e.g. attaching auth to a WebSocket upgrade
    /// handshake, where auth is validated once at connect rather than per call.
    /// May trigger a browser-based authentication flow when no token is stored;
    /// concurrent callers coalesce behind a single flow rather than each opening
    /// their own browser prompt.
    pub async fn auth_header(&self) -> Result<Option<String>> {
        self.ensure_auth_header(true).await
    }

    /// Ensure a valid auth header is available and return it.
    ///
    /// Returns `None` immediately if `requires_auth` is false or `node_name` is
    /// unset. A stored token is checked against its expiry: if it is expired it
    /// is refreshed before use; if it lapses within [`TOKEN_REFRESH_SKEW_SECS`]
    /// it is refreshed proactively (best-effort). An empty stored token is
    /// treated as "no credentials". When no usable token exists, this triggers a
    /// full (possibly browser-based) authentication flow — but concurrent
    /// callers share one flow via [`Self::refresh_or_reauth`].
    async fn ensure_auth_header(&self, requires_auth: bool) -> Result<Option<String>> {
        if !requires_auth {
            return Ok(None);
        }
        let Some(node_name) = &self.node_name else {
            return Ok(None);
        };

        // An empty access token (e.g. a logged-out record persisted by the
        // default `remove_tokens`) counts as "no credentials", not a valid
        // bearer value.
        let stored = match self.client_storage.load_tokens(node_name).await {
            Ok(Some(tokens)) if tokens.is_usable() => Some(tokens),
            Ok(_) => None,
            Err(e) => {
                // Surface storage failures so the user can diagnose disk/permission issues
                // rather than silently falling through to a re-authentication prompt.
                bail!("Failed to load stored tokens for '{}': {}", node_name, e);
            }
        };

        if let Some(tokens) = stored {
            if tokens.is_expired() {
                // Hard-expired — must obtain a fresh token before proceeding.
                self.refresh_or_reauth(Some(&tokens.access_token)).await?;
                return self.current_auth_header(node_name).await;
            }
            if tokens.expires_soon(TOKEN_REFRESH_SKEW_SECS) {
                // Still valid but lapsing soon — refresh proactively to avoid a
                // wasted 401, but don't fail the request if the refresh can't
                // complete. Prefer the freshly-stored token (another task may
                // have refreshed concurrently); fall back to the local token
                // only if storage no longer yields a usable one. That fallback
                // is safe: control flow above guarantees `tokens` is not yet
                // expired here, only near expiry.
                if let Err(e) = self.refresh_or_reauth(Some(&tokens.access_token)).await {
                    tracing::debug!("proactive token refresh failed, using current token: {e}");
                }
                if let Some(header) = self.current_auth_header(node_name).await? {
                    return Ok(Some(header));
                }
            }
            return Ok(Some(tokens.auth_header()));
        }

        // No usable token — acquire one (single-flight).
        self.refresh_or_reauth(None).await?;
        self.current_auth_header(node_name).await
    }

    /// Read the currently-stored token as a bearer header, if usable.
    async fn current_auth_header(&self, node_name: &str) -> Result<Option<String>> {
        match self.client_storage.load_tokens(node_name).await {
            Ok(Some(tokens)) if tokens.is_usable() => Ok(Some(tokens.auth_header())),
            Ok(_) => Ok(None),
            Err(e) => bail!("Failed to load stored tokens for '{}': {}", node_name, e),
        }
    }

    /// Obtain a fresh token, coalescing concurrent callers behind `auth_lock`.
    ///
    /// `stale_access` is the access token that just failed (or is about to
    /// lapse). After taking the lock we re-check the stored token: if it already
    /// differs from `stale_access`, another task refreshed while we waited, so
    /// we return without issuing a second `/auth/refresh`. This is what stops N
    /// concurrent 401s from each spending the one-time refresh token (and each
    /// opening a browser prompt on fallback).
    async fn refresh_or_reauth(&self, stale_access: Option<&str>) -> Result<()> {
        let Some(node_name) = &self.node_name else {
            bail!(
                "Authentication required but no node name is available to load or persist tokens"
            );
        };

        let _guard = self.auth_lock.lock().await;

        // Someone else may have refreshed while we waited for the lock.
        if let Some(stale) = stale_access {
            if let Ok(Some(current)) = self.client_storage.load_tokens(node_name).await {
                if current.is_usable() && current.access_token != stale {
                    return Ok(());
                }
            }
        }

        // Try a token refresh first; fall back to full (possibly interactive)
        // re-authentication on any failure, including a missing refresh token.
        let new_tokens = match self.refresh_token().await {
            Ok(tokens) => tokens,
            Err(_) => self
                .authenticator
                .authenticate(&self.api_url)
                .await
                .map_err(|auth_err| eyre!("Authentication failed: {}", auth_err))?,
        };

        self.client_storage
            .update_tokens(node_name, &new_tokens)
            .await
    }

    /// Execute a request with automatic token refresh / re-authentication on 401.
    ///
    /// The closure receives a fresh `Option<String>` auth header on **every** call,
    /// including retries after token refresh. This ensures stale tokens are never
    /// reused across retry attempts.
    ///
    /// On a 401 the session is always re-authenticated so subsequent calls work,
    /// but the failed request is only *replayed* when `idempotent` is true.
    /// Non-idempotent requests (POST/PATCH) are not replayed, to avoid
    /// duplicating a create/governance operation the origin may already have
    /// applied before the 401 was returned.
    async fn execute_request_with_auth_retry<F, Fut>(
        &self,
        requires_auth: bool,
        idempotent: bool,
        request_builder: F,
    ) -> Result<reqwest::Response>
    where
        F: Fn(Option<String>) -> Fut,
        Fut: std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
    {
        let mut retry_count = 0;
        const MAX_RETRIES: u32 = 2;

        loop {
            // Load a fresh auth header on EVERY iteration so retries use up-to-date tokens.
            let auth_header = self.ensure_auth_header(requires_auth).await?;

            let response = request_builder(auth_header).await?;

            if response.status() == 401 && retry_count < MAX_RETRIES {
                let Some(node_name) = &self.node_name else {
                    bail!(
                        "Authentication required but no node name is available to load or persist tokens"
                    );
                };

                // Re-authenticate so the session is healed regardless of verb.
                // Pass the currently-stored access token as the "stale" marker
                // for single-flight dedup.
                let stale = self
                    .client_storage
                    .load_tokens(node_name)
                    .await
                    .ok()
                    .flatten()
                    .map(|tokens| tokens.access_token.clone());
                self.refresh_or_reauth(stale.as_deref()).await?;

                if !idempotent {
                    // Non-idempotent verb: do not replay the body. The session
                    // is now re-authenticated; surface the 401 so the caller can
                    // retry the write deliberately rather than risk a duplicate.
                    bail!(
                        "Request rejected with 401 and was not replayed to avoid duplicating a \
                         non-idempotent operation; the session has been re-authenticated — please retry."
                    );
                }

                retry_count += 1;
                // Loop back — next iteration loads fresh tokens.
                continue;
            }

            if response.status() == 403 {
                bail!("Access denied — your token may not have sufficient permissions.");
            }

            if !response.status().is_success() {
                let status = response.status();
                // Cap the error body read so a hostile node can't OOM us on the
                // error path, then extract only known-safe message fields.
                let body = read_body_capped(response, MAX_ERROR_BODY_BYTES)
                    .await
                    .unwrap_or_else(|e| {
                        tracing::debug!("failed to read HTTP error response body: {e}");
                        Vec::new()
                    });
                let body = String::from_utf8_lossy(&body);
                // Return a typed error carrying the numeric status so callers can
                // classify the failure (e.g. 404 → not-found) by matching the
                // variant rather than parsing the message string. The rendered
                // message is still status-prefixed via `extract_error_message`,
                // so the `Display` output stays "HTTP {code}[: detail]".
                return Err(ClientError::Http {
                    status: status.as_u16(),
                    message: extract_error_message(&body, status),
                }
                .into());
            }

            return Ok(response);
        }
    }

    async fn refresh_token(&self) -> Result<JwtToken> {
        let Some(node_name) = &self.node_name else {
            return Err(eyre!("No tokens available to refresh"));
        };

        let Some(tokens) = self.client_storage.load_tokens(node_name).await? else {
            return Err(eyre!("No tokens available to refresh"));
        };

        let refresh_token = tokens
            .refresh_token
            .clone()
            .ok_or_else(|| eyre!("No refresh token available"))?;

        self.try_refresh_token(&tokens.access_token, &refresh_token)
            .await
    }

    async fn try_refresh_token(&self, access_token: &str, refresh_token: &str) -> Result<JwtToken> {
        // Resolve via `resolve_path` (not a raw `join`) so a reverse-proxy base
        // path on `api_url` is preserved — an absolute `join("/auth/refresh")`
        // would drop it and send the refresh to the wrong origin, the same class
        // of bug the request/ws_url paths already guard against.
        let refresh_url = resolve_path(&self.api_url, "auth/refresh")?;

        #[derive(serde::Serialize)]
        struct RefreshRequest {
            access_token: String,
            refresh_token: String,
        }

        #[derive(serde::Deserialize, Debug)]
        struct RefreshResponse {
            access_token: String,
            refresh_token: String,
        }

        #[derive(serde::Deserialize, Debug)]
        struct WrappedResponse {
            data: RefreshResponse,
        }

        let request_body = RefreshRequest {
            access_token: access_token.to_owned(),
            refresh_token: refresh_token.to_owned(),
        };

        let response = self
            .client
            .post(refresh_url)
            .json(&request_body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = read_body_capped(response, MAX_ERROR_BODY_BYTES)
                .await
                .unwrap_or_else(|e| {
                    tracing::debug!("failed to read token-refresh error response body: {e}");
                    Vec::new()
                });
            let body = String::from_utf8_lossy(&body);
            return Err(eyre!(
                "Token refresh failed: {}",
                extract_error_message(&body, status)
            ));
        }

        let body = read_body_capped(response, MAX_JSON_BODY_BYTES).await?;
        let wrapped_response: WrappedResponse = serde_json::from_slice(&body)?;

        Ok(JwtToken::with_refresh(
            wrapped_response.data.access_token,
            wrapped_response.data.refresh_token,
        ))
    }

    /// Update the stored JWT tokens using the storage interface
    pub async fn update_tokens(&self, new_tokens: &JwtToken) -> Result<()> {
        if let Some(node_name) = &self.node_name {
            self.client_storage
                .update_tokens(node_name, new_tokens)
                .await
        } else {
            // For external connections without a node name, we can't update storage
            // This is expected behavior
            Ok(())
        }
    }

    /// Detect the authentication mode for this connection.
    ///
    /// Probes `admin-api/contexts` (a protected endpoint) and inspects the response:
    /// - `401` → `AuthMode::Required`
    /// - `2xx` → `AuthMode::None` (server responded without demanding auth)
    /// - anything else (including `404`) → `AuthMode::Required` (safe default)
    /// - network error → `AuthMode::None` (node unreachable / no admin API)
    ///
    /// Note: unlike the previous implementation, **localhost / 127.0.0.1 no longer
    /// bypasses the probe**. Local nodes now go through the same detection so that
    /// locally-deployed nodes with auth enabled are handled correctly.
    pub async fn detect_auth_mode(&self) -> Result<AuthMode> {
        // Probe a protected endpoint — if it returns 401, auth is required.
        // admin-api/health is intentionally public, so we probe a protected endpoint instead.
        // Use `resolve_path` so a reverse-proxy base path is preserved and the
        // probe doesn't depend on `api_url` having a trailing slash (a raw
        // relative `join` drops the last base segment when it doesn't).
        let probe_url = resolve_path(&self.api_url, "admin-api/contexts")?;

        match self.client.get(probe_url).send().await {
            Ok(response) => {
                if response.status() == 401 {
                    // 401 Unauthorized means authentication is required
                    Ok(AuthMode::Required)
                } else if response.status().is_success() {
                    // 2xx without auth challenge means no authentication required.
                    // Note: 404 is intentionally excluded — a protected endpoint can return 404
                    // (e.g., empty contexts list) while still requiring auth for mutations.
                    Ok(AuthMode::None)
                } else {
                    // Other status codes, assume authentication is required for safety
                    Ok(AuthMode::Required)
                }
            }
            Err(e) => {
                // If we can't reach the endpoint, assume no authentication
                // required — common for local nodes without the admin API. This
                // also fires when a reverse-proxy base path is misconfigured or
                // the proxy is down, so log the cause at debug to make that
                // diagnosable rather than silently skipping auth.
                tracing::debug!("detect_auth_mode: probe request failed, assuming no auth: {e}");
                Ok(AuthMode::None)
            }
        }
    }
}

/// Build the target URL for `path` against `base`, splitting off any query
/// string and rejecting path traversal.
///
/// `Url::set_path` neither percent-encodes `/` within a segment nor collapses
/// `.`/`..` dot-segments, so an interpolated identifier such as
/// `../packages/evil` would let a request escape its intended
/// `admin-api/groups/...` prefix and reach a *different* admin endpoint after a
/// proxy normalizes the path. Each segment is rejected if it decodes to a `.`
/// or `..` dot-segment (so a percent-encoded `%2e%2e` can't slip past the guard
/// and be normalized to `..` at the origin) or contains a control character.
///
/// Segments are **appended** to the base URL's existing path via
/// `path_segments_mut`, so a reverse-proxy base path (e.g.
/// `https://host/calimero/node1/`) is preserved — consistent with
/// [`Client::ws_url`] — rather than being discarded by an absolute `set_path`.
pub(crate) fn resolve_path(base: &Url, path: &str) -> Result<Url> {
    use percent_encoding::percent_decode_str;

    let (path_part, query_part) = match path.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path, None),
    };

    let mut url = base.clone();
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|()| eyre!("api_url cannot be a base URL"))?;
        // Drop a trailing empty segment from the base (e.g. the one a trailing
        // `/` produces) so appending doesn't create a `//` in the path.
        segments.pop_if_empty();

        for segment in path_part.split('/') {
            if segment.is_empty() {
                continue;
            }
            let decoded = percent_decode_str(segment).decode_utf8_lossy();
            if decoded == "." || decoded == ".." {
                bail!("refusing request path with traversal segment: {path_part:?}");
            }
            if decoded.chars().any(char::is_control) {
                bail!("refusing request path with control character: {path_part:?}");
            }
            let _ = segments.push(segment);
        }
    }

    if let Some(query) = query_part {
        url.set_query(Some(query));
    }
    Ok(url)
}

/// Read a response body into memory with a hard byte cap (inclusive: a body of
/// exactly `max_bytes` is accepted; anything larger is rejected).
///
/// Protects against OOM from a hostile or buggy node returning (or slowly
/// streaming) a huge body. When a `Content-Length` header is present and over
/// the cap it is rejected up front; a missing header (e.g. chunked transfer) or
/// a header that lies about a smaller size is still caught by the per-chunk
/// tally, so bytes are never accumulated past the cap regardless of what the
/// node advertises.
pub(crate) async fn read_body_capped(response: Response, max_bytes: usize) -> Result<Vec<u8>> {
    if let Some(len) = response.content_length() {
        if len > max_bytes as u64 {
            bail!("response body of {len} bytes exceeds the {max_bytes}-byte limit");
        }
    }

    // Reserve up to the advertised length, but never more than the cap, so an
    // inflated `Content-Length` can't trigger a huge up-front allocation. The
    // early return above already bounds `len` to the cap; the `min` keeps this
    // safe independently of that check (defensive against future refactoring).
    let hint = response
        .content_length()
        .map_or(0, |len| len.min(max_bytes as u64) as usize);

    let mut response = response;
    let mut buf = Vec::with_capacity(hint);
    while let Some(chunk) = response.chunk().await? {
        // `saturating_add` guards the (only theoretical) usize overflow on
        // 32-bit targets when both operands are near the max.
        if buf.len().saturating_add(chunk.len()) > max_bytes {
            bail!("response body exceeds the {max_bytes}-byte limit");
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Extract a human-readable error message from an HTTP error response body.
///
/// Only extracts from known-safe structured fields (`error.message`, `error`, `message`).
/// Falls back to "HTTP {status}" rather than including raw body text to avoid
/// leaking sensitive server-internal details.  All extracted strings are capped at
/// 300 characters for consistency.
fn extract_error_message(body: &str, status: reqwest::StatusCode) -> String {
    let trimmed = body.trim();

    if trimmed.is_empty() {
        return format!("HTTP {}", status.as_u16());
    }

    const MAX_LEN: usize = 300;

    let truncate = |s: &str| -> String {
        if s.chars().count() > MAX_LEN {
            format!("{}…", s.chars().take(MAX_LEN).collect::<String>())
        } else {
            s.to_owned()
        }
    };

    // Try to parse as JSON and extract a meaningful message from known-safe fields.
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
        // { "error": { "message": "..." } }
        if let Some(msg) = json
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return format!("HTTP {}: {}", status.as_u16(), truncate(msg));
        }
        // { "error": "..." }
        if let Some(msg) = json.get("error").and_then(|m| m.as_str()) {
            return format!("HTTP {}: {}", status.as_u16(), truncate(msg));
        }
        // { "message": "..." }
        if let Some(msg) = json.get("message").and_then(|m| m.as_str()) {
            return format!("HTTP {}: {}", status.as_u16(), truncate(msg));
        }
    }

    // Non-JSON or no known error field — return just the status code to avoid body leakage.
    format!("HTTP {}", status.as_u16())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_path_builds_normal_path() {
        let base = Url::parse("https://node.example/").unwrap();
        let url = resolve_path(&base, "admin-api/groups/abc/members").unwrap();
        assert_eq!(url.path(), "/admin-api/groups/abc/members");
        assert_eq!(url.query(), None);
    }

    #[test]
    fn resolve_path_splits_query() {
        let base = Url::parse("https://node.example/").unwrap();
        let url = resolve_path(&base, "admin-api/blobs?context_id=xyz").unwrap();
        assert_eq!(url.path(), "/admin-api/blobs");
        assert_eq!(url.query(), Some("context_id=xyz"));
    }

    #[test]
    fn resolve_path_rejects_dotdot_traversal() {
        let base = Url::parse("https://node.example/").unwrap();
        // An interpolated group id of `..` must not be able to climb out of the
        // `admin-api/groups` prefix into a different admin endpoint.
        assert!(resolve_path(&base, "admin-api/groups/../packages/evil").is_err());
    }

    #[test]
    fn resolve_path_rejects_percent_encoded_traversal() {
        let base = Url::parse("https://node.example/").unwrap();
        // `%2e%2e` decodes to `..` — a proxy may normalize it before routing, so
        // it must be rejected just like a literal `..`.
        assert!(resolve_path(&base, "admin-api/groups/%2e%2e/evil").is_err());
        assert!(resolve_path(&base, "admin-api/groups/%2E%2E/evil").is_err());
    }

    #[test]
    fn resolve_path_preserves_reverse_proxy_base_path() {
        // A base URL with a mount path (behind a reverse proxy) must be kept:
        // the request lands under the base, not at the host root.
        let base = Url::parse("https://host/calimero/node1/").unwrap();
        let url = resolve_path(&base, "admin-api/contexts?foo=bar").unwrap();
        assert_eq!(url.path(), "/calimero/node1/admin-api/contexts");
        assert_eq!(url.query(), Some("foo=bar"));

        // A base without a trailing slash still appends rather than replacing
        // its last segment.
        let base = Url::parse("https://host/calimero/node1").unwrap();
        let url = resolve_path(&base, "admin-api/contexts").unwrap();
        assert_eq!(url.path(), "/calimero/node1/admin-api/contexts");
    }

    #[test]
    fn resolve_path_rejects_single_dot_and_control_chars() {
        let base = Url::parse("https://node.example/").unwrap();
        assert!(resolve_path(&base, "admin-api/./groups").is_err());
        assert!(resolve_path(&base, "admin-api/groups/a\nb").is_err());
    }

    #[test]
    fn request_type_idempotency_classification() {
        assert!(RequestType::Get.is_idempotent());
        assert!(RequestType::Put.is_idempotent());
        assert!(RequestType::Delete.is_idempotent());
        assert!(RequestType::DeleteWithBody.is_idempotent());
        assert!(!RequestType::Post.is_idempotent());
        assert!(!RequestType::Patch.is_idempotent());
    }

    #[test]
    fn test_empty_body() {
        let s = reqwest::StatusCode::INTERNAL_SERVER_ERROR;
        assert_eq!(extract_error_message("", s), "HTTP 500");
        assert_eq!(extract_error_message("   ", s), "HTTP 500");
    }

    #[test]
    fn test_json_nested_error_message() {
        let s = reqwest::StatusCode::BAD_REQUEST;
        let body = r#"{"error":{"message":"invalid request"}}"#;
        assert_eq!(extract_error_message(body, s), "HTTP 400: invalid request");
    }

    #[test]
    fn test_json_flat_error_string() {
        let s = reqwest::StatusCode::UNAUTHORIZED;
        let body = r#"{"error":"unauthorized"}"#;
        assert_eq!(extract_error_message(body, s), "HTTP 401: unauthorized");
    }

    #[test]
    fn test_json_message_field() {
        let s = reqwest::StatusCode::NOT_FOUND;
        let body = r#"{"message":"not found"}"#;
        assert_eq!(extract_error_message(body, s), "HTTP 404: not found");
    }

    #[test]
    fn test_invalid_json_returns_status() {
        let s = reqwest::StatusCode::INTERNAL_SERVER_ERROR;
        assert_eq!(extract_error_message("not valid json {", s), "HTTP 500");
    }

    #[test]
    fn test_plain_text_body_returns_status() {
        let s = reqwest::StatusCode::SERVICE_UNAVAILABLE;
        assert_eq!(extract_error_message("Service unavailable", s), "HTTP 503");
    }

    #[test]
    fn test_json_no_known_fields_returns_status() {
        let s = reqwest::StatusCode::BAD_REQUEST;
        let body = r#"{"data":null,"code":42}"#;
        assert_eq!(extract_error_message(body, s), "HTTP 400");
    }

    #[test]
    fn test_long_json_message_truncated() {
        let s = reqwest::StatusCode::BAD_REQUEST;
        let long_msg = "x".repeat(400);
        let body = format!(r#"{{"message":"{long_msg}"}}"#);
        let result = extract_error_message(&body, s);
        // "HTTP 400: " (10 chars) + 300 x's + "…" (1 char) = 312 chars
        assert!(result.starts_with("HTTP 400: "));
        assert!(result.ends_with('…'));
        // The message portion should be truncated to 300 chars + ellipsis
        let msg_part = result.strip_prefix("HTTP 400: ").unwrap();
        assert_eq!(msg_part.chars().filter(|&c| c == 'x').count(), 300);
    }
}
