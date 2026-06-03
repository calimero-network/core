use calimero_governance_store::{MembershipRepository, MetaRepository};
use std::collections::{btree_map, BTreeMap};
use std::mem;
use std::sync::Arc;

use actix::{ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
use calimero_context_client::client::ContextClient;
use calimero_context_client::local_governance::GroupOp;
use calimero_context_client::messages::{CreateContextRequest, CreateContextResponse};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_node_primitives::client::NodeClient;

use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextConfigParams, ContextId};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::delta::{CausalDelta, StorageDelta};
use calimero_store::{key, types, Store};
use eyre::{bail, OptionExt};
use rand::rngs::StdRng;
use rand::SeedableRng;
use tokio::sync::{Mutex, OwnedMutexGuard};
use tracing::{debug, error, warn};

use super::execute::execute;
use super::execute::storage::{ContextPrivateStorage, ContextStorage};
use crate::handlers::execute::{persist_signed_signatures, sign_authorized_actions};
use crate::{BoundedCache, ContextManager, ContextMeta};
use calimero_governance_store::governance_broadcast::ObserveDelivery;

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
            name,
            ..
        }: CreateContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let identity_secret = identity_secret.or_else(|| {
            let (_, sk) = self.node_namespace_identity(&group_id)?;
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
            name,
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
            name,
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
                        Arc::clone(&act.ack_router),
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
                        name,
                    )
                    .into_actor(act)
                })
                .map_ok(move |root_hash, act, _ctx| {
                    if let Some(meta) = act.contexts.get_mut(&context_meta_for_map_ok.id) {
                        // this should almost always exist, but with an LruCache, it
                        // may not. And if it's been evicted, the next execution will
                        // re-create it with data from the store, so it's not a problem

                        meta.meta.root_hash = root_hash;
                        meta.meta.service_name = context_meta_for_map_ok.service_name.clone();
                    }

                    CreateContextResponse {
                        context_id: context_meta_for_map_ok.id,
                        identity,
                        group_id: Some(group_id_for_response),
                        group_created: false,
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
    name: Option<String>,
}

impl Prepared<'_> {
    fn new(
        node_client: &NodeClient,
        context_client: &ContextClient,
        contexts: &mut BoundedCache<ContextId, ContextMeta>,
        applications: &mut BoundedCache<ApplicationId, Application>,
        seed: Option<[u8; 32]>,
        application_id: &ApplicationId,
        identity_secret: Option<PrivateKey>,
        group_id: ContextGroupId,
        name: Option<String>,
        datastore: &Store,
    ) -> eyre::Result<Self> {
        let external_config = ContextConfigParams {
            application_id: None,
            application_revision: 0,
            members_revision: 0,
            service_name: None,
        };

        let mut effective_app_id = *application_id;
        let meta = MetaRepository::new(datastore)
            .load(&group_id)?
            .ok_or_eyre("group not found")?;

        let identity_pk = identity_secret
            .as_ref()
            .ok_or_eyre("identity_secret required for group context creation")?
            .public_key();

        if !MembershipRepository::new(datastore).is_member(&group_id, &identity_pk)? {
            bail!("identity is not a member of group '{group_id:?}'");
        }

        if !MembershipRepository::new(datastore).is_admin_or_has_capability(
            &group_id,
            &identity_pk,
            MemberCapabilities::CAN_CREATE_CONTEXT,
        )? {
            bail!(
                "identity lacks permission to create a context in group '{group_id:?}' \
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

        let mut rng = rand::thread_rng();

        let sender_key = PrivateKey::random(&mut rng);

        let identity_secret = identity_secret.unwrap_or_else(|| PrivateKey::random(&mut rng));

        // Resolve the application (fetching on a cache miss) *before* evicting,
        // so a failed lookup never wastes an eviction — evict only once the
        // insert is guaranteed to happen.
        let application = match applications.get(&effective_app_id) {
            Some(existing) => existing.clone(),
            None => {
                let fetched = node_client
                    .get_application(&effective_app_id)?
                    .ok_or_eyre("application not found")?;
                // Confirmed absent in the match arm above, so `insert_new`
                // (which caps the cache) is safe.
                applications.insert_new(effective_app_id, fetched).clone()
            }
        };

        // Make room for the freshly created context. Placed after the
        // membership/capability validation above (so failed auth never drains
        // the cache) and immediately before the derivation loop that inserts
        // via the raw `entry()` escape hatch (the VacantEntry transmute below).
        contexts.evict_if_full();

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

        let meta = Context::new(context_id, effective_app_id, Hash::default());

        let context = entry.insert(ContextMeta {
            meta,
            lock: Arc::new(Mutex::new(context_id)),
        });

        Ok(Self {
            external_config,
            application,
            context,
            context_secret,
            identity,
            identity_secret,
            sender_key,
            group_id,
            name,
        })
    }
}

async fn create_context(
    datastore: Store,
    node_client: NodeClient,
    _context_client: ContextClient,
    ack_router: Arc<calimero_context_client::local_governance::AckRouter>,
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
    name: Option<String>,
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

    // Returns `(db-row, actions)` — actions are kept alongside the
    // serialized delta so we can notify the node-side DeltaStore to
    // update its in-memory DAG without having to borsh-deserialize
    // the serialized blob back out.
    // Commit storage BEFORE building the init delta — we need a
    // `Store` handle to drive `persist_signed_signatures`, which
    // writes the signed `signature_data` back into the freshly
    // committed entity index entries. Without this, the bootstrap
    // entities created during `init()` keep the `[0; 64]`
    // placeholder signature emitted by `save_raw` (which runs
    // inside the WASM host call and has no access to the identity
    // private key), and HashComparison sync — plus the new
    // per-entity Snapshot verification (#2387) — would reject them
    // on every peer that tries to apply the snapshot.
    let datastore = storage.commit()?;
    let _private_datastore = private_storage.commit()?;

    let init_delta = if let Some(root_hash) = outcome.root_hash {
        context.root_hash = root_hash.into();

        // CRITICAL: Create delta and set dag_heads for init()
        // This ensures newly joined nodes can sync via delta protocol
        let mut actions = if !outcome.artifact.is_empty() {
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

        // Sign the bootstrap actions and persist the signed
        // `signature_data` to local storage — same flow
        // `execute_method` runs for regular method calls. The
        // signed actions then go into the broadcast CausalDelta
        // below so peers receive verifiable state, and the local
        // index entries are patched so subsequent HashComparison /
        // Snapshot responses ship verifiable state too.
        // Partial-commit caveat: `storage.commit()` above already
        // wrote the bootstrap entities to RocksDB (with placeholder
        // signatures from `save_raw`). If signing or
        // signature-persist fails below and we `bail!` out, those
        // entities remain in the store as orphans — no
        // `ContextConfig` / `ContextMeta` keys point to them, and
        // no init delta references them, but the data is on disk.
        // The user can retry context creation with the same group;
        // a new context_id is generated each time, so the orphans
        // don't conflict with the retry.
        //
        // Bailing is the right call here even with that cost: if
        // we silently continued, the broadcast `CausalDelta` would
        // carry placeholder-signed actions, peers would reject
        // them, and the context would exist but be unusable. A
        // proper fix (transactional commit or post-failure
        // cleanup pass that walks `ContextStateKey` for this
        // context_id and deletes orphan entries) is a follow-up.
        if !actions.is_empty() {
            if let Err(e) = sign_authorized_actions(&mut actions, &identity_secret) {
                error!(?e, %context.id, "Failed to sign init actions");
                bail!("Failed to sign init actions: {:?}", e);
            }
            if let Err(e) =
                persist_signed_signatures(&datastore, &context, &identity_secret, &actions)
            {
                error!(?e, %context.id, "Failed to persist signed init signatures");
                bail!("Failed to persist signed init signatures: {:?}", e);
            }
        }

        // Always create a genesis delta. The parent should be `[0; 32]` (genesis).
        // This way, the DAG will have a head that is associated with a delta even if state is empty.
        let hlc = calimero_storage::env::hlc_timestamp();
        // Genesis parent
        let parents = vec![[0u8; 32]];
        let delta_id = CausalDelta::compute_id(&parents, &actions, &hlc);

        context.dag_heads = vec![delta_id];

        // Persist the init delta so peers can request it. Uses the
        // now-signed actions so peers applying the genesis delta
        // verify against real signatures, not the placeholder.
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
            // Genesis predates any governance op; no author claim to
            // verify on the DAG-catchup path.
            author_id: None,
            governance_position_blob: None,
            // Genesis has no author signature to record.
            delta_signature: None,
        };

        debug!(
            context_id = %context.id,
            delta_id = ?delta_id,
            actions_count = actions.len(),
            "Created genesis delta with dag_heads"
        );

        Some((delta, actions))
    } else {
        None
    };

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
    if let Some((delta, actions)) = init_delta {
        handle.put(
            &key::ContextDagDelta::new(context.id, delta.delta_id),
            &delta,
        )?;

        debug!(
            context_id = %context.id,
            delta_id = ?delta.delta_id,
            "Persisted init delta to database"
        );

        // Register into the in-memory DAG so sync doesn't have to
        // rescan the DB to pick up this newly-persisted genesis delta.
        node_client.notify_local_applied_delta(
            calimero_node_primitives::client::LocalAppliedDelta {
                context_id: context.id,
                delta_id: delta.delta_id,
                parents: delta.parents.clone(),
                hlc: delta.hlc,
                expected_root_hash: delta.expected_root_hash,
                actions,
            },
        );
    }

    // Write ContextIdentity so the sync key-share can find keys for this
    // context. The creator is already a GroupMember (admin) with keys
    // stored there.
    //
    // Ordering: written BEFORE `sign_apply_and_publish` below. Reason —
    // `sign_apply_and_publish` calls `sign_apply_local_group_op_borsh`
    // which applies the op locally and emits `OpEvent::ContextRegistered`
    // *synchronously* via `op_events::notify`. That wakes the
    // `auto_follow` handler task, which tries to sync the new context
    // via `choose_owned_identity`. If we wrote `ContextIdentity` *after*
    // the publish (as the original code did), that auto-follow sync
    // would race ahead, find no key in the datastore, and bail with
    // `no owned identities found for context: <id>` — visible in
    // sync-regression run 25914871901's node-1 log at
    // `11:21:34.587 ... 11:21:36.588`. Sync then enters exponential
    // backoff (2 → 4 → 8 → 16 s) which can outlive realistic test
    // deadlines. Writing the identity first closes the race; the
    // identity is purely local state with no dependency on
    // `ContextRegistered` being applied first.
    handle.put(
        &key::ContextIdentity::new(context.id, identity),
        &types::ContextIdentity {
            private_key: Some(*identity_secret),
            sender_key: Some(*sender_key),
        },
    )?;

    drop(handle);

    // Register context in group BEFORE subscribing so that a registration
    // failure does not leave a subscribed-but-unregistered context.
    // Note: membership was verified in Prepared::new(); a TOCTOU gap exists
    // because the async create_context future may interleave with other actor
    // messages (e.g. RemoveGroupMembers), but the window is small and the
    // worst case is a single context associated with a since-removed member.
    {
        let sk = PrivateKey::from(*identity_secret);
        let report = calimero_governance_store::sign_apply_and_publish(
            &datastore,
            &node_client,
            &ack_router,
            &group_id,
            &sk,
            GroupOp::ContextRegistered {
                context_id: context.id,
                application_id: context.application_id,
                blob_id: application.blob.bytecode,
                source: application.source.to_string(),
                service_name: context.service_name.clone(),
            },
        )
        .await?;
        report.observe("create_context", "ContextRegistered");
    }

    node_client.subscribe(&context.id).await?;
    node_client.subscribe_namespace(group_id.to_bytes()).await?;

    if let Some(ref name_str) = name {
        let sk = PrivateKey::from(*identity_secret);
        let report = calimero_governance_store::sign_apply_and_publish(
            &datastore,
            &node_client,
            &ack_router,
            &group_id,
            &sk,
            GroupOp::ContextMetadataSet {
                context_id: context.id,
                name: Some(name_str.clone()),
                data: BTreeMap::new(),
            },
        )
        .await?;
        report.observe("create_context", "ContextMetadataSet");
    }

    Ok(context.root_hash)
}
