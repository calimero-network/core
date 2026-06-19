use calimero_governance_store::{
    CapabilitiesRepository, GroupKeyring, MetaRepository, NamespaceRepository,
};
use std::borrow::Cow;
// Removed: NonZeroUsize (replaced with CausalDelta)
use std::time::Instant;

use actix::{
    ActorFuture, ActorFutureExt, ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture,
};
use calimero_context_client::client::crypto::ContextIdentity;
use calimero_context_client::client::ContextClient;
use calimero_context_client::messages::{
    ExecuteError, ExecuteEvent, ExecuteRequest, ExecuteResponse, MigrationParams,
};
use calimero_context_client::{ContextAtomic, ContextAtomicKey, ContextGuard};
use calimero_context_config::types::{ContextGroupId, GovernanceParentEdge};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::events::{
    ContextEvent, ContextEventPayload, ExecutionEvent, NodeEvent, StateMutationPayload,
    XCallOutcome, XCallPayload,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_runtime::logic::Outcome;
use calimero_storage::{
    address::Id,
    delta::{CausalDelta, StorageDelta},
    env::{with_runtime_env, RuntimeEnv},
    index::Index,
    interface::Interface,
    store::MainStorage,
};
use std::collections::HashSet;
use std::sync::Arc;

use calimero_store::{key, types, Store};
use calimero_utils_actix::global_runtime;
use calimero_wasm_abi::schema::MethodIntent;
use either::Either;
use eyre::{bail, WrapErr};
use futures_util::future::TryFutureExt;
use memchr::memmem;
use tracing::{debug, error, info, warn};

use crate::error::ContextError;
use crate::handlers::update_application::{
    clear_migration_failed, create_storage_callbacks, persist_migration_failed,
    update_application_id, update_application_with_migration,
};
use crate::ContextManager;
use calimero_context_client::group::MigrationFailureKind;
use calimero_governance_store::metrics::ExecutionLabels;

mod governance_position;
mod signing;
pub mod storage;
mod upgrade_gate;

use governance_position::compute_governance_position_for_context;
pub(crate) use signing::{persist_signed_signatures, sign_authorized_actions};
use storage::{ContextPrivateStorage, ContextStorage, ReadOnlyContextStorage};
use upgrade_gate::{
    maybe_lazy_upgrade, resolve_producing_app_key, should_block, upgrade_blocks_write,
    upgrade_rejects_committed_write, LazyUpgradeAction,
};

impl Handler<ExecuteRequest> for ContextManager {
    type Result = ActorResponse<Self, <ExecuteRequest as Message>::Result>;

    fn handle(
        &mut self,
        ExecuteRequest {
            context: context_id,
            executor,
            method,
            payload,
            aliases,
            atomic,
            xcall_origin,
        }: ExecuteRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        info!(
            %context_id,
            method,
            "Executing method in context"
        );
        debug!(
            %context_id,
            %executor,
            method,
            aliases = ?aliases,
            payload_len = payload.len(),
            atomic = %match atomic {
                None => "no",
                Some(ContextAtomic::Lock) => "acquire",
                Some(ContextAtomic::Held(_)) => "yes",
            },
            "Execution request details"
        );

        // --- Read-only intent lookup (before the context borrow) ---
        // Query the read-only method set now, while we still have an unambiguous
        // &mut self. After `get_or_fetch_context` the borrow-checker treats self
        // as mutably borrowed through `context`, and won't allow a second
        // (immutable) field access. We clone the result so the borrow is fully
        // released before context is fetched.
        //
        // This is safe: `read_only_methods` is a BoundedCache<key, Arc<HashSet>>
        // populated alongside the module cache; a cold miss (None) silently
        // defaults to the write lock.
        let is_state_op = "__calimero_sync_next" == method;

        // Shadow ACL-plane feed (additive — nothing reads the projection yet).
        // Capture the Shared anchors this sync-apply touches + the delta id,
        // decoded here while `payload` is still owned (execution moves it). After
        // the apply succeeds we read back the RAW rotation entries those anchors
        // recorded for this delta (with their signer) and fold them in — the
        // independent source, not the resolver's merged output. `None` for
        // non-`CausalActions` / writer-free deltas.
        let acl_shadow_objects = if is_state_op {
            match borsh::from_slice::<StorageDelta>(&payload) {
                Ok(StorageDelta::CausalActions {
                    effective_writers,
                    delta_id,
                    ..
                }) if !effective_writers.is_empty() => {
                    Some((effective_writers.into_keys().collect::<Vec<_>>(), delta_id))
                }
                _ => None,
            }
        } else {
            None
        };

        let is_read_only_call = 'ro: {
            if is_state_op || matches!(atomic, Some(ContextAtomic::Held(_))) {
                break 'ro false;
            }
            // We don't yet have `context`, so we can't form the full cache key
            // yet. Peek at `contexts` to get the application_id + service_name,
            // then look up read_only_methods.  Both are reads with no structural
            // changes, so this is safe even though contexts is &mut below.
            let Some(cm) = self.contexts.get(&context_id) else {
                break 'ro false; // not cached yet — conservative write lock
            };
            let application_id = cm.meta.application_id;
            let service_name = cm.meta.service_name.clone();
            // Blob-keyed lookup: the read-only sets are keyed by the executing
            // bytecode blob (per-context binding), with the cached row blob as
            // fallback. A miss defaults to the write lock (fail-safe).
            let Some(blob) = self.executing_blob_for_context(&context_id).or_else(|| {
                self.applications
                    .get(&application_id)
                    .map(|app| app.blob.bytecode)
            }) else {
                break 'ro false;
            };
            let Some(set) = self.read_only_methods.get(&(blob, service_name)).cloned() else {
                break 'ro false;
            };
            set.contains(method.as_str())
        };

        let context = match self.get_or_fetch_context(&context_id) {
            Ok(Some(context)) => context,
            Ok(None) => return ActorResponse::reply(Err(ExecuteError::ContextNotFound)),
            Err(err) => {
                error!(%err, "failed to execute request");

                return ActorResponse::reply(Err(ExecuteError::InternalError));
            }
        };

        let current_application_id = context.meta.application_id;

        if !is_state_op && *context.meta.root_hash == [0; 32] {
            return ActorResponse::reply(Err(ExecuteError::Uninitialized));
        }

        let (guard, is_atomic) = match atomic {
            None => {
                let g = if is_read_only_call {
                    context.lock_read()
                } else {
                    context.lock()
                };
                (g, false)
            }
            Some(ContextAtomic::Lock) => (context.lock(), true),
            Some(ContextAtomic::Held(ContextAtomicKey(guard))) => (Either::Left(guard), true),
        };

        // In-progress-upgrade write-gate: while the owning group is `InProgress`,
        // refuse writes (a committed write risks cross-version drift with
        // already-migrated group-mates) but keep serving reads from the
        // pre-migration root. Read-vs-write intent isn't known upstream, so
        // user-call writes are caught post-execution in `internal_execute` (we
        // only record the group here); state-ops are known writes, refused now.
        // `LazyOnAccess` upgrades write `Completed`, never `InProgress`.
        let mut block_writes_for_group = None;
        match calimero_governance_store::get_group_for_context(&self.datastore, &context_id) {
            Ok(Some(group_id)) => {
                match calimero_governance_store::UpgradesRepository::new(&self.datastore)
                    .load(&group_id)
                {
                    Ok(Some(upgrade)) => {
                        if should_block(self.config.migration_v2, &upgrade.status) {
                            if is_state_op {
                                // Known write — refuse before execution.
                                warn!(
                                    %context_id,
                                    ?group_id,
                                    method,
                                    is_state_op,
                                    "refusing state-op execute: group upgrade in progress"
                                );
                                return ActorResponse::reply(Err(
                                    ExecuteError::UpgradeInProgress { group_id },
                                ));
                            }
                            // User call: allow it to execute against the
                            // pre-migration root; reject post-execution only if
                            // it actually mutates state (a write). Reads pass.
                            block_writes_for_group = Some(group_id);
                        } else if self.config.migration_v2 && upgrade_blocks_write(&upgrade.status)
                        {
                            // The freeze was bypassed by `migration_v2`; log so a
                            // canary operator can tell the flag skipped it (not a
                            // missing upgrade row). Stragglers are absorbed.
                            debug!(
                                %context_id,
                                ?group_id,
                                "migration_v2: bypassing InProgress write-freeze (flag on)"
                            );
                        }
                    }
                    Ok(None) => {
                        // No upgrade row for this group → not in progress, allow.
                    }
                    Err(err) => {
                        error!(
                            %context_id,
                            ?group_id,
                            %err,
                            "cascade gate: failed to load GroupUpgradeStatus"
                        );
                        return ActorResponse::reply(Err(ExecuteError::InternalError));
                    }
                }
            }
            Ok(None) => {
                // Context not registered to any group → no cascade gate applies.
            }
            Err(err) => {
                error!(
                    %context_id,
                    %err,
                    "cascade gate: failed to resolve owning group"
                );
                return ActorResponse::reply(Err(ExecuteError::InternalError));
            }
        }

        // Lazy upgrade: if context belongs to a LazyOnAccess group and is stale,
        // trigger an upgrade before executing the method.
        // Note: placed after context.lock() so that `context` borrow is released
        // before we access self.datastore.
        // Skip for sync operations — the state payload was produced by the old app
        // version and must be applied as-is, not against a newly upgraded WASM.
        // Also skip while a write-gating upgrade is in progress: `InProgress` is
        // set only on the cascade emitter, whose eager propagator owns the
        // migration, so a user call here must not trigger its own redundant
        // per-call migration (a read is served from the current committed root;
        // a write is refused post-execution).
        let lazy_upgrade_params = if is_state_op || block_writes_for_group.is_some() {
            None
        } else {
            maybe_lazy_upgrade(&self.datastore, &context_id, &current_application_id)
        };

        match self.context_client.context_config(&context_id) {
            Ok(Some(_)) => {}
            Ok(None) => {
                error!(%context_id, "missing context config for context");

                return ActorResponse::reply(Err(ExecuteError::InternalError));
            }
            Err(err) => {
                error!(%err, "failed to execute request");

                return ActorResponse::reply(Err(ExecuteError::InternalError));
            }
        }

        let identity = match self.context_client.get_identity(&context_id, &executor) {
            Ok(Some(
                identity @ ContextIdentity {
                    private_key: Some(_),
                    ..
                },
            )) => identity,
            Ok(_) => {
                return ActorResponse::reply(Err(ExecuteError::Unauthorized {
                    context_id,
                    public_key: executor,
                }))
            }
            Err(err) => {
                error!(%err, "failed to execute request");

                return ActorResponse::reply(Err(ExecuteError::InternalError));
            }
        };

        let private_key = identity.private_key.expect(
            "infallible (verified before): missing private key in ContextIdentity for signing",
        );

        // Issue #2256: align state-delta crypto with subgroup-visibility
        // model. An `Open` subgroup *whose entire ancestor chain to the
        // namespace is also Open* is by definition readable by every
        // namespace member (including inheritance-eligible parent
        // members), so its context state deltas are encrypted with the
        // *namespace* key — not the subgroup's own per-subgroup key.
        // This is symmetric with the governance-op encryption choice in
        // `GroupGovernancePublisher`. The chain check (rather than just
        // immediate visibility) prevents widening the crypto boundary
        // for a subgroup that sits behind a `Restricted` ancestor — the
        // membership walk would refuse inheritance there, so namespace
        // members must not be given decrypt access to its content.
        // Restricted (or unset) subgroups, and Open subgroups behind a
        // Restricted ancestor, continue to use their own per-subgroup
        // key.
        let (sender_key, broadcast_key_id) =
            match calimero_governance_store::get_group_for_context(&self.datastore, &context_id) {
                Ok(Some(gid)) => {
                    // Errors from `resolve_namespace` or
                    // `is_open_chain_to_namespace` mean we cannot reliably
                    // decide *which* key (subgroup vs. namespace) to encrypt
                    // with — they signal store corruption (cyclic parent
                    // edges, missing namespace metadata, etc.). Silently
                    // falling back to the subgroup key would mask the
                    // corruption *and* mis-encrypt for inheritance-eligible
                    // receivers, who'd then fail to decrypt. Mirror the
                    // governance-publisher path: propagate the error.
                    let ns_id = match NamespaceRepository::new(&self.datastore).resolve(&gid) {
                        Ok(ns_id) => ns_id,
                        Err(err) => {
                            error!(
                                group_id = ?gid,
                                ?context_id,
                                %err,
                                "state-delta encryption: resolve_namespace failed",
                            );
                            return ActorResponse::reply(Err(ExecuteError::InternalError));
                        }
                    };
                    let key_group_id = match CapabilitiesRepository::new(&self.datastore)
                        .is_open_chain_to_namespace(&gid, &ns_id)
                    {
                        Ok(true) => ns_id,
                        Ok(false) => gid,
                        Err(err) => {
                            error!(
                                group_id = ?gid,
                                namespace_id = ?ns_id,
                                ?context_id,
                                %err,
                                "state-delta encryption: is_open_chain_to_namespace failed",
                            );
                            return ActorResponse::reply(Err(ExecuteError::InternalError));
                        }
                    };
                    // Group-context branch: the group/namespace key is the
                    // *authoritative* encryption key. Falling back to
                    // `identity.sender_key` here would produce ciphertext
                    // that other group members cannot decrypt — silent
                    // state divergence across the cluster. With the
                    // Phase 9 join-time KeyDelivery wait in place, the
                    // joiner should already hold this key by the time
                    // they call `execute`. If we get here with no key,
                    // it's a real "key not yet delivered" condition and
                    // the caller needs to know — surface it loudly
                    // rather than silently mis-encrypting.
                    match GroupKeyring::new(&self.datastore, key_group_id).load_current_key() {
                        Ok(Some((kid, gk))) => (PrivateKey::from(gk), kid),
                        Ok(None) => {
                            // Surface the "key not yet delivered" condition as
                            // a typed retry-able variant so admin/client
                            // surfaces can distinguish it from permanent
                            // failures (which still return `InternalError`).
                            // The local DAG is healthy and the membership row
                            // exists; only the group key is missing — the
                            // gossip-fallback path or a fresh `join_group`
                            // retry will resolve this.
                            error!(
                                group_id = ?gid,
                                key_group_id = ?key_group_id,
                                ?context_id,
                                "state-delta encryption: group key not yet delivered \
                                 (KeyDelivery pending or failed) — refusing sender_key fallback \
                                 to avoid mis-encrypting for inheritance-eligible receivers"
                            );
                            return ActorResponse::reply(Err(ExecuteError::GroupKeyPending {
                                context_id,
                            }));
                        }
                        Err(err) => {
                            error!(
                                group_id = ?gid,
                                key_group_id = ?key_group_id,
                                ?context_id,
                                %err,
                                "state-delta encryption: load_current_group_key failed",
                            );
                            return ActorResponse::reply(Err(ExecuteError::InternalError));
                        }
                    }
                }
                Ok(None) => {
                    // Non-group context: legitimately falls back to the
                    // identity's own sender_key.
                    if let Some(sk) = identity.sender_key {
                        (sk, [0u8; 32])
                    } else {
                        return ActorResponse::reply(Err(ExecuteError::InternalError));
                    }
                }
                Err(err) => {
                    error!(
                        ?context_id,
                        %err,
                        "state-delta encryption: get_group_for_context failed",
                    );
                    return ActorResponse::reply(Err(ExecuteError::InternalError));
                }
            };

        // Resolve the producing-app-key for the broadcast envelope. Runs
        // synchronously here so the `Option<[u8;32]>` (Copy) can be
        // captured by value in the async external_task closure below.
        // `get_group_for_context` is called a second time internally;
        // that's one extra O(1) store read per execute, acceptable here.
        //
        // SECURITY TRADEOFF (fence hole, accepted): a store error here stamps
        // `None` ⇒ this delta is not fenceable by receivers (they treat `None`
        // as "no fence decision possible" and apply it). We accept that narrow
        // hole as a liveness-over-strictness tradeoff for a transient/local
        // store fault — failing execute on a store hiccup would harm liveness
        // far more than the rare unfenceable delta — and surface it at `warn!`
        // so the gap is observable rather than silent.
        let producing_app_key: Option<[u8; 32]> =
            match resolve_producing_app_key(&self.datastore, &context_id) {
                Ok(v) => v,
                Err(err) => {
                    warn!(
                        ?context_id,
                        %err,
                        "resolve_producing_app_key failed, stamping None on broadcast"
                    );
                    None
                }
            };

        debug!(
            public_key = ?identity.public_key,
            public_key = %identity.public_key,
            "ContextManager: keys",
        );

        let payload =
            match substitute_aliases_in_payload(&self.node_client, context_id, payload, &aliases) {
                Ok(payload) => payload,
                Err(err) => {
                    error!(%err, "failed to execute request");

                    return ActorResponse::reply(Err(err));
                }
            };

        let guard_task = async move {
            match guard {
                Either::Left(guard) => guard,
                Either::Right(task) => task.await,
            }
        }
        .into_actor(self);

        // Extract actor-owned values for the lazy upgrade path synchronously so the
        // context_task future can call update_application_id / update_application_with_migration
        // directly without routing through the actor mailbox (which would deadlock while an
        // ActorFuture is in flight on the same actor).
        let lazy_upgrade_task = guard_task.map(move |guard, _act, _ctx| {
            if let Some(action) = lazy_upgrade_params {
                info!(%context_id, %executor, "performing lazy upgrade before execution");
                return Ok(Either::Right((guard, action)));
            }
            Ok(Either::Left(guard))
        });

        let context_task = lazy_upgrade_task.and_then(move |either, act, _ctx| {
            let (guard, action) = match either {
                Either::Left(guard) => {
                    return async move { Ok(guard) }.into_actor(act).boxed_local()
                }
                Either::Right(parts) => parts,
            };
            match action {
                // Replay the group's upgrade ladder hop by hop, re-resolving
                // after each committed hop. The per-access budget bounds a
                // pathological marker-write failure loop; a longer ladder
                // resumes on the next access from the last committed rung.
                //
                // A marker-less context (a fresh joiner whose group has since
                // advanced) is routed here with `bound` = its current row
                // version. Seed the activation marker to it so the replay starts
                // from the real version AND execution binds to it — without the
                // seed, a blocked hop would fall through to the group-target
                // bytecode and run new code on un-migrated state.
                LazyUpgradeAction::Replay { bound } => {
                    if crate::activation::activated_blob(&act.datastore, &context_id).is_none() {
                        crate::activation::record_activation(&act.datastore, &context_id, bound);
                    }
                    act.replay_upgrade_ladder(
                        guard,
                        context_id,
                        executor,
                        ContextManager::LADDER_HOP_BUDGET,
                    )
                }
                // Marker-less context: the pre-ladder single jump to the
                // group's current target, method from the group-level hint.
                LazyUpgradeAction::SingleJump {
                    target_application_id: target_app,
                    migrate_method: migrate,
                    target_app_key,
                } => {
                    let datastore = act.datastore.clone();
                    let node_client = act.node_client.clone();
                    let context_client = act.context_client.clone();
                    let context_meta = act.contexts.get(&context_id).map(|c| c.meta.clone());
                    let application = act.applications.get(&target_app).cloned();
                    let cid = context_id;
                    if let Some(method) = migrate {
                        let migration_params = MigrationParams { method: method.clone() };
                        let service_name = context_meta.as_ref().and_then(|c| c.service_name.clone());
                        // The migrate must execute the TARGET bytecode. Load it
                        // straight from the group's recorded target blob
                        // (fetching from peers when absent) — the application
                        // row is a download cache and may still hold the
                        // previous version.
                        let blob_node_client = node_client.clone();
                        async move {
                            ensure_blob_local(&blob_node_client, &cid, target_app_key).await
                        }
                        .into_actor(act)
                        .then(move |blob_local, act, _ctx| {
                            // Carry `blob_local` forward: the migrate runs the
                            // TARGET bytecode only when the blob was actually
                            // local. If we fell back to the row's (possibly
                            // stale) bytecode, the activation marker must NOT be
                            // recorded below.
                            let module_fut = if blob_local {
                                act.get_module_for_blob(target_app_key.into(), service_name)
                                    .boxed_local()
                            } else {
                                // Legacy groups (randomly-seeded app_key that
                                // resolves to no blob) and failed fetches: the
                                // row's bytecode is the only available truth.
                                act.evict_application_caches(target_app);
                                act.get_module(target_app, service_name).boxed_local()
                            };
                            // `module_fut` is an ActorFuture, so pair `blob_local`
                            // with its result via ActorFutureExt::map (not a plain
                            // async block, which can't await an ActorFuture).
                            module_fut.map(move |m, _act, _ctx| (blob_local, m))
                        })
                            .then(move |(blob_local, module_result), act, _ctx| {
                                // Re-read cached values; they may have been refreshed during load
                                let context_meta =
                                    act.contexts.get(&cid).map(|c| c.meta.clone());
                                let application = act.applications.get(&target_app).cloned();
                                let migration_v2 = act.config.migration_v2;
                                async move {
                                    match module_result {
                                        Ok(module) => {
                                            match update_application_with_migration(
                                                datastore.clone(),
                                                node_client,
                                                context_client,
                                                cid,
                                                context_meta,
                                                target_app,
                                                application,
                                                executor,
                                                Some(migration_params),
                                                module,
                                                migration_v2,
                                            )
                                            .await
                                            {
                                                Ok(_) if blob_local => {
                                                    // Unified activation marker: the single
                                                    // up-to-date signal for the gate, the lazy
                                                    // trigger, and the rollup. Recorded only when
                                                    // the migrate ran the TARGET bytecode.
                                                    crate::activation::record_activation(
                                                        &datastore,
                                                        &cid,
                                                        target_app_key,
                                                    );
                                                }
                                                Ok(_) => {
                                                    // Migrate ran against the application row
                                                    // (target blob unavailable). Do NOT record
                                                    // activation, so the lazy trigger keeps
                                                    // retrying instead of wedging the context on
                                                    // old bytecode behind an up-to-date marker.
                                                    warn!(
                                                        %cid,
                                                        %target_app,
                                                        "lazy migrate ran against the application row (target blob unavailable); not recording activation"
                                                    );
                                                }
                                                Err(err) => {
                                                    warn!(
                                                        %cid,
                                                        %target_app,
                                                        %err,
                                                        "lazy upgrade (migration) failed, proceeding with current application"
                                                    );
                                                }
                                            }
                                        }
                                        Err(err) => {
                                            warn!(
                                                %cid,
                                                %target_app,
                                                %err,
                                                "failed to load module for lazy upgrade migration"
                                            );
                                        }
                                    }
                                    Ok(guard)
                                }
                                .into_actor(act)
                            })
                            .boxed_local()
                    } else {
                        // No migration. A same-id (bundle) code-only bump
                        // activates by marker move alone: fetch the target
                        // blob if absent (sync pre-stages it, but a peer can
                        // also serve it on demand) and record the activation.
                        // The application row is never reinstalled — the
                        // per-context binding decides what executes. A failed
                        // fetch must NOT record the marker, or the lazy
                        // trigger would stop retrying while the node still
                        // runs the old build.
                        let blob_node_client = node_client.clone();
                        async move {
                            ensure_blob_local(&blob_node_client, &cid, target_app_key).await
                        }
                        .into_actor(act)
                        .then(move |blob_available, act, _ctx| {
                            if blob_available {
                                // Drop the cached application row so the
                                // update below re-reads it fresh.
                                act.evict_application_caches(target_app);
                            }
                            let marker_datastore = act.datastore.clone();
                            async move {
                                match update_application_id(
                                    datastore,
                                    node_client,
                                    context_client,
                                    cid,
                                    context_meta,
                                    target_app,
                                    application,
                                    executor,
                                )
                                .await
                                {
                                    Ok(_) if blob_available => {
                                        // Marker AFTER the flip: the update
                                        // records the row's blob, which for a
                                        // same-id bump may still be the
                                        // previous version — the group target
                                        // (what this context executes now)
                                        // must win.
                                        crate::activation::record_activation(
                                            &marker_datastore,
                                            &cid,
                                            target_app_key,
                                        );
                                    }
                                    Ok(_) => {}
                                    Err(err) => {
                                        warn!(
                                            %cid,
                                            %target_app,
                                            %err,
                                            "lazy upgrade failed, proceeding with current application"
                                        );
                                    }
                                }
                                Ok(guard)
                            }
                            .into_actor(act)
                        })
                        .boxed_local()
                    }
                }
            }
        });

        // Re-fetch context after possible lazy upgrade (application_id may have changed)
        let context_task =
            context_task.map(move |guard_result: eyre::Result<ContextGuard>, act, _ctx| {
                let guard = guard_result?;
                let Some(context) = act.get_or_fetch_context(&context_id)? else {
                    bail!(ContextError::ContextDeleted { context_id });
                };

                Ok((guard, context.meta.clone()))
            });

        let module_task = context_task.and_then(move |(guard, context), act, _ctx| {
            // Per-context bytecode binding: a context executes the blob its
            // activation marker points at, else the blob its group's
            // `app_key` points at, else the application row (non-group
            // contexts, legacy groups). Cost: a couple of bloom-filtered
            // point-gets, noise next to the wasm call they precede.
            let module_fut = match act.executing_blob_for_context(&context.id) {
                Some(blob) => act
                    .get_module_for_blob(blob, context.service_name.clone())
                    .boxed_local(),
                None => act
                    .get_module(context.application_id, context.service_name.clone())
                    .boxed_local(),
            };
            module_fut.map_ok(move |module, _act, _ctx| (guard, context, module))
        });

        let execution_count = self.metrics.as_ref().map(|m| m.execution_count.clone());
        let execution_duration = self.metrics.as_ref().map(|m| m.execution_duration.clone());

        let execute_task = module_task.and_then(move |(guard, mut context, module), act, _ctx| {
            let datastore = act.datastore.clone();
            let node_client = act.node_client.clone();
            let context_client = act.context_client.clone();

            // For an xcall, deny any method the target app didn't mark
            // `#[app::xcall]`. No declared set ⇒ not gated. Keyed by the
            // executing blob, like the read-only lookup above. Applies to every
            // xcall-dispatched run, including internal methods like
            // `__calimero_sync_next` — a guest must not reach those via xcall
            // (they are never `#[app::xcall]`); the sync path itself carries no
            // origin, so legitimate state ops are unaffected.
            let xcall_blob = act.executing_blob_for_context(&context.id).or_else(|| {
                act.applications
                    .get(&context.application_id)
                    .map(|app| app.blob.bytecode)
            });
            let xcall_denied = xcall_origin.is_some()
                && xcall_blob.is_some_and(|blob| {
                    act.xcall_methods
                        .get(&(blob, context.service_name.clone()))
                        .is_some_and(|set| !set.contains(method.as_str()))
                });

            // Cheap (Arc-backed) clone kept past internal_execute (which moves
            // `datastore`) so a post-call migrate_my_entries can refresh the
            // node-local authored_remaining count (6f.8 drop-after-convert).
            let count_datastore = datastore.clone();

            async move {
                if xcall_denied {
                    warn!(
                        %context_id,
                        function = %method,
                        "xcall denied: not an #[app::xcall] entry point"
                    );
                    bail!(ExecuteError::XCallNotPermitted { context_id });
                }

                let old_root_hash = context.root_hash;

                let start = Instant::now();

                let (outcome, causal_delta, delta_signature, signing_governance_position) =
                    internal_execute(
                        datastore,
                        &node_client,
                        &context_client,
                        module,
                        &guard,
                        &mut context,
                        executor,
                        method.clone().into(),
                        payload.into(),
                        is_state_op,
                        is_read_only_call,
                        block_writes_for_group,
                        &private_key,
                        xcall_origin,
                    )
                    .await?;

                let duration = start.elapsed().as_secs_f64();
                let status = if outcome.returns.is_ok() {
                    "success"
                } else {
                    "failure"
                };

                // Update execution count metrics
                if let Some(execution_count) = execution_count {
                    let _ignored = execution_count
                        .clone()
                        .get_or_create(&ExecutionLabels {
                            context_id: context_id.to_string(),
                            method: method.clone(),
                            status: status.to_owned(),
                        })
                        .inc();
                }

                // Update execution duration metrics
                if let Some(execution_duration) = execution_duration {
                    execution_duration
                        .clone()
                        .get_or_create(&ExecutionLabels {
                            context_id: context_id.to_string(),
                            method: method.clone(),
                            status: status.to_owned(),
                        })
                        .observe(duration);
                }

                info!(
                    %context_id,
                    method,
                    status,
                    "Method execution completed"
                );

                // After the owner converts their authored entries via the
                // SDK-generated migrate_my_entries export, refresh the node-local
                // authored_remaining from the summary's `remaining` so the
                // heartbeat self-report (and the admin rollup) reflect the
                // post-convert count (6f.8). This is self-reported advisory
                // telemetry about THIS node's own pending count — never a gate —
                // so the value is inherently self-attested (like the rest of the
                // heartbeat); we only guard against a nonsense cast by saturating
                // the u64→u32 instead of silently wrapping. Apps that wrap
                // migrate_my_entries under another name simply won't refresh here
                // (acceptable for advisory telemetry).
                if method == "migrate_my_entries" {
                    if let Ok(Some(bytes)) = &outcome.returns {
                        // Only trust a well-formed MigrateMyEntriesSummary
                        // ({converted, remaining}) — deserializing into the typed
                        // shape (both u32 fields required) rejects an unrelated /
                        // error JSON payload that merely happens to carry a
                        // `remaining` key, so a malformed return never writes a
                        // bogus authored_remaining.
                        #[derive(serde::Deserialize)]
                        struct MigrateSummary {
                            #[allow(dead_code)]
                            converted: u32,
                            remaining: u32,
                        }
                        if let Ok(summary) = serde_json::from_slice::<MigrateSummary>(bytes) {
                            crate::handlers::update_application::persist_authored_remaining(
                                &count_datastore,
                                context_id,
                                summary.remaining,
                            );
                        }
                    }
                }
                debug!(
                    %context_id,
                    %executor,
                    method,
                    status,
                    %old_root_hash,
                    new_root_hash=%context.root_hash,
                    artifact_len = outcome.artifact.len(),
                    logs_count = outcome.logs.len(),
                    events_count = outcome.events.len(),
                    xcalls_count = outcome.xcalls.len(),
                    "Execution outcome details"
                );

                Ok((
                    guard,
                    context,
                    outcome,
                    causal_delta,
                    delta_signature,
                    signing_governance_position,
                ))
            }
            .into_actor(act)
        });

        let external_task =
            execute_task.and_then(move |(guard, context, outcome, causal_delta, delta_signature, signing_governance_position), act, _ctx| {
                if let Some(cached_context) = act.contexts.get_mut(&context_id) {
                    debug!(
                        %context_id,
                        old_root = ?cached_context.meta.root_hash,
                        new_root = ?context.root_hash,
                        is_state_op,
                        "Updating cached context root_hash"
                    );
                    cached_context.meta.root_hash = context.root_hash;
                } else {
                    debug!(%context_id, is_state_op, "Context not in cache, will be fetched from DB next time");
                }

                let node_client = act.node_client.clone();
                let context_client = act.context_client.clone();
                // Read-only snapshot for the xcall namespace check below.
                let xcall_datastore = act.datastore.clone();
                let scope_projections = std::sync::Arc::clone(&act.scope_projections);
                // `datastore_for_broadcast` used to recompute the
                // governance position at broadcast time — that recompute
                // produced the persist-vs-broadcast signature mismatch
                // documented on `governance_position_for_broadcast`.
                // The threaded value from `internal_execute` is the
                // single source of truth now, so no fresh store
                // snapshot is needed here.

                async move {
                    if outcome.returns.is_err() {
                        return Ok((guard, context.root_hash, outcome));
                    }

                    // Apply succeeded — recover the RAW rotation entries this
                    // delta recorded for the touched anchors and fold a
                    // SetWriters op (authored by the rotation signer) per
                    // rotation into the scope's shadow projection. Additive;
                    // nothing reads it yet. Reads are synchronous and happen
                    // before the lock; a poisoned lock is ignored, so the shadow
                    // can never affect execution.
                    if let Some((objects, delta_id)) = &acl_shadow_objects {
                        let scope = calimero_op::ScopeId::from(*context_id.digest());
                        let mut ops = Vec::new();
                        for object in objects {
                            let Some(log) = crate::scope_projection::load_rotation_log_direct(
                                &context_client,
                                context_id,
                                *object,
                            ) else {
                                continue;
                            };
                            for entry in log.entries.iter().filter(|e| e.delta_id == *delta_id) {
                                if let Some(op) = crate::scope_projection::op_from_rotation_entry(
                                    *object, scope, entry,
                                ) {
                                    ops.push(op);
                                }
                            }
                        }
                        if !ops.is_empty() {
                            match scope_projections.write() {
                                Ok(mut projections) => {
                                    for op in &ops {
                                        projections.ingest_op(op);
                                    }
                                }
                                // A poisoned lock skips the shadow feed with a
                                // warning; it must never affect execution.
                                Err(err) => tracing::warn!(
                                    %err,
                                    "scope-projections lock poisoned; skipping ACL shadow feed"
                                ),
                            }
                        }
                    }

                    info!(
                        %context_id,
                        %executor,
                        is_state_op,
                        artifact_empty = outcome.artifact.is_empty(),
                        events_count = outcome.events.len(),
                        xcalls_count = outcome.xcalls.len(),
                        "Execution outcome details"
                    );

                    // Event handlers are NOT executed on the sender node.
                    // They are dispatched on receiver nodes only (see state_delta handler).
                    // This is correct because:
                    // 1. The sender already performed its action in the originating method call.
                    // 2. Handlers often need the *receiver's* identity (e.g. acknowledge_shot
                    //    must run as the target player, not the shooter).
                    // 3. Executing on both would cause duplicate CRDT mutations.
                    //
                    // The handler field is preserved in the broadcast so receivers can
                    // pick it up via execute_event_handlers_parsed().

                    // Process cross-context calls
                    // NOTE: XCalls are executed locally on the current node after the main execution completes.
                    // This allows contexts to communicate by calling functions on other contexts.
                    for xcall in &outcome.xcalls {
                        let target_context_id = ContextId::from(xcall.context_id);

                        info!(
                            %context_id,
                            target_context = ?target_context_id,
                            function = %xcall.function,
                            params_len = xcall.params.len(),
                            "Processing cross-context call"
                        );

                        // Best-effort observability event for this xcall (#2137).
                        // Source rides on the wrapper `context_id`; emission
                        // failure must never abort the loop or the source exec.
                        let emit = |outcome: XCallOutcome| {
                            let _ = node_client.send_event(NodeEvent::Context(ContextEvent {
                                context_id,
                                payload: ContextEventPayload::XCall(XCallPayload {
                                    target_context_id,
                                    function: xcall.function.clone(),
                                    outcome,
                                }),
                            }));
                        };

                        // A context may only xcall a target in its own namespace.
                        // A resolution error or unresolved namespace denies.
                        let same_namespace = (|| -> eyre::Result<bool> {
                            let src = resolve_namespace_for_context(&xcall_datastore, &context_id)?;
                            let tgt =
                                resolve_namespace_for_context(&xcall_datastore, &target_context_id)?;
                            Ok(matches!((src, tgt), (Some(a), Some(b)) if a == b))
                        })();
                        if !matches!(same_namespace, Ok(true)) {
                            warn!(
                                %context_id,
                                target_context = ?target_context_id,
                                function = %xcall.function,
                                resolved = ?same_namespace,
                                "xcall denied: namespace boundary"
                            );
                            emit(XCallOutcome::Denied {
                                reason: "namespace boundary".to_owned(),
                            });
                            continue;
                        }

                        // Find an owned member of the target context to execute as
                        // We need to use a member that has permissions on the target context
                        use futures_util::TryStreamExt;
                        let members: Vec<_> = context_client
                            .get_context_members(&target_context_id, Some(true))
                            .try_collect()
                            .await
                            .unwrap_or_default();

                        let Some((target_executor, _is_owned)) = members.first() else {
                            warn!(
                                %context_id,
                                target_context = ?target_context_id,
                                function = %xcall.function,
                                "xcall denied: no owned member of target context"
                            );
                            emit(XCallOutcome::Denied {
                                reason: "no owned member".to_owned(),
                            });
                            continue;
                        };

                        let target_executor = *target_executor;

                        info!(
                            %context_id,
                            target_context = ?target_context_id,
                            target_executor = ?target_executor,
                            "Found owned member for target context"
                        );

                        // Execute as the target's member, tagging the call with
                        // the source context so the target can read it via
                        // `env::xcall_origin()`. The node sets the origin here —
                        // never from guest memory. The target's handler rejects
                        // non-`#[app::xcall]` methods with XCallNotPermitted.
                        let xcall_result = context_client
                            .execute_with_origin(
                                &target_context_id,
                                &target_executor,
                                xcall.function.clone(),
                                xcall.params.clone(),
                                vec![],
                                None,
                                Some(context_id),
                            )
                            .await;

                        match xcall_result {
                            // A normally-finished run returns `Ok(ExecuteResponse)`
                            // even when the target *method* errored — that failure
                            // lives in `returns`, so inspect it before reporting Ok.
                            Ok(response) => match &response.returns {
                                Ok(_) => {
                                    info!(
                                        %context_id,
                                        target_context = ?target_context_id,
                                        function = %xcall.function,
                                        "Cross-context call executed successfully"
                                    );
                                    emit(XCallOutcome::Ok);
                                }
                                Err(err) => {
                                    warn!(
                                        %context_id,
                                        target_context = ?target_context_id,
                                        function = %xcall.function,
                                        %err,
                                        "Cross-context call target method returned an error"
                                    );
                                    emit(XCallOutcome::ExecError {
                                        message: err.to_string(),
                                    });
                                }
                            },
                            // A rejected entry point is a denial, not an exec error.
                            Err(ExecuteError::XCallNotPermitted { .. }) => {
                                warn!(
                                    %context_id,
                                    target_context = ?target_context_id,
                                    function = %xcall.function,
                                    "xcall denied: not an #[app::xcall] entry point"
                                );
                                emit(XCallOutcome::Denied {
                                    reason: "not an xcall entry point".to_owned(),
                                });
                            }
                            Err(err) => {
                                error!(
                                    %context_id,
                                    target_context = ?target_context_id,
                                    function = %xcall.function,
                                    ?err,
                                    "Cross-context call failed"
                                );
                                emit(XCallOutcome::ExecError {
                                    message: err.to_string(),
                                });
                            }
                        }
                    }

                    // Broadcast state deltas to other nodes when:
                    // 1. It's not a state synchronization operation (is_state_op = false)
                    // 2. AND there's a state change artifact (non-empty artifact)
                    //
                    // This ensures that:
                    // - State changes are broadcast when there are actual state changes
                    // - State synchronization operations don't trigger broadcasts (prevents loops)
                    // - Events are still broadcast via WebSocket regardless of state changes
                    if !(is_state_op || outcome.artifact.is_empty()) {
                        info!(
                            %context_id,
                            %executor,
                            is_state_op,
                            artifact_empty = outcome.artifact.is_empty(),
                            events_count = outcome.events.len(),
                            has_delta = causal_delta.is_some(),
                            "Broadcasting state delta and events to other nodes"
                        );

                        if let Some(ref the_delta) = causal_delta {
                            // Serialize events if any were emitted
                            let events_data = if outcome.events.is_empty() {
                                info!(
                                    %context_id,
                                    %executor,
                                    "No events to serialize"
                                );
                                None
                            } else {
                                // Preserve handler fields so receiver nodes can execute them.
                                // Handlers are only executed on receiver nodes, not on the sender.
                                let events_vec: Vec<ExecutionEvent> = outcome
                                    .events
                                    .iter()
                                    .map(|e| ExecutionEvent {
                                        kind: e.kind.clone(),
                                        data: e.data.clone(),
                                        handler: e.handler.clone(),
                                    })
                                    .collect();
                                let serialized = serde_json::to_vec(&events_vec)?;
                                info!(
                                    %context_id,
                                    %executor,
                                    events_count = events_vec.len(),
                                    handlers_with_handlers = events_vec.iter().filter(|e| e.handler.is_some()).count(),
                                    serialized_len = serialized.len(),
                                    "Serializing events for broadcast"
                                );
                                Some(serialized)
                            };

                            // Cross-DAG reference: the EXACT governance cut
                            // `delta_signature` was bound to inside
                            // `internal_execute`. We reuse that captured
                            // value (`signing_governance_position`) instead
                            // of recomputing from `datastore_for_broadcast`
                            // because the local governance state can advance
                            // between the persist and broadcast points
                            // (member added/removed, namespace op landed).
                            // A fresh computation would silently diverge
                            // from the signed payload and receivers would
                            // reject the delta on signature mismatch.
                            let governance_position = signing_governance_position.clone();

                            node_client
                                .broadcast(
                                    &context,
                                    &executor,
                                    &sender_key,
                                    outcome.artifact.clone(),
                                    the_delta.id,
                                    the_delta.parents.clone(),
                                    the_delta.hlc,
                                    events_data,
                                    governance_position,
                                    broadcast_key_id,
                                    // Pre-signed envelope bytes — paired with
                                    // the exact `signing_governance_position`
                                    // above, see the comment there.
                                    delta_signature,
                                    // Resolved synchronously before this
                                    // async closure; `Option<[u8;32]>` is
                                    // Copy so captured by value automatically.
                                    producing_app_key,
                                )
                                .await?;
                        }
                    }

                    // Handler execution is deferred to receiver nodes only.
                    // See state_delta/mod.rs execute_event_handlers_parsed().

                    Ok((guard, context.root_hash, outcome))
                }
                .map_err(|err| {
                    error!(
                    ?err,
                    "execution succeeded, but an error occurred while performing external actions"
                );

                    err
                })
                .into_actor(act)
            });

        let task = external_task
            .map_err(|err, _act, _ctx| {
                err.downcast::<ExecuteError>().unwrap_or_else(|err| {
                    debug!(?err, "an error occurred while executing request");
                    ExecuteError::InternalError
                })
            })
            .map_ok(
                move |(guard, root_hash, outcome), _act, _ctx| ExecuteResponse {
                    returns: outcome.returns.map_err(Into::into),
                    logs: outcome.logs,
                    events: outcome
                        .events
                        .into_iter()
                        .map(|e| ExecuteEvent {
                            kind: e.kind,
                            data: e.data,
                            handler: e.handler,
                        })
                        .collect(),
                    root_hash,
                    artifact: outcome.artifact,
                    atomic: is_atomic.then_some(ContextAtomicKey(guard)),
                },
            );

        ActorResponse::r#async(task)
    }
}

