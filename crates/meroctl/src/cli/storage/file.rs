use std::path::PathBuf;

use async_trait::async_trait;
use eyre::{Context, Result as EyreResult};
use tokio::fs;

use super::{AllProfiles, ProfileConfig, TokenStorage};

pub struct FileStorage {
    profiles_path: PathBuf,
}

impl FileStorage {
    pub fn new() -> Self {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".config"))
            .join("meroctl");

        Self {
            profiles_path: config_dir.join("profiles.json"),
        }
    }

    async fn ensure_config_dir(&self) -> EyreResult<()> {
        if let Some(parent) = self.profiles_path.parent() {
            fs::create_dir_all(parent)
                .await
                .wrap_err("Failed to create config directory")?;
        }
        Ok(())
    }
}

#[async_trait]
impl TokenStorage for FileStorage {
    async fn load_all_profiles(&self) -> EyreResult<AllProfiles> {
        match fs::read_to_string(&self.profiles_path).await {
            Ok(content) => serde_json::from_str(&content).wrap_err("Failed to parse profiles file"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AllProfiles::default()),
            Err(e) => Err(e).wrap_err("Failed to read profiles file"),
        }
    }

    async fn save_all_profiles(&self, profiles: &AllProfiles) -> EyreResult<()> {
        self.ensure_config_dir().await?;

        let content =
            serde_json::to_string_pretty(profiles).wrap_err("Failed to serialize profiles")?;

        fs::write(&self.profiles_path, content)
            .await
            .wrap_err("Failed to write profiles file")?;

        // Set restrictive permissions (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.profiles_path, permissions)
                .wrap_err("Failed to set file permissions")?;
        }

        Ok(())
    }

    async fn store_profile(&self, name: &str, config: &ProfileConfig) -> EyreResult<()> {
        let mut all = self.load_all_profiles().await?;
        drop(all.profiles.insert(name.to_string(), config.clone()));
        self.save_all_profiles(&all).await
    }

    async fn remove_profile(&self, name: &str) -> EyreResult<()> {
        let mut all = self.load_all_profiles().await?;
        drop(all.profiles.remove(name));
        if all.active_profile.as_deref() == Some(name) {
            all.active_profile = None;
        }
        self.save_all_profiles(&all).await
    }

    async fn get_current_profile(&self) -> EyreResult<Option<(String, ProfileConfig)>> {
        let all = self.load_all_profiles().await?;
        match all.active_profile {
            Some(name) => Ok(all.profiles.get(&name).map(|config| (name, config.clone()))),
            None => Ok(None),
        }
    }

    async fn set_current_profile(&self, name: &str) -> EyreResult<()> {
        let mut all = self.load_all_profiles().await?;
        if all.profiles.contains_key(name) {
            all.active_profile = Some(name.to_string());
            self.save_all_profiles(&all).await
        } else {
            Err(eyre::eyre!("Profile {} does not exist", name))
        }
    }

    async fn list_profiles(&self) -> EyreResult<(Vec<String>, Option<String>)> {
        let all = self.load_all_profiles().await?;
        let mut profiles: Vec<_> = all.profiles.keys().cloned().collect();
        profiles.sort();
        Ok((profiles, all.active_profile))
    }

    async fn clear_all(&self) -> EyreResult<()> {
        self.save_all_profiles(&AllProfiles::default()).await
    }
}
