use calimero_governance_store::{
    CapabilitiesRepository, GroupKeyring, MigrationsRepository, NamespaceRepository,
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
use calimero_context_config::types::{ContextGroupId, GovernancePosition};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::events::{
    ContextEvent, ContextEventPayload, ExecutionEvent, NodeEvent, StateMutationPayload,
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
use futures_util::io::Cursor;
use memchr::memmem;
use tracing::{debug, error, info, warn};

use crate::error::ContextError;
use crate::handlers::update_application::{
    create_storage_callbacks, update_application_id, update_application_with_migration,
};
use crate::ContextManager;
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
    upgrade_rejects_committed_write,
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
            let key = (cm.meta.application_id, cm.meta.service_name.clone());
            let Some(set) = self.read_only_methods.get(&key).cloned() else {
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
        let lazy_upgrade_task = guard_task.map(move |guard, act, _ctx| {
            if let Some((target_app_id, migrate_method, group_id)) = lazy_upgrade_params {
                info!(
                    %context_id,
                    %target_app_id,
                    %executor,
                    "performing lazy upgrade before execution"
                );
                let datastore = act.datastore.clone();
                let node_client = act.node_client.clone();
                let context_client = act.context_client.clone();
                let context_meta = act.contexts.get(&context_id).map(|c| c.meta.clone());
                let application = act.applications.get(&target_app_id).cloned();
                return Ok(Either::Right((
                    guard,
                    datastore,
                    node_client,
                    context_client,
                    context_id,
                    target_app_id,
                    context_meta,
                    application,
                    migrate_method,
                    group_id,
                )));
            }
            Ok(Either::Left(guard))
        });

        let context_task = lazy_upgrade_task.and_then(move |either, act, _ctx| {
            match either {
                Either::Left(guard) => async move { Ok(guard) }.into_actor(act).boxed_local(),
                Either::Right((
                    guard,
                    datastore,
                    node_client,
                    context_client,
                    cid,
                    target_app,
                    context_meta,
                    application,
                    migrate,
                    group_id,
                )) => {
                    if let Some(method) = migrate {
                        let migration_params = MigrationParams { method: method.clone() };
                        let service_name = context_meta.as_ref().and_then(|c| c.service_name.clone());
                        act.get_module(target_app, service_name)
                            .then(move |module_result, act, _ctx| {
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
                                                Ok(_) => {
                                                    // Record that this migration was applied so
                                                    // maybe_lazy_upgrade skips it on future accesses.
                                                    if let Err(err) =
                                                        MigrationsRepository::new(&datastore).set_last_migration(&group_id, &cid, &method, )
                                                    {
                                                        warn!(
                                                            %cid,
                                                            %err,
                                                            "failed to record migration marker"
                                                        );
                                                    }
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
                        // No migration: call update_application_id directly — no mailbox.
                        async move {
                            if let Err(err) = update_application_id(
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
                                warn!(
                                    %cid,
                                    %target_app,
                                    %err,
                                    "lazy upgrade failed, proceeding with current application"
                                );
                            }
                            Ok(guard)
                        }
                        .into_actor(act)
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
            act.get_module(context.application_id, context.service_name.clone())
                .map_ok(move |module, _act, _ctx| (guard, context, module))
        });

        let execution_count = self.metrics.as_ref().map(|m| m.execution_count.clone());
        let execution_duration = self.metrics.as_ref().map(|m| m.execution_duration.clone());

        let execute_task = module_task.and_then(move |(guard, mut context, module), act, _ctx| {
            let datastore = act.datastore.clone();
            let node_client = act.node_client.clone();
            let context_client = act.context_client.clone();

            // Cheap (Arc-backed) clone kept past internal_execute (which moves
            // `datastore`) so a post-call migrate_my_entries can refresh the
            // node-local authored_remaining count (6f.8 drop-after-convert).
            let count_datastore = datastore.clone();

            async move {
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
                    )
                    .await?;

                let duration = start.elapsed().as_secs_f64();
                let status = outcome
                    .returns
                    .is_ok()
                    .then_some("success")
                    .unwrap_or("failure");

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
                    let _ignored = execution_duration
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

                        // Find an owned member of the target context to execute as
                        // We need to use a member that has permissions on the target context
                        use futures_util::TryStreamExt;
                        let members: Vec<_> = context_client
                            .get_context_members(&target_context_id, Some(true))
                            .try_collect()
                            .await
                            .unwrap_or_default();

                        let Some((target_executor, _is_owned)) = members.first() else {
                            error!(
                                %context_id,
                                target_context = ?target_context_id,
                                function = %xcall.function,
                                "No owned members found for target context"
                            );
                            continue;
                        };

                        let target_executor = *target_executor;

                        info!(
                            %context_id,
                            target_context = ?target_context_id,
                            target_executor = ?target_executor,
                            "Found owned member for target context"
                        );

                        // Execute the cross-context call with the target context's member
                        let xcall_result = context_client
                            .execute(
                                &target_context_id,
                                &target_executor,
                                xcall.function.clone(),
                                xcall.params.clone(),
                                vec![],
                                None,
                            )
                            .await;

                        match xcall_result {
                            Ok(_) => {
                                info!(
                                    %context_id,
                                    target_context = ?target_context_id,
                                    function = %xcall.function,
                                    "Cross-context call executed successfully"
                                );
                            }
                            Err(err) => {
                                error!(
                                    %context_id,
                                    target_context = ?target_context_id,
                                    function = %xcall.function,
                                    ?err,
                                    "Cross-context call failed"
                                );
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
    pub fn get_module(
        &self,
        application_id: ApplicationId,
        service_name: Option<String>,
    ) -> impl ActorFuture<Self, Output = eyre::Result<calimero_runtime::Module>> + 'static {
        let service_name_for_bytes = service_name.clone();
        let service_name_for_cache = service_name.clone();
        let blob_task = async {}.into_actor(self).map(move |_, act, _ctx| {
            // Fast path: a compiled module is already cached for this
            // (application_id, service_name) key. Every `get_module`
            // call previously paid ~5% CPU to run
            // `Engine::from_precompiled` (wasmer artifact deserialize)
            // even though the same bytes were being deserialized. Serve
            // the cached Module (cheap Arc clone) and skip the entire
            // blob-fetch / deserialize path. Cache entries are
            // invalidated in `update_application` / migration sites.
            if let Some(cached) = act
                .modules
                .get(&(application_id, service_name_for_cache.clone()))
            {
                return Ok(CachedOrBlob::Cached(cached.clone()));
            }

            // Fetch on a cache miss *before* inserting (so a not-installed app
            // never wastes an eviction); `insert_new` caps the cache. This
            // `get_module` path is the dominant `applications` insert site on a
            // long-running node, so it must honour the cap too.
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

            let blob = app
                .resolve_service_blob(service_name.as_deref())
                .ok_or_else(|| {
                    eyre::eyre!(
                        "service '{}' not found in application {} (available: {})",
                        service_name.as_deref().unwrap_or("<none>"),
                        application_id,
                        if app.services.is_empty() {
                            "<single-service>".to_owned()
                        } else {
                            app.services
                                .keys()
                                .map(String::as_str)
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    )
                })?;

            Ok(CachedOrBlob::Blob(blob))
        });

        let module_task = blob_task.and_then(move |cached_or_blob, act, _ctx| {
            let node_client = act.node_client.clone();
            // Operator-configured limits, baked into the engine that compiles
            // or deserializes the module here and applied at execution time.
            let vm_limits = act.vm_limits;

            async move {
                let mut blob = match cached_or_blob {
                    // Fast path: cache hit. Skip blob fetch + deserialize
                    // + compile. `blob_info = None` signals the post-task
                    // closure below that there's nothing new to write
                    // back into `applications` or the module cache.
                    CachedOrBlob::Cached(module) => return Ok((module, None)),
                    CachedOrBlob::Blob(blob) => blob,
                };

                // Staleness anchor for the `map_ok` below. Captures the
                // blob_id we loaded from *before* any recompile rewrites
                // `blob.compiled`. A concurrent `update_application`
                // migration between now and `map_ok` would evict and
                // repopulate `applications[app_id]` with a different
                // blob_id — the post-task closure compares against this
                // anchor and drops both the blob writeback and the
                // module cache insert when they disagree.
                let original_blob_id = blob.compiled;

                if let Some(compiled) = node_client.get_blob_bytes(&blob.compiled, None).await? {
                    let module = unsafe {
                        calimero_runtime::Engine::headless_with_limits(vm_limits)
                            .from_precompiled(&compiled)
                    };

                    match module {
                        Ok(module) => {
                            return Ok((
                                module,
                                Some((blob, service_name_for_bytes, original_blob_id, None)),
                            ))
                        }
                        Err(err) => {
                            debug!(
                                ?err,
                                %application_id,
                                blob_id=%blob.compiled,
                                "failed to load precompiled module, recompiling.."
                            );
                        }
                    }
                }

                debug!(
                    %application_id,
                    blob_id=%blob.compiled,
                    "no usable precompiled module found, compiling.."
                );

                // Use get_application_bytes instead of get_blob_bytes for bytecode
                // because get_application_bytes knows how to extract WASM from bundles
                let Some(bytecode) = node_client
                    .get_application_bytes(&application_id, service_name_for_bytes.as_deref())
                    .await?
                else {
                    bail!(ExecuteError::ApplicationNotInstalled { application_id });
                };

                // Extract the read-only method set from the embedded ABI manifest
                // before moving `bytecode` into the blocking compile task.
                // A missing/unparseable manifest is not an error — the execute
                // path defaults to the write lock (fail-safe).
                let read_only_set = extract_read_only_set(&bytecode);

                // Compile WASM in a blocking task to avoid blocking the async executor.
                // Note: panics during compilation will surface as JoinError.
                let module = global_runtime()
                    .spawn_blocking(move || {
                        calimero_runtime::Engine::with_limits(vm_limits).compile(&bytecode)
                    })
                    .await
                    .wrap_err("WASM compilation task failed")? // JoinError (task panicked/cancelled)
                    ?; // Compilation error

                let compiled = Cursor::new(module.to_bytes()?);

                let (blob_id, _ignored) = node_client.add_blob(compiled, None, None).await?;

                blob.compiled = blob_id;

                node_client.update_compiled_app(
                    &application_id,
                    &blob_id,
                    service_name_for_bytes.as_deref(),
                )?;

                Ok((
                    module,
                    Some((
                        blob,
                        service_name_for_bytes,
                        original_blob_id,
                        read_only_set,
                    )),
                ))
            }
            .into_actor(act)
        });

        module_task
            .map_ok(move |(module, blob_info), act, _ctx| {
                if let Some((blob, svc_name, original_blob_id, read_only_set)) = blob_info {
                    // The `applications` map is the source of truth for
                    // whether this app is still live. Tie both the blob
                    // writeback and the module cache update to the same
                    // guard, plus a staleness check on `original_blob_id`.
                    //
                    // Three cases we need to handle correctly:
                    //
                    // 1. App still present with the same blob_id we
                    //    loaded from → our module is current, write
                    //    back and cache.
                    // 2. App evicted (`get_mut` → None) — an
                    //    `update_application` migration ran and hasn't
                    //    repopulated yet → our work is stale, drop.
                    // 3. App repopulated with a different blob_id —
                    //    another `get_module` call beat us to it with
                    //    fresher data → our work is stale, drop both
                    //    writeback and module cache insert so we don't
                    //    stomp the fresh state.
                    if let Some(app) = act.applications.get_mut(&application_id) {
                        // Both the staleness lookup and the writeback
                        // must mirror `Application::resolve_service_blob`
                        // exactly — that's where `blob` came from at
                        // load time. For single-service bundles called
                        // with `svc_name = None`, `resolve_service_blob`
                        // returns `services.values().next()`, *not*
                        // `app.blob`. Reading `app.blob.compiled` for
                        // the staleness check would fail every time on
                        // such bundles (the two blob_ids would never
                        // match), defeating both the cache and the
                        // recompile writeback.
                        let current_blob_id = match svc_name.as_deref() {
                            None if app.services.is_empty() => Some(app.blob.compiled),
                            None if app.services.len() == 1 => {
                                app.services.values().next().map(|b| b.compiled)
                            }
                            // Multi-service with no name is ambiguous —
                            // `resolve_service_blob` returns None so we
                            // never get here via the happy path. Treat
                            // as stale.
                            None => None,
                            Some(name) => app.services.get(name).map(|b| b.compiled),
                        };

                        if current_blob_id == Some(original_blob_id) {
                            // Writeback target must match the same
                            // resolution. `services.len() == 1` with
                            // `svc_name = None` means the single
                            // service entry is the owner — pull its
                            // key so we can `get_mut` into it without
                            // iterating twice.
                            let target_service_key =
                                if svc_name.is_none() && app.services.len() == 1 {
                                    app.services.keys().next().cloned()
                                } else {
                                    svc_name.clone()
                                };
                            match target_service_key.as_deref() {
                                Some(name) => {
                                    if let Some(svc_blob) = app.services.get_mut(name) {
                                        *svc_blob = blob;
                                    }
                                }
                                None => {
                                    app.blob = blob;
                                }
                            }

                            // `BoundedCache::insert` caps the map: replacing an
                            // already-cached (recompiled) key overwrites in
                            // place, while a new key evicts a by-key-order
                            // victim first when at capacity.
                            let cache_key = (application_id, svc_name);
                            let _ = act.modules.insert(cache_key.clone(), module.clone());
                            // Populate the read-only method set only when we
                            // successfully parsed the embedded ABI (fresh-compile
                            // path). When `read_only_set` is None (precompiled
                            // path where raw bytecode is not re-fetched), skip the
                            // insert so a subsequent fresh compile can populate it
                            // correctly. An absent entry in `read_only_methods`
                            // falls back to the write lock (fail-safe).
                            if let Some(set) = read_only_set {
                                let _ = act.read_only_methods.insert(cache_key, set);
                            }
                        } else {
                            debug!(
                                %application_id,
                                loaded_blob = %original_blob_id,
                                current_blob = ?current_blob_id,
                                "module load result stale — applications was repopulated while loading, dropping"
                            );
                        }
                    }
                }

                module
            })
            .map_err(|err, _act, _ctx| {
                error!(?err, "failed to initialize module for execution");

                err
            })
    }
}

/// Result of the blob-resolution / cache-lookup phase inside
/// [`ContextManager::get_module`]. Either we already have the compiled
/// module (fast path — skip deserialize), or we have the blob metadata
/// pointing at where the compiled bytes live and need to fetch +
/// deserialize.
enum CachedOrBlob {
    Cached(calimero_runtime::Module),
    Blob(calimero_primitives::application::ApplicationBlob),
}

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
) -> eyre::Result<(
    Outcome,
    Option<CausalDelta>,
    Option<[u8; 64]>,
    Option<GovernancePosition>,
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
    let mut governance_position_for_broadcast: Option<GovernancePosition> = None;

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
            if actions.len() != 0 {
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
                persist_signed_signatures(&store, &context, identity_private_key, &actions)
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

            // Leg 4 of the ACL-in-root fold (core#2716): the local write path
            // computed each `Shared` anchor's `own_hash` DURING WASM execution —
            // before this `delta_id` existed — so the originator's own rotation
            // isn't in its rotation log yet and the fold used the pre-rotation
            // writer set. Now that the (signed) delta is built, self-log its
            // rotations + rehash the affected anchors, then recompute the
            // context root so BOTH `context.root_hash` and the delta's
            // `expected_root_hash` reflect the new writer set. Peers fold the
            // same resolved set when they apply the rotation, so every node
            // converges. No-op unless this delta rotates a `Shared` writer set.
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
                        let changed = Interface::<MainStorage>::self_log_and_rehash_own_rotations(
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
                                eyre::eyre!("root index missing after rotation rehash")
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
                module.run(
                    context_id,
                    executor,
                    &method,
                    &input,
                    &mut ro_storage,
                    Some(&mut ro_private),
                    Some(node_client),
                )?
            } else {
                module.run(
                    context_id,
                    executor,
                    &method,
                    &input,
                    &mut storage,
                    Some(&mut private_storage),
                    Some(node_client),
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
                .ok_or_else(|| ExecuteError::AliasResolutionFailed { alias: *alias })?;

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
    use calimero_governance_store::{register_context_in_group, MetaRepository};
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::{ContextId, UpgradePolicy};
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;
    use calimero_store::Store;

    use super::{
        resolve_producing_app_key, should_block, upgrade_blocks_write,
        upgrade_rejects_committed_write,
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
}
