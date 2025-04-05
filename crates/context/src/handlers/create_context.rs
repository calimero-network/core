use std::sync::Arc;

use actix::{ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
use calimero_context_config::client::config::ClientConfig as ExternalClientConfig;
use calimero_context_config::client::utils::humanize_iter;
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::create_context::{
    CreateContextRequest, CreateContextResponse,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextConfigParams, ContextId};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::{key, types, Store};
use eyre::{bail, OptionExt};
use rand::rngs::StdRng;
use rand::SeedableRng;
use tokio::sync::Mutex;

use super::execute::execute;
use super::execute::storage::ContextStorage;
use crate::{ContextManager, ContextMeta};

impl Handler<CreateContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <CreateContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        CreateContextRequest {
            protocol,
            seed,
            application_id,
            identity_secret,
            init_params,
        }: CreateContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let prepared = match Prepared::new(
            &self.node_client,
            &self.context_client,
            &self.external_config,
            &protocol,
            seed,
            &application_id,
            identity_secret,
        ) {
            Ok(res) => res,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let cached_blob = self.blobs.get(&prepared.application.blob).cloned();

        let context = Context::new(prepared.context, prepared.application.id, Hash::default());

        let _ignored = self.contexts.insert(
            context.id,
            ContextMeta {
                meta: context,
                blob: prepared.application.blob,
                lock: None,
            },
        );

        let datastore = self.datastore.clone();

        let blob_id = prepared.application.blob;

        ActorResponse::r#async(
            Box::pin(create_context(
                datastore,
                self.node_client.clone(),
                self.context_client.clone(),
                prepared.external_config,
                context,
                prepared.context_secret,
                prepared.application,
                prepared.identity,
                prepared.identity_secret,
                prepared.sender_key,
                cached_blob,
                init_params,
            ))
            .into_actor(self)
            .and_then(move |(context, lock, blob), act, _ctx| {
                if let Some(meta) = act.contexts.get_mut(&context.id) {
                    // this should almost always exist, but with an LruCache, it
                    // may not. And if it's been evicted, the next execution will
                    // re-create it with data from the store, so it's not a problem

                    meta.meta.root_hash = context.root_hash;
                    meta.lock = Some(lock);
                }

                let _ignored = act.blobs.entry(blob_id).or_insert(blob);

                async move {
                    Ok(CreateContextResponse {
                        context_id: context.id,
                        identity: prepared.identity,
                    })
                }
                .into_actor(act)
            }),
        )
    }
}

struct Prepared {
    external_config: ContextConfigParams<'static>,
    application: Application,
    context: ContextId,
    context_secret: PrivateKey,
    identity: PublicKey,
    identity_secret: PrivateKey,
    sender_key: PrivateKey,
}

impl Prepared {
    fn new(
        node_client: &NodeClient,
        context_client: &ContextClient,
        external_config: &ExternalClientConfig,
        protocol: &str,
        seed: Option<[u8; 32]>,
        application_id: &ApplicationId,
        identity_secret: Option<PrivateKey>,
    ) -> eyre::Result<Self> {
        let Some(external_config) = external_config.params.get(protocol) else {
            bail!(
                "unsupported protocol: {}, expected one of `{}`",
                protocol,
                humanize_iter(external_config.params.keys())
            );
        };

        let external_config = ContextConfigParams {
            protocol: external_config.protocol.clone().into(),
            network_id: external_config.protocol.clone().into(),
            contract_id: external_config.contract_id.clone().into(),
            proxy_contract: "".into(),
            application_revision: 0,
            members_revision: 0,
        };

        let mut rng = rand::thread_rng();

        let sender_key = PrivateKey::random(&mut rng);

        let identity_secret = identity_secret.unwrap_or_else(|| PrivateKey::random(&mut rng));

        let mut context = None;
        for _ in 0..5 {
            let context_secret = if let Some(seed) = seed {
                if context.is_some() {
                    bail!("seed resulted in an already existing context");
                }

                PrivateKey::random(&mut StdRng::from_seed(seed))
            } else {
                PrivateKey::random(&mut rng)
            };

            context = Some(None);

            let context_id = ContextId::from(*context_secret.public_key());

            if !context_client.has_context(&context_id)? {
                context = Some(Some((context_id, context_secret)));

                break;
            }
        }
        let (context, context_secret) = context
            .flatten()
            .ok_or_eyre("failed to derive a context id after 5 tries")?;

        let Some(application) = node_client.get_application(application_id)? else {
            bail!("application not found");
        };

        if !node_client.has_blob(&application.blob)? {
            bail!("application points to dangling blob");
        }

        let identity = identity_secret.public_key();

        Ok(Self {
            external_config,
            application,
            context,
            context_secret,
            identity,
            identity_secret,
            sender_key,
        })
    }
}

async fn create_context(
    datastore: Store,
    node_client: NodeClient,
    context_client: ContextClient,
    external_config: ContextConfigParams<'_>,
    mut context: Context,
    context_secret: PrivateKey,
    application: Application,
    identity: PublicKey,
    identity_secret: PrivateKey,
    sender_key: PrivateKey,
    cached_blob: Option<Arc<Box<[u8]>>>,
    init_params: Vec<u8>,
) -> eyre::Result<(Context, Arc<Mutex<ContextId>>, Arc<Box<[u8]>>)> {
    let blob = match cached_blob {
        Some(blob) => blob,
        None => {
            let Some(blob) = node_client.get_blob_bytes(&application.blob).await? else {
                bail!("fatal: application points to dangling blob, blob store may be corrupted");
            };

            Arc::new(blob)
        }
    };

    let lock = Arc::new(Mutex::new(context.id));

    let storage = ContextStorage::from(datastore, context.id);

    let (outcome, storage) = execute(
        lock.clone(),
        blob.clone(),
        "init",
        init_params,
        identity,
        storage,
    )
    .await?;

    if let Some(res) = outcome.returns? {
        bail!(
            "context initialization returned a value, but it should not: {:?}",
            res
        );
    }

    if let Some(root_hash) = outcome.root_hash {
        context.root_hash = root_hash.into();
    }

    let external_client = context_client.external_client(&context.id, &external_config)?;

    external_client
        .config()
        .add_context(&context_secret, &identity, &application)
        .await?;

    let proxy_contract = external_client.config().get_proxy_contract().await?;

    let datastore = storage.commit()?;

    let mut handle = datastore.handle();

    handle.put(
        &key::ContextConfig::new(context.id),
        &types::ContextConfig::new(
            external_config.protocol.into_owned().into_boxed_str(),
            external_config.network_id.into_owned().into_boxed_str(),
            external_config.contract_id.into_owned().into_boxed_str(),
            proxy_contract.into_boxed_str(),
            external_config.application_revision,
            external_config.members_revision,
        ),
    )?;

    handle.put(
        &key::ContextMeta::new(context.id),
        &types::ContextMeta::new(
            key::ApplicationMeta::new(application.id),
            *context.root_hash,
        ),
    )?;

    handle.put(
        &key::ContextIdentity::new(context.id, identity),
        &types::ContextIdentity {
            private_key: Some(*identity_secret),
            sender_key: Some(*sender_key),
        },
    )?;

    Ok((context, lock, blob))
}
