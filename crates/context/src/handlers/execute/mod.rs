use calimero_governance_store::{
    CapabilitiesRepository, GroupKeyring, MetaRepository, MigrationsRepository, NamespaceRepository,
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
use calimero_context_client::{ContextAtomic, ContextAtomicKey};
use calimero_context_config::types::{ContextGroupId, GovernancePosition};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{Context, ContextId, UpgradePolicy};
use calimero_primitives::events::{
    ContextEvent, ContextEventPayload, ExecutionEvent, NodeEvent, StateMutationPayload,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_runtime::logic::Outcome;
use calimero_storage::{
    action::Action,
    delta::{CausalDelta, StorageDelta},
    entities::StorageType,
    env::{with_runtime_env, RuntimeEnv},
    interface::Interface,
    store::MainStorage,
};
use calimero_store::{key, types, Store};
use calimero_utils_actix::global_runtime;
use either::Either;
use eyre::{bail, WrapErr};
use futures_util::future::TryFutureExt;
use futures_util::io::Cursor;
use memchr::memmem;
use tokio::sync::OwnedMutexGuard;
use tracing::{debug, error, info, warn};

use crate::error::ContextError;
use crate::handlers::update_application::{
    create_storage_callbacks, update_application_id, update_application_with_migration,
};
use crate::ContextManager;
use calimero_governance_store::metrics::ExecutionLabels;

pub mod storage;

use storage::{ContextPrivateStorage, ContextStorage};

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

        let context = match self.get_or_fetch_context(&context_id) {
            Ok(Some(context)) => context,
            Ok(None) => return ActorResponse::reply(Err(ExecuteError::ContextNotFound)),
            Err(err) => {
                error!(%err, "failed to execute request");

                return ActorResponse::reply(Err(ExecuteError::InternalError));
            }
        };

        let current_application_id = context.meta.application_id;

        let is_state_op = "__calimero_sync_next" == method;

        if !is_state_op && *context.meta.root_hash == [0; 32] {
            return ActorResponse::reply(Err(ExecuteError::Uninitialized));
        }

        let (guard, is_atomic) = match atomic {
            None => (context.lock(), false),
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
                        if upgrade_blocks_write(&upgrade.status) {
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
        let context_task = context_task.map(
            move |guard_result: eyre::Result<OwnedMutexGuard<ContextId>>, act, _ctx| {
                let guard = guard_result?;
                let Some(context) = act.get_or_fetch_context(&context_id)? else {
                    bail!(ContextError::ContextDeleted { context_id });
                };

                Ok((guard, context.meta.clone()))
            },
        );

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
                                Some((blob, service_name_for_bytes, original_blob_id)),
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
                    Some((blob, service_name_for_bytes, original_blob_id)),
                ))
            }
            .into_actor(act)
        });

        module_task
            .map_ok(move |(module, blob_info), act, _ctx| {
                if let Some((blob, svc_name, original_blob_id)) = blob_info {
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
                            let _ = act
                                .modules
                                .insert((application_id, svc_name), module.clone());
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

/// Compute the [`GovernancePosition`] to embed in the next state delta from
/// this context.
///
/// Returns `None` for non-group contexts (which have no governance DAG to
/// reference) and on any read failure — receivers will surface the missing
/// position via the apply-time membership check rather than silently
/// relying on it.
fn compute_governance_position_for_context(
    datastore: &Store,
    context_id: &ContextId,
) -> Option<GovernancePosition> {
    let group_id = match calimero_governance_store::get_group_for_context(datastore, context_id) {
        Ok(Some(gid)) => gid,
        Ok(None) => return None,
        Err(err) => {
            tracing::warn!(
                %context_id,
                %err,
                "compute_governance_position: get_group_for_context failed"
            );
            return None;
        }
    };

    let namespace_id = match NamespaceRepository::new(datastore).resolve(&group_id) {
        Ok(ns_id) => ns_id,
        Err(err) => {
            tracing::warn!(
                %context_id,
                group_id = ?group_id,
                %err,
                "compute_governance_position: resolve_namespace failed"
            );
            return None;
        }
    };

    let dag =
        calimero_governance_store::NamespaceDagService::new(datastore, namespace_id.to_bytes());

    // Double-read pattern: governance ops can apply between reading heads
    // and computing the state hash, producing an internally-inconsistent
    // position whose hash and heads disagree. Re-read heads after the hash
    // and bail if they changed — the receiver's heads-equal fast path
    // treats hash mismatch as a hard rejection, so shipping a stale value
    // would spuriously reject legitimate deltas. A true atomic read would
    // require refactoring `compute_group_state_hash` and `read_head_record`
    // to share a `Handle` (snapshot view); the double-read covers the
    // race window with a single extra cheap read.
    let heads_before = match dag.read_head_record() {
        Ok(head) => head.parent_hashes,
        Err(err) => {
            tracing::warn!(
                %context_id,
                group_id = ?group_id,
                %err,
                "compute_governance_position: read_head_record failed (before)"
            );
            return None;
        }
    };

    let group_state_hash = match MetaRepository::new(datastore).compute_state_hash(&group_id) {
        Ok(hash) => hash,
        Err(err) => {
            tracing::warn!(
                %context_id,
                group_id = ?group_id,
                %err,
                "compute_governance_position: compute_group_state_hash failed"
            );
            return None;
        }
    };

    let heads_after = match dag.read_head_record() {
        Ok(head) => head.parent_hashes,
        Err(err) => {
            tracing::warn!(
                %context_id,
                group_id = ?group_id,
                %err,
                "compute_governance_position: read_head_record failed (after)"
            );
            return None;
        }
    };

    // Set-equality, not Vec equality. Storage iteration order isn't guaranteed
    // to be stable across two reads, so a Vec equality check would treat
    // [h1, h2] vs [h2, h1] as a stale read and emit None for every state delta
    // — receivers then reject the delta on the no-position-on-group-context
    // anti-bypass branch and the wire wedges even when the underlying head
    // set didn't actually change.
    let heads_changed = {
        use std::collections::HashSet;
        heads_before.len() != heads_after.len()
            || heads_before.iter().collect::<HashSet<_>>()
                != heads_after.iter().collect::<HashSet<_>>()
    };
    if heads_changed {
        tracing::warn!(
            %context_id,
            group_id = ?group_id,
            "compute_governance_position: governance heads changed mid-read; \
             skipping position to avoid hash/heads divergence"
        );
        return None;
    }

    match GovernancePosition::new(group_id, group_state_hash, heads_after) {
        Ok(pos) => Some(pos),
        Err(err) => {
            // Local DAG has more heads than MAX_GOVERNANCE_DAG_HEADS allows
            // on the wire — refuse to emit rather than ship a position that
            // the receiver's bounded BorshDeserialize will reject. Indicates
            // either pathological concurrent admin activity or local
            // corruption; logging here surfaces it for operators.
            tracing::warn!(
                %context_id,
                group_id = ?group_id,
                %err,
                "compute_governance_position: refusing to embed oversized position"
            );
            None
        }
    }
}

async fn internal_execute(
    datastore: Store,
    node_client: &NodeClient,
    _context_client: &ContextClient,
    module: calimero_runtime::Module,
    guard: &OwnedMutexGuard<ContextId>,
    context: &mut Context,
    executor: PublicKey,
    method: Cow<'static, str>,
    input: Cow<'static, [u8]>,
    is_state_op: bool,
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

            let delta = CausalDelta {
                id: delta_id,
                parents,
                actions,
                hlc,
                expected_root_hash: root_hash,
            };

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
    context: &OwnedMutexGuard<ContextId>,
    module: calimero_runtime::Module,
    executor: PublicKey,
    method: Cow<'static, str>,
    input: Cow<'static, [u8]>,
    mut storage: ContextStorage,
    mut private_storage: ContextPrivateStorage,
    node_client: NodeClient,
) -> eyre::Result<(Outcome, ContextStorage, ContextPrivateStorage)> {
    let context_id = **context;

    global_runtime()
        .spawn_blocking(move || {
            let outcome = module.run(
                context_id,
                executor,
                &method,
                &input,
                &mut storage,
                Some(&mut private_storage),
                Some(node_client),
            )?;
            Ok((outcome, storage, private_storage))
        })
        .await
        .wrap_err("failed to receive execution response")?
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

/// Helper function to sign authorized actions (User and Shared storage).
/// Iterates over actions and signs any that are local and unsigned.
pub(crate) fn sign_authorized_actions(
    actions: &mut [Action],
    identity_private_key: &PrivateKey,
) -> eyre::Result<()> {
    info!(
        actions_count = actions.len(),
        "Signing authorized actions..."
    );
    for action in actions.iter_mut() {
        let action_id = action.id();
        let payload_for_signing = action.payload_for_signing();

        // The nonce was already set by `calimero-storage`:
        // * For Add/Update, it's `metadata.updated_at`.
        // * For DeleteRef, it's `deleted_at`.
        // We just need to ensure the action's nonce field matches
        let (metadata, nonce) = match action {
            Action::Add { metadata, .. } => {
                let nonce = *metadata.updated_at;
                (metadata, nonce)
            }
            Action::Update { metadata, .. } => {
                let nonce = *metadata.updated_at;
                (metadata, nonce)
            }
            Action::DeleteRef {
                metadata,
                deleted_at,
                ..
            } => {
                let nonce = *deleted_at;
                (metadata, nonce)
            }
            Action::Compare { .. } => continue,
        };

        if let StorageType::User {
            owner,
            signature_data: Some(sig_data),
        } = &mut metadata.storage_type
        {
            debug!(
                action_id = ?action_id,
                owner = %owner,
                nonce = %nonce,
                "Received user action from the outcome"
            );

            // Check if it's ours and is currently unsigned (placeholder signature)
            if *owner == identity_private_key.public_key() && sig_data.signature == [0; 64] {
                // Re-set the nonce in sig_data just in case
                sig_data.nonce = nonce;

                // TODO: Add `.map_err`.
                let signature = identity_private_key.sign(&payload_for_signing)?;
                sig_data.signature = signature.to_bytes();

                debug!(
                    action_id = ?action_id,
                    action_id = %action_id,
                    owner = %owner,
                    owner = ?owner.digest(),
                    nonce = %nonce,
                    payload_for_signing = ?payload_for_signing,
                    ed25519_signature = ?signature,
                    signature = ?sig_data.signature,
                    signature_len = sig_data.signature.len(),
                    "Signed user action"
                );
            }
        }

        if let StorageType::Shared {
            writers,
            signature_data: Some(sig_data),
            ..
        } = &mut metadata.storage_type
        {
            let executor_pk = identity_private_key.public_key();
            debug!(
                action_id = ?action_id,
                writer_count = writers.len(),
                executor = %executor_pk,
                nonce = %nonce,
                "Received shared action from the outcome"
            );

            // Sign whenever the placeholder is present. The stamping decision
            // (whether the executor was authorized to act) was already made in
            // save_raw / remove_child_from based on stored ∪ claimed writers,
            // which correctly handles the rotate-self-out case where the
            // executor is no longer in the action's claimed writer set.
            if sig_data.signature == [0; 64] {
                sig_data.nonce = nonce;
                let signature = identity_private_key.sign(&payload_for_signing)?;
                sig_data.signature = signature.to_bytes();

                debug!(
                    action_id = ?action_id,
                    executor = %executor_pk,
                    nonce = %nonce,
                    payload_for_signing = ?payload_for_signing,
                    ed25519_signature = ?signature,
                    "Signed shared action"
                );
            }
        }

        // SharedMember signs exactly like Shared: the placeholder presence is
        // the signal, and the authorization decision (executor ∈ the anchor's
        // writers) was already made in `save_raw` against the anchor's resolved
        // set. A member carries no writer set, so there is nothing to log here
        // beyond the anchor.
        if let StorageType::SharedMember {
            anchor,
            signature_data: Some(sig_data),
            ..
        } = &mut metadata.storage_type
        {
            if sig_data.signature == [0; 64] {
                sig_data.nonce = nonce;
                let signature = identity_private_key.sign(&payload_for_signing)?;
                sig_data.signature = signature.to_bytes();

                debug!(
                    action_id = ?action_id,
                    anchor = %anchor,
                    nonce = %nonce,
                    "Signed shared-member action"
                );
            }
        }

        if let StorageType::User {
            owner: _,
            signature_data: Some(_),
        } = &metadata.storage_type
        {
            debug!(
                action_serialized = ?borsh::to_vec(action)?,
                "After signing user action"
            );
        }
    }
    Ok(())
}

/// Persist the signed `signature_data` from `sign_authorized_actions`
/// back to the local index entry for each upsert action.
///
/// Best-effort: structural mismatches and missing entities are logged
/// and skipped rather than failing the whole execute call. The Action
/// in the broadcast artifact carries the real signature; this function
/// keeps the locally stored entity's metadata in sync so HashComparison
/// (and any other receiver-verifying sync path) ships verifiable state.
///
/// Runs inside a `with_runtime_env` scope built over the post-commit
/// `Store` handle — `Interface::<MainStorage>::update_signature_in_place`
/// reads + writes the entity's `EntityIndex` blob through this runtime
/// env, which routes via `create_storage_callbacks` to the same
/// RocksDB keys that `storage.commit()` just wrote.
pub(crate) fn persist_signed_signatures(
    store: &Store,
    context: &Context,
    identity_private_key: &PrivateKey,
    actions: &[Action],
) -> eyre::Result<()> {
    let callbacks = create_storage_callbacks(store, context.id);
    let context_id_bytes: [u8; 32] = *context.id.as_ref();
    let executor_id_bytes: [u8; 32] = *identity_private_key.public_key().as_ref();
    let env = RuntimeEnv::new(
        callbacks.read,
        callbacks.write,
        callbacks.remove,
        context_id_bytes,
        executor_id_bytes,
    );

    // Collect failures inside the env scope and propagate after.
    // Returning Result lets the caller (`execute_method` or
    // `create_context`) decide whether to abort the transaction:
    // a failed persist leaves the locally stored entity with the
    // `[0; 64]` placeholder signature, so subsequent HashComparison
    // sync would ship the placeholder to peers and trip the
    // receiver's signature verifier. The signed broadcast artifact
    // still carries the real signature for delta-replay receivers,
    // but the local node would be permanently stuck shipping
    // unverifiable HashComparison responses until the next signed
    // write to that entity. Aborting and surfacing the error gives
    // the user a chance to retry.
    let result: eyre::Result<()> = with_runtime_env(env, || {
        for action in actions {
            let (id, storage_type, is_delete) = match action {
                Action::Add { id, metadata, .. } | Action::Update { id, metadata, .. } => {
                    (*id, metadata.storage_type.clone(), false)
                }
                // DeleteRef carries a real signature too (signed by
                // `sign_authorized_actions`). Persist it onto the now-tombstoned
                // index entry — `update_signature_in_place` RMWs the index, which
                // survives the delete — so HashComparison can later ship a
                // *verifiable* signed DeleteRef for the cleared entity (otherwise
                // a User/Shared clear can't converge via HC, only via the delta
                // stream). The tombstone's owner/writer set is unchanged by the
                // delete, so the in-place patch's identity guard still matches.
                // Marked `is_delete` so a persist failure is BEST-EFFORT (see
                // the `Err` arm): unlike Add/Update, a missed tombstone
                // signature only degrades HC clear-convergence — the deletion
                // still propagates via the delta stream — so it must NOT abort
                // the transaction.
                Action::DeleteRef { id, metadata, .. } => {
                    (*id, metadata.storage_type.clone(), true)
                }
                Action::Compare { .. } => continue,
            };
            // Only Shared/User with a REAL signature need the
            // re-persist. Public/Frozen don't carry signatures.
            //
            // Three skip conditions:
            // 1. `signature_data: None` — unsigned bootstrap action;
            //    `sign_authorized_actions` doesn't touch these.
            // 2. `signature_data: Some(SignatureData { signature: [0;
            //    64], .. })` — placeholder that
            //    `sign_authorized_actions` declined to sign (e.g. a
            //    `User` action whose owner ≠ executor, or a `Shared`
            //    action where the executor isn't in the writer set).
            //    Persisting the placeholder here would overwrite the
            //    real signature already stored for that entity.
            // 3. Anything else falls through to
            //    `update_signature_in_place`.
            let signed_with_real_sig = match &storage_type {
                StorageType::Shared {
                    signature_data: Some(sig),
                    ..
                }
                | StorageType::User {
                    signature_data: Some(sig),
                    ..
                }
                | StorageType::SharedMember {
                    signature_data: Some(sig),
                    ..
                } if sig.signature != [0u8; 64] => true,
                _ => false,
            };
            if !signed_with_real_sig {
                continue;
            }
            match Interface::<MainStorage>::update_signature_in_place(id, storage_type) {
                Ok(true) => {
                    debug!(%id, "persisted signed signature_data to local index");
                }
                Ok(false) => {
                    debug!(
                        %id,
                        "skipped signature persist — entity missing from local index \
                         (raced a delete?)"
                    );
                }
                Err(e) if is_delete => {
                    // BEST-EFFORT for deletes: a failed tombstone
                    // signature-persist only means this DeleteRef can't
                    // ship verifiably via HashComparison — the deletion
                    // still converges via the delta stream. Never abort
                    // the transaction over it (the strict path below is
                    // for Add/Update, where a placeholder would make a
                    // *live* entity unverifiable on peers).
                    warn!(
                        %id,
                        error = ?e,
                        "skipped persisting signed DeleteRef signature; HC clear-convergence \
                         degraded for this entity (delta-stream propagation unaffected)"
                    );
                }
                Err(e) => {
                    // Fail loud + propagate. The alternatives
                    // (silent log, metric, ignore) leave the local
                    // entity with a placeholder forever — see the
                    // function-level comment.
                    error!(
                        %id,
                        error = ?e,
                        "failed to persist signed signature_data; local entity would \
                         retain placeholder signature and fail HashComparison \
                         verification on peers — aborting transaction so the user \
                         can retry"
                    );
                    return Err(eyre::eyre!(
                        "persist_signed_signatures: update_signature_in_place failed \
                         for entity {id}: {e:?}"
                    ));
                }
            }
        }
        Ok(())
    });
    result
}

/// Returns `true` when a group-upgrade status should block ALL writes
/// (both user calls and state-op writes such as `__calimero_sync_next`).
///
/// Only `GroupUpgradeStatus::InProgress` blocks.  `Completed` (with or
/// without a timestamp) never blocks.  This is the single source of truth
/// for the cascade-upgrade write-gate decision.
///
/// # Safety invariants
///
/// * `LazyOnAccess` upgrades write `Completed` directly (never `InProgress`),
///   so this fn never returns `true` during a lazy migration.
/// * The eager propagator's own writes go through `UpdateApplicationRequest`
///   → `handlers::update_application`, which bypasses the execute gate
///   entirely — no deadlock is possible.
/// * Sync-pipeline (`__calimero_sync_next`) failures during `InProgress` are
///   retried by the periodic sync cycle once the upgrade reaches `Completed`.
fn upgrade_blocks_write(status: &calimero_store::key::GroupUpgradeStatus) -> bool {
    matches!(
        status,
        calimero_store::key::GroupUpgradeStatus::InProgress { .. }
    )
}

/// Post-execution write-gate decision: during an in-progress upgrade a pure read
/// (`produced_write == false`) is served from the pre-migration root; a
/// side-effecting call is refused. Write-intent is derived post-execution (a
/// committed `root_hash` or queued `xcalls`) because no read-vs-write flag exists
/// upstream (`ExecuteRequest`, RPC, SDK, ABI).
fn upgrade_rejects_committed_write(block_writes: bool, produced_write: bool) -> bool {
    block_writes && produced_write
}

/// Checks if a context belongs to a group with LazyOnAccess policy and
/// needs an upgrade or migration.
///
/// Returns `(target_application_id, migrate_method, group_id)` when an
/// upgrade should be performed.  The `group_id` is included so the caller
/// can record a per-context migration marker after a successful run.
fn maybe_lazy_upgrade(
    datastore: &Store,
    context_id: &ContextId,
    current_application_id: &ApplicationId,
) -> Option<(
    ApplicationId,
    Option<String>,
    calimero_context_config::types::ContextGroupId,
)> {
    use calimero_governance_store;

    // 1. Check if context belongs to a group
    let group_id = match calimero_governance_store::get_group_for_context(datastore, context_id) {
        Ok(Some(gid)) => gid,
        Ok(None) => return None, // not in a group
        Err(err) => {
            debug!(%err, %context_id, "failed to check group for context during lazy upgrade");
            return None;
        }
    };

    // 2. Load group metadata
    let meta = match MetaRepository::new(datastore).load(&group_id) {
        Ok(Some(m)) => m,
        Ok(None) => return None, // group deleted?
        Err(err) => {
            debug!(%err, ?group_id, "failed to load group meta during lazy upgrade");
            return None;
        }
    };

    // 3. Check policy is LazyOnAccess
    if !matches!(meta.upgrade_policy, UpgradePolicy::LazyOnAccess) {
        return None;
    }

    // 4. Extract migration method from group meta (set during upgrade)
    let migrate_method = meta
        .migration
        .as_ref()
        .and_then(|bytes| String::from_utf8(bytes.clone()).ok());

    // 5. Compare current vs target application
    if *current_application_id == meta.target_application_id {
        // IDs match — only proceed if there is a pending migration that
        // hasn't been applied to this context yet.
        let Some(ref method) = migrate_method else {
            return None; // no migration, context is already up to date
        };

        // Check per-context marker set after a successful migration run.
        let already_applied = MigrationsRepository::new(datastore)
            .last_migration(&group_id, context_id)
            .ok()
            .flatten()
            .map(|last| last == *method)
            .unwrap_or(false);

        if already_applied {
            return None; // migration was already applied to this context
        }
        // Fall through: migration is pending.
    }

    info!(
        %context_id,
        ?group_id,
        %current_application_id,
        target_app=%meta.target_application_id,
        "lazy upgrade triggered for context"
    );

    Some((meta.target_application_id, migrate_method, group_id))
}

/// The blob-derived app key the sender is executing under — `GroupMeta.app_key`
/// for the context's owning group (`app_key = blob_id(bytecode)` at group
/// creation / upgrade time).  This is the schema-version discriminator that
/// changes on every app upgrade; `application_id` is version-stable and
/// cannot distinguish v1 from v2 of the same application.
///
/// Returns `Some(app_key)` for group-context deltas; `None` for non-group
/// contexts (no owning group) or when the group meta row cannot be loaded
/// (store error is propagated to the caller as `Err`).
///
/// Stamped onto the state-delta broadcast so receivers can fence
/// stale-schema deltas after a cascade migration.  The fence itself lives
/// in Tasks 8/9 — this function is the testable store-boundary helper.
fn resolve_producing_app_key(
    datastore: &Store,
    context_id: &ContextId,
) -> eyre::Result<Option<[u8; 32]>> {
    let Some(gid) = calimero_governance_store::get_group_for_context(datastore, context_id)? else {
        return Ok(None);
    };
    Ok(MetaRepository::new(datastore)
        .load(&gid)?
        .map(|m| m.app_key))
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

    use super::{resolve_producing_app_key, upgrade_blocks_write, upgrade_rejects_committed_write};
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

    // PR-6a Task 6a.1: the `migration_v2` feature flag must default OFF so
    // master behavior is completely unchanged until the flag is flipped (after
    // 6b lands). The flag lives on `ContextManagerConfig` — the same
    // runtime-tunable knobs struct threaded into this handler via `self.config`.
    #[test]
    fn migration_v2_flag_defaults_off() {
        let cfg = crate::ContextManagerConfig::default();
        assert!(
            !cfg.migration_v2,
            "migration_v2 must default off so master behavior is unchanged"
        );
    }
}
