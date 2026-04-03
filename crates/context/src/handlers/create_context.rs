use std::collections::{btree_map, BTreeMap};
use std::mem;
use std::sync::Arc;

use actix::{ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_context_client::client::ContextClient;
use calimero_context_client::local_governance::GroupOp;
use calimero_context_client::messages::{CreateContextRequest, CreateContextResponse};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{
    Context, ContextConfigParams, ContextId, GroupMemberRole, UpgradePolicy,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::delta::{CausalDelta, StorageDelta};
use calimero_store::key::GroupMetaValue;
use calimero_store::{key, types, Store};
use eyre::{bail, OptionExt};
use rand::rngs::StdRng;
use rand::SeedableRng;
use tokio::sync::{Mutex, OwnedMutexGuard};
use tracing::{debug, warn};

use super::execute::execute;
use super::execute::storage::{ContextPrivateStorage, ContextStorage};
use crate::{group_store, ContextManager, ContextMeta};

impl Handler<CreateContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <CreateContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        CreateContextRequest {
            seed,
            application_id,
            service_name,
            identity_secret,
            init_params,
            group_id,
            alias,
            ..
        }: CreateContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let identity_secret = identity_secret.or_else(|| {
            let gid = group_id.as_ref()?;
            let (_, sk) = self.node_namespace_identity(gid)?;
            Some(PrivateKey::from(sk))
        });

        let prepared = match Prepared::new(
            &self.node_client,
            &self.context_client,
            &mut self.contexts,
            &mut self.applications,
            seed,
            &application_id,
            identity_secret,
            group_id,
            alias,
            &self.datastore,
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
            group_id,
            group_created,
            alias,
        } = prepared;

        let group_id_for_response = group_id;

        let guard = context
            .lock
            .clone()
            .try_lock_owned()
            .expect("logically exclusive");

        let mut context_meta = context.meta.clone();
        context_meta.service_name = service_name;

        let module_task = self.get_module(application.id, context_meta.service_name.clone());

        let context_meta_for_map_ok = context_meta.clone();
        let context_meta_for_map_err = context_meta.clone();

        ActorResponse::r#async(
            module_task
                .and_then(move |module, act, _ctx| {
                    create_context(
                        act.datastore.clone(),
                        act.node_client.clone(),
                        act.context_client.clone(),
                        module,
                        external_config,
                        context_meta,
                        context_secret,
                        application,
                        identity,
                        identity_secret,
                        sender_key,
                        init_params,
                        guard,
                        group_id_for_response,
                        alias,
                    )
                    .into_actor(act)
                })
                .map_ok(move |root_hash, act, _ctx| {
                    if let Some(meta) = act.contexts.get_mut(&context_meta_for_map_ok.id) {
                        // this should almost always exist, but with an LruCache, it
                        // may not. And if it's been evicted, the next execution will
                        // re-create it with data from the store, so it's not a problem

                        meta.meta.root_hash = root_hash;
                    }

                    CreateContextResponse {
                        context_id: context_meta_for_map_ok.id,
                        identity,
                        group_id: Some(group_id_for_response),
                        group_created,
                    }
                })
                .map_err(move |err, act, _ctx| {
                    let _ignored = act.contexts.remove(&context_meta_for_map_err.id);

                    err
                }),
        )
    }
}

struct Prepared<'a> {
    external_config: ContextConfigParams,
    application: Application,
    context: &'a ContextMeta,
    context_secret: PrivateKey,
    identity: PublicKey,
    identity_secret: PrivateKey,
    sender_key: PrivateKey,
    group_id: ContextGroupId,
    group_created: bool,
    alias: Option<String>,
}

