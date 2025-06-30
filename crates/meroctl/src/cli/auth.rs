use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::Query;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use eyre::{bail, eyre, OptionExt, Result};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::timeout;
use url::Url;

use crate::cli::storage::{get_session_cache, JwtToken};
use crate::connection::ConnectionInfo;

#[derive(Debug, Deserialize)]
struct AuthCallback {
    access_token: Option<String>,
    refresh_token: Option<String>,
}

pub async fn authenticate(api_url: &Url) -> Result<JwtToken> {
    let temp_connection = ConnectionInfo::new(api_url.clone(), None, None);
    let auth_mode = temp_connection.detect_auth_mode().await?;

    if auth_mode == "none" {
        bail!("Server does not require authentication");
    }

    // Set up callback server
    let (callback_port, callback_rx) = start_callback_server().await?;
    println!("🌐 Started local callback server on port {}", callback_port);

    let auth_url = build_auth_url(api_url, callback_port)?;
    println!("🔗 Opening browser to: {}", auth_url);

    if let Err(e) = webbrowser::open(&auth_url.to_string()) {
        println!("⚠️  Failed to open browser automatically: {}", e);
        println!("📋 Please manually open this URL in your browser:");
        println!("   {}", auth_url);
    }

    println!("⏳ Waiting for authentication callback (timeout: 300s)...");

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

            Ok(JwtToken {
                access_token,
                refresh_token,
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
                <html>
                    <head><title>Authentication Complete</title></head>
                    <body>
                        <h1>🎉 Authentication Complete!</h1>
                        <p>You can now close this browser window and return to the terminal.</p>
                        <script>window.close();</script>
                    </body>
                </html>
                "#,
                )
            }
        }),
    );

    let _server_handle = tokio::spawn(async move {
        let result = axum::serve(listener, app).await;

        match result {
            Ok(_) => {}
            Err(e) => {
                eprintln!("Callback server error: {}", e);
                if let Ok(mut guard) = tx.lock() {
                    if let Some(sender) = guard.take() {
                        drop(sender.send(Err(format!("Server error: {}", e))));
                    }
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
pub async fn check_authentication(url: &Url, node_name: &str) -> Result<Option<JwtToken>> {
    let temp_connection = ConnectionInfo::new(url.clone(), None, None);
    let auth_mode = temp_connection.detect_auth_mode().await?;

    if auth_mode != "none" {
        println!(
            "🔐 {} requires authentication (mode: {})",
            node_name, auth_mode
        );
        println!("🚀 Starting authentication process...");

        match authenticate(url).await {
            Ok(tokens) => {
                println!("✅ Authentication successful for {}", node_name);
                Ok(Some(tokens))
            }
            Err(e) => {
                bail!("Authentication failed for {}: {}", node_name, e);
            }
        }
    } else {
        println!("ℹ️ {} does not require authentication", node_name);
        Ok(None)
    }
}

/// Helper function for session-based authentication with caching for external connections
/// Returns a ConnectionInfo with appropriate authentication tokens
pub async fn authenticate_with_session_cache(
    url: &Url,
    node_name: &str,
) -> Result<ConnectionInfo> {
    let temp_connection = ConnectionInfo::new(url.clone(), None, None);
    let auth_mode = temp_connection.detect_auth_mode().await?;

    if auth_mode != "none" {
        // Check if we have tokens in session cache for this URL
        let session_cache = get_session_cache();

        if let Some(cached_tokens) = session_cache.get_tokens(url) {
            // We have existing tokens for this URL in session cache
            println!("✅ Using cached authentication for {}", node_name);
            Ok(ConnectionInfo::new(url.clone(), Some(cached_tokens), None))
        } else {
            // Need to authenticate and store in session cache
            println!("🔐 {} requires authentication", node_name);
            match authenticate(url).await {
                Ok(jwt_tokens) => {
                    // Store in session cache for future use during this session
                    session_cache.store_tokens(url, &jwt_tokens);
                    println!("🔐 Tokens cached for session for {}", node_name);

                    Ok(ConnectionInfo::new(url.clone(), Some(jwt_tokens), None))
                }
                Err(e) => {
                    bail!("Authentication failed for {}: {}", node_name, e);
                }
            }
        }
    } else {
        // No authentication required
        Ok(ConnectionInfo::new(url.clone(), None, None))
    }
}
