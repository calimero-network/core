use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::Duration;

use eyre::{eyre, Result};
use hyper::{Body, Request, Response, Server};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tokio::time::timeout;
use url::Url;

use super::storage::{SecureStorage, StorageFactory, TokenStorage};
use super::tokens::AuthTokens;

/// Response from the auth server's token endpoint
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64, // seconds
    permissions: Option<Vec<String>>,
}

/// Main authentication manager
#[derive(Clone)]
pub struct AuthManager {
    profile: String,
    api_url: Url,
    storage: Arc<dyn SecureStorage>,
    client: Client,
}

impl AuthManager {
    /// Create a new auth manager
    pub async fn new(profile: String, api_url: Url) -> Result<Self> {
        let storage = StorageFactory::create_storage(TokenStorage::Auto).await?;
        
        Ok(Self {
            profile,
            api_url,
            storage: Arc::from(storage),
            client: Client::new(),
        })
    }

    /// Get a valid access token, refreshing if necessary
    pub async fn get_valid_token(&self) -> Result<Option<String>> {
        // Check environment variable first
        if let Ok(token) = std::env::var("MEROCTL_TOKEN") {
            if !token.is_empty() {
                return Ok(Some(token));
            }
        }

        // Try stored tokens
        if let Some(tokens) = self.storage.get_tokens(&self.profile).await? {
            // If token is not expired, return it
            if !tokens.is_expired() {
                return Ok(Some(tokens.access_token.clone()));
            }

            // Try to refresh if token is expired
            if let Ok(new_tokens) = self.refresh_with_token(&tokens.refresh_token).await {
                self.storage.store_tokens(&self.profile, &new_tokens).await?;
                return Ok(Some(new_tokens.access_token.clone()));
            }
        }

        Ok(None)
    }

    /// Get stored tokens for a profile (for status display)
    pub async fn get_stored_tokens(&self, profile: &str) -> Result<Option<AuthTokens>> {
        self.storage.get_tokens(profile).await
    }

    /// List all available profiles
    pub async fn list_all_profiles(&self) -> Result<Vec<String>> {
        self.storage.list_profiles().await
    }

    /// Perform browser-based authentication
    pub async fn browser_auth(&self, permissions: &[String]) -> Result<()> {
        // 1. Start local callback server
        let callback_port = Self::find_free_port()?;
        let callback_url = format!("http://localhost:{}/callback", callback_port);
        
        // 2. Build auth URL
        let mut auth_url = self.api_url.clone();
        auth_url.set_path("/auth/login");
        
        let mut query_pairs = vec![
            ("client_id", "meroctl"),
            ("redirect_uri", &callback_url),
            ("profile", &self.profile),
        ];
        
        // Add permissions if specified
        let permissions_str = permissions.join(",");
        if !permissions_str.is_empty() {
            query_pairs.push(("permissions", &permissions_str));
        }
        
        let query_string = query_pairs
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        
        auth_url.set_query(Some(&query_string));
        
        // 3. Start callback server and open browser
        let (tx, rx) = oneshot::channel();
        let server_handle = tokio::spawn(Self::run_callback_server(callback_port, tx));
        
        // Open browser
        println!("Opening browser for authentication...");
        println!("Visit: {}", auth_url);
        
        if let Err(e) = opener::open(auth_url.as_str()) {
            eprintln!("Failed to open browser automatically: {}", e);
            println!("Please visit the URL above to complete authentication");
        }
        
        // 4. Wait for callback with timeout
        let callback_result = timeout(Duration::from_secs(300), rx).await?; // 5 minute timeout
        
        // Cancel the server
        server_handle.abort();
        
        match callback_result {
            Ok(Ok(mut tokens)) => {
                // Update the tokens with correct profile and URL
                tokens.profile = self.profile.clone();
                tokens.node_url = self.api_url.clone();
                
                // Store the tokens
                self.storage.store_tokens(&self.profile, &tokens).await?;
                Ok(())
            }
            Ok(Err(e)) => Err(eyre!("Authentication failed: {}", e)),
            Err(_) => Err(eyre!("Authentication timed out")),
        }
    }

    /// Refresh tokens using the refresh token
    pub async fn refresh_tokens(&self) -> Result<bool> {
        if let Some(tokens) = self.storage.get_tokens(&self.profile).await? {
            if let Ok(new_tokens) = self.refresh_with_token(&tokens.refresh_token).await {
                self.storage.store_tokens(&self.profile, &new_tokens).await?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Logout (clear tokens)
    pub async fn logout(&self, profile: &str) -> Result<()> {
        self.storage.delete_tokens(profile).await
    }

    /// Detect authentication mode by checking the /identity endpoint
    pub async fn detect_auth_mode(&self) -> Result<String> {
        let mut url = self.api_url.clone();
        url.set_path("/identity");
        
        match self.client.get(url).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    #[derive(Deserialize)]
                    struct IdentityResponse {
                        authentication_mode: Option<String>,
                    }
                    
                    if let Ok(identity) = response.json::<IdentityResponse>().await {
                        return Ok(identity.authentication_mode.unwrap_or_else(|| "none".to_string()));
                    }
                }
                
                // Fallback: try a protected endpoint
                self.test_protected_endpoint().await
            }
            Err(_) => {
                // Fallback: try a protected endpoint
                self.test_protected_endpoint().await
            }
        }
    }