impl Prepared<'_> {
    fn new(
        node_client: &NodeClient,
        context_client: &ContextClient,
        contexts: &mut BTreeMap<ContextId, ContextMeta>,
        applications: &mut BTreeMap<ApplicationId, Application>,
        seed: Option<[u8; 32]>,
        application_id: &ApplicationId,
        identity_secret: Option<PrivateKey>,
        group_id: Option<ContextGroupId>,
        alias: Option<String>,
        datastore: &Store,
    ) -> eyre::Result<Self> {
        let external_config = ContextConfigParams {
            application_id: None,
            application_revision: 0,
            members_revision: 0,
        };

        let mut effective_app_id = *application_id;
        if let Some(ref gid) = group_id {
            let meta =
                group_store::load_group_meta(datastore, gid)?.ok_or_eyre("group not found")?;

            let identity_pk = identity_secret
                .as_ref()
                .ok_or_eyre("identity_secret required for group context creation")?
                .public_key();

            if !group_store::check_group_membership(datastore, gid, &identity_pk)? {
                bail!("identity is not a member of group '{gid:?}'");
            }

            if !group_store::is_group_admin_or_has_capability(
                datastore,
                gid,
                &identity_pk,
                MemberCapabilities::CAN_CREATE_CONTEXT,
            )? {
                bail!(
                    "identity lacks permission to create a context in group '{gid:?}' \
                     (not an admin and CAN_CREATE_CONTEXT is not set)"
                );
            }

            if effective_app_id != meta.target_application_id {
                warn!(
                    requested=?effective_app_id,
                    group_target=?meta.target_application_id,
                    "overriding application_id with group target"
                );
                effective_app_id = meta.target_application_id;
            }
        }

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

        let identity = identity_secret.public_key();

        // Auto-create a single-member group when no explicit group_id was provided.
        let group_created = group_id.is_none();
        let group_id = if let Some(gid) = group_id {
            gid
        } else {
            let auto_group_id = ContextGroupId::from(*context_id.as_ref());
            group_store::save_group_meta(
                datastore,
                &auto_group_id,
                &GroupMetaValue {
                    app_key: [0u8; 32],
                    target_application_id: effective_app_id,
                    upgrade_policy: UpgradePolicy::Automatic,
                    created_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    admin_identity: identity,
                    migration: None,
                    auto_join: true,
                },
            )?;
            let sender_key = PrivateKey::random(&mut rng);
            group_store::add_group_member_with_keys(
                datastore,
                &auto_group_id,
                &identity,
                GroupMemberRole::Admin,
                Some(*identity_secret),
                Some(*sender_key),
            )?;
            group_store::store_namespace_identity(
                datastore,
                &auto_group_id,
                &identity,
                &*identity_secret,
                &*sender_key,
            )?;

            let group_key: [u8; 32] = {
                use rand::Rng;
                rng.gen()
            };
            group_store::store_group_key(datastore, &auto_group_id, &group_key)?;

            auto_group_id
        };

        let application = match applications.entry(effective_app_id) {
            btree_map::Entry::Vacant(vacant) => {
                let application = node_client
                    .get_application(&effective_app_id)?
                    .ok_or_eyre("application not found")?;

                vacant.insert(application)
            }
            btree_map::Entry::Occupied(occupied) => occupied.into_mut(),
        };

        let meta = Context::new(context_id, effective_app_id, Hash::default());

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
            group_id,
            group_created,
            alias,
        })
    }
}

