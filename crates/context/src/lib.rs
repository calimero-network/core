#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]

use core::error::Error;
use std::collections::HashSet;
use std::io::Error as IoError;
use std::str::FromStr;
use std::sync::Arc;

use calimero_blobstore::{Blob, BlobManager, Size};
use calimero_context_config::client::config::ClientConfig;
use calimero_context_config::client::env::config::ContextConfig as ContextConfigEnv;
use calimero_context_config::client::env::proxy::ContextProxy;
use calimero_context_config::client::utils::humanize_iter;
use calimero_context_config::client::{AnyTransport, Client as ExternalClient};
use calimero_context_config::repr::{Repr, ReprBytes, ReprTransmute};
use calimero_context_config::types::{
    Application as ApplicationConfig, ApplicationMetadata as ApplicationMetadataConfig,
    ApplicationSource as ApplicationSourceConfig, ContextIdentity, ContextStorageEntry, ProposalId,
};
use calimero_context_config::{Proposal, ProposalAction, ProposalWithApprovals};
use calimero_network::client::NetworkClient;
use calimero_network::types::IdentTopic;
use calimero_node_primitives::{ExecutionRequest, ServerSender};
use calimero_primitives::alias::Alias;
use calimero_primitives::application::{Application, ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{
    Context, ContextConfigParams, ContextId, ContextInvitationPayload,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{
    Alias as AliasKey, Aliasable, ApplicationMeta as ApplicationMetaKey, BlobMeta as BlobMetaKey,
    ContextConfig as ContextConfigKey, ContextIdentity as ContextIdentityKey,
    ContextMeta as ContextMetaKey, ContextState as ContextStateKey, FromKeyParts, Key,
    StoreScopeCompat,
};
use calimero_store::layer::{ReadLayer, WriteLayer};
use calimero_store::types::{
    ApplicationMeta as ApplicationMetaValue, ContextConfig as ContextConfigValue,
    ContextIdentity as ContextIdentityValue, ContextMeta as ContextMetaValue,
};
use calimero_store::Store;
use camino::Utf8PathBuf;
use eyre::{bail, OptionExt, Result as EyreResult};
use futures_util::{AsyncRead, TryStreamExt};
use rand::rngs::StdRng;
use rand::seq::IteratorRandom;
use rand::SeedableRng;
use reqwest::{Client as ReqClient, Url};
use tokio::fs::File;
use tokio::sync::{oneshot, RwLock};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::{error, info};

pub mod config;

use config::ContextConfig;

#[derive(Clone, Debug)]
pub struct ContextManager {
    store: Store,
    client_config: ClientConfig,
    config_client: ExternalClient<AnyTransport>,
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
        let config_client = ExternalClient::from_config(&client_config);

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

            let _ignored = self
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
    pub fn new_private_key(&self) -> PrivateKey {
        PrivateKey::random(&mut rand::thread_rng())
    }

    pub fn create_context(
        &self,
        protocol: &str,
        seed: Option<[u8; 32]>,
        application_id: ApplicationId,
        identity_secret: Option<PrivateKey>,
        initialization_params: Vec<u8>,
        result_sender: oneshot::Sender<EyreResult<(ContextId, PublicKey)>>,
    ) -> EyreResult<()> {
        let Some(config) = self.client_config.params.get(protocol).cloned() else {
            eyre::bail!(
                "unsupported protocol: {}, expected one of `{}`",
                protocol,
                humanize_iter(self.client_config.params.keys())
            );
        };

        let (context_secret, identity_secret) = {
            let mut rng = rand::thread_rng();

            #[expect(clippy::option_if_let_else, reason = "Clearer this way")]
            let context_secret = match seed {
                Some(seed) => PrivateKey::random(&mut StdRng::from_seed(seed)),
                None => PrivateKey::random(&mut rng),
            };

            let identity_secret = identity_secret.unwrap_or_else(|| self.new_private_key());

            (context_secret, identity_secret)
        };

        let mut handle = self.store.handle();

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
            Some(ContextConfigParams {
                protocol: config.protocol.as_str().into(),
                network_id: config.network.as_str().into(),
                contract_id: config.contract_id.as_str().into(),
                proxy_contract: "".into(),
                application_revision: 0,
                members_revision: 0,
            }),
        )?;

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
                .mutate::<ContextConfigEnv>(
                    config.protocol.as_str().into(),
                    config.network.as_str().into(),
                    config.contract_id.as_str().into(),
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
                .send(*context_secret, 0)
                .await?;

            let proxy_contract = this
                .config_client
                .query::<ContextConfigEnv>(
                    config.protocol.as_str().into(),
                    config.network.as_str().into(),
                    config.contract_id.as_str().into(),
                )
                .get_proxy_contract(context.id.rt().expect("infallible conversion"))
                .await?;

            let key = ContextConfigKey::new(context.id);
            let mut config = handle.get(&key)?.ok_or_eyre("expected config to exist")?;
            config.proxy_contract = proxy_contract.into();
            handle.put(&key, &config)?;

            Ok((context.id, identity_secret.public_key()))
        };

        let context_id = context.id;
        let this = self.clone();
        let _ignored = tokio::spawn(async move {
            let result = finalizer.await;

            if result.is_err() {
                if let Err(err) = this.delete_context(&context_id).await {
                    error!(%context_id, %err, "Failed to clean up context after failed creation");
                }
            } else {
                if let Err(err) = this.subscribe(&context.id).await {
                    error!(%context_id, %err, "Failed to subscribe to context after creation");
                }
            }

            let _ignored = result_sender.send(result);
        });

        Ok(())
    }

    fn add_context(
        &self,
        context: &Context,
        identity_secret: PrivateKey,
        context_config: Option<ContextConfigParams<'_>>,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();

        if let Some(context_config) = context_config {
            handle.put(
                &ContextConfigKey::new(context.id),
                &ContextConfigValue::new(
                    context_config.protocol.into_owned().into_boxed_str(),
                    context_config.network_id.into_owned().into_boxed_str(),
                    context_config.contract_id.into_owned().into_boxed_str(),
                    context_config.proxy_contract.into_owned().into_boxed_str(),
                    context_config.application_revision,
                    context_config.members_revision,
                ),
            )?;

            self.save_context(context)?;
        }

        handle.put(
            &ContextIdentityKey::new(context.id, identity_secret.public_key()),
            &ContextIdentityValue {
                private_key: Some(*identity_secret),
                sender_key: Some(*self.new_private_key()),
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
        let (context_id, invitee_id, protocol, network_id, contract_id) =
            invitation_payload.parts()?;

        if identity_secret.public_key() != invitee_id {
            bail!("identity mismatch")
        }

        let handle = self.store.handle();

        let identity_key = ContextIdentityKey::new(context_id, invitee_id);

        if handle.has(&identity_key)? {
            return Ok(None);
        }

        let context_exists = handle.has(&ContextMetaKey::new(context_id))?;
        let mut config = if !context_exists {
            let proxy_contract = self
                .config_client
                .query::<ContextConfigEnv>(
                    protocol.as_str().into(),
                    network_id.as_str().into(),
                    contract_id.as_str().into(),
                )
                .get_proxy_contract(context_id.rt().expect("infallible conversion"))
                .await?;

            Some(ContextConfigParams {
                protocol: protocol.into(),
                network_id: network_id.into(),
                contract_id: contract_id.into(),
                proxy_contract: proxy_contract.into(),
                application_revision: 0,
                members_revision: 0,
            })
        } else {
            None
        };

        let context = self
            .internal_sync_context_config(context_id, config.as_mut())
            .await?;

        if !handle.has(&identity_key)? {
            bail!("unable to join context: not a member, invalid invitation?")
        }

        self.add_context(&context, identity_secret, config)?;
        self.subscribe(&context.id).await?;

        let _ignored = self.state.write().await.pending_catchup.insert(context_id);

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
            ..
        }) = handle.get(&ContextIdentityKey::new(context_id, inviter_id))?
        else {
            return Ok(None);
        };

        let nonce = self
            .config_client
            .query::<ContextConfigEnv>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.contract.as_ref().into(),
            )
            .fetch_nonce(
                context_id.rt().expect("infallible conversion"),
                inviter_id.rt().expect("infallible conversion"),
            )
            .await?
            .ok_or_eyre("The inviter doesen't exist")?;

        self.config_client
            .mutate::<ContextConfigEnv>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.contract.as_ref().into(),
            )
            .add_members(
                context_id.rt().expect("infallible conversion"),
                &[invitee_id.rt().expect("infallible conversion")],
            )
            .send(requester_secret, nonce)
            .await?;

        let invitation_payload = ContextInvitationPayload::new(
            context_id,
            invitee_id,
            context_config.protocol.into_string().into(),
            context_config.network.into_string().into(),
            context_config.contract.into_string().into(),
        )?;

        Ok(Some(invitation_payload))
    }

    pub async fn sync_context_config(&self, context_id: ContextId) -> EyreResult<Context> {
        self.internal_sync_context_config(context_id, None).await
    }

    async fn internal_sync_context_config(
        &self,
        context_id: ContextId,
        config: Option<&mut ContextConfigParams<'_>>,
    ) -> EyreResult<Context> {
        let mut handle = self.store.handle();

        let context = handle.get(&ContextMetaKey::new(context_id))?;

        let mut alt_config = config.as_ref().map_or_else(
            || {
                let Some(config) = handle.get(&ContextConfigKey::new(context_id))? else {
                    eyre::bail!("Context config not found")
                };

                Ok(Some(ContextConfigParams {
                    protocol: config.protocol.into_string().into(),
                    network_id: config.network.into_string().into(),
                    contract_id: config.contract.into_string().into(),
                    proxy_contract: config.proxy_contract.into_string().into(),
                    application_revision: config.application_revision,
                    members_revision: config.members_revision,
                }))
            },
            |_| Ok(None),
        )?;

        let mut config = config;
        let context_exists = alt_config.is_some();
        let Some(config) = config.as_deref_mut().or(alt_config.as_mut()) else {
            eyre::bail!("Context config not found")
        };

        let client = self.config_client.query::<ContextConfigEnv>(
            config.protocol.as_ref().into(),
            config.network_id.as_ref().into(),
            config.contract_id.as_ref().into(),
        );

        let members_revision = client
            .members_revision(context_id.rt().expect("infallible conversion"))
            .await?;

        if !context_exists || members_revision != config.members_revision {
            config.members_revision = members_revision;

            for (offset, length) in (0..).map(|i| (100_usize.saturating_mul(i), 100)) {
                let members = client
                    .members(
                        context_id.rt().expect("infallible conversion"),
                        offset,
                        length,
                    )
                    .await?;

                if members.is_empty() {
                    break;
                }

                for member in members {
                    let member = member.as_bytes().into();

                    let key = ContextIdentityKey::new(context_id, member);

                    if !handle.has(&key)? {
                        handle.put(
                            &key,
                            &ContextIdentityValue {
                                private_key: None,
                                sender_key: Some(*self.new_private_key()),
                            },
                        )?;
                    }
                }
            }
        }

        let application_revision = client
            .application_revision(context_id.rt().expect("infallible conversion"))
            .await?;

        let mut application_id = None;

        if !context_exists || application_revision != config.application_revision {
            config.application_revision = application_revision;

            let application = client
                .application(context_id.rt().expect("infallible conversion"))
                .await?;

            let application_id = {
                let id = application.id.as_bytes().into();
                application_id = Some(id);
                id
            };

            if !self.is_application_installed(&application_id)? {
                let source = Url::parse(&application.source.0)?;

                let metadata = application.metadata.0.to_vec();

                let derived_application_id = match source.scheme() {
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

                if application_id != derived_application_id {
                    bail!("application mismatch")
                }
            }
        }

        if let Some(config) = alt_config {
            handle.put(
                &ContextConfigKey::new(context_id),
                &ContextConfigValue::new(
                    config.protocol.into_owned().into_boxed_str(),
                    config.network_id.into_owned().into_boxed_str(),
                    config.contract_id.into_owned().into_boxed_str(),
                    config.proxy_contract.into_owned().into_boxed_str(),
                    config.application_revision,
                    config.members_revision,
                ),
            )?;
        }

        context.map_or_else(
            || {
                Ok(Context::new(
                    context_id,
                    application_id.expect("must've been defined"),
                    Hash::default(),
                ))
            },
            |meta| {
                let context = Context::new(
                    context_id,
                    application_id.unwrap_or_else(|| meta.application.application_id()),
                    meta.root_hash.into(),
                );

                self.save_context(&context)?;

                Ok(context)
            },
        )
    }

    pub async fn is_context_pending_catchup(&self, context_id: &ContextId) -> bool {
        self.state.read().await.pending_catchup.contains(context_id)
    }

    pub async fn get_n_pending_sync_context(&self, amount: usize) -> Vec<ContextId> {
        self.state
            .read()
            .await
            .pending_catchup
            .iter()
            .copied()
            .choose_multiple(&mut rand::thread_rng(), amount)
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
        K: FromKeyParts<Error: Error + Send + Sync>,
    {
        let expected_length = Key::<K::Components>::len();

        if context_id.len().saturating_add(N) != expected_length {
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

            #[expect(clippy::iter_with_drain, reason = "reallocation would be a bad idea")]
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

    pub fn get_context_identities(
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

    pub fn get_sender_key(
        &self,
        context_id: &ContextId,
        public_key: &PublicKey,
    ) -> EyreResult<Option<PrivateKey>> {
        let handle = self.store.handle();
        let key = handle
            .get(&ContextIdentityKey::new(*context_id, *public_key))?
            .and_then(|ctx_identity| ctx_identity.sender_key);

        Ok(key.map(PrivateKey::from))
    }

    pub fn update_sender_key(
        &self,
        context_id: &ContextId,
        public_key: &PublicKey,
        sender_key: &PrivateKey,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let mut identity = handle
            .get(&ContextIdentityKey::new(*context_id, *public_key))?
            .ok_or_eyre("unknown identity")?;

        identity.sender_key = Some(**sender_key);

        handle.put(
            &ContextIdentityKey::new(*context_id, *public_key),
            &identity,
        )?;

        Ok(())
    }

    pub fn get_private_key(
        &self,
        context_id: ContextId,
        public_key: PublicKey,
    ) -> EyreResult<Option<PrivateKey>> {
        let handle = self.store.handle();

        let key = ContextIdentityKey::new(context_id, public_key);

        let Some(value) = handle.get(&key)? else {
            return Ok(None);
        };

        Ok(value.private_key.map(PrivateKey::from))
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

    pub fn context_has_owned_identity(
        &self,
        context_id: ContextId,
        public_key: PublicKey,
    ) -> EyreResult<bool> {
        let handle = self.store.handle();

        let key = ContextIdentityKey::new(context_id, public_key);

        let Some(value) = handle.get(&key)? else {
            return Ok(false);
        };

        Ok(value.private_key.is_some())
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

    pub async fn update_application_id(
        &self,
        context_id: ContextId,
        application_id: ApplicationId,
        signer_id: PublicKey,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();

        let key = ContextMetaKey::new(context_id);

        let Some(mut context_meta) = handle.get(&key)? else {
            bail!("Context not found")
        };

        let Some(application) = self.get_application(&application_id)? else {
            bail!("Application with id {:?} not found", application_id)
        };

        let Some(ContextIdentityValue {
            private_key: Some(requester_secret),
            ..
        }) = handle.get(&ContextIdentityKey::new(context_id, signer_id))?
        else {
            bail!("'{}' is not a member of '{}'", signer_id, context_id)
        };

        let Some(context_config) = handle.get(&ContextConfigKey::new(context_id))? else {
            bail!(
                "Failed to retrieve ContextConfig for context ID: {}",
                context_id
            );
        };

        let nonce = self
            .config_client
            .query::<ContextConfigEnv>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.contract.as_ref().into(),
            )
            .fetch_nonce(
                context_id.rt().expect("infallible conversion"),
                signer_id.rt().expect("infallible conversion"),
            )
            .await?
            .ok_or_eyre("Not a member")?;

        self.config_client
            .mutate::<ContextConfigEnv>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.contract.as_ref().into(),
            )
            .update_application(
                context_id.rt().expect("infallible conversion"),
                ApplicationConfig::new(
                    application.id.rt().expect("infallible conversion"),
                    application.blob.rt().expect("infallible conversion"),
                    application.size,
                    ApplicationSourceConfig(application.source.to_string().into()),
                    ApplicationMetadataConfig(Repr::new(application.metadata.into())),
                ),
            )
            .send(requester_secret, nonce)
            .await?;

        context_meta.application = ApplicationMetaKey::new(application_id);

        handle.put(&key, &context_meta)?;

        Ok(())
    }

    pub async fn update_context_proxy(
        &self,
        context_id: ContextId,
        public_key: PublicKey,
    ) -> EyreResult<()> {
        let handle = self.store.handle();

        let Some(context_config) = handle.get(&ContextConfigKey::new(context_id))? else {
            bail!("context not found");
        };

        let Some(ContextIdentityValue {
            private_key: Some(signing_key),
            ..
        }) = handle.get(&ContextIdentityKey::new(context_id, public_key))?
        else {
            bail!("no private key found for signer");
        };

        let nonce = self
            .config_client
            .query::<ContextConfigEnv>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.contract.as_ref().into(),
            )
            .fetch_nonce(
                context_id.rt().expect("infallible conversion"),
                public_key.rt().expect("infallible conversion"),
            )
            .await?
            .ok_or_eyre("The inviter doesen't exist")?;

        self.config_client
            .mutate::<ContextConfigEnv>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.contract.as_ref().into(),
            )
            .update_proxy_contract(context_id.rt().expect("infallible conversion"))
            .send(signing_key, nonce)
            .await?;

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
        self.blob_manager.has(blob_id)
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

        let response = ReqClient::new().get(url).send().await?;

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
        let handle = self.store.handle();

        let Some(application) = handle.get(&ApplicationMetaKey::new(*application_id))? else {
            return Ok(None);
        };

        let Some(mut stream) = self.get_blob(application.blob.blob_id())? else {
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

    pub fn get_blob(&self, blob_id: BlobId) -> EyreResult<Option<Blob>> {
        let Some(stream) = self.blob_manager.get(blob_id)? else {
            return Ok(None);
        };

        Ok(Some(stream))
    }

    pub fn is_application_blob_installed(&self, blob_id: BlobId) -> EyreResult<bool> {
        self.blob_manager.has(blob_id)
    }

    pub async fn propose(
        &self,
        context_id: ContextId,
        signer_id: PublicKey,
        proposal_id: ProposalId,
        actions: Vec<ProposalAction>,
    ) -> EyreResult<()> {
        let handle = self.store.handle();
        let Some(context_config) = handle.get(&ContextConfigKey::new(context_id))? else {
            bail!(
                "Failed to retrieve ContextConfig for context ID: {}",
                context_id
            );
        };

        let Some(ContextIdentityValue {
            private_key: Some(signing_key),
            ..
        }) = handle.get(&ContextIdentityKey::new(context_id, signer_id))?
        else {
            bail!("No private key found for signer");
        };

        let _ignored = self
            .config_client
            .mutate::<ContextProxy>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.proxy_contract.as_ref().into(),
            )
            .propose(
                proposal_id,
                signer_id.rt().expect("infallible conversion"),
                actions,
            )
            .send(signing_key)
            .await?;

        Ok(())
    }

    pub async fn approve(
        &self,
        context_id: ContextId,
        signer_id: PublicKey,
        proposal_id: ProposalId,
    ) -> EyreResult<()> {
        let handle = self.store.handle();

        let Some(context_config) = handle.get(&ContextConfigKey::new(context_id))? else {
            bail!("Context not found");
        };

        let Some(ContextIdentityValue {
            private_key: Some(signing_key),
            ..
        }) = handle.get(&ContextIdentityKey::new(context_id, signer_id))?
        else {
            bail!("No private key found for signer");
        };

        let _ignored = self
            .config_client
            .mutate::<ContextProxy>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.proxy_contract.as_ref().into(),
            )
            .approve(signer_id.rt().expect("infallible conversion"), proposal_id)
            .send(signing_key)
            .await?;

        Ok(())
    }

    pub async fn get_proposals(
        &self,
        context_id: ContextId,
        offset: usize,
        limit: usize,
    ) -> EyreResult<Vec<Proposal>> {
        let handle = self.store.handle();

        let Some(context_config) = handle.get(&ContextConfigKey::new(context_id))? else {
            bail!("Context not found");
        };

        let response = self
            .config_client
            .query::<ContextProxy>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.proxy_contract.as_ref().into(),
            )
            .proposals(offset, limit)
            .await;

        match response {
            Ok(proposals) => Ok(proposals),
            Err(err) => Err(eyre::eyre!("Failed to fetch proposals: {}", err)),
        }
    }

    pub async fn get_proposal(
        &self,
        context_id: ContextId,
        proposal_id: ProposalId,
    ) -> EyreResult<Proposal> {
        let handle = self.store.handle();

        let Some(context_config) = handle.get(&ContextConfigKey::new(context_id))? else {
            bail!("Context not found");
        };

        let response = self
            .config_client
            .query::<ContextProxy>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.proxy_contract.as_ref().into(),
            )
            .proposal(proposal_id)
            .await?;

        response.ok_or_eyre("no proposal found with the specified ID")
    }

    pub async fn get_number_of_active_proposals(&self, context_id: ContextId) -> EyreResult<u16> {
        let handle = self.store.handle();

        let Some(context_config) = handle.get(&ContextConfigKey::new(context_id))? else {
            bail!("Context not found");
        };

        self.config_client
            .query::<ContextProxy>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.proxy_contract.as_ref().into(),
            )
            .get_number_of_active_proposals()
            .await
            .map_err(|err| eyre::eyre!("Failed to fetch proposals: {}", err))
    }

    pub async fn get_number_of_proposal_approvals(
        &self,
        context_id: ContextId,
        proposal_id: ProposalId,
    ) -> EyreResult<ProposalWithApprovals> {
        let handle = self.store.handle();

        let Some(context_config) = handle.get(&ContextConfigKey::new(context_id))? else {
            bail!("Context not found");
        };

        self.config_client
            .query::<ContextProxy>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.proxy_contract.as_ref().into(),
            )
            .get_number_of_proposal_approvals(proposal_id)
            .await
            .map_err(|err| eyre::eyre!("Failed to fetch number of proposal approvals: {}", err))
    }

    pub async fn get_proposal_approvers(
        &self,
        context_id: ContextId,
        proposal_id: ProposalId,
    ) -> EyreResult<Vec<ContextIdentity>> {
        let handle = self.store.handle();

        let Some(context_config) = handle.get(&ContextConfigKey::new(context_id))? else {
            bail!("Context not found");
        };

        self.config_client
            .query::<ContextProxy>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.proxy_contract.as_ref().into(),
            )
            .get_proposal_approvers(proposal_id)
            .await
            .map_err(|err| eyre::eyre!("Failed to fetch proposal approvers: {}", err))
    }

    pub async fn get_context_value(
        &self,
        context_id: ContextId,
        key: Vec<u8>,
    ) -> EyreResult<Vec<u8>> {
        let handle = self.store.handle();

        let Some(context_config) = handle.get(&ContextConfigKey::new(context_id))? else {
            bail!("Context not found");
        };

        let response = self
            .config_client
            .query::<ContextProxy>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.proxy_contract.as_ref().into(),
            )
            .get_context_value(key)
            .await
            .map_err(|err| eyre::eyre!("Failed to fetch context value: {}", err))?;
        Ok(response)
    }

    pub async fn get_context_storage_entries(
        &self,
        context_id: ContextId,
        offset: usize,
        limit: usize,
    ) -> EyreResult<Vec<ContextStorageEntry>> {
        let handle = self.store.handle();

        let Some(context_config) = handle.get(&ContextConfigKey::new(context_id))? else {
            bail!("Context not found");
        };

        let response = self
            .config_client
            .query::<ContextProxy>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.proxy_contract.as_ref().into(),
            )
            .get_context_storage_entries(offset, limit)
            .await
            .map_err(|err| eyre::eyre!("Failed to fetch context storage entries: {}", err))?;
        Ok(response)
    }

    pub async fn get_proxy_id(&self, context_id: ContextId) -> EyreResult<String> {
        let handle = self.store.handle();
        let Some(context_config) = handle.get(&ContextConfigKey::new(context_id))? else {
            bail!("Context not found");
        };

        let proxy_contract = context_config.proxy_contract.into();

        Ok(proxy_contract)
    }

    pub async fn get_peers_count(&self) -> usize {
        self.network_client.peer_count().await
    }

    pub fn create_alias<T>(
        &self,
        alias: Alias<T>,
        scope: Option<T::Scope>,
        value: T,
    ) -> EyreResult<()>
    where
        T: Aliasable<Scope: StoreScopeCompat> + Into<Hash>,
    {
        let mut handle = self.store.handle();

        let alias_key =
            AliasKey::new(scope, alias).ok_or_eyre("alias requires scope to be present")?;

        if handle.has(&alias_key)? {
            bail!("alias already exists");
        }

        handle.put(&alias_key, &value.into())?;

        Ok(())
    }

    pub fn delete_alias<T>(&self, alias: Alias<T>, scope: Option<T::Scope>) -> EyreResult<()>
    where
        T: Aliasable<Scope: StoreScopeCompat>,
    {
        let mut handle = self.store.handle();

        let alias_key =
            AliasKey::new(scope, alias).ok_or_eyre("alias requires scope to be present")?;

        handle.delete(&alias_key)?;

        Ok(())
    }

    pub fn lookup_alias<T>(&self, alias: Alias<T>, scope: Option<T::Scope>) -> EyreResult<Option<T>>
    where
        T: Aliasable<Scope: StoreScopeCompat> + From<Hash>,
    {
        let handle = self.store.handle();

        let alias_key =
            AliasKey::new(scope, alias).ok_or_eyre("alias requires scope to be present")?;

        let Some(value) = handle.get(&alias_key)? else {
            return Ok(None);
        };

        Ok(Some(value.into()))
    }

    pub fn resolve_alias<T>(
        &self,
        alias: Alias<T>,
        scope: Option<T::Scope>,
    ) -> EyreResult<Option<T>>
    where
        T: Aliasable<Scope: StoreScopeCompat> + From<Hash> + FromStr<Err: Into<eyre::Report>>,
    {
        if let Some(value) = self.lookup_alias(alias, scope)? {
            return Ok(Some(value));
        }

        Ok(alias.as_str().parse().ok())
    }

    pub fn list_aliases<T>(
        &self,
        scope: Option<T::Scope>,
    ) -> EyreResult<Vec<(Alias<T>, T, Option<T::Scope>)>>
    where
        T: Aliasable + From<Hash>,
        T::Scope: Copy + PartialEq + StoreScopeCompat,
    {
        let handle = self.store.handle();

        let mut iter = handle.iter::<AliasKey>()?;

        let first = scope
            .map(|scope| {
                iter.seek(AliasKey::new_unchecked::<T>(Some(scope), [0; 50]))
                    .transpose()
                    .map(|k| (k, iter.read()))
            })
            .flatten();

        let mut aliases = vec![];

        for (k, v) in first.into_iter().chain(iter.entries()) {
            let (k, v) = (k?, v?);

            if let Some(expected_scope) = &scope {
                let Some(found_scope) = k.scope::<T>() else {
                    eyre::bail!("scope mismatch: {:?}", k);
                };

                if &found_scope != expected_scope {
                    break;
                }
            }

            let Some(alias) = k.alias() else {
                continue;
            };

            aliases.push((alias, v.into(), k.scope::<T>()));
        }

        Ok(aliases)
    }
}
