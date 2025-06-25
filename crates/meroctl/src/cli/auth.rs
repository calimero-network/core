use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::io::{self, Write};

use axum::extract::Query;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use clap::{Parser, Subcommand};
use eyre::{bail, eyre, OptionExt, Result as EyreResult};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::timeout;
use url::Url;

use crate::cli::storage::{create_storage, JwtToken, ProfileConfig};
use crate::cli::Environment;
use crate::connection::ConnectionInfo;

#[derive(Debug, Parser)]
pub struct AuthCommand {
    #[command(subcommand)]
    pub action: AuthAction,
}

#[derive(Debug, Subcommand)]
pub enum AuthAction {
    /// Authenticate with the server using browser-based login
    Login(LoginCommand),
    /// Remove stored authentication tokens
    Logout(LogoutCommand),
    /// Show authentication info for a profile
    Info(InfoCommand),
    /// Set the active profile to use for all commands
    SetActive(SetActiveCommand),
    /// List all available authentication profiles
    List(ListCommand),
    /// Clear all authentication profiles and tokens
    Clear(ClearCommand),
}

#[derive(Debug, Parser)]
pub struct LoginCommand {
    /// API endpoint URL to authenticate with
    #[arg(long, value_name = "URL")]
    pub api: Url,
    
    /// Profile name to store authentication tokens for
    #[arg(long, default_value = "default")]
    pub profile: String,
    
    /// Permissions to request during authentication (comma-separated)
    #[arg(long)]
    pub permissions: Option<String>,
    
    /// Port range to use for local callback server (e.g., 8080-8090)
    #[arg(long, default_value = "9080-9090")]
    pub port_range: String,

    /// Timeout for the authentication flow in seconds
    #[arg(long, default_value = "300")]
    pub timeout: u64,
}

#[derive(Debug, Parser)]
pub struct LogoutCommand {
    /// Profile name to remove authentication tokens for
    #[arg(long, default_value = "default")]
    pub profile: String,
}

#[derive(Debug, Parser)]
pub struct InfoCommand {
    /// Profile name to check authentication info for
    #[arg(long)]
    pub profile: String,
}

#[derive(Debug, Parser)]
pub struct SetActiveCommand {
    /// Profile name to set as active
    #[arg(long)]
    pub profile: String,
}

#[derive(Debug, Parser)]
pub struct ListCommand {
    // No additional arguments needed for listing profiles
}

#[derive(Debug, Parser)]
pub struct ClearCommand {
    /// Skip confirmation prompt
    #[arg(long, short)]
    pub force: bool,
}

#[derive(Debug, Deserialize)]
struct AuthCallback {
    access_token: Option<String>,
    refresh_token: Option<String>,
}

impl AuthCommand {
    pub async fn run(&self, environment: &Environment) -> EyreResult<()> {
        match &self.action {
            AuthAction::Login(cmd) => cmd.run(environment).await,
            AuthAction::Logout(cmd) => cmd.run(environment).await,
            AuthAction::Info(cmd) => cmd.run(environment).await,
            AuthAction::SetActive(cmd) => cmd.run(environment).await,
            AuthAction::List(cmd) => cmd.run(environment).await,
            AuthAction::Clear(cmd) => cmd.run(environment).await,
        }
    }
}

