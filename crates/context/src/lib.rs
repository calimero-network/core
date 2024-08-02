use std::collections::HashSet;
use std::sync::Arc;

use calimero_network::client::NetworkClient;
use camino::Utf8PathBuf;
use futures_util::TryStreamExt;
use reqwest::Url;
use tokio::fs;
use tokio::sync::RwLock;
use tracing::info;

pub mod config;

#[derive(Clone)]
pub struct ContextManager {
    pub store: calimero_store::Store,
    pub blob_manager: calimero_blobstore::BlobManager,
    pub network_client: NetworkClient,
    state: Arc<RwLock<State>>,
}

#[derive(Default)]
struct State {
    pending_initial_catchup: HashSet<calimero_primitives::context::ContextId>,
}

impl ContextManager {
    pub async fn start(
        store: calimero_store::Store,
        blob_manager: calimero_blobstore::BlobManager,
        network_client: NetworkClient,
    ) -> eyre::Result<Self> {
        let this = ContextManager {
            store,
            blob_manager,
            network_client,
            state: Default::default(),
        };

        this.boot().await?;

        Ok(this)
    }

    async fn boot(&self) -> eyre::Result<()> {
        let handle = self.store.handle();

        let mut iter = handle.iter::<calimero_store::key::ContextMeta>()?;

        for key in iter.keys() {
            let key = key?;

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
        if !self.is_application_installed(&context.application_id)? {
            eyre::bail!("Application is not installed on node.")
        }

        let mut handle = self.store.handle();

        handle.put(
            &calimero_store::key::ContextMeta::new(context.id),
            &calimero_store::types::ContextMeta {
                application: calimero_store::key::ApplicationMeta::new(context.application_id),
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
            application_id: ctx_meta.application.application_id(),
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

        let mut iter = handle.iter::<calimero_store::key::ContextMeta>()?;

        let mut ids = vec![];

        if let Some(start) = start {
            if let Some(key) = iter.seek(calimero_store::key::ContextMeta::new(start))? {
                ids.push(key.context_id());
            }
        }

        for key in iter.keys() {
            ids.push(key?.context_id());
        }

        Ok(ids)
    }

    pub fn get_contexts(
        &self,
        start: Option<calimero_primitives::context::ContextId>,
    ) -> eyre::Result<Vec<calimero_primitives::context::Context>> {
        let handle = self.store.handle();

        let mut iter = handle.iter::<calimero_store::key::ContextMeta>()?;

        let mut contexts = vec![];

        if let Some(start) = start {
            // todo! Iter shouldn't behave like DBIter, first next should return sought element
            if let Some(key) = iter.seek(calimero_store::key::ContextMeta::new(start))? {
                let value: calimero_store::types::ContextMeta = iter.read()?;

                contexts.push(calimero_primitives::context::Context {
                    id: key.context_id(),
                    application_id: value.application.application_id(),
                    last_transaction_hash: value.last_transaction_hash.into(),
                });
            }
        }

        for (k, v) in iter.entries() {
            let (k, v) = (k?, v?);
            contexts.push(calimero_primitives::context::Context {
                id: k.context_id(),
                application_id: v.application.application_id(),
                last_transaction_hash: v.last_transaction_hash.into(),
            });
        }

        Ok(contexts)
    }

    pub async fn update_application_id(
        &self,
        context_id: calimero_primitives::context::ContextId,
        application_id: calimero_primitives::application::ApplicationId,
    ) -> eyre::Result<()> {
        let mut handle = self.store.handle();

        let key = calimero_store::key::ContextMeta::new(context_id);

        let Some(mut value) = handle.get(&key)? else {
            eyre::bail!("Context not found")
        };

        value.application = calimero_store::key::ApplicationMeta::new(application_id);

        handle.put(&key, &value)?;

        Ok(())
    }
}

// vv~ these would be more appropriate in an ApplicationManager
impl ContextManager {
    async fn install_application(
        &self,
        blob_id: calimero_primitives::blobs::BlobId,
        source: http::Uri,
        version: Option<semver::Version>,
    ) -> eyre::Result<calimero_primitives::application::ApplicationId> {
        let application = calimero_store::types::ApplicationMeta {
            blob: calimero_store::key::BlobMeta::new(blob_id),
            version: version.map(|v| v.to_string().into_boxed_str()),
            source: source.to_string().into_boxed_str(),
        };

        let application_id = calimero_primitives::application::ApplicationId::from(
            *calimero_primitives::hash::Hash::hash_borsh(&application)?,
        );

        let mut handle = self.store.handle();

        handle.put(
            &calimero_store::key::ApplicationMeta::new(application_id),
            &application,
        )?;

        Ok(application_id)
    }

    pub async fn install_application_from_path(
        &self,
        path: Utf8PathBuf,
        version: Option<semver::Version>,
    ) -> eyre::Result<calimero_primitives::application::ApplicationId> {
        let file = fs::File::open(&path).await?;

        let blob_id = self
            .blob_manager
            .put(tokio_util::io::ReaderStream::new(file))
            .await?;

        let Ok(uri) = reqwest::Url::from_file_path(path) else {
            eyre::bail!("non-absolute path")
        };

        self.install_application(blob_id, uri.as_str().parse()?, version)
            .await
    }

    pub async fn install_application_from_url(
        &self,
        url: Url,
        version: Option<semver::Version>,
    ) -> eyre::Result<calimero_primitives::application::ApplicationId> {
        let uri = url.as_str().parse()?;

        let response = reqwest::Client::new().get(url).send().await?;

        let blob_id = self.blob_manager.put(response.bytes_stream()).await?;

        self.install_application(blob_id, uri, version).await
    }

    pub async fn list_installed_applications(
        &self,
    ) -> eyre::Result<Vec<calimero_primitives::application::Application>> {
        let handle = self.store.handle();

        let mut iter = handle.iter::<calimero_store::key::ApplicationMeta>()?;

        let mut applications = vec![];

        for (id, app) in iter.entries() {
            let (id, app) = (id?, app?);

            applications.push(calimero_primitives::application::Application {
                id: id.application_id(),
                blob: app.blob.blob_id(),
                version: app.version.as_deref().map(str::parse).transpose()?,
                source: app.source.parse()?,
            })
        }

        Ok(applications)
    }

    pub fn is_application_installed(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
    ) -> eyre::Result<bool> {
        let handle = self.store.handle();

        let Some(application) =
            handle.get(&calimero_store::key::ApplicationMeta::new(*application_id))?
        else {
            return Ok(false);
        };

        if !handle.has(&application.blob)? {
            eyre::bail!("fatal: application points to danling blob");
        }

        Ok(true)
    }

    pub async fn load_application_blob(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
    ) -> eyre::Result<Option<Vec<u8>>> {
        let handle = self.store.handle();

        let Some(application) =
            handle.get(&calimero_store::key::ApplicationMeta::new(*application_id))?
        else {
            return Ok(None);
        };

        let Some(mut stream) = self.blob_manager.get(application.blob.blob_id()).await? else {
            eyre::bail!("fatal: application points to dangling blob");
        };

        // todo! we can preallocate the right capacity here
        // todo! once `blob_manager::get` -> Blob{size}:Stream
        let mut buf = vec![];

        // todo! guard against loading excessively large blobs into memory

        while let Some(chunk) = stream.try_next().await? {
            buf.extend_from_slice(&chunk);
        }

        Ok(Some(buf))
    }
}
