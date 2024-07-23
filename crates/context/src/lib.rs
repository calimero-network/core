use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_file as symlink;
use std::sync::Arc;

use calimero_network::client::NetworkClient;
use camino::Utf8PathBuf;
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::types::{BlockReference, Finality, FunctionArgs};
use near_primitives::views::QueryRequest;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing::{error, info};

pub mod config;

#[derive(Clone)]
pub struct ContextManager {
    pub config: config::ApplicationConfig,
    pub store: calimero_store::Store,
    pub network_client: NetworkClient,
    state: Arc<RwLock<State>>,
}

#[derive(Default)]
struct State {
    pending_initial_catchup: HashSet<calimero_primitives::context::ContextId>,
}

impl ContextManager {
    pub async fn start(
        config: &config::ApplicationConfig,
        store: calimero_store::Store,
        network_client: NetworkClient,
    ) -> eyre::Result<Self> {
        let this = ContextManager {
            config: config.clone(),
            store,
            network_client,
            state: Default::default(),
        };

        this.boot().await?;

        Ok(this)
    }

    async fn boot(&self) -> eyre::Result<()> {
        let handle = self.store.handle();

        let mut iter = handle.iter(&calimero_store::key::ContextMeta::new([0; 32].into()))?;

        for key in iter.keys() {
            self.state
                .write()
                .await
                .pending_initial_catchup
                .insert(key.context_id());

            self.subscribe(&key.context_id()).await?;
        }

        Ok(())
    }

    async fn subscribe(
        &self,
        context_id: &calimero_primitives::context::ContextId,
    ) -> eyre::Result<()> {
        self.network_client
            .subscribe(calimero_network::types::IdentTopic::new(context_id))
            .await?;

        info!(%context_id, "Subscribed to context");

        Ok(())
    }

    async fn unsubscribe(
        &self,
        context_id: &calimero_primitives::context::ContextId,
    ) -> eyre::Result<()> {
        self.network_client
            .unsubscribe(calimero_network::types::IdentTopic::new(context_id))
            .await?;

        info!(%context_id, "Unsubscribed from context");

        Ok(())
    }
}

impl ContextManager {
    pub async fn add_context(
        &self,
        context: calimero_primitives::context::Context,
    ) -> eyre::Result<()> {
        if !self.is_application_installed(&context.application_id) {
            eyre::bail!("Application is not installed on node.")
        }

        let mut handle = self.store.handle();

        handle.put(
            &calimero_store::key::ContextMeta::new(context.id),
            &calimero_store::types::ContextMeta {
                application_id: context.application_id.0.into(),
                last_transaction_hash: context.last_transaction_hash.into(),
            },
        )?;

        self.subscribe(&context.id).await?;

        Ok(())
    }

    pub async fn join_context(
        &self,
        context_id: &calimero_primitives::context::ContextId,
    ) -> eyre::Result<Option<()>> {
        if self
            .state
            .read()
            .await
            .pending_initial_catchup
            .contains(&context_id)
        {
            return Ok(None);
        }

        self.state
            .write()
            .await
            .pending_initial_catchup
            .insert(*context_id);

        self.subscribe(context_id).await?;

        info!(%context_id,  "Joined context with pending initial catchup");

        Ok(Some(()))
    }

    pub async fn is_context_pending_initial_catchup(
        &self,
        context_id: &calimero_primitives::context::ContextId,
    ) -> bool {
        self.state
            .read()
            .await
            .pending_initial_catchup
            .contains(context_id)
    }

    pub async fn clear_context_pending_initial_catchup(
        &self,
        context_id: &calimero_primitives::context::ContextId,
    ) -> bool {
        self.state
            .write()
            .await
            .pending_initial_catchup
            .remove(context_id)
    }

    pub fn get_context(
        &self,
        context_id: &calimero_primitives::context::ContextId,
    ) -> eyre::Result<Option<calimero_primitives::context::Context>> {
        let handle = self.store.handle();

        let key = calimero_store::key::ContextMeta::new(*context_id);

        let Some(ctx_meta) = handle.get(&key)? else {
            return Ok(None);
        };

        Ok(Some(calimero_primitives::context::Context {
            id: *context_id,
            application_id: ctx_meta.application_id.into_string().into(),
            last_transaction_hash: ctx_meta.last_transaction_hash.into(),
        }))
    }

    pub async fn delete_context(
        &self,
        context_id: &calimero_primitives::context::ContextId,
    ) -> eyre::Result<bool> {
        let mut handle = self.store.handle();

        let key = calimero_store::key::ContextMeta::new(*context_id);

        if !handle.has(&key)? {
            return Ok(false);
        }

        handle.delete(&key)?;

        self.unsubscribe(context_id).await?;

        Ok(true)
    }

    pub fn get_context_ids(
        &self,
        start: Option<calimero_primitives::context::ContextId>,
    ) -> eyre::Result<Vec<calimero_primitives::context::ContextId>> {
        let handle = self.store.handle();

        let mut iter = handle.iter(&calimero_store::key::ContextMeta::new(
            start.map_or_else(|| [0; 32].into(), Into::into),
        ))?;

        let contexts = iter.keys().map(|key| key.context_id());

        Ok(contexts.collect())
    }

    pub fn get_contexts(
        &self,
        start: Option<calimero_primitives::context::ContextId>,
    ) -> eyre::Result<Vec<calimero_primitives::context::Context>> {
        let handle = self.store.handle();

        let mut iter = handle.iter(&calimero_store::key::ContextMeta::new(
            start.map_or_else(|| [0; 32].into(), Into::into),
        ))?;

        let contexts = iter
            .entries()
            .map(|(k, v)| calimero_primitives::context::Context {
                id: k.context_id(),
                application_id: v.application_id.into_string().into(),
                last_transaction_hash: v.last_transaction_hash.into(),
            });

        Ok(contexts.collect())
    }