impl LoginCommand {
    pub async fn run(&self, _environment: &Environment) -> EyreResult<()> {
        // Use the API URL from the command arguments
        let api_url = &self.api;

        // Create a temporary connection for auth detection (no auth)
        let temp_connection = ConnectionInfo::new(api_url.clone(), None, None);

        // 1. Detect authentication mode
        println!("🔍 Detecting authentication mode...");
        let auth_mode = temp_connection.detect_auth_mode().await?;
        
        if auth_mode == "none" {
            bail!("Server does not require authentication");
        }

        println!("✅ Server requires authentication (mode: {})", auth_mode);

        // 2. Start local callback server
        let (callback_port, callback_rx) = self.start_callback_server().await?;
        println!("🌐 Started local callback server on port {}", callback_port);

        // 3. Build auth URL and open browser
        let auth_url = self.build_auth_url(api_url, callback_port)?;
        println!("🔗 Opening browser to: {}", auth_url);
        
        if let Err(e) = webbrowser::open(&auth_url.to_string()) {
            println!("⚠️  Failed to open browser automatically: {}", e);
            println!("📋 Please manually open this URL in your browser:");
            println!("   {}", auth_url);
        }

        // 4. Wait for callback with timeout
        println!("⏳ Waiting for authentication callback (timeout: {}s)...", self.timeout);
        
        let auth_result = timeout(
            Duration::from_secs(self.timeout),
            callback_rx
        ).await
        .map_err(|_| eyre!("Authentication timed out after {} seconds", self.timeout))?
        .map_err(|_| eyre!("Callback server error"))?;

        // 5. Process callback result
        match auth_result {
            Ok(callback) => {
                let access_token = callback.access_token
                    .ok_or_eyre("No access token received")?;
                let refresh_token = callback.refresh_token;

                // 6. Store tokens
                let jwt_token = JwtToken {
                    access_token,
                    refresh_token,
                };

                let profile_config = ProfileConfig {
                    node_url: api_url.clone(),
                    token: Some(jwt_token),
                };

                let storage = create_storage();
                storage.store_profile(&self.profile, &profile_config).await?;

                println!("✅ Authentication successful!");
                println!("🔐 Tokens stored for profile: {}", self.profile);
            }
            Err(e) => {
                bail!("Authentication failed: {}", e);
            }
        }

        Ok(())
    }

