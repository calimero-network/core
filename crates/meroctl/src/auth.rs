use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const AUTH_TIMEOUT_SECS: u64 = 120;

use camino::Utf8PathBuf;

use axum::extract::Query;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use calimero_client::{auth, get_session_cache, AuthMode, JwtToken};
use eyre::{bail, eyre, OptionExt, Result};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::timeout;
use url::Url;

use crate::connection::ConnectionInfo;
use crate::output::{InfoLine, Output, WarnLine};
use crate::storage::FileTokenStorage;

#[derive(Debug, Deserialize)]
struct AuthCallback {
    access_token: Option<String>,
    refresh_token: Option<String>,
}

pub async fn authenticate(api_url: &Url, output: Output) -> Result<JwtToken> {
    let temp_connection = ConnectionInfo::new(
        api_url.clone(),
        None,
        create_cli_authenticator(output),
        FileTokenStorage::new(),
    );
    let auth_mode = temp_connection.detect_auth_mode().await?;

    if auth_mode == AuthMode::None {
        bail!("Server does not require authentication");
    }

    // Set up callback server
    let (callback_port, callback_rx) = start_callback_server().await?;

    let auth_url = build_auth_url(api_url, callback_port)?;

    output.write(&InfoLine(
        "Opening browser for authentication — you have 2 minutes to complete sign-in.",
    ));

    if let Err(e) = webbrowser::open(&auth_url.to_string()) {
        let warning_msg = format!(
            "Failed to open browser: {}. Please manually open this URL: {}",
            e, auth_url
        );
        output.write(&WarnLine(&warning_msg));
    }

    let auth_result = timeout(Duration::from_secs(AUTH_TIMEOUT_SECS), callback_rx)
        .await
        .map_err(|_| eyre!("Authentication timed out — please try again"))?
        .map_err(|_| eyre!("Callback server error"))?;

    match auth_result {
        Ok(callback) => {
            let access_token = callback
                .access_token
                .ok_or_eyre("No access token received")?;
            let refresh_token = callback.refresh_token;

            Ok(if let Some(refresh) = refresh_token {
                JwtToken::with_refresh(access_token, refresh)
            } else {
                JwtToken::new(access_token)
            })
        }
        Err(e) => {
            bail!("Authentication failed: {}", e);
        }
    }
}

