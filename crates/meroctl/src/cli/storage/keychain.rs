use std::sync::RwLock;

use async_trait::async_trait;
use eyre::{Context, Result as EyreResult};
use keyring::Entry;

use super::{AllProfiles, ProfileConfig, TokenStorage};

const SERVICE_NAME: &str = "meroctl";
const STORAGE_KEY: &str = "profiles";

pub struct KeychainStorage {
    entry: Entry,
    cache: RwLock<Option<AllProfiles>>,
}

impl KeychainStorage {
    pub fn new() -> Self {
        Self {
            entry: Entry::new(SERVICE_NAME, STORAGE_KEY).expect("Failed to create keychain entry"),
            cache: RwLock::new(None),
        }
    }

    pub fn is_available() -> bool {
        Entry::new(SERVICE_NAME, STORAGE_KEY).is_ok()
    }

    async fn load_profiles_cached(&self) -> EyreResult<AllProfiles> {
        {
            let cache = self.cache.read().unwrap();
            if let Some(ref cached) = *cache {
                return Ok(cached.clone());
            }
        }

        let profiles = self.load_from_keychain().await?;
        {
            let mut cache = self.cache.write().unwrap();
            *cache = Some(profiles.clone());
        }
        Ok(profiles)
    }

    async fn load_from_keychain(&self) -> EyreResult<AllProfiles> {
        match self.entry.get_password() {
            Ok(data) => {
                serde_json::from_str(&data).wrap_err("Failed to deserialize profiles from keychain")
            }
            Err(keyring::Error::NoEntry) => Ok(AllProfiles::default()),
            Err(e) => Err(e).wrap_err("Failed to read profiles from keychain"),
        }
    }

    async fn save_profiles_with_cache(&self, profiles: &AllProfiles) -> EyreResult<()> {
        let data = serde_json::to_string(profiles).wrap_err("Failed to serialize profiles")?;

        self.entry
            .set_password(&data)
            .wrap_err("Failed to store profiles in keychain")?;

        {
            let mut cache = self.cache.write().unwrap();
            *cache = Some(profiles.clone());
        }

        Ok(())
    }
}

#[async_trait]
impl TokenStorage for KeychainStorage {
    async fn load_all_profiles(&self) -> EyreResult<AllProfiles> {
        self.load_profiles_cached().await
    }

    async fn save_all_profiles(&self, profiles: &AllProfiles) -> EyreResult<()> {
        self.save_profiles_with_cache(profiles).await
    }

    async fn store_profile(&self, name: &str, config: &ProfileConfig) -> EyreResult<()> {
        let mut all = self.load_profiles_cached().await?;
        drop(all.profiles.insert(name.to_string(), config.clone()));
        self.save_profiles_with_cache(&all).await
    }

    async fn remove_profile(&self, name: &str) -> EyreResult<()> {
        let mut all = self.load_profiles_cached().await?;
        let profile_existed = all.profiles.remove(name).is_some();

        if !profile_existed {
            return Ok(());
        }

        if all.active_profile.as_deref() == Some(name) {
            all.active_profile = None;
        }

        self.save_profiles_with_cache(&all).await
    }

    async fn get_current_profile(&self) -> EyreResult<Option<(String, ProfileConfig)>> {
        let all = self.load_profiles_cached().await?;
        match all.active_profile {
            Some(name) => match all.profiles.get(&name) {
                Some(config) => Ok(Some((name, config.clone()))),
                None => Ok(None),
            },
            None => Ok(None),
        }
    }

    async fn set_current_profile(&self, name: &str) -> EyreResult<()> {
        let mut all = self.load_profiles_cached().await?;

        if !all.profiles.contains_key(name) {
            return Err(eyre::eyre!("Profile '{}' does not exist", name));
        }

        if all.active_profile.as_deref() != Some(name) {
            all.active_profile = Some(name.to_string());
            self.save_profiles_with_cache(&all).await?;
        }

        Ok(())
    }

    async fn list_profiles(&self) -> EyreResult<(Vec<String>, Option<String>)> {
        let all = self.load_profiles_cached().await?;
        let mut profiles: Vec<String> = all.profiles.keys().cloned().collect();
        profiles.sort_unstable();
        Ok((profiles, all.active_profile))
    }

    async fn clear_all(&self) -> EyreResult<()> {
        let empty_profiles = AllProfiles::default();
        self.save_profiles_with_cache(&empty_profiles).await
    }
}