    async fn start_callback_server(&self) -> EyreResult<(u16, oneshot::Receiver<Result<AuthCallback, String>>)> {
        let (port_start, port_end) = self.parse_port_range()?;
        
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(std::sync::Mutex::new(Some(tx)));

        // Try to bind to a port in the range
        let mut listener = None;
        let mut bound_port = 0;
        
        for port in port_start..=port_end {
            match TcpListener::bind(("127.0.0.1", port)).await {
                Ok(l) => {
                    bound_port = port;
                    listener = Some(l);
                    break;
                }
                Err(_) => continue,
            }
        }

        let listener = listener
            .ok_or_eyre(format!("Could not bind to any port in range {}-{}", port_start, port_end))?;

        // Create axum router
        let app = Router::new()
            .route("/callback", get({
                let tx = Arc::clone(&tx);
                move |query: Query<HashMap<String, String>>| async move {
                    println!("🔍 Callback received parameters: {:?}", query.0);
                    
                    let callback = AuthCallback {
                        access_token: query.get("access_token").cloned(),
                        refresh_token: query.get("refresh_token").cloned(),
                    };

                    println!("🔍 Parsed callback: {:?}", callback);

                    // Send result through channel
                    if let Ok(mut guard) = tx.lock() {
                        if let Some(sender) = guard.take() {
                            drop(sender.send(Ok(callback)));
                        }
                    }

                    Html(r#"
                    <html>
                        <head><title>Authentication Complete</title></head>
                        <body>
                            <h1>🎉 Authentication Complete!</h1>
                            <p>You can now close this browser window and return to the terminal.</p>
                            <script>window.close();</script>
                        </body>
                    </html>
                    "#)
                }
            }));

        // Start server in background
        let _server_handle = tokio::spawn(async move {  
            let result = axum::serve(listener, app).await;
            
            match result {
                Ok(_) => {
                    println!("🔍 DEBUG: axum::serve completed successfully");
                }
                Err(e) => {
                    println!("🔍 DEBUG: axum::serve failed with error: {}", e);
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

    fn parse_port_range(&self) -> EyreResult<(u16, u16)> {
        let parts: Vec<&str> = self.port_range.split('-').collect();
        match parts.len() {
            1 => {
                let port = parts[0].parse()
                    .map_err(|_| eyre!("Invalid port number: {}", parts[0]))?;
                Ok((port, port))
            }
            2 => {
                let start = parts[0].parse()
                    .map_err(|_| eyre!("Invalid start port: {}", parts[0]))?;
                let end = parts[1].parse()
                    .map_err(|_| eyre!("Invalid end port: {}", parts[1]))?;
                if start > end {
                    bail!("Start port {} must be <= end port {}", start, end);
                }
                Ok((start, end))
            }
            _ => bail!("Invalid port range format. Use 'port' or 'start-end'"),
        }
    }

    fn build_auth_url(&self, api_url: &Url, callback_port: u16) -> EyreResult<Url> {
        let mut auth_url = api_url.join("/auth/login")
            .map_err(|e| eyre!("Failed to build auth URL: {}", e))?;

        let callback_url = format!("http://127.0.0.1:{}/callback", callback_port);
        
        {
            let mut query_pairs = auth_url.query_pairs_mut();
            let _ = query_pairs.append_pair("callback-url", &callback_url);
            
            if let Some(permissions) = &self.permissions {
                let _ = query_pairs.append_pair("permissions", permissions);
            }
            
            // Add app-url parameter with the node's base URL (remove trailing slash)
            let app_url = api_url.as_str().trim_end_matches('/');
            let _ = query_pairs.append_pair("app-url", app_url);
        }

        Ok(auth_url)
    }
}

impl LogoutCommand {
    pub async fn run(&self, _environment: &Environment) -> EyreResult<()> {
        println!("Logging out profile: {}", self.profile);
        
        let storage = create_storage();
        
        // Check if profile exists
        match storage.load_profile(&self.profile).await? {
            Some(_) => {
                storage.remove_profile(&self.profile).await?;
                println!("Successfully logged out profile: {}", self.profile);
            }
            None => {
                println!("Profile '{}' is not logged in", self.profile);
            }
        }

        Ok(())
    }
}

impl InfoCommand {
    pub async fn run(&self, _environment: &Environment) -> EyreResult<()> {
        
        let storage = create_storage();
        
        match storage.load_profile(&self.profile).await? {
            Some(config) => {
                println!("Node URL: {}", config.node_url);
                
                if let Some(token) = config.token {
                    println!("Access Token: {}", token.access_token);
                    
                    if let Some(ref refresh_token) = token.refresh_token {
                        println!("Refresh Token: {}", refresh_token);
                    } else {
                        println!("Refresh Token: None");
                    }
                } else {
                    println!("No token stored (this shouldn't happen)");
                }
            }
            None => {
                println!("Profile '{}' does not exist", self.profile);
            }
        }

        Ok(())
    }
}

impl SetActiveCommand {
    pub async fn run(&self, _environment: &Environment) -> EyreResult<()> {
        println!("Setting active profile: {}", self.profile);
        
        let storage = create_storage();
        
        // Check if profile exists
        match storage.load_profile(&self.profile).await? {
            Some(_) => {
                storage.set_current_profile(&self.profile).await?;
                println!("Successfully set active profile: {}", self.profile);
            }
            None => {
                println!("Profile '{}' does not exist", self.profile);
                println!("Available profiles:");
                // Use the combined method to avoid additional keychain access
                let (profiles, _current) = storage.list_profiles().await?;
                for profile in profiles {
                    println!("{}", profile);
                }
            }
        }

        Ok(())
    }
}

impl ListCommand {
    pub async fn run(&self, _environment: &Environment) -> EyreResult<()> {
        let storage = create_storage();
        
        let (profiles, current_profile) = storage.list_profiles().await?;
        
        if profiles.is_empty() {
            println!("No profiles found");
            println!("Create your first profile with: meroctl auth login --profile <name>");
        } else {
            println!("Available profiles:");
            for profile in profiles {
                let is_current = current_profile.as_ref() == Some(&profile);
                let marker = if is_current { "<-- ACTIVE" } else { "" };
                println!("{} {}", profile, marker);
            }
        }

        Ok(())
    }
}

impl ClearCommand {
    pub async fn run(&self, _environment: &Environment) -> EyreResult<()> {
        // Confirmation prompt (unless --force is used)
        if !self.force {
            print!("Are you sure you want to clear all profiles? (y/N): ");
            io::stdout().flush()?;
            
            let mut input = String::new();
            let _ = io::stdin().read_line(&mut input)?;
            
            let input = input.trim().to_lowercase();
            if input != "y" && input != "yes" {
                println!("Cancelled");
                return Ok(());
            }
        }

        let storage = create_storage();

        // Clear all profiles using the storage method (single keychain access)
        storage.clear_all().await?;
        
        println!("Successfully deleted all profiles");

        Ok(())
    }
} 