async fn start_callback_server() -> Result<(u16, oneshot::Receiver<Result<AuthCallback, String>>)> {
    let (tx, rx) = oneshot::channel();
    let tx = Arc::new(Mutex::new(Some(tx)));

    let (start_port, end_port) = (9080u16, 9090u16);

    let mut listener = None;
    let mut bound_port = 0;

    for port in start_port..=end_port {
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => {
                bound_port = port;
                listener = Some(l);
                break;
            }
            Err(_) => continue,
        }
    }

    let listener = listener.ok_or_else(|| {
        eyre!(
            "Failed to bind to any port in range {}-{}",
            start_port,
            end_port
        )
    })?;

    let app = Router::new().route(
        "/callback",
        get({
            let tx = Arc::clone(&tx);
            move |Query(params): Query<HashMap<String, String>>| async move {
                // Check if we have tokens as query parameters
                if params.contains_key("access_token") {
                    let callback = AuthCallback {
                        access_token: params.get("access_token").cloned(),
                        refresh_token: params.get("refresh_token").cloned(),
                    };

                    if let Ok(mut guard) = tx.lock() {
                        if let Some(sender) = guard.take() {
                            drop(sender.send(Ok(callback)));
                        }
                    }

                    return Html(
                        r##"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Authentication Complete</title>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
            background-color: #111111;
            color: #ffffff;
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
        }
        .card {
            background-color: #1c1c1c;
            border: 1px solid rgba(255, 255, 255, 0.1);
            border-radius: 0.75rem;
            padding: 3rem 2rem;
            max-width: 420px;
            width: 90%;
            text-align: center;
        }
        .icon {
            width: 56px;
            height: 56px;
            background-color: rgba(255, 122, 0, 0.15);
            border-radius: 50%;
            display: flex;
            align-items: center;
            justify-content: center;
            margin: 0 auto 1.5rem;
        }
        .icon svg { width: 28px; height: 28px; }
        h1 {
            font-size: 1.5rem;
            font-weight: 700;
            line-height: 2rem;
            margin-bottom: 0.75rem;
        }
        p {
            font-size: 0.875rem;
            color: rgba(255, 255, 255, 0.7);
            line-height: 1.5;
        }
        .accent { color: #ff7a00; }
    </style>
</head>
<body>
    <div class="card">
        <div class="icon">
            <svg viewBox="0 0 24 24" fill="none" stroke="#ff7a00" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                <polyline points="20 6 9 17 4 12"></polyline>
            </svg>
        </div>
        <h1>You're <span class="accent">authenticated</span></h1>
        <p>You can close this window and return to the terminal.</p>
    </div>
</body>
</html>
                "##,
                    );
                }

                // No query parameters - serve HTML page that extracts tokens from fragments
                Html(
                    r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Authenticating</title>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
            background-color: #111111;
            color: #ffffff;
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
        }
        .card {
            background-color: #1c1c1c;
            border: 1px solid rgba(255, 255, 255, 0.1);
            border-radius: 0.75rem;
            padding: 3rem 2rem;
            max-width: 420px;
            width: 90%;
            text-align: center;
        }
        .spinner {
            width: 40px;
            height: 40px;
            border: 3px solid rgba(255, 122, 0, 0.2);
            border-top-color: #ff7a00;
            border-radius: 50%;
            animation: spin 0.8s linear infinite;
            margin: 0 auto 1.5rem;
        }
        @keyframes spin {
            to { transform: rotate(360deg); }
        }
        h1 { font-size: 1.5rem; font-weight: 700; margin-bottom: 0.75rem; }
        p { font-size: 0.875rem; color: rgba(255, 255, 255, 0.7); line-height: 1.5; }
        .error { color: #ff4444; margin-top: 1rem; font-size: 0.875rem; display: none; }
    </style>
</head>
<body>
    <div class="card">
        <div class="spinner"></div>
        <h1>Authenticating</h1>
        <p>Completing sign-in, please wait...</p>
        <p class="error" id="err"></p>
    </div>
    <script>
        (function() {
            const hash = window.location.hash.substring(1);
            const params = new URLSearchParams(hash);
            const accessToken = params.get('access_token');
            const refreshToken = params.get('refresh_token');
            if (accessToken) {
                const q = new URLSearchParams();
                q.set('access_token', accessToken);
                if (refreshToken) q.set('refresh_token', refreshToken);
                window.location.href = window.location.origin + window.location.pathname + '?' + q.toString();
            } else {
                document.querySelector('.spinner').style.display = 'none';
                document.querySelector('h1').textContent = 'Authentication failed';
                document.querySelector('p').textContent = 'No tokens found in the URL.';
                document.getElementById('err').style.display = 'block';
                document.getElementById('err').textContent = 'Please close this window and try again.';
            }
        })();
    </script>
</body>
</html>
                "#,
                )
            }
        }),
    );

    let _server_handle = tokio::spawn(async move {
        let result = axum::serve(listener, app).await;

        if let Err(e) = result {
            if let Ok(mut guard) = tx.lock() {
                if let Some(sender) = guard.take() {
                    drop(sender.send(Err(format!("Server error: {}", e))));
                }
            }
        }
    });

    Ok((bound_port, rx))
}

fn build_auth_url(api_url: &Url, callback_port: u16) -> Result<Url> {
    let mut auth_url = api_url.clone();
    auth_url.set_path("/auth/login");
    let _ = auth_url
        .query_pairs_mut()
        .append_pair(
            "callback-url",
            &format!("http://127.0.0.1:{}/callback", callback_port),
        )
        .append_pair("app-url", api_url.as_str().trim_end_matches('/'))
        .append_pair("permissions", "admin");

    Ok(auth_url)
}