impl ContextManager {
    /// Load the module for `application_id` via its row: resolve the row's
    /// top-level bytecode blob (the bundle blob for bundles, the raw wasm
    /// blob otherwise) and delegate to [`Self::get_module_for_blob`]. The
    /// row is a download-cache pointer ("latest fetched") — contexts bound
    /// to a specific version load through their blob directly.
    /// Max ladder hops one access replays — bounds a pathological
    /// marker-write failure loop; ladders are realistically 1-3 rungs and a
    /// longer one resumes on the next access from the last committed rung.
    const LADDER_HOP_BUDGET: u8 = 8;

    /// Replay the group's upgrade ladder for one context: each rung runs in
    /// that release's own bytecode, with its method resolved from the two
    /// blobs' embedded ABIs — the group-level migration hint is never
    /// executed here, since it describes only the group's most recent hop.
    /// A blocked or failed hop stops the walk and the call proceeds on the
    /// context's current version; activation is recorded per committed hop,
    /// so the next access resumes from a real version. `budget` bounds one
    /// access's hops (a longer ladder simply resumes on the next access).
    fn replay_upgrade_ladder(
        &mut self,
        guard: ContextGuard,
        context_id: ContextId,
        executor: PublicKey,
        budget: u8,
    ) -> actix::fut::LocalBoxActorFuture<Self, eyre::Result<ContextGuard>> {
        use calimero_governance_store::{get_group_for_context, UpgradeLadderRepository};

        let datastore = self.datastore.clone();
        let next = get_group_for_context(&datastore, &context_id)
            .ok()
            .flatten()
            .and_then(|gid| {
                let meta = MetaRepository::new(&datastore).load(&gid).ok().flatten()?;
                let bound = crate::activation::activated_blob(&datastore, &context_id)?;
                let ladder = UpgradeLadderRepository::new(&datastore)
                    .load(&gid)
                    .unwrap_or_default();
                crate::activation::next_rung(
                    &ladder,
                    bound,
                    meta.app_key,
                    meta.target_application_id,
                )
                .map(|rung| (rung, bound))
            });

        let Some((rung, bound)) = next else {
            return async move { Ok(guard) }.into_actor(self).boxed_local();
        };
        if budget == 0 {
            warn!(%context_id, "ladder hop budget exhausted; resuming on next access");
            return async move { Ok(guard) }.into_actor(self).boxed_local();
        }

        let rung_app_key = rung.app_key;
        let rung_application_id = rung.application_id;

        info!(
            %context_id,
            from = %hex::encode(bound),
            to = %hex::encode(rung_app_key),
            "replaying upgrade ladder hop"
        );

        let node_client = self.node_client.clone();
        async move {
            // Binding a marker to an absent blob would wedge the context, so
            // the blob must be local (fetched from peers if needed) before
            // anything else.
            if !ensure_blob_local(&node_client, &context_id, rung_app_key).await {
                eyre::bail!("rung bytecode blob not available locally or from peers");
            }
            crate::handlers::upgrade_group::resolve_upgrade_from_abis(
                &node_client,
                bound,
                rung_app_key,
            )
            .await
        }
        .into_actor(self)
        .then(move |resolved, act, _ctx| {
            let migration = match resolved {
                Ok(m) => m,
                Err(err) => {
                    // Rung blob unobtainable or its ABI unreadable: the context
                    // stays on its current real version and surfaces as stranded
                    // for operator resync. Retried on next access.
                    persist_migration_failed(
                        &act.datastore,
                        context_id,
                        MigrationFailureKind::NoMigrationPath,
                    );
                    warn!(
                        %context_id, %err,
                        "ladder hop blocked; proceeding with current application"
                    );
                    return async move { Ok(guard) }.into_actor(act).boxed_local();
                }
            };
            let datastore = act.datastore.clone();
            let node_client = act.node_client.clone();
            let context_client = act.context_client.clone();
            let context_meta = act.contexts.get(&context_id).map(|c| c.meta.clone());

            if let Some(params) = migration {
                let service_name = context_meta.as_ref().and_then(|c| c.service_name.clone());
                let migration_v2 = act.config.migration_v2;
                act.get_module_for_blob(rung_app_key.into(), service_name)
                    .then(move |module_result, act, _ctx| {
                        // Re-read cached values; they may have been refreshed
                        // during the module load.
                        let context_meta = act.contexts.get(&context_id).map(|c| c.meta.clone());
                        let application = act.applications.get(&rung_application_id).cloned();
                        async move {
                            let module = module_result?;
                            let _ = update_application_with_migration(
                                datastore.clone(),
                                node_client,
                                context_client,
                                context_id,
                                context_meta,
                                rung_application_id,
                                application,
                                executor,
                                Some(params),
                                module,
                                migration_v2,
                            )
                            .await?;
                            crate::activation::record_activation(
                                &datastore,
                                &context_id,
                                rung_app_key,
                            );
                            Ok(())
                        }
                        .into_actor(act)
                        .then(move |hop: eyre::Result<()>, act, _ctx| match hop {
                            Ok(()) => {
                                act.replay_upgrade_ladder(guard, context_id, executor, budget - 1)
                            }
                            Err(err) => {
                                // Rung resolved but its migrate failed to apply:
                                // surface ApplyFailed (not the stale resolve-time
                                // NoMigrationPath). Stuck on current; retried on
                                // next access, marker self-clears on success.
                                persist_migration_failed(
                                    &act.datastore,
                                    context_id,
                                    MigrationFailureKind::ApplyFailed,
                                );
                                warn!(
                                    %context_id, %err,
                                    "ladder hop failed, proceeding with current application"
                                );
                                async move { Ok(guard) }.into_actor(act).boxed_local()
                            }
                        })
                        .boxed_local()
                    })
                    .boxed_local()
            } else {
                // Code-only rung: no wasm runs — flip the application id and
                // move the marker.
                act.evict_application_caches(rung_application_id);
                let application = act.applications.get(&rung_application_id).cloned();
                async move {
                    let _ = update_application_id(
                        datastore.clone(),
                        node_client,
                        context_client,
                        context_id,
                        context_meta,
                        rung_application_id,
                        application,
                        executor,
                    )
                    .await?;
                    crate::activation::record_activation(&datastore, &context_id, rung_app_key);
                    clear_migration_failed(&datastore, context_id);
                    Ok(())
                }
                .into_actor(act)
                .then(move |hop: eyre::Result<()>, act, _ctx| match hop {
                    Ok(()) => act.replay_upgrade_ladder(guard, context_id, executor, budget - 1),
                    Err(err) => {
                        // Code-only rung swap failed: surface ApplyFailed so the
                        // context reports its real failure mode, not a stale one.
                        persist_migration_failed(
                            &act.datastore,
                            context_id,
                            MigrationFailureKind::ApplyFailed,
                        );
                        warn!(
                            %context_id, %err,
                            "ladder hop failed, proceeding with current application"
                        );
                        async move { Ok(guard) }.into_actor(act).boxed_local()
                    }
                })
                .boxed_local()
            }
        })
        .boxed_local()
    }

