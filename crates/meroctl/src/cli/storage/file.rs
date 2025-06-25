use std::path::PathBuf;

use async_trait::async_trait;
use eyre::{Context, Result as EyreResult};
use tokio::fs;

use super::{ProfileConfig, ProfileTokens, TokenStorage};

pub struct FileStorage {
    tokens_path: PathBuf,
}

impl FileStorage {
    pub fn new() -> Self {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".config"))
            .join("meroctl");

        Self {
            tokens_path: config_dir.join("tokens.json"),
        }
    }

    async fn ensure_config_dir(&self) -> EyreResult<()> {
        if let Some(parent) = self.tokens_path.parent() {
            fs::create_dir_all(parent)
                .await
                .wrap_err("Failed to create config directory")?;
        }
        Ok(())
    }

    async fn load_all_tokens(&self) -> EyreResult<ProfileTokens> {
        match fs::read_to_string(&self.tokens_path).await {
            Ok(content) => serde_json::from_str(&content).wrap_err("Failed to parse tokens file"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ProfileTokens::default()),
            Err(e) => Err(e).wrap_err("Failed to read tokens file"),
        }
    }

    async fn save_all_tokens(&self, tokens: &ProfileTokens) -> EyreResult<()> {
        self.ensure_config_dir().await?;

        let content =
            serde_json::to_string_pretty(tokens).wrap_err("Failed to serialize tokens")?;

        fs::write(&self.tokens_path, content)
            .await
            .wrap_err("Failed to write tokens file")?;

        // Set restrictive permissions (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.tokens_path, permissions)
                .wrap_err("Failed to set file permissions")?;
        }

        Ok(())
    }
}

#[async_trait]
impl TokenStorage for FileStorage {
    async fn store_profile(&self, profile: &str, config: &ProfileConfig) -> EyreResult<()> {
        let mut tokens = self.load_all_tokens().await?;
        drop(tokens.profiles.insert(profile.to_string(), config.clone()));

        // Set as current profile if it's the first one
        if tokens.current_profile.is_empty() {
            tokens.current_profile = profile.to_string();
        }

        self.save_all_tokens(&tokens).await
    }

    async fn load_profile(&self, profile: &str) -> EyreResult<Option<ProfileConfig>> {
        let tokens = self.load_all_tokens().await?;
        Ok(tokens.profiles.get(profile).cloned())
    }

    async fn remove_profile(&self, profile: &str) -> EyreResult<()> {
        let mut tokens = self.load_all_tokens().await?;
        drop(tokens.profiles.remove(profile));

        // If we removed the current profile, switch to another one or clear
        if tokens.current_profile == profile {
            tokens.current_profile = tokens
                .profiles
                .keys()
                .next()
                .unwrap_or(&String::new())
                .to_string();
        }

        self.save_all_tokens(&tokens).await
    }

    async fn clear_all(&self) -> EyreResult<()> {
        // Remove the tokens file
        if let Err(e) = fs::remove_file(&self.tokens_path).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(e).wrap_err("Failed to remove tokens file");
            }
        }
        Ok(())
    }

    async fn set_current_profile(&self, profile: &str) -> EyreResult<()> {
        let mut tokens = self.load_all_tokens().await?;

        if !tokens.profiles.contains_key(profile) {
            return Err(eyre::eyre!("Profile '{}' does not exist", profile));
        }

        tokens.current_profile = profile.to_string();
        self.save_all_tokens(&tokens).await
    }

    async fn list_profiles(&self) -> EyreResult<(Vec<String>, Option<String>)> {
        let tokens = self.load_all_tokens().await?;
        let profiles = tokens.profiles.keys().cloned().collect();
        let current = if tokens.current_profile.is_empty() {
            None
        } else {
            Some(tokens.current_profile)
        };
        Ok((profiles, current))
    }

    async fn get_current_profile(&self) -> EyreResult<Option<(String, ProfileConfig)>> {
        let tokens = self.load_all_tokens().await?;
        if tokens.current_profile.is_empty() {
            Ok(None)
        } else {
            match tokens.profiles.get(&tokens.current_profile) {
                Some(config) => Ok(Some((tokens.current_profile, config.clone()))),
                None => Ok(None), // Current profile points to non-existent profile
            }
        }
    }
}
