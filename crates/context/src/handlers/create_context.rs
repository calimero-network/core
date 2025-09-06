use std::collections::{btree_map, BTreeMap};
use std::mem;
use std::num::NonZeroUsize;
use std::sync::Arc;

use actix::{ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
use calimero_context_config::client::config::ClientConfig as ExternalClientConfig;
use calimero_context_config::client::utils::humanize_iter;
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::{CreateContextRequest, CreateContextResponse};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextConfigParams, ContextId};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::{key, types, Store};
use eyre::{bail, OptionExt};
use rand::rngs::StdRng;
use rand::SeedableRng;
use tokio::sync::{Mutex, OwnedMutexGuard};

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
            &mut self.contexts,
            &mut self.applications,
            protocol,
            seed,
            &application_id,
            identity_secret,
        ) {
            Ok(res) => res,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let Prepared {
            external_config,
            application,
            context,
            context_secret,
            identity,
            identity_secret,
            sender_key,
        } = prepared;

        let guard = context
            .lock
            .clone()
            .try_lock_owned()
            .expect("logically exclusive");

        let context = context.meta;

        let module_task = self.get_module(application_id);

        ActorResponse::r#async(
            module_task
                .and_then(move |module, act, _ctx| {
                    create_context(
                        act.datastore.clone(),
                        act.node_client.clone(),
                        act.context_client.clone(),
                        module,
                        external_config,
                        context,
                        context_secret,
                        application,
                        identity,
                        identity_secret,
                        sender_key,
                        init_params,
                        guard,
                    )
                    .into_actor(act)
                })
                .map_ok(move |root_hash, act, _ctx| {
                    if let Some(meta) = act.contexts.get_mut(&context.id) {
                        // this should almost always exist, but with an LruCache, it
                        // may not. And if it's been evicted, the next execution will
                        // re-create it with data from the store, so it's not a problem

                        meta.meta.root_hash = root_hash;
                    }

                    CreateContextResponse {
                        context_id: context.id,
                        identity,
                    }
                })
                .map_err(move |err, act, _ctx| {
                    let _ignored = act.contexts.remove(&context.id);

                    err
                }),
        )
    }
}

struct Prepared<'a> {
    external_config: ContextConfigParams<'static>,
    application: Application,
    context: &'a ContextMeta,
    context_secret: PrivateKey,
    identity: PublicKey,
    identity_secret: PrivateKey,
    sender_key: PrivateKey,
}

impl Prepared<'_> {
    fn new(
        node_client: &NodeClient,
        context_client: &ContextClient,
        external_config: &ExternalClientConfig,
        contexts: &mut BTreeMap<ContextId, ContextMeta>,
        applications: &mut BTreeMap<ApplicationId, Application>,
        protocol: String,
        seed: Option<[u8; 32]>,
        application_id: &ApplicationId,
        identity_secret: Option<PrivateKey>,
    ) -> eyre::Result<Self> {
        let Some(external_config) = external_config.params.get(&protocol) else {
            bail!(
                "unsupported protocol: {}, expected one of `{}`",
                protocol,
                humanize_iter(external_config.params.keys())
            );
        };

        let external_config = ContextConfigParams {
            protocol: protocol.into(),
            network_id: external_config.network.clone().into(),
            contract_id: external_config.contract_id.clone().into(),
            // vv not used for context creation --
            proxy_contract: "".into(),
            application_revision: 0,
            members_revision: 0,
            // ^^ not used for context creation --
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

            if let btree_map::Entry::Vacant(entry) = contexts.entry(context_id) {
                if context_client.has_context(&context_id)? {
                    continue;
                }

                // safety: the VacantEntry only lives as long as this function
                //         and the entry within the BTreeMap is constrained to
                //         the lifetime of the BTreeMap before it is returned
                let entry = unsafe {
                    mem::transmute::<_, btree_map::VacantEntry<'static, ContextId, ContextMeta>>(
                        entry,
                    )
                };

                context = Some(Some((entry, context_id, context_secret)));

                break;
            }
        }
        let (entry, context_id, context_secret) = context
            .flatten()
            .ok_or_eyre("failed to derive a context id after 5 tries")?;

        let application = match applications.entry(*application_id) {
            btree_map::Entry::Vacant(vacant) => {
                let application = node_client
                    .get_application(application_id)?
                    .ok_or_eyre("application not found")?;

                vacant.insert(application)
            }
            btree_map::Entry::Occupied(occupied) => occupied.into_mut(),
        };

        let identity = identity_secret.public_key();

        let meta = Context::new(context_id, *application_id, Hash::default());

        let context = entry.insert(ContextMeta {
            meta,
            lock: Arc::new(Mutex::new(context_id)),
        });

        let application = application.clone();

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
    module: calimero_runtime::Module,
    external_config: ContextConfigParams<'_>,
    mut context: Context,
    context_secret: PrivateKey,
    application: Application,
    identity: PublicKey,
    identity_secret: PrivateKey,
    sender_key: PrivateKey,
    init_params: Vec<u8>,
    guard: OwnedMutexGuard<ContextId>,
) -> eyre::Result<Hash> {
    let storage = ContextStorage::from(datastore, context.id);

    let (outcome, storage) = execute(
        &guard,
        module,
        identity,
        "init".into(),
        init_params.into(),
        storage,
        node_client.clone(),
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

    let config_client = external_client.config();

    config_client
        .add_context(&context_secret, &identity, &application)
        .await?;

    let proxy_contract = config_client.get_proxy_contract().await?;

    let datastore = storage.commit()?;

    let height = NonZeroUsize::MIN;

    context_client.put_state_delta(&context.id, &identity, &height, &outcome.artifact)?;

    context_client.set_delta_height(&context.id, &identity, height)?;

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

    node_client.subscribe(&context.id).await?;

    Ok(context.root_hash)
}