    pub fn get_module(
        &self,
        application_id: ApplicationId,
        service_name: Option<String>,
    ) -> impl ActorFuture<Self, Output = eyre::Result<calimero_runtime::Module>> + 'static {
        async {}
            .into_actor(self)
            .map(move |_, act, _ctx| {
                // Fetch on a cache miss *before* inserting (so a not-installed
                // app never wastes an eviction); `insert_new` caps the cache.
                if !act.applications.contains_key(&application_id) {
                    let Some(app) = act.node_client.get_application(&application_id)? else {
                        bail!(ExecuteError::ApplicationNotInstalled { application_id });
                    };
                    let _ = act.applications.insert_new(application_id, app);
                }
                let app = act
                    .applications
                    .get(&application_id)
                    .expect("application just inserted or already cached");

                Ok(app.blob.bytecode)
            })
            .and_then(move |blob, act, _ctx| act.get_module_for_blob(blob, service_name))
    }

    /// Load (compile + cache) the module for a content-addressed bytecode
    /// blob — THE module-loading path: contexts execute the blob their
    /// activation marker / group `app_key` points at, independent of what
    /// the shared application row currently holds. For bundle blobs,
    /// `service_name` selects the service wasm inside the bundle.
    ///
    /// Cached in `modules` under `(blob_id, service_name)`; content
    /// addressing makes reuse always sound (same blob ⇒ same module), so
    /// entries never need eviction. The read-only method set is populated
    /// alongside from the embedded ABI.
    pub fn get_module_for_blob(
        &self,
        blob_id: calimero_primitives::blobs::BlobId,
        service_name: Option<String>,
    ) -> impl ActorFuture<Self, Output = eyre::Result<calimero_runtime::Module>> + 'static {
        let cache_key = (blob_id, service_name.clone());
        let lookup_key = cache_key.clone();

        async {}
            .into_actor(self)
            .map(move |_, act, _ctx| {
                if let Some(cached) = act.modules.get(&lookup_key) {
                    return Either::Left(cached.clone());
                }
                Either::Right((act.node_client.clone(), act.vm_limits))
            })
            .then(move |either, act, _ctx| {
                let (node_client, vm_limits) = match either {
                    Either::Left(module) => {
                        return actix::fut::ready(Ok(module)).into_actor(act).boxed_local()
                    }
                    Either::Right(parts) => parts,
                };
                async move {
                    let Some(bytecode) = node_client
                        .application_bytes_from_blob(&blob_id, service_name.as_deref())
                        .await?
                    else {
                        bail!("bytecode blob {} not found in blobstore", blob_id);
                    };
                    // Extract the read-only and xcall method sets from the ABI
                    // before the bytes move into the compile task. A missing
                    // manifest is fine: read-only defaults to the write lock,
                    // and an absent xcall set just leaves the method ungated.
                    let read_only_set = extract_read_only_set(&bytecode);
                    let xcall_set = extract_xcall_set(&bytecode);
                    let module = calimero_utils_actix::global_runtime()
                        .spawn_blocking(move || {
                            calimero_runtime::Engine::with_limits(vm_limits).compile(&bytecode)
                        })
                        .await
                        .wrap_err("WASM compilation task failed")??;
                    Ok((module, read_only_set, xcall_set))
                }
                .into_actor(act)
                .map_ok(
                    move |(module, read_only_set, xcall_set): (calimero_runtime::Module, _, _),
                          act,
                          _ctx| {
                        let _ = act.modules.insert(cache_key.clone(), module.clone());
                        if let Some(set) = read_only_set {
                            let _ = act.read_only_methods.insert(cache_key.clone(), set);
                        }
                        // Cached like read_only_methods, keyed by the same blob.
                        if let Some(set) = xcall_set {
                            let _ = act.xcall_methods.insert(cache_key, set);
                        }
                        module
                    },
                )
                .boxed_local()
            })
            .map_err(|err, _act, _ctx| {
                error!(?err, "failed to initialize module for execution");

                err
            })
    }
}

