use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::Query;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use clap::{Parser, Subcommand};
use eyre::{bail, eyre, OptionExt, Result};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::timeout;
use url::Url;

use crate::cli::storage::{get_storage, JwtToken, ProfileConfig};
use crate::cli::Environment;
use crate::connection::ConnectionInfo;

#[derive(Debug, Parser)]
pub struct AuthCommand {
    #[command(subcommand)]
    pub action: AuthAction,
}

#[derive(Debug, Subcommand)]
pub enum AuthAction {
    /// Clear all cached authentication tokens from keychain
    Clear(ClearCommand),
}

#[derive(Debug, Parser)]
pub struct ClearCommand {
    #[arg(long, short)]
    pub force: bool,
}

#[derive(Debug, Deserialize)]
struct AuthCallback {
    access_token: Option<String>,
    refresh_token: Option<String>,
}

impl AuthCommand {
    pub async fn run(&self, _environment: &Environment) -> Result<()> {
        match &self.action {
            AuthAction::Clear(cmd) => cmd.run().await,
        }
    }
}

impl ClearCommand {
    pub async fn run(&self) -> Result<()> {
        if !self.force {
            print!("Are you sure you want to clear all cached tokens? (y/N): ");
            io::stdout().flush()?;

            let mut input = String::new();
            let _ = io::stdin().read_line(&mut input)?;

            let input = input.trim().to_lowercase();
            if input != "y" && input != "yes" {
                println!("Cancelled");
                return Ok(());
            }
        }

        let storage = get_storage();
        storage.clear_all().await?;

        println!("✅ Successfully cleared all cached authentication tokens");

        Ok(())
    }
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
        .append_pair("callback-url", &format!("http://127.0.0.1:{}/callback", callback_port))
        .append_pair("app-url", api_url.as_str().trim_end_matches('/'));

    Ok(auth_url)
}

/// Helper function to authenticate against a URL if required
/// Returns Some(tokens) if authentication was needed and successful, None if no auth required
pub async fn check_authentication(url: &Url, node_name: &str) -> Result<Option<JwtToken>> {
    let temp_connection = ConnectionInfo::new(url.clone(), None, None);
    let auth_mode = temp_connection.detect_auth_mode().await?;
    
    if auth_mode != "none" {
        println!("🔐 {} requires authentication (mode: {})", node_name, auth_mode);
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

/// Helper function for keychain-based authentication with caching
/// Returns a ConnectionInfo with appropriate authentication tokens
pub async fn authenticate_with_keychain_cache(url: &Url, keychain_key: &str, node_name: &str) -> Result<ConnectionInfo> {    
    let temp_connection = ConnectionInfo::new(url.clone(), None, None);
    let auth_mode = temp_connection.detect_auth_mode().await?;
    
    if auth_mode != "none" {
        // Check if we have tokens in keychain for this URL
        let storage = get_storage();
        
        match storage.load_profile(keychain_key).await? {
            Some(profile_config) if profile_config.node_url == *url && profile_config.token.is_some() => {
                // We have existing tokens for this URL
                println!("✅ Using cached authentication for {}", node_name);
                Ok(ConnectionInfo::new(url.clone(), profile_config.token, None))
            }
            _ => {
                // Need to authenticate and store in keychain
                println!("🔐 {} requires authentication", node_name);
                match authenticate(url).await {
                    Ok(jwt_tokens) => {
                        // Store in keychain for future use
                        let profile_config = ProfileConfig {
                            auth_profile: keychain_key.to_string(),
                            node_url: url.clone(),
                            token: Some(jwt_tokens.clone()),
                        };
                        storage.store_profile(keychain_key, &profile_config).await?;
                        println!("🔐 Tokens cached in keychain for {}", node_name);
                        
                        Ok(ConnectionInfo::new(url.clone(), Some(jwt_tokens), None))
                    }
                    Err(e) => {
                        bail!("Authentication failed for {}: {}", node_name, e);
                    }
                }
            }
        }
    } else {
        // No authentication required
        Ok(ConnectionInfo::new(url.clone(), None, None))
    }
}
