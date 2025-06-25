use async_trait::async_trait;
use eyre::Result as EyreResult;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::{ProfileConfig, ProfileTokens, TokenStorage};

#[derive(Debug)]
pub struct MemoryStorage {
    data: Arc<RwLock<ProfileTokens>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(ProfileTokens::default())),
        }
    }
}

#[async_trait]
impl TokenStorage for MemoryStorage {
    async fn store_profile(&self, profile: &str, config: &ProfileConfig) -> EyreResult<()> {
        let mut data = self.data.write().await;
        drop(data.profiles.insert(profile.to_string(), config.clone()));
        
        // Set as current profile if it's the first one
        if data.current_profile.is_empty() {
            data.current_profile = profile.to_string();
        }
        
        Ok(())
    }

    async fn load_profile(&self, profile: &str) -> EyreResult<Option<ProfileConfig>> {
        let data = self.data.read().await;
        Ok(data.profiles.get(profile).cloned())
    }

    async fn remove_profile(&self, profile: &str) -> EyreResult<()> {
        let mut data = self.data.write().await;
        drop(data.profiles.remove(profile));
        
        // If we removed the current profile, switch to another one or clear
        if data.current_profile == profile {
            data.current_profile = data.profiles.keys().next()
                .unwrap_or(&String::new()).to_string();
        }
        
        Ok(())
    }

    async fn clear_all(&self) -> EyreResult<()> {
        let mut data = self.data.write().await;
        data.profiles.clear();
        data.current_profile.clear();
        Ok(())
    }

    async fn set_current_profile(&self, profile: &str) -> EyreResult<()> {
        let mut data = self.data.write().await;
        
        if !data.profiles.contains_key(profile) {
            return Err(eyre::eyre!("Profile '{}' does not exist", profile));
        }
        
        data.current_profile = profile.to_string();
        Ok(())
    }

    async fn list_profiles(&self) -> EyreResult<(Vec<String>, Option<String>)> {
        let data = self.data.read().await;
        let profiles = data.profiles.keys().cloned().collect();
        let current = if data.current_profile.is_empty() {
            None
        } else {
            Some(data.current_profile.clone())
        };
        Ok((profiles, current))
    }

    async fn get_current_profile(&self) -> EyreResult<Option<(String, ProfileConfig)>> {
        let data = self.data.read().await;
        if data.current_profile.is_empty() {
            Ok(None)
        } else {
            match data.profiles.get(&data.current_profile) {
                Some(config) => Ok(Some((data.current_profile.clone(), config.clone()))),
                None => Ok(None), // Current profile points to non-existent profile
            }
        }
    }
} 