/// Ensure the bytecode blob is present in the local blobstore, fetching it
/// from peers (it is announced at upgrade time; the sync gate leaves
/// BlobShare open for exactly this) when absent. Pure byte movement — the
/// application row is never touched; per-context binding decides what
/// executes. `false` ⇒ unavailable (zero/legacy key, fetch failure): the
/// caller falls back and the next access retries.
async fn ensure_blob_local(
    node_client: &NodeClient,
    context_id: &ContextId,
    blob: [u8; 32],
) -> bool {
    if blob == [0u8; 32] {
        return false;
    }
    let blob_id = calimero_primitives::blobs::BlobId::from(blob);
    match node_client.has_blob(&blob_id) {
        Ok(true) => return true,
        Ok(false) => {}
        Err(err) => {
            warn!(%context_id, %blob_id, %err, "lazy upgrade: blobstore lookup failed");
            return false;
        }
    }
    info!(%context_id, %blob_id, "lazy upgrade: fetching target bytecode blob");
    // A successful peer fetch persists the blob into the local blobstore,
    // so the subsequent module load finds it.
    match node_client.get_blob_bytes(&blob_id, Some(context_id)).await {
        Ok(Some(_)) => true,
        Ok(None) => {
            warn!(%context_id, %blob_id, "lazy upgrade: target blob not available locally or from peers");
            false
        }
        Err(err) => {
            warn!(%context_id, %blob_id, %err, "lazy upgrade: target blob fetch failed");
            false
        }
    }
}

