use std::collections::HashSet;
use std::io::Error as IoError;
use std::sync::Arc;

use calimero_blobstore::BlobManager;
use calimero_context_config::client::{ContextConfigClient, RelayOrNearTransport};
use calimero_network::client::NetworkClient;
use calimero_network::types::IdentTopic;
use calimero_node_primitives::{ExecutionRequest, Finality, ServerSender};
use calimero_primitives::application::{Application, ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{
    ApplicationMeta as ApplicationMetaKey, BlobMeta as BlobMetaKey,
    ContextIdentity as ContextIdentityKey, ContextMeta as ContextMetaKey,
};
use calimero_store::types::{
    ApplicationMeta, ContextIdentity as ContextIdentityValue, ContextMeta,
};
use calimero_store::Store;
use camino::Utf8PathBuf;
use eyre::{bail, Result as EyreResult};
use futures_util::TryStreamExt;
use rand::rngs::StdRng;
use rand::SeedableRng;
use reqwest::{Client, Url};
use semver::Version;
use tokio::fs::File;
use tokio::sync::{oneshot, RwLock};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::info;

pub mod config;

use config::ContextConfig;

#[derive(Clone, Debug)]
pub struct ContextManager {
    client_config: ContextConfigClient<RelayOrNearTransport>,
    store: Store,
    blob_manager: BlobManager,
    network_client: NetworkClient,
    server_sender: ServerSender,
    state: Arc<RwLock<State>>,
}

#[derive(Debug, Default)]
struct State {
    pending_catchup: HashSet<ContextId>,
}

impl ContextManager {
    pub async fn start(
        config: &ContextConfig,
        store: Store,
        blob_manager: BlobManager,
        server_sender: ServerSender,
        network_client: NetworkClient,
    ) -> EyreResult<Self> {
        let this = Self {
            client_config: ContextConfigClient::from_config(&config.client),
            store,
            blob_manager,
            network_client,
            server_sender,
            state: Arc::default(),
        };

        this.boot().await?;

        Ok(this)
    }

    async fn boot(&self) -> EyreResult<()> {
        let handle = self.store.handle();

        let mut iter = handle.iter::<ContextMetaKey>()?;

        for key in iter.keys() {
            let key = key?;

            let _ = self
                .state
                .write()
                .await
                .pending_catchup
                .insert(key.context_id());

            self.subscribe(&key.context_id()).await?;
        }

        Ok(())
    }

    async fn subscribe(&self, context_id: &ContextId) -> EyreResult<()> {
        drop(
            self.network_client
                .subscribe(IdentTopic::new(context_id))
                .await?,
        );

        info!(%context_id, "Subscribed to context");

        Ok(())
    }

    async fn unsubscribe(&self, context_id: &ContextId) -> EyreResult<()> {
        drop(
            self.network_client
                .unsubscribe(IdentTopic::new(context_id))
                .await?,
        );

        info!(%context_id, "Unsubscribed from context");

        Ok(())
    }

    pub async fn create_context(
        &self,
        seed: Option<[u8; 32]>,
        application_id: ApplicationId,
        identity_secret: Option<PrivateKey>,
        initialization_params: Vec<u8>,
    ) -> EyreResult<(ContextId, PublicKey)> {
        let (context_secret, identity_secret) = {
            let mut rng = rand::thread_rng();

            let context_secret = match seed {
                Some(seed) => PrivateKey::random(&mut StdRng::from_seed(seed)),
                None => PrivateKey::random(&mut rng),
            };

            let identity_secret =
                identity_secret.map_or_else(|| PrivateKey::random(&mut rng), PrivateKey::from);

            (context_secret, identity_secret)
        };

        let context_id = ContextId::from(*context_secret.public_key());

        if self.get_context(&context_id)?.is_some() {
            bail!("Context already exists on node.")
        }

        let context = Context {
            id: context_id,
            application_id,
            last_transaction_hash: Default::default(),
        };

        self.add_context(&context)?;

        let (tx, _) = oneshot::channel();

        self.server_sender
            .send(ExecutionRequest::new(
                context.id,
                "init".to_owned(),
                initialization_params,
                identity_secret.public_key(),
                tx,
                Some(Finality::Local),
            ))
            .await?;

        let mut handle = self.store.handle();

        handle.put(
            &ContextIdentityKey::new(context.id, identity_secret.public_key()),
            &ContextIdentityValue {
                private_key: Some(*identity_secret),
            },
        )?;

        self.subscribe(&context.id).await?;

        Ok((context.id, identity_secret.public_key()))
    }

    pub fn add_context(&self, context: &Context) -> EyreResult<()> {
        if !self.is_application_installed(&context.application_id)? {
            bail!("Application is not installed on node.")
        }

        let mut handle = self.store.handle();

        handle.put(
            &ContextMetaKey::new(context.id),
            &ContextMeta::new(
                ApplicationMetaKey::new(context.application_id),
                context.last_transaction_hash.into(),
            ),
        )?;

        Ok(())
    }

    pub async fn join_context(
        &self,
        context_id: ContextId,
        identity_secret: Option<PrivateKey>,
    ) -> EyreResult<Option<PublicKey>> {
        if self
            .state
            .read()
            .await
            .pending_catchup
            .contains(&context_id)
        {
            return Ok(None);
        }

        let identity_secret =
            identity_secret.unwrap_or_else(|| PrivateKey::random(&mut rand::thread_rng()));

        let mut handle = self.store.handle();

        handle.put(
            &ContextIdentityKey::new(context_id, identity_secret.public_key()),
            &ContextIdentityValue {
                private_key: Some(*identity_secret),
            },
        )?;

        let _ = self.state.write().await.pending_catchup.insert(context_id);

        self.subscribe(&context_id).await?;

        info!(%context_id, "Joined context with pending catchup");

        Ok(Some(identity_secret.public_key()))
    }

    pub async fn is_context_pending_catchup(&self, context_id: &ContextId) -> bool {
        self.state.read().await.pending_catchup.contains(context_id)
    }

    pub async fn get_any_pending_catchup_context(&self) -> Option<ContextId> {
        self.state
            .read()
            .await
            .pending_catchup
            .iter()
            .next()
            .copied()
    }

    pub async fn clear_context_pending_catchup(&self, context_id: &ContextId) -> bool {
        self.state.write().await.pending_catchup.remove(context_id)
    }

    pub fn get_context(&self, context_id: &ContextId) -> EyreResult<Option<Context>> {
        let handle = self.store.handle();

        let key = ContextMetaKey::new(*context_id);

        let Some(ctx_meta) = handle.get(&key)? else {
            return Ok(None);
        };

        Ok(Some(Context {
            id: *context_id,
            application_id: ctx_meta.application.application_id(),
            last_transaction_hash: ctx_meta.last_transaction_hash.into(),
        }))
    }

    pub async fn delete_context(&self, context_id: &ContextId) -> EyreResult<bool> {
        let mut handle = self.store.handle();

        let key = ContextMetaKey::new(*context_id);

        if !handle.has(&key)? {
            return Ok(false);
        }

        handle.delete(&key)?;

        self.unsubscribe(context_id).await?;

        Ok(true)
    }

    pub fn get_contexts_ids(&self, start: Option<ContextId>) -> EyreResult<Vec<ContextId>> {
        let handle = self.store.handle();

        let mut iter = handle.iter::<ContextMetaKey>()?;

        let mut ids = vec![];

        if let Some(start) = start {
            if let Some(key) = iter.seek(ContextMetaKey::new(start))? {
                ids.push(key.context_id());
            }
        }

        for key in iter.keys() {
            ids.push(key?.context_id());
        }

        Ok(ids)
    }

    fn get_context_identities(
        &self,
        context_id: ContextId,
        only_owned_identities: bool,
    ) -> EyreResult<Vec<PublicKey>> {
        let handle = self.store.handle();

        let mut iter = handle.iter::<ContextIdentityKey>()?;
        let mut ids = Vec::<PublicKey>::new();

        let first = 'first: {
            let Some(k) = iter
                .seek(ContextIdentityKey::new(context_id, [0; 32].into()))
                .transpose()
            else {
                break 'first None;
            };

            Some((k, iter.read()))
        };

        for (k, v) in first.into_iter().chain(iter.entries()) {
            let (k, v) = (k?, v?);

            if k.context_id() != context_id {
                break;
            }

            if !only_owned_identities || v.private_key.is_some() {
                ids.push(k.public_key());
            }
        }
        Ok(ids)
    }

    pub fn get_context_members_identities(
        &self,
        context_id: ContextId,
    ) -> EyreResult<Vec<PublicKey>> {
        self.get_context_identities(context_id, false)
    }

    // Iterate over all identities in a context (from members and mine)
    // and return only public key of identities which contains private key (in value)
    // If there is private key then it means that identity is mine.
    pub fn get_context_owned_identities(
        &self,
        context_id: ContextId,
    ) -> EyreResult<Vec<PublicKey>> {
        self.get_context_identities(context_id, true)
    }

    pub fn get_contexts(&self, start: Option<ContextId>) -> EyreResult<Vec<Context>> {
        let handle = self.store.handle();

        let mut iter = handle.iter::<ContextMetaKey>()?;

        let mut contexts = vec![];

        if let Some(start) = start {
            // todo! Iter shouldn't behave like DBIter, first next should return sought element
            if let Some(key) = iter.seek(ContextMetaKey::new(start))? {
                let value: ContextMeta = iter.read()?;

                contexts.push(Context {
                    id: key.context_id(),
                    application_id: value.application.application_id(),
                    last_transaction_hash: value.last_transaction_hash.into(),
                });
            }
        }

        for (k, v) in iter.entries() {
            let (k, v) = (k?, v?);
            contexts.push(Context {
                id: k.context_id(),
                application_id: v.application.application_id(),
                last_transaction_hash: v.last_transaction_hash.into(),
            });
        }

        Ok(contexts)
    }

    pub fn update_application_id(
        &self,
        context_id: ContextId,
        application_id: ApplicationId,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();

        let key = ContextMetaKey::new(context_id);

        let Some(mut value) = handle.get(&key)? else {
            bail!("Context not found")
        };

        value.application = ApplicationMetaKey::new(application_id);

        handle.put(&key, &value)?;

        Ok(())
    }

    // vv~ these would be more appropriate in an ApplicationManager

    fn install_application(
        &self,
        blob_id: BlobId,
        source: &ApplicationSource,
        version: Option<Version>,
        metadata: Vec<u8>,
    ) -> EyreResult<ApplicationId> {
        let application = ApplicationMeta::new(
            BlobMetaKey::new(blob_id),
            version.map(|v| v.to_string().into_boxed_str()),
            source.to_string().into_boxed_str(),
            metadata.into_boxed_slice(),
        );

        let application_id = ApplicationId::from(*Hash::hash_borsh(&application)?);

        let mut handle = self.store.handle();

        handle.put(&ApplicationMetaKey::new(application_id), &application)?;

        Ok(application_id)
    }

    pub async fn install_application_from_path(
        &self,
        path: Utf8PathBuf,
        version: Option<Version>,
        metadata: Vec<u8>,
    ) -> EyreResult<ApplicationId> {
        let file = File::open(&path).await?;

        let meta = file.metadata().await?;

        let blob_id = self
            .blob_manager
            .put_sized(
                Some(calimero_blobstore::Size::Exact(meta.len())),
                file.compat(),
            )
            .await?;

        let Ok(uri) = Url::from_file_path(path) else {
            bail!("non-absolute path")
        };

        self.install_application(blob_id, &(uri.as_str().parse()?), version, metadata)
    }

    #[allow(clippy::similar_names)]
    pub async fn install_application_from_url(
        &self,
        url: Url,
        version: Option<Version>,
        metadata: Vec<u8>,
        // hash: Hash,
        // todo! BlobMgr should return hash of content
    ) -> EyreResult<ApplicationId> {
        let uri = url.as_str().parse()?;

        let response = Client::new().get(url).send().await?;

        let blob_id = self
            .blob_manager
            .put_sized(
                response
                    .content_length()
                    .map(calimero_blobstore::Size::Exact),
                response
                    .bytes_stream()
                    .map_err(IoError::other)
                    .into_async_read(),
            )
            .await?;

        // todo! if blob hash doesn't match, remove it

        self.install_application(blob_id, &uri, version, metadata)
    }

    pub fn list_installed_applications(&self) -> EyreResult<Vec<Application>> {
        let handle = self.store.handle();

        let mut iter = handle.iter::<ApplicationMetaKey>()?;

        let mut applications = vec![];

        for (id, app) in iter.entries() {
            let (id, app) = (id?, app?);
            applications.push(Application::new(
                id.application_id(),
                app.blob.blob_id(),
                app.version.as_deref().map(str::parse).transpose()?,
                app.source.parse()?,
                app.metadata.to_vec(),
            ));
        }

        Ok(applications)
    }

    pub fn is_application_installed(&self, application_id: &ApplicationId) -> EyreResult<bool> {
        let handle = self.store.handle();

        let Some(application) = handle.get(&ApplicationMetaKey::new(*application_id))? else {
            return Ok(false);
        };

        if !handle.has(&application.blob)? {
            bail!(
                "fatal: application `{}` points to danling blob `{}`",
                application_id,
                application.blob.blob_id()
            );
        }

        Ok(true)
    }

    pub fn get_application(
        &self,
        application_id: &ApplicationId,
    ) -> EyreResult<Option<Application>> {
        let handle = self.store.handle();

        let Some(application) = handle.get(&ApplicationMetaKey::new(*application_id))? else {
            return Ok(None);
        };

        Ok(Some(Application::new(
            *application_id,
            application.blob.blob_id(),
            application.version.as_deref().map(str::parse).transpose()?,
            application.source.parse()?,
            application.metadata.to_vec(),
        )))
    }

    pub async fn load_application_blob(
        &self,
        application_id: &ApplicationId,
    ) -> EyreResult<Option<Vec<u8>>> {
        let handle = self.store.handle();

        let Some(application) = handle.get(&ApplicationMetaKey::new(*application_id))? else {
            return Ok(None);
        };

        let Some(mut stream) = self.blob_manager.get(application.blob.blob_id())? else {
            bail!("fatal: application points to dangling blob");
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