async fn create_context(
    datastore: Store,
    node_client: NodeClient,
    _context_client: ContextClient,
    module: calimero_runtime::Module,
    external_config: ContextConfigParams,
    mut context: Context,
    _context_secret: PrivateKey,
    application: Application,
    identity: PublicKey,
    identity_secret: PrivateKey,
    sender_key: PrivateKey,
    init_params: Vec<u8>,
    guard: OwnedMutexGuard<ContextId>,
    group_id: ContextGroupId,
    alias: Option<String>,
) -> eyre::Result<Hash> {
    let storage = ContextStorage::from(datastore.clone(), context.id);
    // Create private storage (node-local, NOT synchronized)
    let private_storage = ContextPrivateStorage::from(datastore, context.id);

    let (outcome, storage, private_storage) = execute(
        &guard,
        module,
        identity,
        "init".into(),
        init_params.into(),
        storage,
        private_storage,
        node_client.clone(),
    )
    .await?;

    if let Some(res) = outcome.returns? {
        bail!(
            "context initialization returned a value, but it should not: {:?}",
            res
        );
    }

    let init_delta = if let Some(root_hash) = outcome.root_hash {
        context.root_hash = root_hash.into();

        // CRITICAL: Create delta and set dag_heads for init()
        // This ensures newly joined nodes can sync via delta protocol
        let actions = if !outcome.artifact.is_empty() {
            // Extract actions from init artifact
            match borsh::from_slice::<StorageDelta>(&outcome.artifact) {
                Ok(StorageDelta::Actions(actions)) => actions,
                Ok(_) => {
                    warn!("Unexpected StorageDelta variant during init");
                    vec![]
                }
                Err(e) => {
                    warn!(?e, "Failed to deserialize init artifact");
                    vec![]
                }
            }
        } else {
            vec![]
        };

        // Always create a genesis delta. The parent should be `[0; 32]` (genesis).
        // This way, the DAG will have a head that is associated with a delta even if state is empty.
        let hlc = calimero_storage::env::hlc_timestamp();
        // Genesis parent
        let parents = vec![[0u8; 32]];
        let delta_id = CausalDelta::compute_id(&parents, &actions, &hlc);

        context.dag_heads = vec![delta_id];

        // Persist the init delta so peers can request it
        let serialized_actions = borsh::to_vec(&actions)?;

        let delta = types::ContextDagDelta {
            delta_id,
            parents,
            actions: serialized_actions,
            hlc,
            applied: true,
            expected_root_hash: root_hash,
            // Genesis delta has no events
            events: None,
        };

        debug!(
            context_id = %context.id,
            delta_id = ?delta_id,
            actions_count = actions.len(),
            "Created genesis delta with dag_heads"
        );

        Some(delta)
    } else {
        None
    };

    let datastore = storage.commit()?;
    let _private_datastore = private_storage.commit()?;

    let mut handle = datastore.handle();

    handle.put(
        &key::ContextConfig::new(context.id),
        &types::ContextConfig::new(
            external_config.application_revision,
            external_config.members_revision,
        ),
    )?;

    handle.put(
        &key::ContextMeta::new(context.id),
        &types::ContextMeta::new(
            key::ApplicationMeta::new(application.id),
            *context.root_hash,
            context.dag_heads.clone(),
            context.service_name.as_deref().map(Box::from),
        ),
    )?;

    // Persist init delta if created
    if let Some(delta) = init_delta {
        handle.put(
            &key::ContextDagDelta::new(context.id, delta.delta_id),
            &delta,
        )?;

        debug!(
            context_id = %context.id,
            delta_id = ?delta.delta_id,
            "Persisted init delta to database"
        );
    }

    drop(handle);

    // Register context in group BEFORE subscribing so that a registration
    // failure does not leave a subscribed-but-unregistered context.
    // Note: membership was verified in Prepared::new(); a TOCTOU gap exists
    // because the async create_context future may interleave with other actor
    // messages (e.g. RemoveGroupMembers), but the window is small and the
    // worst case is a single context associated with a since-removed member.
    {
        let sk = PrivateKey::from(*identity_secret);
        group_store::sign_apply_and_publish(
            &datastore,
            &node_client,
            &group_id,
            &sk,
            GroupOp::ContextRegistered {
                context_id: context.id,
                application_id: context.application_id,
                blob_id: application.blob.bytecode,
                source: application.source.to_string(),
            },
        )
        .await?;
    }

    // Write ContextIdentity so the sync key-share can find keys for this context.
    // The creator is already a GroupMember (admin) with keys stored there.
    let mut handle = datastore.handle();
    handle.put(
        &key::ContextIdentity::new(context.id, identity),
        &types::ContextIdentity {
            private_key: Some(*identity_secret),
            sender_key: Some(*sender_key),
        },
    )?;
    drop(handle);

    node_client.subscribe(&context.id).await?;
    node_client.subscribe_namespace(group_id.to_bytes()).await?;

    if let Some(ref alias_str) = alias {
        let sk = PrivateKey::from(*identity_secret);
        group_store::sign_apply_and_publish(
            &datastore,
            &node_client,
            &group_id,
            &sk,
            GroupOp::ContextAliasSet {
                context_id: context.id,
                alias: alias_str.clone(),
            },
        )
        .await?;
    }

    Ok(context.root_hash)
}