/// Store-level executing-blob resolution for a context: its activation
/// marker (the blob it last activated), else its owning group's recorded
/// target blob. The `bool` is `true` when the blob came from the group
/// `app_key` (callers gate that branch on local blob presence — legacy
/// groups carry randomly-seeded keys that resolve to nothing). `None` ⇒
/// fall back to the application row.
/// Where a context's bound bytecode blob was resolved from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundBlobSource {
    /// The per-context activation marker — already executed locally.
    ActivationMarker,
    /// The group's recorded target blob — may not be fetched yet.
    GroupKey,
}

pub(crate) fn bound_blob_for_context(
    store: &Store,
    context_id: &ContextId,
) -> Option<([u8; 32], BoundBlobSource)> {
    if let Some(blob) = crate::activation::activated_blob(store, context_id) {
        return Some((blob, BoundBlobSource::ActivationMarker));
    }
    let group_id = calimero_governance_store::get_group_for_context(store, context_id)
        .ok()
        .flatten()?;
    let meta = MetaRepository::new(store).load(&group_id).ok().flatten()?;
    (meta.app_key != [0u8; 32]).then_some((meta.app_key, BoundBlobSource::GroupKey))
}

impl ContextManager {
    /// The bytecode blob this context executes (per-context binding):
    /// activation marker → group target blob (when locally present) →
    /// `None` (callers fall back to the application row).
    pub(crate) fn executing_blob_for_context(
        &self,
        context_id: &ContextId,
    ) -> Option<calimero_primitives::blobs::BlobId> {
        let (blob, source) = bound_blob_for_context(&self.datastore, context_id)?;
        let blob_id = calimero_primitives::blobs::BlobId::from(blob);
        if source == BoundBlobSource::GroupKey
            && !self.node_client.has_blob(&blob_id).unwrap_or(false)
        {
            // Legacy randomly-seeded app_key (or not-yet-fetched target):
            // nothing to execute under that key — use the row.
            return None;
        }
        Some(blob_id)
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "orthogonal args (runtime deps, context identity, crypto keys, module) on a split-brain-critical handler; no cohesive grouping"
)]
async fn internal_execute(
    datastore: Store,
    node_client: &NodeClient,
    _context_client: &ContextClient,
    module: calimero_runtime::Module,
    guard: &ContextGuard,
    context: &mut Context,
    executor: PublicKey,
    method: Cow<'static, str>,
    input: Cow<'static, [u8]>,
    is_state_op: bool,
    // Whether the caller holds a shared read guard (not an exclusive write guard).
    // When true, a `ReadOnlyContextStorage` wrapper is passed to the runtime so
    // that write host-calls are silenced — a read-lock execution must not mutate
    // shared state. A non-empty artifact post-execution indicates a misbehaving
    // or mis-declared method and is treated as an error.
    is_read_only_call: bool,
    // `Some(group_id)` while the owning group's upgrade is `InProgress`; the
    // post-exec gate then serves reads but refuses writes (see `handle()`).
    block_writes_for_group: Option<ContextGroupId>,
    identity_private_key: &PrivateKey,
    // Source context when this run was dispatched via `xcall`; threaded to the
    // runtime so the guest can read `env::xcall_origin()`. `None` for direct
    // calls.
    xcall_origin: Option<ContextId>,
) -> eyre::Result<(
    Outcome,
    Option<CausalDelta>,
    Option<[u8; 64]>,
    Option<GovernanceParentEdge>,
)> {
    let executor_is_read_only = !is_state_op
        && NamespaceRepository::new(&datastore)
            .is_read_only_for_context(&context.id, &executor)
            .unwrap_or(false);

    // B3 user-storage extension: a state op authored by a non-member of
    // the context's owning group is dropped after WASM execution, mirroring
    // the ReadOnly handling below. The receive-path cross-DAG check
    // (`membership_status_at`) already rejects deltas from non-members on
    // peers; without this companion check at the local-execute path, a
    // removed member's WASM would still mutate local state (including
    // their own User-storage entries). Those mutations never propagate —
    // peers reject them — but they accumulate as silent divergence from
    // the canonical view until a sync round repairs the local state.
    // Discarding the outcome here closes that gap.
    //
    // Read-only calls (`is_state_op == false`) are unaffected — reads of
    // a context's state are allowed for anyone with local access; if the
    // method genuinely mutates storage despite being read-typed, the
    // ReadOnly discard below clears it on the same path.
    //
    // Live-state check vs. the receive path's forward-only check: at
    // execute time there is no signed governance cut to evaluate
    // membership against, so this consults current membership. The two
    // checks complement each other rather than duplicate — receive-path
    // governs whether a remote delta is accepted; this governs whether
    // local WASM may produce one.
    // Fail-closed on store error: an authorization gate that fails open
    // on transient store errors silently grants permission exactly when
    // the check is most needed (storage degradation). The non-group-
    // context happy-path already returns `Ok(true)` inside the helper,
    // so this fail-closed only affects genuine error cases — and there
    // the safer answer is "drop the state op." Asymmetry with the
    // `is_read_only_for_context` call above (`.unwrap_or(false)` reads
    // as fail-open for that check because `false` means "not
    // read-only" → allow) is deliberate: the ReadOnly check is a
    // defense-in-depth post-discard, while this is a primary
    // authorization gate.
    let executor_not_authorized_for_state_op = is_state_op
        && !NamespaceRepository::new(&datastore)
            .is_authorized_for_context_state_op(&context.id, &executor)
            .unwrap_or(false);

    let storage = ContextStorage::from(datastore.clone(), context.id);
    let private_storage = ContextPrivateStorage::from(datastore, context.id);
    let (mut outcome, storage, private_storage) = execute(
        guard,
        module,
        executor,
        method.clone(),
        input,
        storage,
        private_storage,
        node_client.clone(),
        is_read_only_call,
        xcall_origin,
    )
    .await?;

    debug!(
        context_id = %context.id,
        method = %method,
        is_state_op,
        has_root_hash = outcome.root_hash.is_some(),
        artifact_len = outcome.artifact.len(),
        events_count = outcome.events.len(),
        returns_ok = outcome.returns.is_ok(),
        "WASM execution completed"
    );

    if outcome.returns.is_err() {
        warn!(
            context_id = %context.id,
            method = %method,
            error = ?outcome.returns,
            "WASM execution returned error"
        );
        return Ok((outcome, None, None, None));
    }

    'fine: {
        if outcome.root_hash.is_some() && outcome.artifact.is_empty() {
            debug!(
                context_id = %context.id,
                has_root_hash = true,
                artifact_empty = true,
                is_state_op,
                "Outcome has root hash but empty artifact - checking mitigation"
            );

            if is_state_op {
                // fixme! temp mitigation for a potential state inconsistency
                break 'fine;
            }

            bail!(ContextError::StateInconsistency);
        }
    }

    let mut causal_delta = None;
    // Populated when we sign the locally-produced delta envelope so the
    // outer `execute` task can carry the same signature bytes into the
    // gossip broadcast. Stays `None` when no delta was produced (e.g.,
    // empty artifact) or signing wasn't applicable.
    let mut delta_signature_for_broadcast: Option<[u8; 64]> = None;
    // Captured alongside the signature: the EXACT
    // `governance_position` the signature was computed against. The
    // outer broadcast site MUST reuse this value rather than
    // recomputing from a fresh store snapshot — between the persist
    // and broadcast points the local governance state can advance
    // (a member is added/removed, a namespace op lands), and
    // recomputing would produce a different position that no longer
    // matches the signed payload, so receivers would reject the
    // delta on signature mismatch. This single source of truth
    // collapses that race window.
    let mut governance_position_for_broadcast: Option<GovernanceParentEdge> = None;

    if executor_is_read_only && outcome.root_hash.is_some() {
        info!(
            context_id = %context.id,
            %executor,
            method = %method,
            "ReadOnly member attempted state mutation — discarding changes"
        );
        outcome.root_hash = None;
        outcome.artifact.clear();
        outcome.xcalls.clear();
        return Ok((outcome, None, None, None));
    }

    // Defence-in-depth: a method declared read-only in the ABI should never
    // produce a state mutation (the ReadOnlyContextStorage wrapper silences
    // writes at the host-call boundary). If the artifact is non-empty here,
    // the declaration is wrong or the wrapper leaked — reject rather than commit.
    if is_read_only_call && outcome.root_hash.is_some() {
        warn!(
            context_id = %context.id,
            %executor,
            method = %method,
            "method declared #[app::view] produced a state mutation — discarding (ABI mismatch)"
        );
        outcome.root_hash = None;
        outcome.artifact.clear();
        outcome.xcalls.clear();
        return Ok((outcome, None, None, None));
    }

    if executor_not_authorized_for_state_op && outcome.root_hash.is_some() {
        info!(
            context_id = %context.id,
            %executor,
            method = %method,
            "Non-member attempted state mutation — discarding changes (B3 user-storage extension)"
        );
        outcome.root_hash = None;
        outcome.artifact.clear();
        outcome.xcalls.clear();
        return Ok((outcome, None, None, None));
    }

    // In-progress upgrade: a pure read falls through and is served from the
    // pre-migration root; a side-effecting call is refused (cross-version drift
    // risk), committing and dispatching nothing. "Side-effecting" = a committed
    // state mutation (`root_hash`) OR queued cross-context calls (`xcalls`),
    // which the external-actions stage would otherwise fire after this returns.
    if let Some(group_id) = block_writes_for_group {
        // `block_writes` is necessarily true here; refuse the call only if it had
        // a side effect (committed state or queued xcalls).
        if upgrade_rejects_committed_write(
            true,
            outcome.root_hash.is_some() || !outcome.xcalls.is_empty(),
        ) {
            info!(
                context_id = %context.id,
                %executor,
                method = %method,
                ?group_id,
                "refusing write: group upgrade in progress (a read would have been served)"
            );
            return Err(ExecuteError::UpgradeInProgress { group_id }.into());
        }
    }

    // Always update root_hash if present (even if storage is empty)
    // This is critical for state_ops like __calimero_sync_next where actions
    // are applied inside WASM but storage appears empty
    if let Some(root_hash) = outcome.root_hash {
        debug!(
            context_id = %context.id,
            old_root = ?context.root_hash,
            new_root = ?Hash::from(root_hash),
            is_state_op,
            storage_empty = storage.is_empty(),
            "Updating context root_hash after execution"
        );
        context.root_hash = root_hash.into();

        // Commit storage and persist metadata
        let store = storage.commit()?;
        // Commit private storage (node-local, NOT synchronized)
        // Private storage changes are not included in sync deltas
        let _private_store = private_storage.commit()?;

        // Create causal delta for non-state ops with non-empty artifacts
        if !is_state_op && !outcome.artifact.is_empty() {
            // Extract actions from artifact for DAG persistence
            let mut actions = match borsh::from_slice::<StorageDelta>(&outcome.artifact) {
                Ok(StorageDelta::Actions(actions)) => actions,
                Ok(_) => {
                    warn!("Unexpected StorageDelta variant, using empty actions");
                    vec![]
                }
                Err(e) => {
                    warn!(
                        ?e,
                        "Failed to deserialize artifact for DAG, using empty actions"
                    );
                    vec![]
                }
            };

            // The artifact was `StorageDelta::Actions`.
            if !actions.is_empty() {
                info!(
                    context_id = %context.id,
                    actions_count = actions.len(),
                    "Received several actions. Verify if there any user actions..."
                );
                sign_authorized_actions(&mut actions, identity_private_key)
                    .wrap_err("Failed to sign user actions")?;

                // Persist the signed `signature_data` back to local
                // storage for each upsert action. `save_raw` runs
                // inside the WASM host call and has no access to the
                // identity private key, so it stamps the metadata
                // with a placeholder signature (`[0; 64]`) and the
                // locally stored entity retains it. Without this
                // step, HashComparison sync would ship the
                // placeholder to peers and signature verification on
                // receivers would fail — exactly the cascade that
                // broke the e2e on this branch when the wire format
                // started carrying authorization verbatim.
                //
                // We construct a temporary `RuntimeEnv` over the
                // calimero-store handle so `Interface::<MainStorage>`
                // can read/write the index entries directly. Only the
                // `signature_data` portion of an existing entity's
                // `storage_type` is updated;
                // `update_signature_in_place` rejects any structural
                // change (variant flip, writer-set or owner change),
                // so the merkle hash and the entity's
                // access-control triple stay invariant.
                persist_signed_signatures(&store, context, identity_private_key, &actions)
                    .wrap_err("Failed to persist signed signature_data after execute")?;

                // Re-serialize the *signed* actions into a new artifact
                let new_artifact = borsh::to_vec(&StorageDelta::Actions(actions.clone()))?;
                outcome.artifact = new_artifact;
            }

            // Refresh the DAG heads from the authoritative store before
            // choosing this write's parents. `context` (hence `dag_heads`) was
            // snapshotted in `get_or_fetch_context` BEFORE this handler took
            // the per-context lock, so an inbound delta that committed new
            // heads while we waited for the lock would be missed here.
            // Authoring on the stale head forks the DAG: the new delta's
            // parents exclude an already-applied ancestor — e.g. a writer-set
            // rotation the executor has locally applied — and every peer then
            // rejects it (`writers_at(parents)` resolves the pre-rotation set),
            // a permanent split-brain. The inbound apply now holds this same
            // lock across its `dag_heads` commit (see
            // `DeltaStore::add_delta_internal`), so once we hold the guard the
            // persisted heads are current.
            if let Ok(Some(meta)) = store.handle().get(&key::ContextMeta::new(context.id)) {
                if context.dag_heads != meta.dag_heads {
                    context.dag_heads = meta.dag_heads;
                }
            }

            // Use current DAG heads as parents, verifying they exist in RocksDB
            let parents = if context.dag_heads.is_empty() {
                // Genesis case: parent is the zero hash
                vec![[0u8; 32]]
            } else {
                // Filter out parents that aren't persisted yet (cascaded deltas)
                let mut verified_parents = Vec::new();
                for head in &context.dag_heads {
                    if *head == [0u8; 32] {
                        verified_parents.push(*head);
                        continue;
                    }

                    // Check if this parent is actually in RocksDB
                    let db_key = key::ContextDagDelta::new(context.id, *head);
                    if store.handle().get(&db_key).is_ok_and(|v| v.is_some()) {
                        verified_parents.push(*head);
                    } else {
                        warn!(
                            context_id = %context.id,
                            parent_id = ?head,
                            "DAG head not in RocksDB - skipping as parent (likely cascaded delta not yet persisted)"
                        );
                    }
                }

                // If NO parents verified, use genesis
                if verified_parents.is_empty() {
                    warn!(
                        context_id = %context.id,
                        "No DAG heads in RocksDB - using genesis as parent"
                    );
                    vec![[0u8; 32]]
                } else {
                    verified_parents
                }
            };

            let hlc = calimero_storage::env::hlc_timestamp();
            let delta_id = CausalDelta::compute_id(&parents, &actions, &hlc);

            let mut delta = CausalDelta {
                id: delta_id,
                parents,
                actions,
                hlc,
                expected_root_hash: root_hash,
            };

            // Leg 4 of rotation-log convergence (core#2716): the local write
            // path persisted each `Shared` anchor and its children DURING WASM
            // execution — before this `delta_id` existed — so the originator's
            // own rotation isn't in its hashed rotation-log collection yet. Now
            // that the (signed) delta is built, self-log its rotations (the
            // `insert` propagates each new child's hash into the anchor's
            // `full_hash` and up to the root), then recompute the context root
            // so BOTH `context.root_hash` and the delta's `expected_root_hash`
            // reflect the new writer set. Peers log the same entries when they
            // apply the rotation, so every node converges. No-op unless this
            // delta rotates a `Shared` writer set.
            {
                let callbacks = create_storage_callbacks(&store, context.id);
                let env = RuntimeEnv::new(
                    callbacks.read,
                    callbacks.write,
                    callbacks.remove,
                    *context.id.as_ref(),
                    *identity_private_key.public_key().as_ref(),
                );
                let recomputed_root =
                    with_runtime_env(env, || -> eyre::Result<Option<[u8; 32]>> {
                        let changed = Interface::<MainStorage>::self_log_own_rotations(
                            &delta.actions,
                            delta.id,
                            delta.hlc,
                        )?;
                        if !changed {
                            return Ok(None);
                        }
                        let root_id = Id::new(*context.id.as_ref());
                        let (full_hash, _) = Index::<MainStorage>::get_hashes_for(root_id)?
                            .ok_or_else(|| {
                                eyre::eyre!("root index missing after rotation self-log")
                            })?;
                        Ok(Some(full_hash))
                    })?;
                if let Some(full_hash) = recomputed_root {
                    context.root_hash = full_hash.into();
                    delta.expected_root_hash = full_hash;
                }
            }

            // Update context's DAG heads to this new delta
            context.dag_heads = vec![delta.id];

            causal_delta = Some(delta);
        } else if !is_state_op {
            // No delta created (empty artifact), but state changed
            // Use root_hash as dag_head fallback to enable sync
            // This happens when init() creates state but doesn't generate actions
            if context.dag_heads.is_empty() {
                warn!(
                    context_id = %context.id,
                    root_hash = ?root_hash,
                    artifact_empty = outcome.artifact.is_empty(),
                    "State changed but no delta created - using root_hash as dag_head fallback"
                );
                context.dag_heads = vec![root_hash];
            }
        }

        // Persist context metadata when root_hash changes
        let mut handle = store.handle();

        debug!(
            context_id = %context.id,
            root_hash = ?context.root_hash,
            dag_heads_count = context.dag_heads.len(),
            is_state_op,
            "Persisting context metadata to database"
        );

        handle.put(
            &key::ContextMeta::new(context.id),
            &types::ContextMeta::new(
                key::ApplicationMeta::new(context.application_id),
                *context.root_hash,
                context.dag_heads.clone(),
                context.service_name.as_deref().map(Box::from),
            ),
        )?;

        // Also persist the delta itself for serving to peers who request it
        if let Some(ref delta) = causal_delta {
            let serialized_actions = borsh::to_vec(&delta.actions)?;

            // Compute the governance position for the cross-DAG check
            // that DAG-catchup responders advertise on the wire. Mirrors
            // the position computed for the broadcast envelope above so
            // peers that pull this delta via `request_dag_heads_and_sync`
            // can run the same `membership_status_at` check the gossip
            // path runs.
            let governance_position = compute_governance_position_for_context(&store, &context.id);
            let governance_position_blob = governance_position
                .as_ref()
                .and_then(|gp| borsh::to_vec(gp).ok());

            // Sign the canonical envelope payload with the author's
            // identity key. Signature binds `(context_id, delta_id,
            // author_id, governance_position)` together so a current
            // group-key holder can't relabel a foreign delta as their
            // own (or vice versa) on the wire — receivers reject any
            // mismatch via `verify_delta_signature`. The same signature
            // is persisted on the row and passed back to the broadcast
            // site so the gossip and DAG-catchup paths advertise the
            // same bytes.
            let signature_payload =
                calimero_node_primitives::sync::delta_auth::delta_signature_payload(
                    context.id,
                    delta.id,
                    executor,
                    governance_position.as_ref(),
                )?;
            let delta_signature = Some(identity_private_key.sign(&signature_payload)?.to_bytes());
            delta_signature_for_broadcast = delta_signature;
            // Pin the exact position the signature was bound to so
            // the broadcast site can advertise it verbatim instead of
            // recomputing (see `governance_position_for_broadcast`'s
            // declaration for why recomputation is unsafe).
            governance_position_for_broadcast = governance_position.clone();

            handle.put(
                &key::ContextDagDelta::new(context.id, delta.id),
                &types::ContextDagDelta {
                    delta_id: delta.id,
                    parents: delta.parents.clone(),
                    actions: serialized_actions,
                    hlc: delta.hlc,
                    applied: true,
                    expected_root_hash: delta.expected_root_hash,
                    events: None, // No events stored for locally created deltas
                    author_id: Some(executor),
                    governance_position_blob,
                    delta_signature,
                },
            )?;

            debug!(
                context_id = %context.id,
                delta_id = ?delta.id,
                "Persisted delta to database for future requests"
            );

            // Keep the in-memory DeltaStore in sync with the write we
            // just made. Without this the sync path would have to
            // rescan the DB every ~2s to pick up locally-created
            // deltas; instead the DAG is updated at write time and
            // `load_persisted_deltas` only runs on startup.
            node_client.notify_local_applied_delta(
                calimero_node_primitives::client::LocalAppliedDelta {
                    context_id: context.id,
                    delta_id: delta.id,
                    parents: delta.parents.clone(),
                    hlc: delta.hlc,
                    expected_root_hash: delta.expected_root_hash,
                    actions: delta.actions.clone(),
                },
            );
        }

        debug!(
            context_id = %context.id,
            root_hash = ?context.root_hash,
            dag_heads_count = context.dag_heads.len(),
            is_state_op,
            "Context metadata persisted successfully"
        );
    }

    // Emit state mutation to WebSocket clients (frontends) if there are events or state changes
    // Note: This is separate from node-to-node DAG broadcast (lines 408-419)
    if !outcome.events.is_empty() || outcome.root_hash.is_some() {
        let new_root = outcome
            .root_hash
            .map(|h| h.into())
            .unwrap_or((*context.root_hash).into());

        let events_vec = outcome
            .events
            .iter()
            .map(|e| ExecutionEvent {
                kind: e.kind.clone(),
                data: e.data.clone(),
                handler: e.handler.clone(),
            })
            .collect();

        node_client.send_event(NodeEvent::Context(ContextEvent {
            context_id: context.id,
            payload: ContextEventPayload::StateMutation(
                StateMutationPayload::with_root_and_events(new_root, events_vec),
            ),
        }))?;
    }

    Ok((
        outcome,
        causal_delta,
        delta_signature_for_broadcast,
        governance_position_for_broadcast,
    ))
}