    async fn test_protected_endpoint(&self) -> Result<String> {
        let mut url = self.api_url.clone();
        url.set_path("/api/contexts"); // Try a typical protected endpoint
        
        match self.client.get(url).send().await {
            Ok(response) => {
                if response.status() == 401 {
                    Ok("forward".to_string())
                } else {
                    Ok("none".to_string())
                }
            }
            Err(_) => Ok("unknown".to_string()),
        }
    }

    async fn refresh_with_token(&self, refresh_token: &str) -> Result<AuthTokens> {
        let mut url = self.api_url.clone();
        url.set_path("/auth/refresh");
        
        #[derive(Serialize)]
        struct RefreshRequest {
            refresh_token: String,
        }
        
        let request = RefreshRequest {
            refresh_token: refresh_token.to_string(),
        };
        
        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await?;
            
        if !response.status().is_success() {
            return Err(eyre!("Failed to refresh token: {}", response.status()));
        }
        
        let token_response: TokenResponse = response.json().await?;
        
        let expires_at = chrono::Utc::now() + chrono::Duration::seconds(token_response.expires_in as i64);
        
        Ok(AuthTokens::new(
            self.profile.clone(),
            self.api_url.clone(),
            token_response.access_token,
            token_response.refresh_token,
            expires_at,
            token_response.permissions.unwrap_or_default(),
        ))
    }

    fn find_free_port() -> Result<u16> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        Ok(port)
    }

    async fn run_callback_server(
        port: u16,
        tx: oneshot::Sender<Result<AuthTokens>>,
    ) {
        use hyper::service::{make_service_fn, service_fn};

        let tx = Arc::new(tokio::sync::Mutex::new(Some(tx)));

        let make_svc = make_service_fn(move |_conn| {
            let tx = tx.clone();
            async move {
                Ok::<_, Infallible>(service_fn(move |req| {
                    let tx = tx.clone();
                    async move {
                        Self::handle_callback(req, tx).await
                    }
                }))
            }
        });

        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let server = Server::bind(&addr).serve(make_svc);

        if let Err(e) = server.await {
            eprintln!("Callback server error: {}", e);
        }
    }

    async fn handle_callback(
        req: Request<Body>,
        tx: Arc<tokio::sync::Mutex<Option<oneshot::Sender<Result<AuthTokens>>>>>,
    ) -> Result<Response<Body>, Infallible> {
        use hyper::StatusCode;

        let uri = req.uri();
        let path = uri.path();

        if path == "/callback" {
            if let Some(query) = uri.query() {
                let params: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
                    .into_owned()
                    .collect();

                if let Some(access_token) = params.get("access_token") {
                    let refresh_token = params.get("refresh_token").cloned().unwrap_or_default();
                    let expires_in: u64 = params
                        .get("expires_in")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(3600);
                    
                    let permissions: Vec<String> = params
                        .get("permissions")
                        .map(|p| p.split(',').map(|s| s.trim().to_string()).collect())
                        .unwrap_or_default();

                    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64);

                    let tokens = AuthTokens::new(
                        "default".to_string(), // Will be updated by the caller
                        "http://localhost".parse().unwrap(), // Will be updated by the caller
                        access_token.clone(),
                        refresh_token,
                        expires_at,
                        permissions,
                    );

                    // Send the tokens
                    if let Some(sender) = tx.lock().await.take() {
                        let _ = sender.send(Ok(tokens));
                    }

                    let response_body = r#"
                        <html>
                            <head><title>Authentication Successful</title></head>
                            <body style="font-family: Arial, sans-serif; text-align: center; margin-top: 50px;">
                                <h1>✅ Authentication Successful</h1>
                                <p>You can now close this window and return to the terminal.</p>
                                <script>setTimeout(() => window.close(), 2000);</script>
                            </body>
                        </html>
                    "#;

                    return Ok(Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "text/html")
                        .body(Body::from(response_body))
                        .unwrap());
                }

                if let Some(error) = params.get("error") {
                    if let Some(sender) = tx.lock().await.take() {
                        let _ = sender.send(Err(eyre!("Authentication error: {}", error)));
                    }
                }
            }
        }

        // Default response
        Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("Not found"))
            .unwrap())
    }
} 