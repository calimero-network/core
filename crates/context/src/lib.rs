use std::collections::HashSet;
use std::io::Error as IoError;
use std::sync::Arc;

use calimero_blobstore::{Blob, BlobManager, Size};
use calimero_context_config::client::config::ContextConfigClientConfig;
use calimero_context_config::client::{ContextConfigClient, RelayOrNearTransport};
use calimero_context_config::repr::{Repr, ReprBytes, ReprTransmute};
use calimero_context_config::types::{
    Application as ApplicationConfig, ApplicationMetadata as ApplicationMetadataConfig,
    ApplicationSource as ApplicationSourceConfig,
};
use calimero_network::client::NetworkClient;
use calimero_network::types::IdentTopic;
use calimero_node_primitives::{ExecutionRequest, ServerSender};
use calimero_primitives::application::{Application, ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{
    Context, ContextConfigParams, ContextId, ContextInvitationPayload,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{
    ApplicationMeta as ApplicationMetaKey, BlobMeta as BlobMetaKey,
    ContextConfig as ContextConfigKey, ContextIdentity as ContextIdentityKey,
    ContextMeta as ContextMetaKey, ContextState as ContextStateKey, FromKeyParts, Key,
};
use calimero_store::layer::{ReadLayer, WriteLayer};
use calimero_store::types::{
    ApplicationMeta as ApplicationMetaValue, ContextConfig as ContextConfigValue,
    ContextIdentity as ContextIdentityValue, ContextMeta as ContextMetaValue,
};
use calimero_store::Store;
use camino::Utf8PathBuf;
use ed25519_dalek::ed25519::signature::SignerMut;
use ed25519_dalek::SigningKey;
use eyre::{bail, Result as EyreResult};
use futures_util::{AsyncRead, TryStreamExt};
use rand::rngs::StdRng;
use rand::SeedableRng;
use reqwest::{Client, Url};
use tokio::fs::File;
use tokio::sync::{oneshot, RwLock};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::{error, info};

pub mod config;

use config::ContextConfig;

#[derive(Clone, Debug)]
pub struct ContextManager {
    store: Store,
    client_config: ContextConfigClientConfig,
    config_client: ContextConfigClient<RelayOrNearTransport>,
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
        let client_config = config.client.clone();
        let config_client = ContextConfigClient::from_config(&client_config);

        let this = Self {
            store,
            client_config,
            config_client,
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

    #[must_use]
    pub fn new_identity(&self) -> PrivateKey {
        PrivateKey::random(&mut rand::thread_rng())
    }

    pub async fn create_context(
        &self,
        seed: Option<[u8; 32]>,
        application_id: ApplicationId,
        identity_secret: Option<PrivateKey>,
        initialization_params: Vec<u8>,
        result_sender: oneshot::Sender<EyreResult<(ContextId, PublicKey)>>,
    ) -> EyreResult<()> {
        let (context_secret, identity_secret) = {
            let mut rng = rand::thread_rng();

            #[expect(clippy::option_if_let_else, reason = "Clearer this way")]
            let context_secret = match seed {
                Some(seed) => PrivateKey::random(&mut StdRng::from_seed(seed)),
                None => PrivateKey::random(&mut rng),
            };

            let identity_secret = identity_secret.unwrap_or_else(|| self.new_identity());

            (context_secret, identity_secret)
        };

        let handle = self.store.handle();

        let context = {
            let context_id = ContextId::from(*context_secret.public_key());

            if handle.has(&ContextMetaKey::new(context_id))? {
                bail!("Context already exists on node.")
            }

            Context::new(context_id, application_id, Hash::default())
        };

        let Some(application) = self.get_application(&context.application_id)? else {
            bail!("Application is not installed on node.")
        };

        self.add_context(
            &context,
            identity_secret,
            ContextConfigParams {
                network_id: self.client_config.new.network.as_str().into(),
                contract_id: self.client_config.new.contract_id.as_str().into(),
            },
            true,
        )
        .await?;

        let (tx, rx) = oneshot::channel();

        let this = self.clone();
        let finalizer = async move {
            this.server_sender
                .send(ExecutionRequest::new(
                    context.id,
                    "init".to_owned(),
                    initialization_params,
                    identity_secret.public_key(),
                    tx,
                ))
                .await?;

            if let Some(return_value) = rx.await??.returns? {
                bail!(
                    "Unexpected return value from init method: {:?}",
                    return_value
                )
            }

            this.config_client
                .mutate(
                    this.client_config.new.network.as_str().into(),
                    this.client_config.new.contract_id.as_str().into(),
                    context.id.rt().expect("infallible conversion"),
                )
                .add_context(
                    context.id.rt().expect("infallible conversion"),
                    identity_secret
                        .public_key()
                        .rt()
                        .expect("infallible conversion"),
                    ApplicationConfig::new(
                        application.id.rt().expect("infallible conversion"),
                        application.blob.rt().expect("infallible conversion"),
                        application.size,
                        ApplicationSourceConfig(application.source.to_string().into()),
                        ApplicationMetadataConfig(Repr::new(application.metadata.into())),
                    ),
                )
                .send(|b| SigningKey::from_bytes(&context_secret).sign(b))
                .await?;

            Ok((context.id, identity_secret.public_key()))
        };

        let this = self.clone();
        let context_id = context.id;
        let _ignored = tokio::spawn(async move {
            let result = finalizer.await;

            if result.is_err() {
                if let Err(err) = this.delete_context(&context_id).await {
                    error!(%context_id, %err, "Failed to clean up context after failed creation");
                }
            }

            if let Err(err) = this.subscribe(&context.id).await {
                error!(%context_id, %err, "Failed to subscribe to context after creation");
            }

            let _ignored = result_sender.send(result);
        });

        Ok(())
    }

    async fn add_context(
        &self,
        context: &Context,
        identity_secret: PrivateKey,
        context_config: ContextConfigParams<'_>,
        is_new: bool,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();

        if is_new {
            handle.put(
                &ContextConfigKey::new(context.id),
                &ContextConfigValue::new(
                    context_config.network_id.into_owned().into_boxed_str(),
                    context_config.contract_id.into_owned().into_boxed_str(),
                ),
            )?;

            self.save_context(context)?;
        }

        handle.put(
            &ContextIdentityKey::new(context.id, identity_secret.public_key()),
            &ContextIdentityValue {
                private_key: Some(*identity_secret),
            },
        )?;

        Ok(())
    }

    pub fn save_context(&self, context: &Context) -> EyreResult<()> {
        let mut handle = self.store.handle();

        handle.put(
            &ContextMetaKey::new(context.id),
            &ContextMetaValue::new(
                ApplicationMetaKey::new(context.application_id),
                context.root_hash.into(),
            ),
        )?;

        Ok(())
    }

    pub async fn join_context(
        &self,
        identity_secret: PrivateKey,
        invitation_payload: ContextInvitationPayload,
    ) -> EyreResult<Option<(ContextId, PublicKey)>> {
        let (context_id, invitee_id, network_id, contract_id) = invitation_payload.parts()?;

        if identity_secret.public_key() != invitee_id {
            bail!("identity mismatch")
        }

        let mut handle = self.store.handle();

        let identity_key = ContextIdentityKey::new(context_id, invitee_id);

        if handle.has(&identity_key)? {
            return Ok(None);
        }

        let network_id = network_id.as_str();
        let contract_id = contract_id.as_str();

        let client = self
            .config_client
            .query(network_id.into(), contract_id.into());

        for (offset, length) in (0..).map(|i| (100_usize.saturating_mul(i), 100)) {
            let members = client
                .members(
                    context_id.rt().expect("infallible conversion"),
                    offset,
                    length,
                )
                .await?
                .parse()?;

            if members.is_empty() {
                break;
            }

            for member in members {
                let member = member.as_bytes().into();

                let key = ContextIdentityKey::new(context_id, member);

                if !handle.has(&key)? {
                    handle.put(&key, &ContextIdentityValue { private_key: None })?;
                }
            }
        }

        if !handle.has(&identity_key)? {
            bail!("unable to join context: not a member, invalid invitation?")
        }

        let response = client
            .application(context_id.rt().expect("infallible conversion"))
            .await?;

        let application = response.parse()?;

        let context = Context::new(
            context_id,
            application.id.as_bytes().into(),
            Hash::default(),
        );

        let context_exists = handle.has(&ContextMetaKey::new(context_id))?;

        if !self.is_application_installed(&context.application_id)? {
            let source = Url::parse(&application.source.0)?;

            let metadata = application.metadata.0.to_vec();

            let application_id = match source.scheme() {
                "http" | "https" => {
                    self.install_application_from_url(source, metadata, None)
                        .await?
                }
                _ => self.install_application(
                    application.blob.as_bytes().into(),
                    application.size,
                    &source.into(),
                    metadata,
                )?,
            };

            if application_id != context.application_id {
                bail!("application mismatch")
            }
        }

        self.add_context(
            &context,
            identity_secret,
            ContextConfigParams {
                network_id: network_id.into(),
                contract_id: contract_id.into(),
            },
            !context_exists,
        )
        .await?;

        self.subscribe(&context.id).await?;

        let _ = self.state.write().await.pending_catchup.insert(context_id);

        info!(%context_id, "Joined context with pending catchup");

        Ok(Some((context_id, invitee_id)))
    }

    #[expect(clippy::similar_names, reason = "Different enough")]
    pub async fn invite_to_context(
        &self,
        context_id: ContextId,
        inviter_id: PublicKey,
        invitee_id: PublicKey,
    ) -> EyreResult<Option<ContextInvitationPayload>> {
        let handle = self.store.handle();

        let Some(context_config) = handle.get(&ContextConfigKey::new(context_id))? else {
            return Ok(None);
        };

        let Some(ContextIdentityValue {
            private_key: Some(requester_secret),
        }) = handle.get(&ContextIdentityKey::new(context_id, inviter_id))?
        else {
            return Ok(None);
        };

        self.config_client
            .mutate(
                context_config.network.as_ref().into(),
                context_config.contract.as_ref().into(),
                inviter_id.rt().expect("infallible conversion"),
            )
            .add_members(
                context_id.rt().expect("infallible conversion"),
                &[invitee_id.rt().expect("infallible conversion")],
            )
            .send(|b| SigningKey::from_bytes(&requester_secret).sign(b))
            .await?;

        let invitation_payload = ContextInvitationPayload::new(
            context_id,
            invitee_id,
            context_config.network.into_string().into(),
            context_config.contract.into_string().into(),
        )?;

        Ok(Some(invitation_payload))
    }

    pub async fn is_context_pending_catchup(&self, context_id: &ContextId) -> bool {
        self.state.read().await.pending_catchup.contains(context_id)
    }

    pub async fn get_any_pending_sync_context(&self) -> Option<ContextId> {
        self.state
            .read()
            .await
            .pending_catchup
            .iter()
            .next()
            .copied()
    }

    pub async fn clear_context_pending_sync(&self, context_id: &ContextId) -> bool {
        self.state.write().await.pending_catchup.remove(context_id)
    }

    pub fn get_context(&self, context_id: &ContextId) -> EyreResult<Option<Context>> {
        let handle = self.store.handle();

        let key = ContextMetaKey::new(*context_id);

        let Some(ctx_meta) = handle.get(&key)? else {
            return Ok(None);
        };

        Ok(Some(Context::new(
            *context_id,
            ctx_meta.application.application_id(),
            ctx_meta.root_hash.into(),
        )))
    }

    pub async fn delete_context(&self, context_id: &ContextId) -> EyreResult<bool> {
        let mut handle = self.store.handle();

        let key = ContextMetaKey::new(*context_id);

        // todo! perhaps we shouldn't bother checking?
        if !handle.has(&key)? {
            return Ok(false);
        }

        handle.delete(&key)?;
        handle.delete(&ContextConfigKey::new(*context_id))?;

        self.delete_context_scoped::<ContextIdentityKey, 32>(context_id, [0; 32], None)?;
        self.delete_context_scoped::<ContextStateKey, 32>(context_id, [0; 32], None)?;

        self.unsubscribe(context_id).await?;

        Ok(true)
    }

    #[expect(clippy::unwrap_in_result, reason = "pre-validated")]
    fn delete_context_scoped<K, const N: usize>(
        &self,
        context_id: &ContextId,
        offset: [u8; N],
        end: Option<[u8; N]>,
    ) -> EyreResult<()>
    where
        K: FromKeyParts<Error: std::error::Error + Send + Sync>,
    {
        let expected_length = Key::<K::Components>::len();

        if context_id.len() + N != expected_length {
            bail!(
                "key length mismatch, expected: {}, got: {}",
                Key::<K::Components>::len(),
                N
            )
        }

        let mut keys = vec![];

        let mut key = context_id.to_vec();

        let end = end
            .map(|end| {
                key.extend_from_slice(&end);

                let end = Key::<K::Components>::try_from_slice(&key).expect("length pre-matched");

                K::try_from_parts(end)
            })
            .transpose()?;

        // fixme! store.handle() is prolematic here for lifetime reasons
        let mut store = self.store.clone();

        'outer: loop {
            key.truncate(context_id.len());
            key.extend_from_slice(&offset);

            let offset = Key::<K::Components>::try_from_slice(&key).expect("length pre-matched");

            let mut iter = store.iter()?;

            let first = iter.seek(K::try_from_parts(offset)?).transpose();

            if first.is_none() {
                break;
            }

            for k in first.into_iter().chain(iter.keys()) {
                let k = k?;

                let key = k.as_key();

                if let Some(end) = end {
                    if key == end.as_key() {
                        break 'outer;
                    }
                }

                if !key.as_bytes().starts_with(&**context_id) {
                    break 'outer;
                }

                keys.push(k);

                if keys.len() == 100 {
                    break;
                }
            }

            drop(iter);

            for k in keys.drain(..) {
                store.delete(&k)?;
            }
        }

        Ok(())
    }

    pub fn get_contexts_ids(&self, start: Option<ContextId>) -> EyreResult<Vec<ContextId>> {
        let handle = self.store.handle();

        let mut iter = handle.iter::<ContextMetaKey>()?;

        let start = start.and_then(|s| iter.seek(ContextMetaKey::new(s)).transpose());

        let mut ids = vec![];

        for key in start.into_iter().chain(iter.keys()) {
            ids.push(key?.context_id());
        }

        Ok(ids)
    }

    pub fn has_context_identity(
        &self,
        context_id: ContextId,
        public_key: PublicKey,
    ) -> EyreResult<bool> {
        let handle = self.store.handle();

        Ok(handle.has(&ContextIdentityKey::new(context_id, public_key))?)
    }

    fn get_context_identities(
        &self,
        context_id: ContextId,
        only_owned_identities: bool,
    ) -> EyreResult<Vec<PublicKey>> {
        let handle = self.store.handle();

        let mut iter = handle.iter::<ContextIdentityKey>()?;

        let first = iter
            .seek(ContextIdentityKey::new(context_id, [0; 32].into()))
            .transpose()
            .map(|k| (k, iter.read()));

        let mut ids = Vec::new();

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

        // todo! Iter shouldn't behave like DBIter, first next should return sought element
        let start =
            start.and_then(|s| Some((iter.seek(ContextMetaKey::new(s)).transpose()?, iter.read())));

        for (k, v) in start.into_iter().chain(iter.entries()) {
            let (k, v) = (k?, v?);
            contexts.push(Context::new(
                k.context_id(),
                v.application.application_id(),
                v.root_hash.into(),
            ));
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

    #[expect(clippy::similar_names, reason = "Different enough")]
    pub async fn add_blob<S: AsyncRead>(
        &self,
        stream: S,
        expected_size: Option<u64>,
        expected_hash: Option<Hash>,
    ) -> EyreResult<(BlobId, u64)> {
        let (blob_id, hash, size) = self
            .blob_manager
            .put_sized(expected_size.map(Size::Exact), stream)
            .await?;

        if matches!(expected_hash, Some(expected_hash) if hash != expected_hash) {
            bail!("fatal: blob hash mismatch");
        }

        if matches!(expected_size, Some(expected_size) if size != expected_size) {
            bail!("fatal: blob size mismatch");
        }

        Ok((blob_id, size))
    }

    pub fn has_blob_available(&self, blob_id: BlobId) -> EyreResult<bool> {
        Ok(self.blob_manager.has(blob_id)?)
    }

    // vv~ these would be more appropriate in an ApplicationManager

    fn install_application(
        &self,
        blob_id: BlobId,
        size: u64,
        source: &ApplicationSource,
        metadata: Vec<u8>,
    ) -> EyreResult<ApplicationId> {
        let application = ApplicationMetaValue::new(
            BlobMetaKey::new(blob_id),
            size,
            source.to_string().into_boxed_str(),
            metadata.into_boxed_slice(),
        );

        let application_id = ApplicationId::from(*Hash::hash_borsh(&application)?);

        let mut handle = self.store.handle();

        handle.put(&ApplicationMetaKey::new(application_id), &application)?;

        Ok(application_id)
    }

    pub fn uninstall_application(&self, application_id: ApplicationId) -> EyreResult<()> {
        let application_meta_key = ApplicationMetaKey::new(application_id);
        let mut handle = self.store.handle();
        handle.delete(&application_meta_key)?;
        Ok(())
    }

    pub async fn install_application_from_path(
        &self,
        path: Utf8PathBuf,
        metadata: Vec<u8>,
    ) -> EyreResult<ApplicationId> {
        let path = path.canonicalize_utf8()?;

        let file = File::open(&path).await?;

        let expected_size = file.metadata().await?.len();

        let (blob_id, size) = self
            .add_blob(file.compat(), Some(expected_size), None)
            .await?;

        let Ok(uri) = Url::from_file_path(path) else {
            bail!("non-absolute path")
        };

        self.install_application(blob_id, size, &(uri.as_str().parse()?), metadata)
    }

    #[expect(clippy::similar_names, reason = "Different enough")]
    pub async fn install_application_from_url(
        &self,
        url: Url,
        metadata: Vec<u8>,
        expected_hash: Option<Hash>,
    ) -> EyreResult<ApplicationId> {
        let uri = url.as_str().parse()?;

        let response = Client::new().get(url).send().await?;

        let expected_size = response.content_length();

        let (blob_id, size) = self
            .add_blob(
                response
                    .bytes_stream()
                    .map_err(IoError::other)
                    .into_async_read(),
                expected_size,
                expected_hash,
            )
            .await?;

        self.install_application(blob_id, size, &uri, metadata)
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
                app.size,
                app.source.parse()?,
                app.metadata.to_vec(),
            ));
        }

        Ok(applications)
    }

    pub fn is_application_installed(&self, application_id: &ApplicationId) -> EyreResult<bool> {
        let handle = self.store.handle();

        if let Some(application) = handle.get(&ApplicationMetaKey::new(*application_id))? {
            if self.has_blob_available(application.blob.blob_id())? {
                return Ok(true);
            }
        }

        Ok(false)
    }

    // todo! add process that polls updates
    // pub async fn get_latest_application(&self, context_id: ContextId) -> EyreResult<Application> {
    //     let client = self.config_client.query(
    //         self.client_config.new.network.as_str().into(),
    //         self.client_config.new.contract_id.as_str().into(),
    //     );

    //     let response = client
    //         .application(context_id.rt().expect("infallible conversion"))
    //         .await?;

    //     let application = response.parse()?;

    //     Ok(Application::new(
    //         application.id.as_bytes().into(),
    //         application.blob.as_bytes().into(),
    //         application.size,
    //         ApplicationSource::from_str(&application.source.0)?,
    //         application.metadata.0.into_inner().into_owned(),
    //     ))
    // }

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
            application.size,
            application.source.parse()?,
            application.metadata.to_vec(),
        )))
    }

    pub async fn load_application_blob(
        &self,
        application_id: &ApplicationId,
    ) -> EyreResult<Option<Vec<u8>>> {
        let Some(mut stream) = self.get_application_blob(application_id)? else {
            return Ok(None);
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

    pub fn get_application_blob(&self, application_id: &ApplicationId) -> EyreResult<Option<Blob>> {
        let handle = self.store.handle();

        let Some(application) = handle.get(&ApplicationMetaKey::new(*application_id))? else {
            return Ok(None);
        };

        let Some(stream) = self.blob_manager.get(application.blob.blob_id())? else {
            bail!("fatal: application points to dangling blob");
        };

        Ok(Some(stream))
    }

    pub fn is_application_blob_installed(&self, blob_id: BlobId) -> EyreResult<bool> {
        Ok(self.blob_manager.has(blob_id)?)
    }
}