#[allow(clippy::too_many_arguments, reason = "execution context is wide")]
pub async fn execute(
    context: &ContextGuard,
    module: calimero_runtime::Module,
    executor: PublicKey,
    method: Cow<'static, str>,
    input: Cow<'static, [u8]>,
    mut storage: ContextStorage,
    mut private_storage: ContextPrivateStorage,
    node_client: NodeClient,
    is_read_only_call: bool,
    xcall_origin: Option<ContextId>,
) -> eyre::Result<(Outcome, ContextStorage, ContextPrivateStorage)> {
    let context_id = **context;

    global_runtime()
        .spawn_blocking(move || {
            let outcome = if is_read_only_call {
                // Wrap storage in a read-only view: write host calls are silenced
                // so a method holding a shared read guard cannot mutate shared
                // state. The post-exec assertion on outcome.root_hash / artifact
                // catches any method that nonetheless produced a mutation.
                let mut ro_storage = ReadOnlyContextStorage::new(&mut storage);
                let mut ro_private = ReadOnlyContextStorage::new(&mut private_storage);
                module.run_with_origin(
                    context_id,
                    executor,
                    &method,
                    &input,
                    &mut ro_storage,
                    Some(&mut ro_private),
                    Some(node_client),
                    xcall_origin,
                )?
            } else {
                module.run_with_origin(
                    context_id,
                    executor,
                    &method,
                    &input,
                    &mut storage,
                    Some(&mut private_storage),
                    Some(node_client),
                    xcall_origin,
                )?
            };
            Ok((outcome, storage, private_storage))
        })
        .await
        .wrap_err("failed to receive execution response")?
}

