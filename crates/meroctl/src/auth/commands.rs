use clap::{Parser, Subcommand};
use eyre::Result;

use crate::cli::Environment;
use super::manager::AuthManager;

#[derive(Debug, Parser)]
#[command(about = "Manage authentication")]
pub struct AuthCommand {
    #[command(subcommand)]
    pub action: AuthAction,
}

#[derive(Debug, Subcommand)]
pub enum AuthAction {
    /// Login to authenticate with the node
    Login {
        /// Profile name to use
        #[arg(long, short, default_value = "default")]
        profile: String,
        
        /// Permissions to request (comma-separated)
        #[arg(long, value_delimiter = ',')]
        permissions: Vec<String>,
        
        /// Context ID to authenticate for
        #[arg(long)]
        context_id: Option<String>,
        
        /// Force re-authentication even if valid tokens exist
        #[arg(long)]
        force: bool,
    },
    
    /// Logout (clear stored tokens)
    Logout {
        /// Profile name to clear
        #[arg(long, short, default_value = "default")]
        profile: String,
        
        /// Clear all profiles
        #[arg(long)]
        all: bool,
    },
    
    /// Show authentication status
    Status {
        /// Show status for specific profile (default: all profiles)
        #[arg(long, short)]
        profile: Option<String>,
        
        /// Show detailed information
        #[arg(long)]
        verbose: bool,
    },
    
    /// Refresh authentication tokens
    Refresh {
        /// Profile name to refresh
        #[arg(long, short, default_value = "default")]
        profile: String,
    },
}

impl AuthCommand {
    pub async fn run(&self, environment: &Environment) -> Result<()> {
        match &self.action {
            AuthAction::Login {
                profile,
                permissions,
                context_id,
                force,
            } => {
                self.handle_login(environment, profile, permissions, context_id, *force)
                    .await
            }
            AuthAction::Logout { profile, all } => {
                self.handle_logout(environment, profile, *all).await
            }
            AuthAction::Status { profile, verbose } => {
                self.handle_status(environment, profile.as_deref(), *verbose)
                    .await
            }
            AuthAction::Refresh { profile } => {
                self.handle_refresh(environment, profile).await
            }
        }
    }

    async fn handle_login(
        &self,
        environment: &Environment,
        profile: &str,
        permissions: &[String],
        context_id: &Option<String>,
        force: bool,
    ) -> Result<()> {
        println!("🔐 Logging in with profile: {}", profile);
        
        // Get the API URL from the environment
        let connection = environment.connection.as_ref()
            .ok_or_else(|| eyre::eyre!("No connection information available"))?;
        
        let auth_manager = AuthManager::new(profile.to_string(), connection.api_url.clone()).await?;
        
        // Check if we already have valid tokens
        if !force {
            if let Some(_tokens) = auth_manager.get_valid_token().await? {
                println!("✅ Already authenticated with valid tokens");
                return Ok(());
            }
        }
        
        // Build permission list
        let mut requested_permissions = permissions.to_vec();
        if let Some(ctx_id) = context_id {
            requested_permissions.push(format!("context[{}]", ctx_id));
        }
        if requested_permissions.is_empty() {
            requested_permissions.push("context:read".to_string());
        }
        
        // Perform browser authentication
        auth_manager.browser_auth(&requested_permissions).await?;
        
        println!("✅ Authentication successful for profile: {}", profile);
        Ok(())
    }

    async fn handle_logout(
        &self,
        environment: &Environment,
        profile: &str,
        all: bool,
    ) -> Result<()> {
        let connection = environment.connection.as_ref()
            .ok_or_else(|| eyre::eyre!("No connection information available"))?;
        
        let auth_manager = AuthManager::new(profile.to_string(), connection.api_url.clone()).await?;
        
        if all {
            let profiles = auth_manager.list_all_profiles().await?;
            for p in profiles {
                auth_manager.logout(&p).await?;
                println!("🔓 Logged out from profile: {}", p);
            }
            println!("✅ Logged out from all profiles");
        } else {
            auth_manager.logout(profile).await?;
            println!("🔓 Logged out from profile: {}", profile);
        }
        
        Ok(())
    }

    async fn handle_status(
        &self,
        environment: &Environment,
        profile: Option<&str>,
        verbose: bool,
    ) -> Result<()> {
        let connection = environment.connection.as_ref()
            .ok_or_else(|| eyre::eyre!("No connection information available"))?;
        
        // Use default profile if none specified
        let profile_name = profile.unwrap_or("default");
        let auth_manager = AuthManager::new(profile_name.to_string(), connection.api_url.clone()).await?;
        
        if let Some(specific_profile) = profile {
            // Show status for specific profile
            self.show_profile_status(&auth_manager, specific_profile, verbose).await?;
        } else {
            // Show status for all profiles
            let profiles = auth_manager.list_all_profiles().await?;
            if profiles.is_empty() {
                println!("No authentication profiles found");
                return Ok(());
            }
            
            for p in profiles {
                self.show_profile_status(&auth_manager, &p, verbose).await?;
                if verbose {
                    println!(); // Add spacing between profiles in verbose mode
                }
            }
        }
        
        Ok(())
    }

    async fn show_profile_status(
        &self,
        auth_manager: &AuthManager,
        profile: &str,
        verbose: bool,
    ) -> Result<()> {
        if let Some(tokens) = auth_manager.get_stored_tokens(profile).await? {
            let status = if tokens.is_expired() {
                "❌ Expired"
            } else if tokens.expires_within(chrono::Duration::minutes(10)) {
                "⚠️  Expires soon"
            } else {
                "✅ Valid"
            };
            
            println!("Profile: {} - {}", profile, status);
            
            if verbose {
                println!("  Node: {}", tokens.node_url);
                println!("  Expires: {}", tokens.expires_at.format("%Y-%m-%d %H:%M:%S UTC"));
                println!("  Time left: {}", format_duration(tokens.time_until_expiry()));
                if !tokens.permissions.is_empty() {
                    println!("  Permissions: {}", tokens.permissions.join(", "));
                }
            }
        } else {
            println!("Profile: {} - ❌ Not authenticated", profile);
        }
        
        Ok(())
    }

    async fn handle_refresh(
        &self,
        environment: &Environment,
        profile: &str,
    ) -> Result<()> {
        let connection = environment.connection.as_ref()
            .ok_or_else(|| eyre::eyre!("No connection information available"))?;
        
        let auth_manager = AuthManager::new(profile.to_string(), connection.api_url.clone()).await?;
        
        println!("🔄 Refreshing tokens for profile: {}", profile);
        
        if auth_manager.refresh_tokens().await? {
            println!("✅ Tokens refreshed successfully");
        } else {
            println!("❌ Failed to refresh tokens. Please login again.");
            return Err(eyre::eyre!("Token refresh failed"));
        }
        
        Ok(())
    }
}

/// Format a duration in human-readable form
fn format_duration(duration: chrono::Duration) -> String {
    if duration.num_seconds() < 0 {
        return "expired".to_string();
    }
    
    let total_seconds = duration.num_seconds();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    
    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
} 