/// Helper function to authenticate against a URL if required
/// Returns Some(tokens) if authentication was needed and successful, None if no auth required
pub async fn check_authentication(
    url: &Url,
    node_name: &str,
    output: Output,
) -> Result<Option<JwtToken>> {
    let temp_connection = ConnectionInfo::new(
        url.clone(),
        None,
        create_cli_authenticator(output),
        FileTokenStorage::new(),
    );
    let auth_mode = temp_connection.detect_auth_mode().await?;

    if auth_mode != AuthMode::None {
        match authenticate(url, output).await {
            Ok(tokens) => Ok(Some(tokens)),
            Err(e) => {
                bail!("Authentication failed for {}: {}", node_name, e);
            }
        }
    } else {
        Ok(None)
    }
}

/// Helper function for session-based authentication with caching for external connections
/// Returns a ConnectionInfo with appropriate authentication tokens.
///
/// `local_node_path` should be `Some(path)` when the node is a local node found via the
/// filesystem (so it is persisted as `NodeConnection::Local`).  Pass `None` for remote/URL nodes.
pub async fn authenticate_with_session_cache(
    url: &Url,
    node_name: &str,
    local_node_path: Option<&Utf8PathBuf>,
    output: Output,
) -> Result<ConnectionInfo> {
    let temp_connection = ConnectionInfo::new(
        url.clone(),
        None,
        create_cli_authenticator(output),
        FileTokenStorage::new(),
    );
    let auth_mode = temp_connection.detect_auth_mode().await?;

    if auth_mode != AuthMode::None {
        // Check if we have tokens in session cache for this URL
        let session_cache = get_session_cache();

        if let Some(_cached_tokens) = session_cache.get_tokens(url.as_str()).await {
            // We have existing tokens for this URL in session cache
            Ok(ConnectionInfo::new(
                url.clone(),
                Some(node_name.to_owned()),
                create_cli_authenticator(output),
                FileTokenStorage::new(),
            ))
        } else {
            // Need to authenticate and store in session cache
            match authenticate(url, output).await {
                Ok(jwt_tokens) => {
                    // Store in session cache for future use during this session
                    session_cache.store_tokens(url.as_str(), &jwt_tokens).await;

                    // Persist the node in config so FileTokenStorage can use tokens across
                    // sessions. Reload config immediately before writing to reduce (but not
                    // eliminate) the TOCTOU window; only insert when the key is absent so an
                    // explicit `node add` entry is never overwritten.
                    persist_node_in_config(node_name, url, local_node_path, &jwt_tokens).await?;

                    Ok(ConnectionInfo::new(
                        url.clone(),
                        Some(node_name.to_owned()),
                        create_cli_authenticator(output),
                        FileTokenStorage::new(),
                    ))
                }
                Err(e) => {
                    bail!("Authentication failed for {}: {}", node_name, e);
                }
            }
        }
    } else {
        // No authentication required
        Ok(ConnectionInfo::new(
            url.clone(),
            None,
            create_cli_authenticator(output),
            FileTokenStorage::new(),
        ))
    }
}

