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
use crate::output::{InfoLine, Report, WarnLine};

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

    let auth_url = build_auth_url(api_url, callback_port)?;

    let info_msg = format!("Trying to open browser to start authentication");
    InfoLine(&info_msg).report();

    if let Err(e) = webbrowser::open(&auth_url.to_string()) {
        let warning_msg = format!(
            "Failed to open browser: {}. Please manually open this URL: {}",
            e, auth_url
        );
        WarnLine(&warning_msg).report();
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
                            
                            .countdown {
                                font-size: 1rem;
                                opacity: 0.8;
                                font-weight: 500;
                            }
                            
                            .progress-bar {
                                width: 100%;
                                height: 4px;
                                background: rgba(255, 255, 255, 0.2);
                                border-radius: 2px;
                                overflow: hidden;
                                margin-top: 1rem;
                            }
                            
                            .progress-fill {
                                height: 100%;
                                background: linear-gradient(90deg, #4CAF50, #45a049);
                                border-radius: 2px;
                                animation: countdown 5s linear forwards;
                            }
                            
                            @keyframes countdown {
                                from {
                                    width: 100%;
                                }
                                to {
                                    width: 0%;
                                }
                            }
                        </style>
                    </head>
                    <body>
                        <div class="container">
                            <div class="emoji">🎉</div>
                            <h1>Authentication Complete!</h1>
                            <p>You can now close this browser window and return to the terminal.</p>
                            <div class="countdown">This window will close automatically in <span id="timer">5</span> seconds</div>
                            <div class="progress-bar">
                                <div class="progress-fill"></div>
                            </div>
                        </div>
                        
                        <script>
                            let countdown = 5;
                            const timerElement = document.getElementById('timer');
                            const countdownElement = document.querySelector('.countdown');
                            
                            const interval = setInterval(() => {
                                countdown--;
                                timerElement.textContent = countdown;
                                
                                if (countdown <= 0) {
                                    clearInterval(interval);
                                    
                                    // Try to close the window
                                    const closed = window.close();
                                    
                                    // If window.close() didn't work, show a message
                                    setTimeout(() => {
                                        countdownElement.innerHTML = 'You can safely close this window now';
                                        countdownElement.style.fontSize = '1.1rem';
                                        countdownElement.style.opacity = '1';
                                        countdownElement.style.fontWeight = '600';
                                    }, 100);
                                }
                            }, 1000);
                            
                            // Fallback: try to close after delay and show message if it fails
                            setTimeout(() => {
                                window.close();
                                setTimeout(() => {
                                    countdownElement.innerHTML = 'You can safely close this window now';
                                    countdownElement.style.fontSize = '1.1rem';
                                    countdownElement.style.opacity = '1';
                                    countdownElement.style.fontWeight = '600';
                                }, 100);
                            }, 5000);
                            
                            // Add keyboard listener for easy closing
                            document.addEventListener('keydown', (e) => {
                                if (e.key === 'Escape' || e.key === 'Enter' || e.key === ' ') {
                                    window.close();
                                }
                            });
                            
                            // Add click listener to close on click
                            document.addEventListener('click', () => {
                                window.close();
                            });
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
pub async fn check_authentication(url: &Url, node_name: &str) -> Result<Option<JwtToken>> {
    let temp_connection = ConnectionInfo::new(url.clone(), None, None);
    let auth_mode = temp_connection.detect_auth_mode().await?;

    if auth_mode != "none" {
        match authenticate(url).await {
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
pub async fn authenticate_with_session_cache(url: &Url, node_name: &str) -> Result<ConnectionInfo> {
    let temp_connection = ConnectionInfo::new(url.clone(), None, None);
    let auth_mode = temp_connection.detect_auth_mode().await?;

    if auth_mode != "none" {
        // Check if we have tokens in session cache for this URL
        let session_cache = get_session_cache();

        if let Some(cached_tokens) = session_cache.get_tokens(url) {
            // We have existing tokens for this URL in session cache
            Ok(ConnectionInfo::new(url.clone(), Some(cached_tokens), None))
        } else {
            // Need to authenticate and store in session cache
            match authenticate(url).await {
                Ok(jwt_tokens) => {
                    // Store in session cache for future use during this session
                    session_cache.store_tokens(url, &jwt_tokens);

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
