use std::collections::HashSet;
use std::io::Error as IoError;
use std::str::FromStr;
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
use calimero_node_primitives::{ExecutionRequest, Finality, ServerSender};
use calimero_primitives::application::{Application, ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{Context, ContextId, ContextInvitationPayload};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{
    ApplicationMeta as ApplicationMetaKey, BlobMeta as BlobMetaKey,
    ContextConfig as ContextConfigKey, ContextIdentity as ContextIdentityKey,
    ContextMeta as ContextMetaKey, ContextState as ContextStateKey,
    ContextTransaction as ContextTransactionKey,
};
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
use tracing::info;

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
    ) -> EyreResult<(ContextId, PublicKey)> {
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

        self.config_client
            .mutate(
                self.client_config.new.network.as_str().into(),
                self.client_config.new.contract_id.as_str().into(),
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

        self.add_context(&context, identity_secret, true).await?;

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

        Ok((context.id, identity_secret.public_key()))
    }

    async fn add_context(
        &self,
        context: &Context,
        identity_secret: PrivateKey,
        is_new: bool,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();

        if is_new {
            handle.put(
                &ContextConfigKey::new(context.id),
                &ContextConfigValue::new(
                    self.client_config.new.network.as_str().into(),
                    self.client_config.new.contract_id.as_str().into(),
                ),
            )?;

            handle.put(
                &ContextMetaKey::new(context.id),
                &ContextMetaValue::new(
                    ApplicationMetaKey::new(context.application_id),
                    context.root_hash.into(),
                ),
            )?;

            self.subscribe(&context.id).await?;
        }

        handle.put(
            &ContextIdentityKey::new(context.id, identity_secret.public_key()),
            &ContextIdentityValue {
                private_key: Some(*identity_secret),
            },
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
            bail!("unable to join context: not a member, ask for an invite")
        };

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
                "http" | "https" => self.install_application_from_url(source, metadata).await?,
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

        self.add_context(&context, identity_secret, !context_exists)
            .await?;

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

        Ok(Some(Context::new(
            *context_id,
            ctx_meta.application.application_id(),
            ctx_meta.root_hash.into(),
        )))
    }

    pub async fn delete_context(&self, context_id: &ContextId) -> EyreResult<bool> {
        let mut handle = self.store.handle();

        let key = ContextMetaKey::new(*context_id);

        if !handle.has(&key)? {
            return Ok(false);
        }

        handle.delete(&key)?;

        handle.delete(&ContextConfigKey::new(*context_id))?;

        {
            let mut keys = vec![];

            let mut iter = handle.iter::<ContextIdentityKey>()?;

            let first = iter
                .seek(ContextIdentityKey::new(*context_id, [0; 32].into()))
                .transpose();

            for k in first.into_iter().chain(iter.keys()) {
                let k = k?;

                if k.context_id() != *context_id {
                    break;
                }

                keys.push(k);
            }

            drop(iter);

            for k in keys {
                handle.delete(&k)?;
            }
        }

        {
            let mut keys = vec![];

            let mut iter = handle.iter::<ContextStateKey>()?;

            let first = iter
                .seek(ContextStateKey::new(*context_id, [0; 32]))
                .transpose();

            for k in first.into_iter().chain(iter.keys()) {
                let k = k?;

                if k.context_id() != *context_id {
                    break;
                }

                keys.push(k);
            }

            drop(iter);

            for k in keys {
                handle.delete(&k)?;
            }
        }

        {
            let mut keys = vec![];

            let mut iter = handle.iter::<ContextTransactionKey>()?;

            let first = iter
                .seek(ContextTransactionKey::new(*context_id, [0; 32]))
                .transpose();

            for k in first.into_iter().chain(iter.keys()) {
                let k = k?;

                if k.context_id() != *context_id {
                    break;
                }

                keys.push(k);
            }

            drop(iter);

            for k in keys {
                handle.delete(&k)?;
            }
        }

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
                let value = iter.read()?;

                contexts.push(Context::new(
                    key.context_id(),
                    value.application.application_id(),
                    value.root_hash.into(),
                ));
            }
        }

        for (k, v) in iter.entries() {
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

        let meta = file.metadata().await?;

        let expected_size = meta.len();

        let (blob_id, size) = self
            .blob_manager
            .put_sized(Some(Size::Exact(expected_size)), file.compat())
            .await?;

        if size != expected_size {
            bail!("fatal: file size mismatch")
        }

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
        // hash: Hash,
        // todo! BlobMgr should return hash of content
    ) -> EyreResult<ApplicationId> {
        let uri = url.as_str().parse()?;

        let response = Client::new().get(url).send().await?;

        let expected_size = response.content_length();

        let (blob_id, size) = self
            .blob_manager
            .put_sized(
                expected_size.map(Size::Exact),
                response
                    .bytes_stream()
                    .map_err(IoError::other)
                    .into_async_read(),
            )
            .await?;

        if matches!(expected_size, Some(expected_size) if size != expected_size) {
            bail!("fatal: content size mismatch")
        }

        // todo! if blob hash doesn't match, remove it

        self.install_application(blob_id, size, &uri, metadata)
    }

    #[expect(clippy::similar_names, reason = "Different enough")]
    pub async fn install_application_from_stream<AR>(
        &self,
        expected_size: u64,
        stream: AR,
        source: &ApplicationSource,
        metadata: Vec<u8>,
        // hash: Hash,
        // todo! BlobMgr should return hash of content
    ) -> EyreResult<ApplicationId>
    where
        AR: AsyncRead,
    {
        let (blob_id, size) = self
            .blob_manager
            .put_sized(Some(Size::Exact(expected_size)), stream)
            .await?;

        if size != expected_size {
            bail!("fatal: content size mismatch: {} {}", size, expected_size)
        }

        // todo! if blob hash doesn't match, remove it

        self.install_application(blob_id, size, source, metadata)
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
            if handle.has(&application.blob)? {
                return Ok(true);
            }
        };

        Ok(false)
    }

    pub async fn get_latest_application(&self, context_id: ContextId) -> EyreResult<Application> {
        let client = self.config_client.query(
            self.client_config.new.network.as_str().into(),
            self.client_config.new.contract_id.as_str().into(),
        );

        let response = client
            .application(context_id.rt().expect("infallible conversion"))
            .await?;

        let application = response.parse()?;

        Ok(Application::new(
            application.id.as_bytes().into(),
            application.blob.as_bytes().into(),
            application.size,
            ApplicationSource::from_str(&application.source.0)?,
            application.metadata.0.into_inner().into_owned(),
        ))
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
