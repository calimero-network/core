use async_trait::async_trait;
use eyre::{Context, Result as EyreResult};
use keyring::Entry;

use super::{ProfileConfig, ProfileTokens, TokenStorage};

const SERVICE_NAME: &str = "meroctl";
const TOKENS_KEY: &str = "auth_tokens";
const PROFILE_KEY: &str = "current_profile";

pub struct KeychainStorage {
    service_name: String,
}

impl KeychainStorage {
    pub fn new() -> Self {
        Self {
            service_name: SERVICE_NAME.to_string(),
        }
    }

    pub fn is_available() -> bool {
        // Test if we can create an entry (this will work on macOS, Windows, Linux with proper setup)
        Entry::new(SERVICE_NAME, "test").is_ok()
    }

    fn get_tokens_entry(&self) -> EyreResult<Entry> {
        Entry::new(&self.service_name, TOKENS_KEY)
            .wrap_err("Failed to create keychain entry for tokens")
    }

    fn get_profile_entry(&self) -> EyreResult<Entry> {
        Entry::new(&self.service_name, PROFILE_KEY)
            .wrap_err("Failed to create keychain entry for profile")
    }

    async fn load_all_tokens(&self) -> EyreResult<ProfileTokens> {
        let entry = self.get_tokens_entry()?;

        match entry.get_password() {
            Ok(data) => {
                serde_json::from_str(&data).wrap_err("Failed to deserialize tokens from keychain")
            }
            Err(keyring::Error::NoEntry) => Ok(ProfileTokens::default()),
            Err(e) => Err(e).wrap_err("Failed to read tokens from keychain"),
        }
    }

    async fn save_all_tokens(&self, tokens: &ProfileTokens) -> EyreResult<()> {
        let entry = self.get_tokens_entry()?;
        let data = serde_json::to_string(tokens).wrap_err("Failed to serialize tokens")?;

        entry
            .set_password(&data)
            .wrap_err("Failed to store tokens in keychain")
    }
}

#[async_trait]
impl TokenStorage for KeychainStorage {
    async fn store_profile(&self, profile: &str, config: &ProfileConfig) -> EyreResult<()> {
        let mut tokens = self.load_all_tokens().await?;
        let _unused = tokens.profiles.insert(profile.to_string(), config.clone());

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
        let _unused = tokens.profiles.remove(profile);

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
        let entry = self.get_tokens_entry()?;
        let profile_entry = self.get_profile_entry()?;

        // Ignore errors if entries don't exist
        let _unused = entry.delete_password();
        let _unused = profile_entry.delete_password();

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
