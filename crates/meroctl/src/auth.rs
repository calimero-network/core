use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::Query;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use calimero_client::storage::{get_session_cache, JwtToken};
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
        None,
        create_cli_authenticator(output),
        FileTokenStorage::new(),
    );
    let auth_mode = temp_connection.detect_auth_mode().await?;

    if auth_mode == "none" {
        bail!("Server does not require authentication");
    }

    // Set up callback server
    let (callback_port, callback_rx) = start_callback_server().await?;

    let auth_url = build_auth_url(api_url, callback_port)?;

    let info_msg = format!("Opening browser to start authentication");
    output.write(&InfoLine(&info_msg));

    if let Err(e) = webbrowser::open(&auth_url.to_string()) {
        let warning_msg = format!(
            "Failed to open browser: {}. Please manually open this URL: {}",
            e, auth_url
        );
        output.write(&WarnLine(&warning_msg));
    }

    let auth_result = timeout(Duration::from_secs(300), callback_rx)
        .await
        .map_err(|_| eyre!("Authentication timed out after 300 seconds"))?
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
                let callback = AuthCallback {
                    access_token: params.get("access_token").cloned(),
                    refresh_token: params.get("refresh_token").cloned(),
                };

                if let Ok(mut guard) = tx.lock() {
                    if let Some(sender) = guard.take() {
                        drop(sender.send(Ok(callback)));
                    }
                }

                Html(
                    r#"
                <!DOCTYPE html>
                <html lang="en">
                    <head>
                        <meta charset="UTF-8">
                        <meta name="viewport" content="width=device-width, initial-scale=1.0">
                        <title>Authentication Complete</title>
                        <style>
                            * {
                                margin: 0;
                                padding: 0;
                                box-sizing: border-box;
                            }
                            
                            body {
                                font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
                                background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
                                min-height: 100vh;
                                display: flex;
                                align-items: center;
                                justify-content: center;
                                color: white;
                            }
                            
                            .container {
                                text-align: center;
                                background: rgba(255, 255, 255, 0.1);
                                backdrop-filter: blur(10px);
                                border-radius: 20px;
                                padding: 3rem 2rem;
                                border: 1px solid rgba(255, 255, 255, 0.2);
                                box-shadow: 0 8px 32px rgba(0, 0, 0, 0.1);
                                max-width: 400px;
                                width: 90%;
                            }
                            
                            .emoji {
                                font-size: 4rem;
                                margin-bottom: 1rem;
                                animation: bounce 2s infinite;
                            }
                            
                            @keyframes bounce {
                                0%, 20%, 50%, 80%, 100% {
                                    transform: translateY(0);
                                }
                                40% {
                                    transform: translateY(-10px);
                                }
                                60% {
                                    transform: translateY(-5px);
                                }
                            }
                            
                            h1 {
                                font-size: 2rem;
                                margin-bottom: 1rem;
                                font-weight: 600;
                            }
                            
                            p {
                                font-size: 1.1rem;
                                margin-bottom: 1.5rem;
                                opacity: 0.9;
                                line-height: 1.5;
                            }
                            
                            .message {
                                font-size: 1.1rem;
                                opacity: 1;
                                font-weight: 600;
                            }
                        </style>
                    </head>
                    <body>
                        <div class="container">
                            <div class="emoji">🎉</div>
                            <h1>Authentication Complete!</h1>
                            <p>You can now close this browser window and return to the terminal.</p>
                            <div class="message">You can now close this page</div>
                        </div>
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
        None,
        create_cli_authenticator(output),
        FileTokenStorage::new(),
    );
    let auth_mode = temp_connection.detect_auth_mode().await?;

    if auth_mode != "none" {
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
/// Returns a ConnectionInfo with appropriate authentication tokens
pub async fn authenticate_with_session_cache(
    url: &Url,
    node_name: &str,
    output: Output,
) -> Result<ConnectionInfo> {
    let temp_connection = ConnectionInfo::new(
        url.clone(),
        None,
        None,
        create_cli_authenticator(output),
        FileTokenStorage::new(),
    );
    let auth_mode = temp_connection.detect_auth_mode().await?;

    if auth_mode != "none" {
        // Check if we have tokens in session cache for this URL
        let session_cache = get_session_cache();

        if let Some(cached_tokens) = session_cache.get_tokens(url.as_str()).await {
            // We have existing tokens for this URL in session cache
            Ok(ConnectionInfo::new(
                url.clone(),
                Some(cached_tokens),
                None,
                create_cli_authenticator(output),
                FileTokenStorage::new(),
            ))
        } else {
            // Need to authenticate and store in session cache
            match authenticate(url, output).await {
                Ok(jwt_tokens) => {
                    // Store in session cache for future use during this session
                    session_cache.store_tokens(url.as_str(), &jwt_tokens).await;

                    Ok(ConnectionInfo::new(
                        url.clone(),
                        Some(jwt_tokens),
                        None,
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
            None,
            create_cli_authenticator(output),
            FileTokenStorage::new(),
        ))
    }
}

/// Meroctl-specific implementation of ClientAuthenticator
///
/// This authenticator is designed to work with meroctl's Output type
/// and provides browser-based authentication flows for the CLI.
pub struct MeroctlAuthenticator {
    /// Output handler for meroctl
    output: Box<dyn calimero_client::auth::MeroctlOutputHandler + Send + Sync>,
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
        // Since we can't clone the trait object, we'll create a new one
        // This is a limitation, but it's acceptable for now
        Self {
            output: Box::new(NoOpOutputHandler),
        }
    }
}

impl MeroctlAuthenticator {
    /// Create a new meroctl authenticator
    pub fn new(output: Box<dyn calimero_client::auth::MeroctlOutputHandler + Send + Sync>) -> Self {
        Self { output }
    }
}

#[async_trait::async_trait]
impl calimero_client::ClientAuthenticator for MeroctlAuthenticator {
    async fn authenticate(
        &self,
        api_url: &url::Url,
    ) -> eyre::Result<calimero_client::storage::JwtToken> {
        // For now, implement a simple authentication flow directly
        self.output.display_message("Starting authentication...");

        // Try to open the authentication URL in the browser
        if let Err(e) = self.output.open_browser(api_url) {
            self.output
                .display_error(&format!("Failed to open browser: {}", e));
            self.output
                .display_message(&format!("Please manually visit: {}", api_url));
        }

        // Wait for user to complete authentication
        self.output.display_message(
            "Please complete authentication in your browser, then provide the access token.",
        );
        let access_token = self.output.wait_for_input("Enter access token: ")?;

        if access_token.is_empty() {
            return Err(eyre::eyre!("Access token cannot be empty"));
        }

        // Ask for refresh token if available
        self.output
            .display_message("Do you have a refresh token? (y/n)");
        let has_refresh = self.output.wait_for_input("y/n: ")?.to_lowercase() == "y";

        let token = if has_refresh {
            let refresh_token = self.output.wait_for_input("Enter refresh token: ")?;
            calimero_client::storage::JwtToken::with_refresh(access_token, refresh_token)
        } else {
            calimero_client::storage::JwtToken::new(access_token)
        };

        self.output.display_success("Authentication successful!");
        Ok(token)
    }

    async fn refresh_tokens(
        &self,
        refresh_token: &str,
    ) -> eyre::Result<calimero_client::storage::JwtToken> {
        // For now, we'll use a simple approach - ask user for new token
        self.output
            .display_message("Refreshing authentication tokens...");

        self.output
            .display_message("Please provide new access token:");
        let access_token = self.output.wait_for_input("Enter new access token: ")?;

        if access_token.is_empty() {
            return Err(eyre::eyre!("Access token cannot be empty"));
        }

        let token = calimero_client::storage::JwtToken::new(access_token);
        self.output.display_success("Token refresh successful!");
        Ok(token)
    }

    async fn handle_auth_failure(
        &self,
        api_url: &url::Url,
    ) -> eyre::Result<calimero_client::storage::JwtToken> {
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
        self.output.wait_for_input("Press Enter when done: ")?;

        // Try to authenticate again
        self.authenticate(api_url).await
    }

    async fn check_auth_required(&self, _api_url: &url::Url) -> eyre::Result<bool> {
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

/// No-op output handler for cloning
struct NoOpOutputHandler;

impl calimero_client::auth::MeroctlOutputHandler for NoOpOutputHandler {
    fn display_message(&self, _message: &str) {}
    fn display_error(&self, _error: &str) {}
    fn display_success(&self, _message: &str) {}
    fn open_browser(&self, _url: &url::Url) -> eyre::Result<()> {
        Ok(())
    }
    fn wait_for_input(&self, _prompt: &str) -> eyre::Result<String> {
        Ok(String::new())
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

impl calimero_client::auth::MeroctlOutputHandler for MeroctlOutputWrapper {
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

        Ok(input.trim().to_string())
    }
}

/// Type alias for the authenticator from calimero-client
pub type CliAuthenticator = MeroctlAuthenticator;

/// Helper function to create a new CliAuthenticator
pub fn create_cli_authenticator(output: Output) -> CliAuthenticator {
    let wrapper = MeroctlOutputWrapper::new(output);
    MeroctlAuthenticator::new(Box::new(wrapper))
}