/// Extract the set of read-only method names from a WASM module's embedded ABI.
///
/// Returns `None` on any parse failure so callers default to the write lock.
/// Methods are declared read-only by the app author via `#[app::view]`; the ABI
/// emitter stores `MethodIntent::ReadOnly` in the embedded manifest section.
fn extract_read_only_set(bytecode: &[u8]) -> Option<Arc<HashSet<String>>> {
    let manifest = calimero_wasm_abi::embed::read_embedded_state_schema(bytecode)?;
    let set: HashSet<String> = manifest
        .methods
        .into_iter()
        .filter(|m| m.intent == MethodIntent::ReadOnly)
        .map(|m| m.name)
        .collect();
    Some(Arc::new(set))
}

/// The `#[app::xcall]` method names declared in a module's embedded ABI, or
/// `None` if the manifest is absent/unparseable or declares none (the method is
/// then left ungated). A returned set is always non-empty.
fn extract_xcall_set(bytecode: &[u8]) -> Option<Arc<HashSet<String>>> {
    let manifest = calimero_wasm_abi::embed::read_embedded_state_schema(bytecode)?;
    let set: HashSet<String> = manifest
        .methods
        .into_iter()
        .filter(|m| m.xcall_callable)
        .map(|m| m.name)
        .collect();
    if set.is_empty() {
        None
    } else {
        Some(Arc::new(set))
    }
}

/// Resolve a context to its namespace root group, or `None` if it is not
/// registered in any group. An xcall is allowed only when source and target
/// resolve to the same namespace; callers treat `Err`/`None` as deny.
fn resolve_namespace_for_context(
    store: &calimero_store::Store,
    context_id: &ContextId,
) -> eyre::Result<Option<ContextGroupId>> {
    let Some(group_id) = calimero_governance_store::get_group_for_context(store, context_id)?
    else {
        return Ok(None);
    };
    let namespace =
        calimero_governance_store::NamespaceRepository::new(store).resolve(&group_id)?;
    Ok(Some(namespace))
}