    // todo! do this only when initializing contexts
    // todo! start refining to blob API
    pub async fn install_application(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
        // todo! permit None version for latest
        version: &semver::Version,
        path: &str,
        hash: Option<&str>,
    ) -> eyre::Result<()> {
        self.download_and_install_release(&application_id, &version, &path, hash)
            .await?;

        Ok(())
    }

    pub async fn install_dev_application(
        &self,
        application_id: calimero_primitives::application::ApplicationId,
        version: &semver::Version,
        path: Utf8PathBuf,
    ) -> eyre::Result<()> {
        self.link_release(&application_id, version, &path)?;

        Ok(())
    }

    pub async fn update_context_application_id(
        &self,
        context_id: calimero_primitives::context::ContextId,
        application_id: calimero_primitives::application::ApplicationId,
    ) -> eyre::Result<()> {
        let mut handle = self.store.handle();

        let key = calimero_store::key::ContextMeta::new(context_id);

        let Some(mut value) = handle.get(&key)? else {
            eyre::bail!("Context not found")
        };

        value.application_id = application_id.0.into();

        handle.put(&key, &value)?;

        Ok(())
    }

    pub async fn list_installed_applications(
        &self,
    ) -> eyre::Result<Vec<calimero_primitives::application::Application>> {
        if !self.config.dir.exists() {
            return Ok(vec![]);
        }

        let Ok(entries) = fs::read_dir(&self.config.dir) else {
            eyre::bail!("Failed to read application directory");
        };

        let mut applications = vec![];

        entries.filter_map(|entry| entry.ok()).for_each(|entry| {
            if let Some(file_name) = entry.file_name().to_str() {
                let application_id = file_name.to_string().into();
                if let Some((version, _)) = self.get_latest_application_info(&application_id) {
                    applications.push(calimero_primitives::application::Application {
                        id: application_id,
                        version,
                    });
                }
            }
        });

        Ok(applications)
    }

    pub fn is_application_installed(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
    ) -> bool {
        self.get_latest_application_info(application_id).is_some()
    }

    pub fn load_application_blob(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
    ) -> eyre::Result<Vec<u8>> {
        let Some((_, path)) = self.get_latest_application_info(application_id) else {
            eyre::bail!("failed to get application with id: {}", application_id)
        };

        Ok(fs::read(&path)?)
    }

    pub fn get_application_latest_version(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
    ) -> eyre::Result<semver::Version> {
        let Some((version, _)) = self.get_latest_application_info(application_id) else {
            eyre::bail!("failed to get application with id: {}", application_id)
        };

        Ok(version)
    }

    async fn download_and_install_release(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
        version: &semver::Version,
        path: &str,
        hash: Option<&str>,
    ) -> eyre::Result<bool> {
        // todo! download to a tempdir
        // todo! Blob API
        let base_path = format!("{}/{}/{}", self.config.dir, application_id, version);
        fs::create_dir_all(&base_path)?;

        let file_path = format!("{}/binary.wasm", base_path);
        if fs::metadata(&file_path).is_ok() {
            return Ok(false);
        }

        let mut file = File::create(&file_path)?;

        let mut response = reqwest::Client::new().get(path).send().await?;
        let mut hasher = Sha256::new();
        while let Some(chunk) = response.chunk().await? {
            hasher.update(&chunk);
            file.write_all(&chunk)?;
        }
        let result = hasher.finalize();
        let blob_hash = format!("{:x}", result);

        if let Some(hash) = hash {
            if blob_hash.as_str() != hash {
                if let Err(e) = std::fs::remove_file(&file_path) {
                    error!(%e, "Failed to delete file after failed verification");
                }

                eyre::bail!("Release hash does not match the hash of the downloaded file");
            }
        }

        Ok(true)
    }

    fn link_release(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
        version: &semver::Version,
        link_path: &camino::Utf8Path,
    ) -> eyre::Result<bool> {
        let base_path = format!("{}/{}/{}", self.config.dir, application_id, version);
        fs::create_dir_all(&base_path)?;

        let file_path = format!("{}/binary.wasm", base_path);
        if fs::metadata(&file_path).is_ok() {
            return Ok(false);
        }

        info!("Application file saved at: {}", file_path);
        if let Err(err) = symlink(link_path, &file_path) {
            eyre::bail!("Symlinking failed: {}", err);
        }

        info!(
            "Application {} linked to node\nPath to linked file at {}",
            application_id, file_path
        );

        Ok(true)
    }

    fn get_latest_application_info(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
    ) -> Option<(semver::Version, String)> {
        let application_base_path = self.config.dir.join(application_id.to_string());

        if let Ok(entries) = fs::read_dir(&application_base_path) {
            let mut versions_with_binary = entries
                .filter_map(|entry| {
                    let entry = entry.ok()?;
                    let entry_path = entry.path();

                    let version =
                        semver::Version::parse(entry_path.file_name()?.to_string_lossy().as_ref())
                            .ok()?;

                    let binary_path = entry_path.join("binary.wasm");
                    binary_path.exists().then_some((version, binary_path))
                })
                .collect::<Vec<_>>();

            versions_with_binary.sort_by(|a, b| b.0.cmp(&a.0));

            let version_with_binary = versions_with_binary.first();
            let version = match version_with_binary {
                Some((version, path)) => {
                    Some((version.clone(), path.to_string_lossy().into_owned()))
                }
                None => None,
            };
            version
        } else {
            None
        }
    }
}