/// Persist a node entry and its fresh tokens in the meroctl config file.
///
/// * If the node is not yet in config it is inserted as `Local` (when `local_node_path` is
///   `Some`) or `Remote` (otherwise).
/// * If the node already exists its tokens are **always updated** so that
///   `FileTokenStorage::load_tokens` finds the fresh credentials and does not trigger a
///   redundant browser-auth prompt on the next request.
///
/// Config is reloaded immediately before the write to reduce (but not eliminate) the
/// TOCTOU race window.
async fn persist_node_in_config(
    node_name: &str,
    url: &Url,
    local_node_path: Option<&Utf8PathBuf>,
    jwt_tokens: &JwtToken,
) -> Result<()> {
    // Reload config just before mutating to reduce (but not eliminate) the TOCTOU window.
    let mut config = crate::config::Config::load().await?;

    let stored_tokens = Some(crate::storage::JwtToken {
        access_token: jwt_tokens.access_token.clone(),
        refresh_token: jwt_tokens.refresh_token.clone(),
    });

    if let Some(conn) = config.nodes.get_mut(node_name) {
        // Node already registered — update tokens so FileTokenStorage sees fresh credentials.
        match conn {
            crate::config::NodeConnection::Local { jwt_tokens: t, .. }
            | crate::config::NodeConnection::Remote { jwt_tokens: t, .. } => {
                *t = stored_tokens;
            }
        }
    } else {
        // New node — register with the correct connection type.
        let conn = if let Some(path) = local_node_path {
            crate::config::NodeConnection::Local {
                path: path.clone(),
                jwt_tokens: stored_tokens,
            }
        } else {
            crate::config::NodeConnection::Remote {
                url: url.clone(),
                jwt_tokens: stored_tokens,
            }
        };
        config.nodes.insert(node_name.to_owned(), conn);
    }

    config.save().await?;
    Ok(())
}

/// Meroctl-specific implementation of ClientAuthenticator
///
/// This authenticator is designed to work with meroctl's Output type
/// and provides browser-based authentication flows for the CLI.
pub struct MeroctlAuthenticator {
    /// Output handler for meroctl
    output: Box<dyn calimero_client::MeroctlOutputHandler + Send + Sync>,
    /// Raw output kept for cloning (Output is Copy)
    raw_output: Output,
}

impl std::fmt::Debug for MeroctlAuthenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeroctlAuthenticator")
            .field("output", &"<dyn MeroctlOutputHandler>")
            .finish()
    }
}

impl Clone for MeroctlAuthenticator {
    fn clone(&self) -> Self {
        create_cli_authenticator(self.raw_output)
    }
}

impl MeroctlAuthenticator {
    pub fn new(
        output: Box<dyn calimero_client::MeroctlOutputHandler + Send + Sync>,
        raw_output: Output,
    ) -> Self {
        Self { output, raw_output }
    }
}

#[async_trait::async_trait]
impl calimero_client::ClientAuthenticator for MeroctlAuthenticator {
    async fn authenticate(&self, api_url: &Url) -> Result<JwtToken> {
        // Use the proper OAuth authentication flow
        self.output.display_message(
            "Opening browser for authentication — you have 2 minutes to complete sign-in.",
        );

        // Set up callback server
        let (callback_port, callback_rx) = start_callback_server().await?;

        let auth_url = build_auth_url(api_url, callback_port)?;

        // Open the OAuth URL in the browser
        if let Err(e) = self.output.open_browser(&auth_url) {
            self.output
                .display_error(&format!("Failed to open browser: {}", e));
            self.output
                .display_message(&format!("Please manually visit: {}", auth_url));
        }

        // Wait for the OAuth callback
        let auth_result = timeout(Duration::from_secs(AUTH_TIMEOUT_SECS), callback_rx)
            .await
            .map_err(|_| eyre!("Authentication timed out — please try again"))?
            .map_err(|_| eyre!("Callback server error"))?;

        match auth_result {
            Ok(callback) => {
                let access_token = callback
                    .access_token
                    .ok_or_eyre("No access token received")?;
                let refresh_token = callback.refresh_token;

                let token = if let Some(refresh) = refresh_token {
                    JwtToken::with_refresh(access_token, refresh)
                } else {
                    JwtToken::new(access_token)
                };

                self.output
                    .display_success("OAuth authentication successful!");
                Ok(token)
            }
            Err(e) => {
                self.output
                    .display_error(&format!("OAuth authentication failed: {}", e));
                Err(eyre!("OAuth authentication failed: {}", e))
            }
        }
    }