fn substitute_aliases_in_payload(
    node_client: &NodeClient,
    context_id: ContextId,
    payload: Vec<u8>,
    aliases: &[Alias<PublicKey>],
) -> Result<Vec<u8>, ExecuteError> {
    if aliases.is_empty() {
        return Ok(payload);
    }

    // todo! evaluate a byte-version of calimero_server[build]::replace
    // todo! ref: https://github.com/calimero-network/core/blob/6deb2db81a65e0b5c86af9fe2950cf9019ab61af/crates/server/build.rs#L139-L175

    let mut result = Vec::with_capacity(payload.len());
    let mut remaining = &payload[..];

    for alias in aliases {
        let needle_str = format!("{{{alias}}}");
        let needle = needle_str.into_bytes();

        while let Some(pos) = memmem::find(remaining, &needle) {
            result.extend_from_slice(&remaining[..pos]);

            let public_key = node_client
                .resolve_alias(*alias, Some(context_id))
                .map_err(|_| ExecuteError::InternalError)?
                .ok_or(ExecuteError::AliasResolutionFailed { alias: *alias })?;

            // Substitution hot path: bs58-encode the 32-byte key into a
            // stack buffer rather than allocating a fresh String per alias.
            let mut buf = [0u8; 45];
            let len = bs58::encode(public_key.as_ref() as &[u8; 32])
                .onto(&mut buf[..])
                .expect("base58 encoding cannot fail for fixed 32-byte input");
            result.extend_from_slice(&buf[..len]);

            remaining = &remaining[pos + needle.len()..];
        }
    }

    result.extend_from_slice(remaining);

    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{
        register_context_in_group, MetaRepository, NamespaceRepository,
    };
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::{ContextId, UpgradePolicy};
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;
    use calimero_store::Store;

    use super::{
        extract_xcall_set, resolve_namespace_for_context, resolve_producing_app_key, should_block,
        upgrade_blocks_write, upgrade_rejects_committed_write,
    };
    use calimero_store::key::GroupUpgradeStatus;

    fn fresh_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// Construct a minimal `GroupMetaValue` with the given `app_key`.
    fn group_meta_with_app_key(app_key: [u8; 32]) -> GroupMetaValue {
        let dummy_pk = PublicKey::from([0xAB; 32]);
        GroupMetaValue {
            app_key,
            target_application_id: ApplicationId::from([0xCC; 32]),
            upgrade_policy: UpgradePolicy::Automatic,
            created_at: 1_700_000_000,
            admin_identity: dummy_pk,
            owner_identity: dummy_pk,
            migration: None,
            auto_join: false,
        }
    }

    #[test]
    fn resolve_namespace_for_context_returns_root_namespace() {
        let store = fresh_store();
        // namespace N (root) ⊃ subgroup ⊃ context
        let ns = ContextGroupId::from([0xAA; 32]);
        let sub = ContextGroupId::from([0xBB; 32]);
        let ctx = ContextId::from([0xCC; 32]);

        NamespaceRepository::new(&store)
            .nest(&ns, &sub)
            .expect("nest subgroup under namespace");
        register_context_in_group(&store, &sub, &ctx).expect("register context in subgroup");

        // resolve walks sub → ns (the root group is the namespace)
        let got = resolve_namespace_for_context(&store, &ctx).expect("resolve ok");
        assert_eq!(got, Some(ns));
    }

    #[test]
    fn resolve_namespace_for_context_unregistered_is_none() {
        let store = fresh_store();
        let ctx = ContextId::from([0xDD; 32]);
        let got = resolve_namespace_for_context(&store, &ctx).expect("resolve ok");
        assert_eq!(got, None);
    }

    #[test]
    fn extract_xcall_set_none_on_non_wasm() {
        // No embedded ABI manifest ⇒ None (method left ungated).
        assert!(extract_xcall_set(b"not a wasm module").is_none());
        assert!(extract_xcall_set(&[]).is_none());
    }

    #[test]
    fn resolve_producing_app_key_returns_group_meta_app_key() {
        let store = fresh_store();
        let context_id = ContextId::from([0xF1; 32]);
        let group_id = ContextGroupId::from([0xF2; 32]);

        register_context_in_group(&store, &group_id, &context_id)
            .expect("register_context_in_group");
        MetaRepository::new(&store)
            .save(&group_id, &group_meta_with_app_key([0x22; 32]))
            .expect("save group meta");

        assert_eq!(
            resolve_producing_app_key(&store, &context_id).unwrap(),
            Some([0x22; 32])
        );
    }

    #[test]
    fn resolve_producing_app_key_none_for_non_group_context() {
        let store = fresh_store();
        // context_id was never registered in any group
        let context_id = ContextId::from([0xF3; 32]);

        assert_eq!(
            resolve_producing_app_key(&store, &context_id).unwrap(),
            None
        );
    }

    #[test]
    fn resolve_producing_app_key_none_when_meta_absent() {
        // Context is registered under a group, but no `GroupMetaValue` was
        // ever written for that group — the resolver must return `None`
        // (no app_key to stamp) rather than erroring.
        let store = fresh_store();
        let context_id = ContextId::from([0xF4; 32]);
        let group_id = ContextGroupId::from([0xF5; 32]);

        register_context_in_group(&store, &group_id, &context_id)
            .expect("register_context_in_group");

        assert_eq!(
            resolve_producing_app_key(&store, &context_id).unwrap(),
            None
        );
    }

    #[test]
    fn upgrade_blocks_write_in_progress() {
        let status = GroupUpgradeStatus::InProgress {
            total: 5,
            completed: 2,
            failed: 0,
        };
        assert!(
            upgrade_blocks_write(&status),
            "InProgress should block writes"
        );
    }

    #[test]
    fn upgrade_blocks_write_completed() {
        let status = GroupUpgradeStatus::Completed { completed_at: None };
        assert!(
            !upgrade_blocks_write(&status),
            "Completed should not block writes"
        );
    }

    #[test]
    fn upgrade_blocks_write_completed_with_timestamp() {
        let status = GroupUpgradeStatus::Completed {
            completed_at: Some(1_700_000_000),
        };
        assert!(
            !upgrade_blocks_write(&status),
            "Completed (with timestamp) should not block writes"
        );
    }

    // During an in-progress upgrade, reads stay available while writes are
    // refused; intent comes from whether the call mutated state. Locks that.

    #[test]
    fn write_during_in_progress_upgrade_is_rejected() {
        assert!(
            upgrade_rejects_committed_write(/* block_writes */ true, /* produced_write */ true),
            "a state-mutating call during InProgress must be refused"
        );
    }

    #[test]
    fn read_during_in_progress_upgrade_is_allowed() {
        assert!(
            !upgrade_rejects_committed_write(/* block_writes */ true, /* produced_write */ false),
            "a read (no state mutation) during InProgress must be served"
        );
    }

    #[test]
    fn write_when_not_upgrading_is_allowed() {
        assert!(
            !upgrade_rejects_committed_write(/* block_writes */ false, /* produced_write */ true),
            "a write outside any in-progress upgrade must not be gated"
        );
    }

    #[test]
    fn read_when_not_upgrading_is_allowed() {
        assert!(
            !upgrade_rejects_committed_write(/* block_writes */ false, /* produced_write */ false),
            "a read outside any in-progress upgrade must not be gated"
        );
    }

    // PR-6b Task 6b.8: the `migration_v2` feature flag now defaults ON, the
    // flip enabled by both PR-6a (no-freeze) and PR-6b (absorb-don't-drop
    // straggler safety net) having landed. The flag lives on
    // `ContextManagerConfig` — the same runtime-tunable knobs struct threaded
    // into this handler via `self.config`. With it on, the group-wide
    // `InProgress` write-freeze no longer fires (see `should_block`).
    #[test]
    fn migration_v2_flag_defaults_on() {
        let cfg = crate::ContextManagerConfig::default();
        assert!(
            cfg.migration_v2,
            "migration_v2 must default on now that 6a + 6b have landed"
        );
    }

    // PR-6a Task 6a.2: characterize today's group-wide freeze. With
    // `migration_v2` OFF (the default), `InProgress` blocks *all* writes —
    // including state-op writes such as `__calimero_sync_next`. This is the
    // freeze that namespace cascades impose group-wide. Locking it here proves
    // 6a.3 (which gates this behind `migration_v2`) only changes flag-ON
    // behavior; the flag-OFF contract stays exactly as it is today.
    #[test]
    fn flag_off_inprogress_blocks_state_op_write() {
        assert!(
            upgrade_blocks_write(&GroupUpgradeStatus::InProgress {
                total: 1,
                completed: 0,
                failed: 0,
            }),
            "today's group-wide freeze: InProgress must block state-op writes"
        );
    }

    // PR-6a Task 6a.3: the cascade write-freeze is gated behind `migration_v2`.
    // `should_block` is `!migration_v2 && upgrade_blocks_write(status)`: with the
    // flag OFF the freeze is unchanged (master behavior); with the flag ON the
    // group-wide `InProgress` freeze stops blocking writes (PR-6b's
    // absorb-don't-drop later keeps stragglers safe once the freeze is gone).
    #[test]
    fn should_block_flag_off_in_progress_blocks() {
        assert!(
            should_block(
                false,
                &GroupUpgradeStatus::InProgress {
                    total: 1,
                    completed: 0,
                    failed: 0,
                },
            ),
            "flag OFF: InProgress must still block writes (unchanged)"
        );
    }

    #[test]
    fn should_block_flag_on_in_progress_does_not_block() {
        assert!(
            !should_block(
                true,
                &GroupUpgradeStatus::InProgress {
                    total: 1,
                    completed: 0,
                    failed: 0,
                },
            ),
            "flag ON: InProgress must not freeze writes group-wide"
        );
    }

    #[test]
    fn should_block_flag_off_completed_does_not_block() {
        assert!(
            !should_block(false, &GroupUpgradeStatus::Completed { completed_at: None }),
            "Completed never blocks, regardless of the flag"
        );
    }

    // Per-context bytecode binding: the executing blob resolves marker →
    // group app_key → None (row fallback). Two contexts sharing one
    // application id but holding different markers must resolve different
    // blobs — the coexistence invariant the module cache re-key enables.

    #[test]
    fn bound_blob_two_contexts_different_markers_resolve_different_blobs() {
        let store = fresh_store();
        let group_id = ContextGroupId::from([0xB0; 32]);
        let ctx_a = ContextId::from([0xB1; 32]);
        let ctx_b = ContextId::from([0xB2; 32]);

        register_context_in_group(&store, &group_id, &ctx_a).expect("register a");
        register_context_in_group(&store, &group_id, &ctx_b).expect("register b");
        MetaRepository::new(&store)
            .save(&group_id, &group_meta_with_app_key([0x33; 32]))
            .expect("save group meta");

        crate::activation::record_activation(&store, &ctx_a, [0x11; 32]);
        crate::activation::record_activation(&store, &ctx_b, [0x22; 32]);

        assert_eq!(
            super::bound_blob_for_context(&store, &ctx_a),
            Some(([0x11; 32], super::BoundBlobSource::ActivationMarker))
        );
        assert_eq!(
            super::bound_blob_for_context(&store, &ctx_b),
            Some(([0x22; 32], super::BoundBlobSource::ActivationMarker))
        );
    }

    #[test]
    fn bound_blob_falls_back_to_group_app_key_without_marker() {
        let store = fresh_store();
        let group_id = ContextGroupId::from([0xB3; 32]);
        let ctx = ContextId::from([0xB4; 32]);

        register_context_in_group(&store, &group_id, &ctx).expect("register");
        MetaRepository::new(&store)
            .save(&group_id, &group_meta_with_app_key([0x44; 32]))
            .expect("save group meta");

        assert_eq!(
            super::bound_blob_for_context(&store, &ctx),
            Some(([0x44; 32], super::BoundBlobSource::GroupKey))
        );
    }

    #[test]
    fn bound_blob_none_for_zero_app_key_or_non_group_context() {
        let store = fresh_store();
        let group_id = ContextGroupId::from([0xB5; 32]);
        let ctx = ContextId::from([0xB6; 32]);

        register_context_in_group(&store, &group_id, &ctx).expect("register");
        MetaRepository::new(&store)
            .save(&group_id, &group_meta_with_app_key([0u8; 32]))
            .expect("save group meta");

        // Zero app_key (legacy) carries no blob identity — row fallback.
        assert_eq!(super::bound_blob_for_context(&store, &ctx), None);
        // Non-group context — row fallback.
        let lone = ContextId::from([0xB7; 32]);
        assert_eq!(super::bound_blob_for_context(&store, &lone), None);
    }
}