    async fn refresh_tokens(&self, _refresh_token: &str) -> Result<JwtToken> {
        // For now, we'll use a simple approach - ask user for new token
        self.output
            .display_message("Refreshing authentication tokens...");

        self.output
            .display_message("Please provide new access token:");
        let access_token = self.output.wait_for_input("Enter new access token: ")?;

        if access_token.is_empty() {
            return Err(eyre::eyre!("Access token cannot be empty"));
        }

        let token = JwtToken::new(access_token);
        self.output.display_success("Token refresh successful!");
        Ok(token)
    }

    async fn handle_auth_failure(&self, api_url: &Url) -> Result<JwtToken> {
        self.output
            .display_error("Authentication failed. Please try again.");

        // Try to open the authentication URL in the browser
        if let Err(e) = self.output.open_browser(api_url) {
            self.output
                .display_error(&format!("Failed to open browser: {}", e));
            self.output
                .display_message(&format!("Please manually visit: {}", api_url));
        }

        // Wait for user to complete authentication
        self.output
            .display_message("Please complete authentication in your browser, then press Enter.");
        let _unused = self.output.wait_for_input("Press Enter when done: ")?;

        // Try to authenticate again
        self.authenticate(api_url).await
    }

    async fn check_auth_required(&self, _api_url: &Url) -> Result<bool> {
        // For now, assume all APIs require authentication
        // In a real implementation, this would check the API health endpoint
        Ok(true)
    }

    fn get_auth_method(&self) -> &'static str {
        "Meroctl Browser-based OAuth"
    }

    fn supports_refresh(&self) -> bool {
        true
    }
}

/// Concrete implementation of MeroctlOutputHandler for meroctl's Output type
#[derive(Debug, Clone)]
pub struct MeroctlOutputWrapper {
    output: Output,
}

impl MeroctlOutputWrapper {
    pub fn new(output: Output) -> Self {
        Self { output }
    }
}

impl auth::MeroctlOutputHandler for MeroctlOutputWrapper {
    fn display_message(&self, message: &str) {
        self.output.write(&InfoLine(message));
    }

    fn display_error(&self, error: &str) {
        self.output.write(&WarnLine(error));
    }

    fn display_success(&self, message: &str) {
        self.output.write(&InfoLine(message));
    }

    fn open_browser(&self, url: &Url) -> Result<()> {
        webbrowser::open(url.as_str())?;
        Ok(())
    }

    fn wait_for_input(&self, prompt: &str) -> Result<String> {
        use std::io::{self, Write};

        print!("{}", prompt);
        io::stdout().flush()?;

        let mut input = String::new();
        drop(io::stdin().read_line(&mut input));

        Ok(input.trim().to_owned())
    }
}

/// Type alias for the authenticator from client
pub type CliAuthenticator = MeroctlAuthenticator;

/// Helper function to create a new CliAuthenticator
pub fn create_cli_authenticator(output: Output) -> CliAuthenticator {
    let wrapper = MeroctlOutputWrapper::new(output);
    MeroctlAuthenticator::new(Box::new(wrapper), output)
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use url::Url;

    use crate::config::{Config, NodeConnection};
    use crate::storage::JwtToken;

    fn make_tokens(access: &str) -> JwtToken {
        JwtToken {
            access_token: access.to_owned(),
            refresh_token: Some("refresh".to_owned()),
        }
    }

    /// Simulates the insert-or-update logic inside persist_node_in_config
    /// for a new Local node.
    #[test]
    fn persist_inserts_new_local_node() {
        let mut config = Config::default();
        let path = Utf8PathBuf::from("/home/user/.calimero");
        let tokens = make_tokens("access1");

        let stored = Some(crate::storage::JwtToken {
            access_token: tokens.access_token.clone(),
            refresh_token: tokens.refresh_token.clone(),
        });

        config.nodes.insert(
            "mynode".to_owned(),
            NodeConnection::Local {
                path: path.clone(),
                jwt_tokens: stored,
            },
        );

        assert_eq!(config.nodes.len(), 1);
        match config.nodes.get("mynode").unwrap() {
            NodeConnection::Local {
                path: p,
                jwt_tokens: t,
            } => {
                assert_eq!(p, &path);
                assert_eq!(t.as_ref().unwrap().access_token, "access1");
            }
            _ => panic!("expected Local"),
        }
    }

    /// Simulates the insert-or-update logic for a new Remote node.
    #[test]
    fn persist_inserts_new_remote_node() {
        let mut config = Config::default();
        let url: Url = "https://example.com".parse().unwrap();
        let tokens = make_tokens("access2");

        config.nodes.insert(
            "remote".to_owned(),
            NodeConnection::Remote {
                url: url.clone(),
                jwt_tokens: Some(crate::storage::JwtToken {
                    access_token: tokens.access_token.clone(),
                    refresh_token: tokens.refresh_token.clone(),
                }),
            },
        );

        match config.nodes.get("remote").unwrap() {
            NodeConnection::Remote {
                url: u,
                jwt_tokens: t,
            } => {
                assert_eq!(u, &url);
                assert_eq!(t.as_ref().unwrap().access_token, "access2");
            }
            _ => panic!("expected Remote"),
        }
    }

    /// Simulates token refresh: existing node entry gets tokens updated in-place.
    #[test]
    fn persist_updates_tokens_for_existing_node() {
        let mut config = Config::default();
        let path = Utf8PathBuf::from("/home/user/.calimero");

        // Insert initial entry
        config.nodes.insert(
            "mynode".to_owned(),
            NodeConnection::Local {
                path: path.clone(),
                jwt_tokens: Some(crate::storage::JwtToken {
                    access_token: "old_access".to_owned(),
                    refresh_token: Some("old_refresh".to_owned()),
                }),
            },
        );

        // Simulate update (as persist_node_in_config does when node exists)
        let new_stored = Some(crate::storage::JwtToken {
            access_token: "new_access".to_owned(),
            refresh_token: Some("new_refresh".to_owned()),
        });
        if let Some(conn) = config.nodes.get_mut("mynode") {
            match conn {
                NodeConnection::Local { jwt_tokens: t, .. }
                | NodeConnection::Remote { jwt_tokens: t, .. } => {
                    *t = new_stored;
                }
            }
        }

        // Verify tokens were updated; path unchanged
        match config.nodes.get("mynode").unwrap() {
            NodeConnection::Local {
                path: p,
                jwt_tokens: t,
            } => {
                assert_eq!(p, &path);
                assert_eq!(t.as_ref().unwrap().access_token, "new_access");
                assert_eq!(
                    t.as_ref().unwrap().refresh_token.as_deref(),
                    Some("new_refresh")
                );
            }
            _ => panic!("expected Local"),
        }
    }

    /// Config TOML round-trip: serialize then deserialize produces identical structure.
    #[test]
    fn config_toml_roundtrip() {
        let mut config = Config::default();
        config.nodes.insert(
            "node1".to_owned(),
            NodeConnection::Local {
                path: Utf8PathBuf::from("/tmp/calimero"),
                jwt_tokens: Some(crate::storage::JwtToken {
                    access_token: "tok".to_owned(),
                    refresh_token: None,
                }),
            },
        );
        config.active_node = Some("node1".to_owned());

        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        let restored: Config = toml::from_str(&toml_str).expect("deserialize");

        assert_eq!(restored.active_node.as_deref(), Some("node1"));
        assert!(restored.nodes.contains_key("node1"));
        match restored.nodes.get("node1").unwrap() {
            NodeConnection::Local { jwt_tokens: t, .. } => {
                assert_eq!(t.as_ref().unwrap().access_token, "tok");
            }
            _ => panic!("expected Local"),
        }
    }
}
