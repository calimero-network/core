use calimero_governance_store::{GroupKeyring, MetaRepository, NamespaceRepository};
use std::borrow::Cow;
// Removed: NonZeroUsize (replaced with CausalDelta)
use std::time::Instant;

use actix::{
    ActorFuture, ActorFutureExt, ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture,
};
use calimero_app_downloader::registry::RegistryCoordsBuf;
use calimero_app_downloader::{AppRequest, Outcome as AcquireOutcome};
use calimero_context_client::client::crypto::ContextIdentity;
use calimero_context_client::client::ContextClient;
use calimero_context_client::local_governance::AckRouter;
use calimero_context_client::messages::{
    ExecuteError, ExecuteEvent, ExecuteRequest, ExecuteResponse, InternalErrorKind,
    MigrationParams, WriteSource,
};
use calimero_context_client::{ContextAtomic, ContextAtomicKey, ContextGuard};
use calimero_context_config::types::{ContextGroupId, GovernanceParentEdge};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::events::{
    ContextEvent, ContextEventPayload, ExecutionEvent, NodeEvent, StateMutationPayload,
    XCallOutcome, XCallPayload,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_runtime::logic::Outcome;
use calimero_storage::delta::{CausalDelta, StorageDelta};
use std::collections::HashSet;
use std::sync::Arc;

use calimero_store::{key, types, Store};
use calimero_utils_actix::global_runtime;
use calimero_wasm_abi::schema::{MethodIntent, XCallCallers};
use either::Either;
use eyre::{bail, WrapErr};
use futures_util::future::TryFutureExt;
use tracing::{debug, error, info, warn};

use crate::error::ContextError;
use crate::handlers::update_application::{
    clear_migration_failed, persist_migration_failed, update_application_id,
    update_application_with_migration,
};
use crate::ContextManager;
use calimero_context_client::group::MigrationFailureKind;
use calimero_governance_store::metrics::ExecutionLabels;

use self::principal::Principal;

mod governance_position;
pub(crate) mod principal;
mod shared_rotations;
pub(crate) use shared_rotations::{refuse_unpublishable, storage_at_current_cut};
mod signing;
pub mod storage;
mod upgrade_gate;

/// Maximum depth of a local xcall cascade. A direct/RPC call runs at depth 0,
/// and each `xcall` it (transitively) triggers runs one level deeper. An
/// execution *at* this depth still runs, but its own xcalls are denied — so
/// executions span depths `0..=MAX_XCALL_DEPTH` and a cascade makes at most
/// `MAX_XCALL_DEPTH` xcall hops beyond the root.
///
/// xcalls dispatch locally and each spawned execution can queue up to
/// `max_xcalls` more (breadth `B`, default 8), so without a depth bound one
/// root call could recurse forever (a cycle A→B→A) or fan out `B^depth`. This
/// cap bounds the executions one root call spawns to `B + B² + … + B^DEPTH`
/// (children at depths 1..=`DEPTH`) — 584 at `B = 8`, `DEPTH = 3`.
///
/// Changing this value also changes the settled hop count asserted by the
/// depth-cap scenario in `apps/xcall-example/workflows/xcall.yml` (it expects
/// `MAX_XCALL_DEPTH + 1`); update that assertion alongside this constant.
const MAX_XCALL_DEPTH: u32 = 3;

/// Prefix of the SDK's own exports, which no event may name as its handler.
const SDK_EXPORT_PREFIX: &str = "__calimero";

use governance_position::compute_governance_position_for_context;
pub(crate) use signing::{persist_signed_signatures, sign_authorized_actions};
use storage::{ContextPrivateStorage, ContextStorage, ReadOnlyContextStorage};
use upgrade_gate::{
    maybe_lazy_upgrade, resolve_producing_bytecode_id, should_block, upgrade_blocks_write,
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
            atomic,
            xcall_origin,
            delegation,
            xcall_depth,
            read_as,
            tee_trigger,
            event_handler,
            write_source,
            governance_position,
        }: ExecuteRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // A direct/RPC call always starts a fresh cascade at depth 0; only an
        // execution dispatched via `xcall` (`xcall_origin` = `Some`) carries a
        // depth, and that depth was set by our own dispatch loop below. Ignore
        // any depth supplied on a non-xcall request so the cap can't be
        // sidestepped by handing the actor an `ExecuteRequest` with a high
        // `xcall_depth`. Invariant: `xcall_origin.is_none()` ⟺ depth 0.
        let xcall_depth = if xcall_origin.is_some() {
            xcall_depth
        } else {
            0
        };

        info!(
            %context_id,
            method,
            "Executing method in context"
        );
        debug!(
            %context_id,
            %executor,
            method,
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
        // (immutable) field access. The lookup yields a bool, so the borrow is
        // released before context is fetched.
        //
        // This is safe: the read-only set rides in the `modules` entry; a cold
        // miss (None) silently defaults to the write lock.
        let is_state_op = "__calimero_sync_next" == method;

        // `RemoteDelta` relaxes the write gate to "is this node a replica", which
        // is only sound for the merge-apply of a peer's delta. The one
        // constructor that sets it names that method itself, so any other
        // pairing is a caller bug: refuse it rather than run it under either
        // rule.
        if write_source == WriteSource::RemoteDelta && !is_state_op {
            error!(
                %context_id,
                method,
                "refusing execute: a remote-delta apply must run __calimero_sync_next"
            );
            return ActorResponse::reply(Err(ExecuteError::Unauthorized {
                context_id,
                public_key: executor,
            }));
        }

        let is_read_only_call = 'ro: {
            if is_state_op || matches!(atomic, Some(ContextAtomic::Held(_))) {
                break 'ro false;
            }
            // The search exports only read, whatever the ABI says (it does not
            // list them): the indexer's calls share the lock with views.
            if calimero_primitives::search::is_export(&method) {
                break 'ro true;
            }
            // We don't yet have `context`, so we can't form the full cache key
            // yet. Peek at `contexts` to get the application_id + service_name,
            // then look up the module's read-only set. Both are reads with no
            // structural changes, so this is safe even though contexts is &mut below.
            let Some(cm) = self.contexts.get(&context_id) else {
                break 'ro false; // not cached yet — conservative write lock
            };
            let application_id = cm.meta.application_id;
            let service_name = cm.meta.service_name.clone();
            // Blob-keyed lookup: the read-only sets are keyed by the executing
            // bytecode blob (per-context binding), with the cached row blob as
            // fallback. A miss defaults to the write lock (fail-safe).
            let Some(blob) = self
                .executing_bytecode_for_context(&context_id)
                .or_else(|| {
                    self.applications
                        .get(&application_id)
                        .map(|app| app.blob.bytecode)
                })
            else {
                break 'ro false;
            };
            self.modules
                .get(&(blob, service_name))
                .and_then(|cached| cached.read_only.as_ref())
                .is_some_and(|set| set.contains(method.as_str()))
        };

        let context = match self.get_or_fetch_context(&context_id) {
            Ok(Some(context)) => context,
            Ok(None) => return ActorResponse::reply(Err(ExecuteError::ContextNotFound)),
            Err(err) => {
                error!(%err, "failed to fetch context");

                return ActorResponse::reply(Err(ExecuteError::InternalError {
                    kind: InternalErrorKind::Context,
                }));
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
        // This gate only cares whether `InProgress` is set, not why: a cascade
        // descendant holds it for its whole propagator walk, same as the initiator.
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
                        return ActorResponse::reply(Err(ExecuteError::InternalError {
                            kind: InternalErrorKind::Group,
                        }));
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
                return ActorResponse::reply(Err(ExecuteError::InternalError {
                    kind: InternalErrorKind::Group,
                }));
            }
        }

        // Lazy upgrade: if this context's group has a pending upgrade and the
        // context is stale, trigger an upgrade before executing the method.
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

                return ActorResponse::reply(Err(ExecuteError::InternalError {
                    kind: InternalErrorKind::Context,
                }));
            }
            Err(err) => {
                error!(%err, "failed to load context config");

                return ActorResponse::reply(Err(ExecuteError::InternalError {
                    kind: InternalErrorKind::Context,
                }));
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
                error!(%err, "failed to load context identity");

                return ActorResponse::reply(Err(ExecuteError::InternalError {
                    kind: InternalErrorKind::Context,
                }));
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
        let (encryption_key, broadcast_key_id) =
            match calimero_governance_store::get_group_for_context(&self.datastore, &context_id) {
                Ok(Some(gid)) => {
                    // An error means the topology is unreadable (cyclic parent
                    // edges, missing namespace meta): refuse rather than guess a key.
                    let key_group_id = match calimero_governance_store::key_covering_group(
                        &self.datastore,
                        &gid,
                    ) {
                        Ok(key_group_id) => key_group_id,
                        Err(err) => {
                            error!(
                                group_id = ?gid,
                                ?context_id,
                                %err,
                                "state-delta encryption: key_covering_group failed",
                            );
                            return ActorResponse::reply(Err(ExecuteError::InternalError {
                                kind: InternalErrorKind::Encryption,
                            }));
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
                            return ActorResponse::reply(Err(ExecuteError::InternalError {
                                kind: InternalErrorKind::Encryption,
                            }));
                        }
                    }
                }
                Ok(None) => {
                    // Every context is created group-registered (create/join
                    // both guarantee a `ContextGroupRef`), so a context with no
                    // group here is an invariant violation — there is no
                    // per-identity encryption key to fall back to. Fail loud
                    // rather than mis-encrypt.
                    error!(
                        ?context_id,
                        "state-delta encryption: context is not registered to any group",
                    );
                    return ActorResponse::reply(Err(ExecuteError::InternalError {
                        kind: InternalErrorKind::Encryption,
                    }));
                }
                Err(err) => {
                    error!(
                        ?context_id,
                        %err,
                        "state-delta encryption: get_group_for_context failed",
                    );
                    return ActorResponse::reply(Err(ExecuteError::InternalError {
                        kind: InternalErrorKind::Encryption,
                    }));
                }
            };

        // Resolve the producing `bytecode_id` for the broadcast envelope. Runs
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
        let producing_bytecode_id: Option<[u8; 32]> =
            match resolve_producing_bytecode_id(&self.datastore, &context_id) {
                Ok(v) => v,
                Err(err) => {
                    warn!(
                        ?context_id,
                        %err,
                        "resolve_producing_bytecode_id failed, stamping None on broadcast"
                    );
                    None
                }
            };

        debug!(
            public_key = ?identity.public_key,
            public_key = %identity.public_key,
            "ContextManager: keys",
        );

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
                debug!(%context_id, %executor, "performing lazy upgrade before execution");
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
                    if crate::activation::activated_bytecode(&act.datastore, &context_id).is_none() {
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
                    target_bytecode_id,
                    coords,
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
                            ensure_blob_local(
                                &blob_node_client,
                                &cid,
                                target_app,
                                target_bytecode_id,
                                coords,
                            )
                            .await
                        }
                        .into_actor(act)
                        .then(move |blob_local, act, _ctx| {
                            // Carry `blob_local` forward: the migrate runs the
                            // TARGET bytecode only when the blob was actually
                            // local. If we fell back to the row's (possibly
                            // stale) bytecode, the activation marker must NOT be
                            // recorded below.
                            let module_fut = if blob_local {
                                act.get_module_for_blob(target_bytecode_id.into(), service_name)
                                    .boxed_local()
                            } else {
                                // Legacy groups (randomly-seeded bytecode_id that
                                // resolves to no blob) and failed fetches: the
                                // row's bytecode is the only available truth.
                                act.evict_application_caches(target_app);
                                act.get_row_module_for_context(cid, target_app, service_name)
                                    .map_ok(|(_blob, module), _act, _ctx| module)
                                    .boxed_local()
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
                                let scope_projections = Arc::clone(&act.scope_projections);
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
                                                scope_projections,
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
                                                        target_bytecode_id,
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
                            ensure_blob_local(
                                &blob_node_client,
                                &cid,
                                target_app,
                                target_bytecode_id,
                                coords,
                            )
                            .await
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
                                            target_bytecode_id,
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
            // `bytecode_id` points at, else the application row (non-group
            // contexts, legacy groups). Cost: a couple of bloom-filtered
            // point-gets, noise next to the wasm call they precede.
            // The blob is carried with the module so the ABI gates below check
            // the bytes that run, not a re-derivation the compile may have raced.
            let module_fut = match act.executing_bytecode_for_context(&context.id) {
                Some(blob) => act
                    .get_module_for_blob(blob, context.service_name.clone())
                    .map_ok(move |module, _act, _ctx| (blob, module))
                    .boxed_local(),
                None => act
                    .get_row_module_for_context(
                        context.id,
                        context.application_id,
                        context.service_name.clone(),
                    )
                    .boxed_local(),
            };
            module_fut.map_ok(move |(blob, module), _act, _ctx| (guard, context, module, blob))
        });

        let execution_count = self.metrics.as_ref().map(|m| m.execution_count.clone());
        let execution_duration = self.metrics.as_ref().map(|m| m.execution_duration.clone());

        // Cloned for the broadcast continuation, which is a sibling link in this
        // chain rather than nested inside the execute closure — so it needs its
        // own binding. Resolved here so the broadcast advertises exactly the
        // bundle the envelope signature was bound to, never a re-derivation.
        let broadcast_delegation = delegation.clone();
        // Likewise the trigger a TEE run's envelope was signed over.
        let broadcast_tee_trigger = tee_trigger.clone();

        let execute_task =
            module_task.and_then(move |(guard, mut context, module, executing_blob), act, _ctx| {
            let datastore = act.datastore.clone();
            let node_client = act.node_client.clone();
            let context_client = act.context_client.clone();
            let scope_projections = std::sync::Arc::clone(&act.scope_projections);
            let search = act.search.clone();
            let ack_router = std::sync::Arc::clone(&act.ack_router);

            // The calling context's application id, resolved once (xcall path
            // only), so a `from_same_app` entry point can compare it to ours. A
            // caller that can't be resolved is treated as a mismatch — fail
            // closed rather than admitting an unknown source.
            let xcall_source_app = xcall_origin.and_then(|origin| {
                context_client
                    .get_context(&origin)
                    .ok()
                    .flatten()
                    .map(|ctx| ctx.application_id)
            });
            // The entry `module_task` just loaded; every gate below fails closed
            // without it.
            let abi = act
                .modules
                .get(&(executing_blob, context.service_name.clone()));

            // Keyed by the blob just loaded, so the policy is the running module's;
            // internal `__calimero_*` methods are never `#[app::xcall]`, so never reachable.
            let xcall_denied = xcall_origin.is_some()
                && xcall_caller_denied(
                    abi.map(|abi| abi.xcall.as_ref()),
                    method.as_str(),
                    xcall_source_app,
                    context.application_id,
                );

            // A peer's delta names the handlers its events run, so an event (or
            // a TEE trigger it fired) runs only a method the ABI declares one.
            let fired_by_event = event_handler
                || matches!(
                    tee_trigger,
                    Some(calimero_context_client::tee_trigger::TeeTriggerCause::Event { .. })
                );
            let handler_refused = fired_by_event
                && (method.starts_with(SDK_EXPORT_PREFIX)
                    || !abi.is_some_and(|abi| abi.handlers.contains(method.as_str())));
            // A blob older than the group's target may lack a handler its newer version declares.
            let handler_awaits_upgrade = handler_refused
                && runs_behind_group_target(&act.datastore, &context.id, &executing_blob);

            // The authorization gate for a delegated read, resolved HERE rather
            // than from the `is_read_only_call` computed for lock selection.
            //
            // Those two look like the same question and are not. Lock selection
            // may answer conservatively: a cold cache yields `false`, it takes
            // the write lock, and the only cost is contention. As an
            // authorization gate that same `false` would refuse a perfectly
            // read-only method whenever its module had not been loaded yet —
            // intermittent 409s that depend on cache warmth. By this point the
            // module has loaded and its entry holds the read-only set for this
            // blob, so the set is the real declared one.
            //
            // `None` here means the module carries no ABI at all; that refuses
            // the read, which is the fail-closed direction.
            let read_refusal = read_as.and_then(|_account| {
                let declared_read_only = abi
                    .and_then(|abi| abi.read_only.as_ref())
                    .is_some_and(|set| set.contains(method.as_str()));

                // The set holds only `ReadOnly` names, so absence covers both
                // `Mutating` and `Unspecified`. They are not distinguishable
                // from the set alone, and a client acts identically on both
                // (mint a warrant), so they share one refusal.
                (!declared_read_only).then_some(ExecuteError::NotReadOnly {
                    context_id: context.id,
                })
            });

            // Whether the run executes as a view, decided like the delegated
            // read gate above: from the module's declared read-only set, now
            // that the module is loaded, not from the lock selection alone. A
            // cold cache (the first call after a restart) makes lock selection
            // take the write lock; a view still runs on the read-only storage
            // and gets the search handle, as it would warm. It never goes the
            // other way: a write is never made read-only.
            let run_read_only = is_read_only_call
                || (!is_state_op
                    && abi
                        .and_then(|abi| abi.read_only.as_ref())
                        .is_some_and(|set| set.contains(method.as_str())));

            // Cheap (Arc-backed) clone kept past internal_execute (which moves
            // `datastore`) so a post-call migrate_my_entries can refresh the
            // node-local authored_remaining count (6f.8 drop-after-convert).
            let count_datastore = datastore.clone();

            async move {
                // The node logs the refusal, naming the method it dispatched.
                if handler_refused {
                    let application_id = context.application_id;
                    bail!(if handler_awaits_upgrade {
                        ExecuteError::EventHandlerAwaitsUpgrade {
                            context_id,
                            application_id,
                        }
                    } else {
                        ExecuteError::NotAnEventHandler {
                            context_id,
                            application_id,
                        }
                    });
                }

                if xcall_denied {
                    warn!(
                        %context_id,
                        function = %method,
                        "xcall denied: not an #[app::xcall] entry point, or caller not permitted by its policy"
                    );
                    bail!(ExecuteError::XCallNotPermitted { context_id });
                }

                // Refused before any execution: a session authorizes reads, and
                // this method is not one.
                if let Some(refusal) = read_refusal {
                    warn!(
                        %context_id,
                        function = %method,
                        "delegated read refused: method is not declared read-only"
                    );
                    bail!(refusal);
                }

                let old_root_hash = context.root_hash;

                let start = Instant::now();

                let executed = internal_execute(
                        datastore,
                        &scope_projections,
                        &node_client,
                        &context_client,
                        module,
                        &guard,
                        &mut context,
                        executor,
                        method.clone().into(),
                        payload.into(),
                        is_state_op,
                        write_source,
                        run_read_only,
                        block_writes_for_group,
                        &private_key,
                        xcall_origin,
                        delegation.as_deref(),
                        read_as,
                        tee_trigger.as_ref(),
                        search,
                        governance_position.as_ref(),
                        &ack_router,
                    )
                    .await;

                let duration = start.elapsed().as_secs_f64();
                // `failure`: the method ran and returned an error. `error`: the
                // node could not run it to completion (storage, module, signing,
                // admission). Recorded before the `?` below, which used to skip
                // both execution metrics on exactly the runs worth counting.
                let status = match &executed {
                    Ok((outcome, ..)) if outcome.returns.is_ok() => "success",
                    Ok(_) => "failure",
                    Err(_) => "error",
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

                let (
                    outcome,
                    causal_delta,
                    delta_signature,
                    signing_governance_position,
                    read_only_write_discarded,
                ) = executed?;

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
                    read_only_write_discarded,
                ))
            }
            .into_actor(act)
        });

        let external_task =
            execute_task.and_then(move |(guard, context, outcome, causal_delta, delta_signature, signing_governance_position, read_only_write_discarded), act, _ctx| {
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

                // `datastore_for_broadcast` used to recompute the
                // governance position at broadcast time — that recompute
                // produced the persist-vs-broadcast signature mismatch
                // documented on `governance_position_for_broadcast`.
                // The threaded value from `internal_execute` is the
                // single source of truth now, so no fresh store
                // snapshot is needed here.

                async move {
                    if outcome.returns.is_err() {
                        return Ok((guard, context.root_hash, outcome, read_only_write_discarded));
                    }

                    debug!(
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

                    // Process cross-context calls.
                    //
                    // Each xcall runs the target's method by sending a fresh
                    // ExecuteRequest back to THIS ContextManager actor via
                    // `execute_with_origin`. Awaiting that inline — while this
                    // future is still the actor's in-flight response — is the
                    // same self-mailbox re-entrancy the lazy-upgrade path
                    // deliberately avoids: it hangs the actor, and for a
                    // self-targeted xcall it also deadlocks on the source
                    // context's execution lock (still held here via `guard`).
                    // So we snapshot each call's fields and dispatch the whole
                    // batch out-of-band on a detached task. The source execute
                    // returns and drops its guard without waiting; the detached
                    // task then re-enters the actor cleanly. xcalls are already
                    // best-effort (their only product is an observability event),
                    // so fire-and-forget matches their contract.
                    if !outcome.xcalls.is_empty() {
                        // `XCall` is neither `Clone` nor constructible outside
                        // its crate (`#[non_exhaustive]`), so lift the owned
                        // fields into a plain tuple the task can take by value.
                        let xcall_jobs: Vec<(ContextId, String, Vec<u8>)> = outcome
                            .xcalls
                            .iter()
                            .map(|xcall| {
                                (
                                    ContextId::from(xcall.context_id),
                                    xcall.function.clone(),
                                    xcall.params.clone(),
                                )
                            })
                            .collect();
                        let xcall_node_client = node_client.clone();
                        let xcall_context_client = context_client.clone();
                        let xcall_store = xcall_datastore.clone();
                        let xcall_task = global_runtime().spawn(async move {
                            use futures_util::TryStreamExt;
                            for (target_context_id, function, params) in xcall_jobs {
                                info!(
                                    %context_id,
                                    target_context = ?target_context_id,
                                    function = %function,
                                    params_len = params.len(),
                                    "Processing cross-context call"
                                );

                                // Best-effort observability event for this xcall.
                                // Source rides on the wrapper `context_id`;
                                // emission failure must never abort the batch.
                                let emit = |outcome: XCallOutcome| {
                                    let _ = xcall_node_client.send_event(NodeEvent::Context(
                                        ContextEvent {
                                            context_id,
                                            payload: ContextEventPayload::XCall(XCallPayload {
                                                target_context_id,
                                                function: function.clone(),
                                                outcome,
                                            }),
                                        },
                                    ));
                                };

                                // Depth cap: an execution already at the maximum
                                // cascade depth may not spawn further xcalls. This
                                // is what bounds the otherwise-unbounded local
                                // recursion — a cycle A→B→A would never terminate,
                                // and each level multiplies the fan-out by up to
                                // `max_xcalls`. `xcall_depth` is this execution's
                                // depth; a child would run one deeper.
                                if xcall_depth >= MAX_XCALL_DEPTH {
                                    warn!(
                                        %context_id,
                                        target_context = ?target_context_id,
                                        function = %function,
                                        xcall_depth,
                                        max = MAX_XCALL_DEPTH,
                                        "xcall denied: depth limit reached"
                                    );
                                    emit(XCallOutcome::Denied {
                                        reason: "xcall depth limit".to_owned(),
                                    });
                                    continue;
                                }

                                // A context may only xcall a target in its OWN
                                // owning group. Gating on the shared namespace
                                // root instead let any sibling context anywhere
                                // in the namespace — including a different, even
                                // Restricted, subgroup — call in and execute as a
                                // member of the target. A resolution error or an
                                // unregistered context denies.
                                let same_group = xcall_same_owning_group(
                                    &xcall_store,
                                    &context_id,
                                    &target_context_id,
                                );
                                if !matches!(same_group, Ok(true)) {
                                    warn!(
                                        %context_id,
                                        target_context = ?target_context_id,
                                        function = %function,
                                        resolved = ?same_group,
                                        "xcall denied: owning group boundary"
                                    );
                                    emit(XCallOutcome::Denied {
                                        reason: "owning group boundary".to_owned(),
                                    });
                                    continue;
                                }

                                // Find an owned member of the target context to
                                // execute as — one that has permissions there.
                                let members: Vec<_> = xcall_context_client
                                    .get_context_members(&target_context_id, Some(true))
                                    .try_collect()
                                    .await
                                    .unwrap_or_default();

                                let Some((target_executor, _is_owned)) = members.first() else {
                                    warn!(
                                        %context_id,
                                        target_context = ?target_context_id,
                                        function = %function,
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

                                // Execute as the target's member, tagging the
                                // call with the source context so the target can
                                // read it via `env::xcall_origin()`. The node sets
                                // the origin here — never from guest memory. The
                                // target's handler rejects non-`#[app::xcall]`
                                // methods with XCallNotPermitted.
                                let xcall_result = xcall_context_client
                                    .execute_with_origin(
                                        &target_context_id,
                                        &target_executor,
                                        function.clone(),
                                        params.clone(),
                                        None,
                                        Some(context_id),
                                        // Child runs one level deeper. The depth
                                        // check above already caps this within
                                        // `MAX_XCALL_DEPTH`; `saturating_add`
                                        // keeps the arithmetic sound regardless.
                                        xcall_depth.saturating_add(1),
                                        // An xcall is this node acting for
                                        // itself in the target context, not for
                                        // the original author: the warrant
                                        // named one context, and honouring it
                                        // across a hop would let one signed
                                        // intent author in a context the member
                                        // never authorized.
                                        None,
                                    )
                                    .await;

                                match xcall_result {
                                    // A normally-finished run returns `Ok` even
                                    // when the target *method* errored — that
                                    // failure lives in `returns`, so inspect it
                                    // before reporting Ok.
                                    Ok(response) => match &response.returns {
                                        Ok(_) => {
                                            info!(
                                                %context_id,
                                                target_context = ?target_context_id,
                                                function = %function,
                                                "Cross-context call executed successfully"
                                            );
                                            emit(XCallOutcome::Ok);
                                        }
                                        Err(err) => {
                                            warn!(
                                                %context_id,
                                                target_context = ?target_context_id,
                                                function = %function,
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
                                            function = %function,
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
                                            function = %function,
                                            ?err,
                                            "Cross-context call failed"
                                        );
                                        emit(XCallOutcome::ExecError {
                                            message: err.to_string(),
                                        });
                                    }
                                }
                            }
                        });
                        // Supervise the detached batch so a panic inside it is
                        // logged rather than silently swallowed (a dropped
                        // JoinHandle discards the panic). The source execute
                        // still does not wait on the xcalls — this supervisor
                        // task is what keeps it fire-and-forget yet observable.
                        drop(global_runtime().spawn(async move {
                            if let Err(err) = xcall_task.await {
                                error!(
                                    %context_id,
                                    %err,
                                    "cross-context call dispatch task terminated abnormally (panic or cancellation)"
                                );
                            }
                        }));
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
                        debug!(
                            %context_id,
                            %executor,
                            is_state_op,
                            artifact_empty = outcome.artifact.is_empty(),
                            events_count = outcome.events.len(),
                            has_delta = causal_delta.is_some(),
                            "Broadcasting state delta and events to other nodes"
                        );

                        if let Some(ref the_delta) = causal_delta {
                            // The same bytes the delta id committed to.
                            let events_data = events_payload(&outcome.events);
                            debug_assert_eq!(
                                events_data.as_deref().map(CausalDelta::hash_events),
                                the_delta.events_hash,
                            );

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

                            // The AUTHOR on the wire, which is the executor's
                            // own key self-authored and the member's under a
                            // warrant — matching the row and the signed
                            // preimage. Diverging here would have peers verify
                            // one author and store another.
                            let broadcast_author = broadcast_delegation
                                .as_ref()
                                .map_or(executor, |d| d.warrant.author_device_key);

                            node_client
                                .broadcast(
                                    &context,
                                    &broadcast_author,
                                    &encryption_key,
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
                                    producing_bytecode_id,
                                    // Matches the row persisted above. Both are
                                    // `Some` together or a peer would verify one
                                    // shape and store the other.
                                    broadcast_delegation.as_deref().cloned(),
                                    // What the envelope was signed over.
                                    // `internal_execute` refuses a trigger it
                                    // would not sign under `SignatureDomain::Tee`, so a
                                    // delta exists here only if it signed one.
                                    broadcast_tee_trigger,
                                )
                                .await?;
                        }
                    }

                    // Handler execution is deferred to receiver nodes only.
                    // See state_delta/mod.rs execute_event_handlers_parsed().

                    Ok((guard, context.root_hash, outcome, read_only_write_discarded))
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
                    // The context went away under a lazy upgrade. That is a
                    // missing context to the caller, not an internal fault.
                    if let Some(ContextError::ContextDeleted { .. }) = err.downcast_ref() {
                        return ExecuteError::ContextNotFound;
                    }
                    debug!(?err, "an error occurred while executing request");
                    ExecuteError::InternalError {
                        kind: InternalErrorKind::Runtime,
                    }
                })
            })
            .map_ok(
                move |(guard, root_hash, outcome, read_only_write_discarded), _act, _ctx| {
                    ExecuteResponse {
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
                        read_only_write_discarded,
                    }
                },
            );

        ActorResponse::r#async(task)
    }
}

impl ContextManager {
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
        let walk = get_group_for_context(&datastore, &context_id)
            .ok()
            .flatten()
            .and_then(|gid| {
                let meta = MetaRepository::new(&datastore).load(&gid).ok().flatten()?;
                let bound = crate::activation::activated_bytecode(&datastore, &context_id)?;
                let ladder = UpgradeLadderRepository::new(&datastore)
                    .load(&gid)
                    .unwrap_or_default();
                Some((ladder, bound, meta.target.bytecode_id))
            });

        let Some((ladder, bound, group_target)) = walk else {
            return async move { Ok(guard) }.into_actor(self).boxed_local();
        };
        let Some(rung) = crate::activation::next_rung(&ladder, bound, group_target) else {
            // Reaching the target is every execute's steady state; a missing hop
            // while behind means the append-rung-before-target invariant broke.
            if bound != group_target {
                debug!(
                    %context_id,
                    bound = %hex::encode(bound),
                    group_target = %hex::encode(group_target),
                    "no ladder hop to replay"
                );
            }
            return async move { Ok(guard) }.into_actor(self).boxed_local();
        };
        if budget == 0 {
            warn!(%context_id, "ladder hop budget exhausted; resuming on next access");
            return async move { Ok(guard) }.into_actor(self).boxed_local();
        }

        let rung_bytecode_id = rung.bytecode_id;
        let rung_application_id = rung.application_id;
        let rung_coords = Some(RegistryCoordsBuf::new(rung.package, rung.version));

        info!(
            %context_id,
            from = %hex::encode(bound),
            to = %hex::encode(rung_bytecode_id),
            "replaying upgrade ladder hop"
        );

        let node_client = self.node_client.clone();
        async move {
            // Binding a marker to an absent blob would wedge the context, so
            // the blob must be local (fetched from peers if needed) before
            // anything else.
            if !ensure_blob_local(
                &node_client,
                &context_id,
                rung_application_id,
                rung_bytecode_id,
                rung_coords,
            )
            .await
            {
                eyre::bail!("rung bytecode blob not available locally or from peers");
            }
            // Replay actuates an already-committed, already-gated governance
            // decision. A rung blob with no embedded ABI at replay time proves
            // the initiator forced code-only (else the emit-side would have
            // refused under the default), so honor that here (force_code_only =
            // true) rather than wedge this lazy member. force only relaxes the
            // absent-evidence arm: a rung whose ABI declares a migration still
            // resolves and runs it.
            let migration = crate::handlers::upgrade_group::resolve_upgrade_from_abis(
                &node_client,
                bound,
                rung_bytecode_id,
                true,
            )
            .await?;
            // The rung's declared state version, recorded with the activation
            // marker below: the upgrade record that carries `to_state_version`
            // never leaves the node that ran `upgrade_group`, so this is the
            // only version signal a member has for state it has migrated.
            let state_version = crate::handlers::upgrade_group::blob_max_state_version(
                &node_client,
                rung_bytecode_id,
            )
            .await;
            Ok::<_, eyre::Report>((migration, state_version))
        }
        .into_actor(self)
        .then(move |resolved, act, _ctx| {
            let (migration, activated_state_version) = match resolved {
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
                let scope_projections = Arc::clone(&act.scope_projections);
                act.get_module_for_blob(rung_bytecode_id.into(), service_name)
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
                                scope_projections,
                            )
                            .await?;
                            crate::activation::record_activation(
                                &datastore,
                                &context_id,
                                rung_bytecode_id,
                            );
                            if let Some(state_version) = activated_state_version {
                                crate::activation::record_activated_state_version(
                                    &datastore,
                                    &context_id,
                                    state_version,
                                );
                            }
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
                    crate::activation::record_activation(&datastore, &context_id, rung_bytecode_id);
                    if let Some(state_version) = activated_state_version {
                        crate::activation::record_activated_state_version(
                            &datastore,
                            &context_id,
                            state_version,
                        );
                    }
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

    /// Load the module of `application_id`'s row for `context_id`, refused before
    /// compiling when the context's group never named its blob: every group shares the row.
    pub(crate) fn get_row_module_for_context(
        &self,
        context_id: ContextId,
        application_id: ApplicationId,
        service_name: Option<String>,
    ) -> impl ActorFuture<
        Self,
        Output = eyre::Result<(calimero_primitives::blobs::BlobId, calimero_runtime::Module)>,
    > + 'static {
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
                let blob = act
                    .applications
                    .get(&application_id)
                    .expect("application just inserted or already cached")
                    .blob
                    .bytecode;
                if !crate::activation::context_group_registers_bytecode(
                    &act.datastore,
                    &context_id,
                    *blob.digest(),
                ) {
                    warn!(
                        %context_id,
                        %application_id,
                        %blob,
                        "refusing an application release the context's group never named"
                    );
                    bail!(ExecuteError::ApplicationNotInstalled { application_id });
                }
                Ok(blob)
            })
            .and_then(move |blob, act, _ctx| {
                act.get_module_for_blob(blob, service_name)
                    .map_ok(move |module, _act, _ctx| (blob, module))
            })
    }

    /// Load (compile + cache) the module for a content-addressed bytecode
    /// blob — THE module-loading path: contexts execute the blob their
    /// activation marker / group `bytecode_id` points at, independent of what
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

        async {}
            .into_actor(self)
            .then(move |(), act, _ctx| {
                if let Some(cached) = act.modules.get(&cache_key) {
                    return actix::fut::ready(Ok(cached.module.clone()))
                        .into_actor(act)
                        .boxed_local();
                }
                // Join a compile already running for this module, or start one.
                let compile = act
                    .compiling
                    .entry(cache_key.clone())
                    .or_insert_with(|| {
                        compile_module(
                            act.node_client.clone(),
                            act.vm_limits,
                            blob_id,
                            service_name,
                        )
                    })
                    .clone();
                compile
                    .into_actor(act)
                    .map(move |compiled, act, _ctx| {
                        let _ = act.compiling.remove(&cache_key);
                        let compiled = compiled.map_err(|err| eyre::eyre!("{err:?}"))?;
                        let module = compiled.module.clone();
                        let _ = act.modules.insert(cache_key, compiled);
                        Ok(module)
                    })
                    .boxed_local()
            })
            .map_err(|err, _act, _ctx| {
                error!(?err, "failed to initialize module for execution");

                err
            })
    }
}

/// A compiled module with the method sets read from its ABI, cached as one
/// entry so the sets are inserted and evicted with the module they gate.
#[derive(Clone, Debug)]
pub(crate) struct CompiledModule {
    module: calimero_runtime::Module,
    /// `#[app::view]` methods; `None` without an ABI (every call takes the write lock).
    read_only: Option<Arc<HashSet<String>>>,
    /// `#[app::xcall]` entry points and their callers; empty denies every xcall.
    xcall: Arc<crate::XCallPolicyMap>,
    /// `#[app::handler]` methods; empty without an ABI, so no event runs anything.
    handlers: Arc<HashSet<String>>,
}

/// A module compile every request that needs the module can wait on.
pub(crate) type SharedCompile = futures_util::future::Shared<
    futures_util::future::BoxFuture<'static, Result<CompiledModule, Arc<eyre::Report>>>,
>;

/// Counts compiles started per blob, so a test can tell a shared compile from
/// two.
#[cfg(test)]
pub(crate) mod tests_support {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use calimero_primitives::blobs::BlobId;

    static COMPILES: Mutex<Option<HashMap<BlobId, usize>>> = Mutex::new(None);

    pub(crate) fn count_compile(blob_id: BlobId) {
        let mut compiles = COMPILES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *compiles
            .get_or_insert_with(HashMap::new)
            .entry(blob_id)
            .or_default() += 1;
    }

    pub(crate) fn compiles(blob_id: &BlobId) -> usize {
        let compiles = COMPILES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        compiles
            .as_ref()
            .and_then(|c| c.get(blob_id).copied())
            .unwrap_or(0)
    }
}

/// Read a module's bytecode and compile it on the blocking pool.
#[expect(
    clippy::result_large_err,
    reason = "The runtime's compile error is large; boxing it would change the \
              runtime's public signature, so it is left for a follow-up."
)]
fn compile_module(
    node_client: NodeClient,
    vm_limits: calimero_runtime::logic::VMLimits,
    blob_id: calimero_primitives::blobs::BlobId,
    service_name: Option<String>,
) -> SharedCompile {
    use futures_util::FutureExt;

    #[cfg(test)]
    tests_support::count_compile(blob_id);

    async move {
        let Some(bytecode) = node_client
            .application_bytes_from_blob(&blob_id, service_name.as_deref())
            .await?
        else {
            bail!("bytecode blob {} not found in blobstore", blob_id);
        };
        // Extract the read-only and xcall method sets from the ABI before the
        // bytes move into the compile task. A missing manifest is fine:
        // read-only defaults to the write lock, and no xcall reaches the module.
        let read_only_set = extract_read_only_set(&bytecode);
        let xcall_policies = extract_xcall_policies(&bytecode);
        let handlers = extract_handler_set(&bytecode);
        let module = global_runtime()
            .spawn_blocking(move || {
                calimero_runtime::Engine::with_limits(vm_limits).compile(&bytecode)
            })
            .await
            .wrap_err("WASM compilation task failed")??;
        Ok(CompiledModule {
            module,
            read_only: read_only_set,
            xcall: xcall_policies,
            handlers,
        })
    }
    .map_err(Arc::new)
    .boxed()
    .shared()
}

/// The upgrade target, from this node's one configured source. `false` ⇒ that
/// source had nothing yet; the caller retries next access.
async fn ensure_blob_local(
    node_client: &NodeClient,
    context_id: &ContextId,
    application_id: ApplicationId,
    bytecode_id: [u8; 32],
    coords: Option<RegistryCoordsBuf>,
) -> bool {
    let bytecode_id = calimero_primitives::blobs::BlobId::from(bytecode_id);
    // A target with no recorded coordinates has none to send. The registry
    // route refuses the unaddressable request; the peer route ignores them.
    let (package, version) = coords
        .as_ref()
        .map_or(("", ""), |coords| (&*coords.package, &*coords.version));
    let outcome = node_client
        .acquire_bytecode(&AppRequest {
            bytecode_id: Some(bytecode_id),
            application_id: Some(application_id),
            package,
            version,
            context_id: Some(context_id),
        })
        .await;

    if outcome == AcquireOutcome::Unavailable {
        warn!(%context_id, %bytecode_id, "lazy upgrade: target bytecode unavailable from the configured source");
        return false;
    }
    true
}

/// Store-level executing-blob resolution for a context: its activation
/// marker (the blob it last activated), else its owning group's recorded
/// target blob. The `bool` is `true` when the blob came from the group
/// `bytecode_id` (callers gate that branch on local blob presence — legacy
/// groups carry randomly-seeded keys that resolve to nothing). `None` ⇒
/// fall back to the application row.
/// Where a context's bound bytecode blob was resolved from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundBytecodeSource {
    /// The per-context activation marker — already executed locally.
    ActivationMarker,
    /// The group's recorded target blob — may not be fetched yet.
    GroupKey,
}

pub(crate) fn bound_bytecode_for_context(
    store: &Store,
    context_id: &ContextId,
) -> Option<([u8; 32], BoundBytecodeSource)> {
    if let Some(blob) = crate::activation::activated_bytecode(store, context_id) {
        return Some((blob, BoundBytecodeSource::ActivationMarker));
    }
    let group_id = calimero_governance_store::get_group_for_context(store, context_id)
        .ok()
        .flatten()?;
    let meta = MetaRepository::new(store).load(&group_id).ok().flatten()?;
    (meta.target.bytecode_id != [0u8; 32])
        .then_some((meta.target.bytecode_id, BoundBytecodeSource::GroupKey))
}

/// Whether the group has moved `context_id` to a blob other than `executing`.
fn runs_behind_group_target(
    store: &Store,
    context_id: &ContextId,
    executing: &calimero_primitives::blobs::BlobId,
) -> bool {
    calimero_governance_store::get_group_for_context(store, context_id)
        .ok()
        .flatten()
        .and_then(|group_id| MetaRepository::new(store).load(&group_id).ok().flatten())
        .is_some_and(|meta| {
            meta.target.bytecode_id != [0u8; 32] && meta.target.bytecode_id != *executing.digest()
        })
}

impl ContextManager {
    /// The bytecode blob this context executes (per-context binding):
    /// activation marker → group target blob (when locally present) →
    /// `None` (callers fall back to the application row).
    pub(crate) fn executing_bytecode_for_context(
        &self,
        context_id: &ContextId,
    ) -> Option<calimero_primitives::blobs::BlobId> {
        let (blob, source) = bound_bytecode_for_context(&self.datastore, context_id)?;
        let blob_id = calimero_primitives::blobs::BlobId::from(blob);
        if source == BoundBytecodeSource::GroupKey
            && !self.node_client.has_blob(&blob_id).unwrap_or(false)
        {
            // Legacy randomly-seeded bytecode_id (or not-yet-fetched target):
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
    // Read for the TEE authority checks at this node's heads, and for cells' writers.
    scope_projections: &Arc<std::sync::RwLock<crate::scope_projection::ScopeProjections>>,
    node_client: &NodeClient,
    context_client: &ContextClient,
    module: calimero_runtime::Module,
    guard: &ContextGuard,
    context: &mut Context,
    executor: PublicKey,
    method: Cow<'static, str>,
    input: Cow<'static, [u8]>,
    is_state_op: bool,
    // Who authored what this run commits: this node, or a peer whose delta it
    // merge-applies. Decides the B3 gate below.
    write_source: WriteSource,
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
    // The author's consent, when this run is on someone else's behalf. Its
    // presence is what makes the principal differ from the signer.
    delegation: Option<&calimero_account::Delegation>,
    // The authenticated caller's account, when this run is a delegated READ.
    // Mutually exclusive with `delegation` by construction: a read carries no
    // warrant and a warranted write sets no `read_as`.
    read_as: Option<calimero_account::AccountId>,
    // What fired this run, when the node's TEE scheduler did. Only honoured on a
    // node whose key is an attested TEE authority for the context; see
    // `tee_authority` below. The delta is signed over it.
    tee_trigger: Option<&calimero_node_primitives::sync::delta_auth::TeeTriggerCause>,
    // Full-text search, when the node runs it. Only an app that declares a
    // search index (exports the extract method) takes part.
    search: Option<std::sync::Arc<calimero_search::SearchService>>,
    // The cut a peer's delta was signed at, where a state op reads writers; else the heads.
    governance_position: Option<&GovernanceParentEdge>,
    // Where the run's writer-set rotations are published.
    ack_router: &AckRouter,
) -> eyre::Result<(
    Outcome,
    Option<CausalDelta>,
    Option<[u8; 64]>,
    Option<GovernanceParentEdge>,
    // Whether the run's writes were discarded because this node is read-only in
    // the context; see `ExecuteResponse::read_only_write_discarded`.
    bool,
)> {
    // A TEE trigger runs as the TEE authority, and only on a node that is one.
    //
    // Checked here, under the execution lock and against this node's governance
    // state, rather than trusted from the request: `tee_trigger` names what the
    // caller wants, and this decides whether the node may give it. Peers repeat
    // the same check before they accept the delta, so a node that lies here
    // gains nothing but a delta nobody applies.
    //
    // The three other principal-changing inputs are unreachable from the only
    // caller that sets `tee_trigger`, and are refused rather than composed: a
    // TEE write on a member's behalf, or into another context, is not a thing
    // this prototype defines.
    let tee_authority = if tee_trigger.is_some() {
        if is_state_op || delegation.is_some() || read_as.is_some() || xcall_origin.is_some() {
            bail!(ExecuteError::Unauthorized {
                context_id: context.id,
                public_key: executor,
            });
        }
        if !calimero_governance_store::is_tee_authority_for_context(
            &datastore,
            &crate::scope_projection::FoldedProjections(scope_projections),
            &context.id,
            &executor,
        )? {
            warn!(
                context_id = %context.id,
                %executor,
                "TEE trigger refused: this node is not an attested TEE authority for the context"
            );
            bail!(ExecuteError::Unauthorized {
                context_id: context.id,
                public_key: executor,
            });
        }
        true
    } else {
        false
    };

    // A TEE authority is a TEE member (`ReadOnlyTee` or `RelayTee`), and that
    // role stays read-only for everything EXCEPT a TEE-triggered run: an
    // ordinary JSON-RPC call on a TEE node still has its writes discarded here.
    //
    // The role is the node's effective one in the context's group, so one held
    // at the namespace root and inherited into an Open subgroup counts exactly
    // as a row in the subgroup would; peers refuse that node's deltas on the
    // same rule. The discard is reported (`read_only_write_discarded`) rather
    // than raised: the node's own event handlers come through here too, and on
    // a replica their writes are dropped by design. The RPC layer refuses.
    //
    // Never for a DELEGATED run. Its writes are the author's, not the
    // executor's, and whether this node may carry them is the warrant gate's
    // question, asked below before anything runs: a TEE relay may, a TEE
    // replica may not, and a read-only author may not through anyone. Letting
    // this discard fire on a delegated run is what made a replica's relayed
    // write vanish behind a `200`.
    let executor_is_read_only = !is_state_op
        && !tee_authority
        && delegation.is_none()
        && NamespaceRepository::new(&datastore)
            .is_read_only_for_context(&context.id, &executor)
            .unwrap_or(false);

    // B3 user-storage extension (#2382): a state op this node may not commit
    // is dropped after WASM execution, mirroring the ReadOnly handling above.
    // The receive-path cross-DAG check (`membership_status_at`) rejects deltas
    // from non-members on peers; this is the local companion, so a node that
    // is not (or no longer) a member does not accumulate state its peers will
    // never accept.
    //
    // WHOSE right to write is checked depends on who authored the state
    // (`write_source`), and conflating the two was a bug: every inbound delta
    // is merge-applied as a `__calimero_sync_next` run whose executor is THIS
    // node, so asking "may the executor author state?" discarded every delta a
    // `ReadOnly` / `ReadOnlyTee` replica was sent. Its DAG still recorded the
    // delta as applied, so it served stale state until the heartbeat noticed
    // the divergence and a snapshot repaired it.
    //
    // * `RemoteDelta` — the author is a peer, and the receive path has already
    //   verified the envelope signature and authorized that author at the
    //   delta's cut (and refused read-only authors). This node only has to
    //   replicate the context: any role, read-only included. A node holding no
    //   role at all still discards, as before — it is no replica.
    // * `Local` — this node is the author (here: a `__calimero_sync_next`
    //   reached by naming the method, e.g. over JSON-RPC), so it must itself be
    //   Admin/Member. Unchanged. A read-only node's ordinary mutating calls are
    //   discarded by the `executor_is_read_only` check above, and a removed
    //   member's node is refused before execution because the removal deleted
    //   its `ContextIdentity` membership marker.
    //
    // Read-only calls (`is_state_op == false`) are unaffected by this gate.
    //
    // Live-state check vs. the receive path's forward-only check: at execute
    // time there is no signed governance cut to evaluate membership against,
    // so this consults current membership.
    //
    // Fail-closed on store error, for both lookups: an authorization gate that
    // fails open on transient store errors grants permission exactly when the
    // check is most needed (storage degradation). A non-group context returns
    // `Ok(true)` from the authorship helper, so this only affects genuine
    // errors — and there the safer answer is "drop the state op." Asymmetry
    // with the `is_read_only_for_context` call above (`.unwrap_or(false)`
    // there reads as fail-open because `false` means "not read-only" → allow)
    // is deliberate: that check is a defense-in-depth post-discard, while this
    // is a primary authorization gate. Here `is_read_only_for_context` can only
    // ADD a permission, so its `unwrap_or(false)` fails closed.
    let executor_not_authorized_for_state_op = is_state_op && {
        let namespaces = NamespaceRepository::new(&datastore);
        let may_author = namespaces
            .is_authorized_for_context_state_op(&context.id, &executor)
            .unwrap_or(false);
        let may_commit = match write_source {
            WriteSource::Local => may_author,
            WriteSource::RemoteDelta => {
                may_author
                    || namespaces
                        .is_read_only_for_context(&context.id, &executor)
                        .unwrap_or(false)
            }
        };
        !may_commit
    };

    // The principal this run observes and is attributed to.
    //
    // Self-authored: resolved here rather than passed down from the RPC layer,
    // because the account is a fact about this node in this context's namespace
    // and not something a caller may assert. `account_for_context` always yields
    // a real account — see its docs for why there is no un-enrolled fallback.
    //
    // Delegated: read off the WARRANT, which is still not a caller assertion —
    // the bundle verified before reaching here, so the account is one the
    // author's own root certified and the author's own device signed for. That
    // is the whole reason the principal can differ from the signer without a
    // caller being able to choose it.
    let principal = match delegation {
        // A TEE-triggered run is attributed to the TEE authority, not to this
        // node's own account: that is the account `TeeOnly` writer sets name, and
        // the one peers resolve this node's signing key to once they have checked
        // the same authority. The device stays this node's key, which signs.
        None if tee_authority => {
            Principal::new(calimero_account::AccountId::TEE_AUTHORITY, executor)
        }
        // A delegated READ: the account comes from the authenticated session,
        // the device from this node. Those halves may differ here, where they
        // may not for a write, because the rule they would break —
        // `user_leaf_author_is_its_owner` on the receive path — is about a leaf,
        // and a read writes none. Nothing this run produces is persisted,
        // signed, or gossiped.
        //
        // Membership is re-checked HERE, on every call, rather than trusted from
        // the session. A relay serves several tenants, so a session that carried
        // a standing right to read would keep serving a member after they were
        // removed from the group — the removal is a governance op this node has
        // already applied, and the only way it reaches the decision is by asking
        // at the moment of the read.
        None if read_as.is_some() => {
            // SAFETY: guarded by `read_as.is_some()` in the arm's condition.
            let account = read_as.expect("read_as is Some in this arm");

            if !calimero_governance_store::account_is_context_member(
                &datastore,
                &context.id,
                &account,
            )? {
                bail!(ExecuteError::NotAMember {
                    context_id: context.id
                });
            }

            Principal::new(account, executor)
        }
        None => Principal::new(
            calimero_governance_store::account_for_context(&datastore, &context.id)?,
            executor,
        ),
        Some(d) => {
            // The same gate the receive path runs, on the authoring side, and it
            // has to be HERE rather than at the persist below: `storage.commit()`
            // makes the state durable before the delta row is written, so a
            // refusal down there would leave a committed mutation with no delta
            // to carry it — divergence from the peers that never accepted it.
            //
            // Under the execution lock, which is what makes this a real
            // check-then-spend: two concurrent intents bearing one warrant
            // serialize here, so the second reads the nonce the first spent.
            //
            // A refusal over a ROLE surfaces as a typed `ExecuteError`, so it
            // reaches a `/intents` caller as a 403 instead of the opaque
            // internal error everything else in here becomes.
            //
            // At no cut, for the same reason as the relay's pre-check: this run
            // is about to be signed at this node's current heads.
            if let Err(err) = calimero_governance_store::warrant_gate::check_delegated_delta(
                &datastore,
                &context.id,
                d,
                calimero_governance_store::AdmissionCut::live(),
            ) {
                use calimero_context_client::messages::DelegatedWriteRefusal;
                use calimero_governance_store::warrant_gate::WarrantRefusal;
                let reason = match err.downcast_ref::<WarrantRefusal>() {
                    Some(WarrantRefusal::ExecutorIsTeeReplica) => {
                        Some(DelegatedWriteRefusal::ExecutorIsTeeReplica)
                    }
                    Some(WarrantRefusal::ExecutorIsReadOnly) => {
                        Some(DelegatedWriteRefusal::ExecutorIsReadOnly)
                    }
                    Some(WarrantRefusal::AuthorIsReadOnly) => {
                        Some(DelegatedWriteRefusal::AuthorIsReadOnly)
                    }
                    _ => None,
                };
                if let Some(reason) = reason {
                    bail!(ExecuteError::DelegatedWriteRefused {
                        context_id: context.id,
                        reason,
                    });
                }
                return Err(err);
            }
            Principal::new(d.warrant.author_account, d.warrant.author_device_key)
        }
    };
    let account = principal.account;
    let sealing = sealing_context(
        &datastore,
        &crate::scope_projection::FoldedProjections(scope_projections),
        &context.id,
        &executor,
        identity_private_key,
        tee_authority,
        delegation.is_some() || read_as.is_some(),
    )?;
    // Pin the governance cut a cell's writers are read at, so the run sees one answer.
    let pinned = shared_rotations::pin_cut(
        &datastore,
        scope_projections,
        context.id,
        governance_position,
    )?;
    let storage = ContextStorage::with_writers_resolver(
        datastore.clone(),
        context.id,
        Arc::clone(&pinned.writers),
    );
    // Kept for the on-behalf gate after the run; private storage takes a clone.
    let on_behalf_store = delegation.is_some().then(|| datastore.clone());
    // Private storage is node-local and keyed by context alone, so on a node
    // executing for accounts — a warranted write, or a read as an account —
    // every account using this context would share ONE bucket, and B could
    // read, delete or promote what A kept private. A run on an account's
    // behalf therefore gets no private store at all; a method that touches one
    // is refused after the run (`PrivateStorageUnavailable`) rather than
    // handed an empty default. The account's private data lives on its own
    // device. This node's own runs keep the per-context store as before.
    let on_behalf = delegation.is_some() || read_as.is_some();
    let private_storage =
        (!on_behalf).then(|| ContextPrivateStorage::from(datastore.clone(), context.id));

    // Search: only for an app that declares an index; any other app pays one
    // export lookup. A view gets the query host function, and tells the
    // indexer the root it saw, which is how a context whose state arrived by
    // snapshot (or moved while search was off) gets built on first use. A
    // peer delta's changed ids are read from its payload now, while `input`
    // is still ours (a local write's come from the artifact after the run).
    // The search exports themselves are the indexer's own reads: they neither
    // search nor wake it.
    let search =
        search.filter(|_| module.exports_function(calimero_primitives::search::EXTRACT_EXPORT));
    let is_search_export = calimero_primitives::search::is_export(&method);
    let mut search_ids = search
        .as_ref()
        .filter(|_| is_state_op)
        .map(|_| crate::search::changed_entity_ids(&input));
    let search_host = search
        .as_ref()
        .filter(|_| is_read_only_call && !is_search_export)
        .map(|s| {
            s.observe(*context.id.as_ref(), *context.root_hash);
            std::sync::Arc::new(crate::search::SearchHostAdapter(std::sync::Arc::clone(s)))
                as std::sync::Arc<dyn calimero_runtime::logic::SearchHost>
        });
    // Self-authored: both halves are this node's own identity. Delegated: both
    // come from the warrant, resolved in the match above — which is what keeps a
    // `User` leaf's owner equal to its delta's author and so survives
    // `user_leaf_author_is_its_owner` on the receive path. See
    // `principal::Principal`.
    let (mut outcome, mut storage, private_storage) = execute(
        guard,
        module,
        principal,
        method.clone(),
        input,
        storage,
        private_storage,
        node_client.clone(),
        is_read_only_call,
        xcall_origin,
        tee_authority,
        sealing,
        search_host,
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

    if let Err(err) = &outcome.returns {
        // The run had no private store because it was on an account's behalf,
        // and the method needed one. Typed, so the client is told what to do
        // (keep that data on the device) rather than shown a method failure;
        // and `bail!` rather than a method error, so nothing of this run is
        // committed or published. On this node's own runs the store is always
        // present, so this arm never fires for them.
        if on_behalf
            && matches!(
                err,
                calimero_runtime::errors::FunctionCallError::HostError(
                    calimero_runtime::errors::HostError::PrivateStorageUnavailable
                )
            )
        {
            bail!(ExecuteError::PrivateStorageUnavailable {
                context_id: context.id
            });
        }
        // Redacted at `warn`: the app's own error bytes and panic text can hold
        // its state, and this line is shipped off the node. See
        // `FunctionCallError::redacted`.
        warn!(
            context_id = %context.id,
            method = %method,
            error = %err.redacted(),
            "WASM execution returned error"
        );
        debug!(
            context_id = %context.id,
            method = %method,
            error = ?err,
            "WASM execution error (unredacted)"
        );
        return Ok((outcome, None, None, None, false));
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

    if executor_is_read_only && run_wrote(&outcome) {
        debug!(
            context_id = %context.id,
            %executor,
            method = %method,
            "ReadOnly member attempted state mutation — discarding changes"
        );
        discard_writes(&mut outcome);
        return Ok((outcome, None, None, None, true));
    }

    // Defence-in-depth: a method declared read-only in the ABI should never
    // produce a state mutation (the ReadOnlyContextStorage wrapper silences
    // writes at the host-call boundary). If the artifact is non-empty here,
    // the declaration is wrong or the wrapper leaked — reject rather than commit.
    if is_read_only_call && run_wrote(&outcome) {
        warn!(
            context_id = %context.id,
            %executor,
            method = %method,
            "method declared #[app::view] produced a state mutation — discarding (ABI mismatch)"
        );
        discard_writes(&mut outcome);
        return Ok((outcome, None, None, None, false));
    }

    if executor_not_authorized_for_state_op && run_wrote(&outcome) {
        debug!(
            context_id = %context.id,
            %executor,
            method = %method,
            ?write_source,
            "Non-member attempted state mutation — discarding changes (B3 user-storage extension)"
        );
        discard_writes(&mut outcome);
        return Ok((outcome, None, None, None, false));
    }

    // In-progress upgrade: a pure read falls through and is served from the
    // pre-migration root; a side-effecting call is refused (cross-version drift
    // risk), committing and dispatching nothing. "Side-effecting" = a committed
    // state mutation (`root_hash`) OR queued cross-context calls (`xcalls`),
    // which the external-actions stage would otherwise fire after this returns.
    if let Some(group_id) = block_writes_for_group {
        // `block_writes` is necessarily true here; refuse the call only if it had
        // a side effect (committed state or queued xcalls).
        if upgrade_rejects_committed_write(true, run_wrote(&outcome) || !outcome.xcalls.is_empty())
        {
            debug!(
                context_id = %context.id,
                %executor,
                method = %method,
                ?group_id,
                "refusing write: group upgrade in progress (a read would have been served)"
            );
            return Err(ExecuteError::UpgradeInProgress { group_id }.into());
        }
    }

    // The entries a delegated run writes for its author are signed by this
    // node, and peers accept them only from a `RelayTee` writing for a member.
    // The warrant gate is wider (it also admits an `Admin` or `Member` holding
    // `CAN_AUTHOR_ON_BEHALF`), so a run that signs an entry asks the narrower
    // rule too, before anything commits. A run that signs none writes nothing
    // on the author's behalf and stays with the warrant gate alone.
    if let (Some(d), Some(store)) = (delegation, on_behalf_store.as_ref()) {
        if !is_state_op && outcome.root_hash.is_some() && artifact_signs_entries(&outcome.artifact)
        {
            if let Some(reason) = on_behalf_refusal(store, &context.id, d.warrant.author_account)? {
                bail!(ExecuteError::DelegatedWriteRefused {
                    context_id: context.id,
                    reason,
                });
            }
        }
    }

    // Publish the run's rotations before its writes are kept and its delta's governance
    // position is read, so that position cites them. A run dropped above rotates nothing.
    let publisher = shared_rotations::Publisher {
        store: &datastore,
        node_client,
        ack_router,
        projections: scope_projections,
        context_id: context.id,
        group_id: pinned.group_id,
        author: account,
    };
    if !outcome.shared_rotations.is_empty() {
        publisher
            .publish(
                shared_rotations::RunKind {
                    delegated: delegation.is_some() || read_as.is_some(),
                    tee: tee_authority,
                    state_op: is_state_op,
                },
                &outcome.shared_rotations,
                &outcome.artifact,
                &pinned.writers,
            )
            .await?;
    }

    // The delta is signed at the heads read now, which can be past the cut the run read at.
    // Its author must still hold there what the run did to every cell it wrote, or the writes
    // are dropped here rather than refused by every peer.
    let creates_delta = outcome.root_hash.is_some() && !is_state_op && !outcome.artifact.is_empty();
    let signing_position = if creates_delta {
        compute_governance_position_for_context(&datastore, &context.id)
    } else {
        None
    };
    if creates_delta {
        publisher.verify_signing_cut(
            &pinned,
            signing_position.as_ref(),
            &outcome.shared_rotations,
            &outcome.artifact,
        )?;
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

        // Search: the changed ids, and the state root before and after, go
        // into the SAME transaction as the state, so the dirty row and the
        // change it names commit together. `before` is read from the
        // committed store, which this run's writes have not reached yet.
        if search.is_some() {
            let ids = search_ids
                .take()
                .unwrap_or_else(|| crate::search::changed_entity_ids(&outcome.artifact));
            let _staged = storage.stage_search_dirty(calimero_search::dirty::Change {
                before: context_client.compute_root_hash(&context.id)?,
                after: root_hash,
                ids: &ids,
            })?;
        }

        // Commit storage and persist metadata
        let store = storage.commit()?;
        if let Some(search) = &search {
            search.notify(*context.id.as_ref());
        }
        // Commit private storage (node-local, NOT synchronized)
        // Private storage changes are not included in sync deltas. A run on an
        // account's behalf opened none, so there is nothing to commit for it.
        if let Some(private_storage) = private_storage {
            let _private_store = private_storage.commit()?;
        }

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
                // A delegated run's entries are written for the author and
                // signed by this node; see `sign_authorized_actions`.
                sign_authorized_actions(
                    &mut actions,
                    identity_private_key,
                    delegation.is_some().then_some(account),
                )
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
                persist_signed_signatures(&store, context, account, identity_private_key, &actions)
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
            let events_hash = events_payload(&outcome.events)
                .as_deref()
                .map(CausalDelta::hash_events);
            let delta_id = CausalDelta::compute_id(&parents, &actions, events_hash.as_ref(), &hlc);

            let delta = CausalDelta {
                id: delta_id,
                parents,
                actions,
                hlc,
                events_hash,
            };
            // Before the delta can become a head: a head served without its events
            // hash matches no peer's check. Keyed and bound by the id, so an orphan is harmless.
            calimero_context_client::delta_events::record_events_hash(
                &store,
                &context.id,
                &delta.id,
                delta.events_hash.as_ref(),
            )?;

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

            // The governance position for the cross-DAG check that DAG-catchup
            // responders advertise on the wire: the one the author's rights were
            // verified at, so peers that pull this delta via
            // `request_dag_heads_and_sync` run the same `membership_status_at`
            // check the gossip path runs.
            let governance_position = signing_position;
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
            //
            // Under a warrant the shape changes but the roles do not: the
            // envelope names the AUTHOR and is signed by THIS node, which is
            // exactly what the self-authored path forbids and what the warrant
            // travelling beside it is what makes checkable. The two preimages
            // are domain-separated, so neither signature verifies on the other's
            // path and a relay cannot strip the warrant and pass the result off
            // as self-authored.
            //
            // A TEE-triggered run signs under a third domain, over what fired it
            // (`tee_authority` is only true with a trigger, and never with a
            // warrant). That is what lets peers tell it from any other delta this
            // key signs, and where they read the firing from.
            let signature_payload = match (delegation, tee_trigger.filter(|_| tee_authority)) {
                (None, Some(trigger)) => {
                    calimero_node_primitives::sync::delta_auth::tee_delta_signature_payload(
                        context.id,
                        delta.id,
                        principal.device,
                        trigger,
                        governance_position.as_ref(),
                        delta.hlc,
                    )?
                }
                (None, None) => {
                    calimero_node_primitives::sync::delta_auth::delta_signature_payload(
                        context.id,
                        delta.id,
                        principal.device,
                        governance_position.as_ref(),
                        delta.hlc,
                    )?
                }
                (Some(d), _) => {
                    calimero_node_primitives::sync::delta_auth::delegated_delta_signature_payload(
                        context.id,
                        delta.id,
                        principal.device,
                        d,
                        governance_position.as_ref(),
                        delta.hlc,
                    )?
                }
            };
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
                    checkpoint_root_hash: None,
                    events: None, // No events stored for locally created deltas
                    // The PRINCIPAL's device, not the signer's. They are the
                    // same value on the self-authored path and deliberately
                    // different under a warrant — this is the field the whole
                    // split exists to make honest.
                    author_id: Some(principal.device),
                    governance_position_blob,
                    delta_signature,
                    delegation: delegation.cloned(),
                },
            )?;
            // Kept beside the row, which cannot grow a field without breaking
            // every row already on disk: a peer that fetches this delta by
            // catchup needs the trigger to verify its signature.
            if let Some(trigger) = tee_trigger.filter(|_| tee_authority) {
                calimero_context_client::tee_trigger::record_delta_trigger(
                    &store,
                    &context.id,
                    &delta.id,
                    trigger,
                )?;
            }

            // Spend the nonce, now that the delta it authorizes is persisted.
            //
            // The authoring node has to do this itself. The receive path spends
            // on apply, but a relay never applies its own delta through that
            // path — so without this the warrant stayed unspent on the one node
            // holding it, and the relay would re-run the same authorization on
            // demand. Every peer would refuse the duplicate, so the divergence
            // is one-sided: the relay applies twice, the network once.
            //
            // After the row, deliberately. If this write fails the nonce is
            // merely still spendable — recoverable, and peers still refuse a
            // duplicate. Spending first and failing to persist would burn the
            // member's nonce for a write that never happened, which is not.
            if let Some(delegation) = delegation {
                calimero_governance_store::warrant_gate::spend_warrant_nonce(
                    &store,
                    &context.id,
                    delegation,
                )?;
            }

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
        false,
    ))
}

/// The keys behind this run's sealing host functions.
///
/// A run may open envelopes sealed to its executor key, with two exceptions.
/// A run on a TEE node opens nothing unless the TEE scheduler fired it: what is
/// sealed to a TEE is sealed to that node's key, and an ordinary JSON-RPC call
/// there runs as the same key. And a delegated run opens nothing, because its
/// principal is someone other than the node whose key it would open with.
///
/// A TEE-triggered run also gets the namespace TEE keys this TEE holds, and
/// seals a `TeeSecret` to the lowest that is not retired, so any TEE authority
/// that holds it, including one admitted later, can open it. Until the namespace
/// has such a key, or while every key this TEE holds is retired because a TEE
/// that held it was removed, it seals to the attested key of every TEE
/// authority instead.
/// Whether a run's artifact carries an entry this node will sign: see
/// [`signing::signs_entries`]. An artifact that is not `StorageDelta::Actions`
/// carries none, matching how the commit below reads it.
pub(crate) fn artifact_signs_entries(artifact: &[u8]) -> bool {
    matches!(
        borsh::from_slice::<StorageDelta>(artifact),
        Ok(StorageDelta::Actions(actions)) if signing::signs_entries(&actions)
    )
}

/// Why peers would refuse the entries a delegated run writes for `author`, or
/// `None` when they accept them: the on-behalf rule
/// (`calimero_governance_store::on_behalf_standing`) asked of this node's own
/// account, live, since the run is about to be signed at this node's heads.
fn on_behalf_refusal(
    datastore: &Store,
    context_id: &ContextId,
    author: calimero_account::AccountId,
) -> eyre::Result<Option<calimero_context_client::messages::DelegatedWriteRefusal>> {
    use calimero_context_client::messages::DelegatedWriteRefusal;
    use calimero_governance_store::OnBehalfRefusal;

    let Some(group_id) = calimero_governance_store::get_group_for_context(datastore, context_id)?
    else {
        // The warrant gate refuses a context in no group before this runs.
        bail!("a delegated write needs a context that belongs to a group");
    };
    let relay = calimero_governance_store::account_for_context(datastore, context_id)?;
    Ok(
        match calimero_governance_store::on_behalf_standing_live(
            datastore, &group_id, relay, author,
        )? {
            Ok(()) => None,
            Err(OnBehalfRefusal::SignerNotARelay) => {
                Some(DelegatedWriteRefusal::ExecutorIsNotARelay)
            }
            Err(OnBehalfRefusal::AccountIsReadOnly) => {
                Some(DelegatedWriteRefusal::AuthorIsReadOnly)
            }
            // The warrant gate, which runs first, refuses this too.
            Err(refusal @ OnBehalfRefusal::AccountNotAMember) => return Err(refusal.into()),
        },
    )
}

fn sealing_context(
    datastore: &Store,
    folded: &dyn calimero_governance_store::FoldedTeeAuthority,
    context_id: &ContextId,
    executor: &PublicKey,
    identity_private_key: &PrivateKey,
    tee_authority: bool,
    delegated: bool,
) -> eyre::Result<calimero_runtime::logic::SealingContext> {
    let may_open = tee_authority
        || (!delegated
            && !calimero_governance_store::is_tee_member_key_for_context(
                datastore, context_id, executor,
            )?);
    let no_vault = || calimero_governance_store::TeeVault {
        held: Vec::new(),
        sealing: None,
    };
    let vault = if tee_authority {
        match calimero_governance_store::get_group_for_context(datastore, context_id)? {
            Some(group_id) => calimero_governance_store::tee_vault(
                datastore,
                folded,
                &group_id,
                identity_private_key,
            )?,
            None => no_vault(),
        }
    } else {
        no_vault()
    };
    let tee_authority_keys = match vault.sealing {
        Some(key) => vec![*key],
        None if tee_authority => calimero_governance_store::tee_authority_keys_for_context(
            datastore, folded, context_id,
        )?
        .into_iter()
        .map(|key| *key)
        .collect(),
        None => Vec::new(),
    };
    let account_devices = if tee_authority {
        account_device_keys(datastore, context_id)?
    } else {
        std::collections::BTreeMap::new()
    };
    Ok(calimero_runtime::logic::SealingContext {
        opener: may_open
            .then(|| std::sync::Arc::new(PrivateKey::from(*identity_private_key.as_bytes()))),
        tee_authority_keys,
        vault_keys: vault.held.into_iter().map(std::sync::Arc::new).collect(),
        account_devices,
    })
}

/// The signing key of every live device of each account bound in the
/// namespace `context_id` belongs to: the keys a TEE-triggered run seals a
/// member's value to (`env::account_device_keys`). A device opens an envelope
/// with its context identity key, which is the binding's signing key.
fn account_device_keys(
    datastore: &Store,
    context_id: &ContextId,
) -> eyre::Result<std::collections::BTreeMap<[u8; 32], Vec<[u8; 32]>>> {
    let Some(group_id) = calimero_governance_store::get_group_for_context(datastore, context_id)?
    else {
        return Ok(std::collections::BTreeMap::new());
    };
    let namespace =
        calimero_governance_store::NamespaceRepository::new(datastore).resolve(&group_id)?;
    Ok(
        calimero_governance_store::AccountBindingRepository::new(datastore)
            .live_devices_by_account(&namespace)?
            .into_iter()
            .map(|(account, bindings)| {
                (
                    *account.as_bytes(),
                    bindings
                        .into_iter()
                        .map(|binding| *binding.sign_pk)
                        .collect(),
                )
            })
            .collect(),
    )
}

/// Whether a run asks to keep anything: state, or a writer-set rotation, which writes no byte.
fn run_wrote(outcome: &Outcome) -> bool {
    outcome.root_hash.is_some() || !outcome.shared_rotations.is_empty()
}

/// Drops everything a run asked to keep.
fn discard_writes(outcome: &mut Outcome) {
    outcome.root_hash = None;
    outcome.artifact.clear();
    outcome.xcalls.clear();
    outcome.shared_rotations.clear();
}

#[allow(clippy::too_many_arguments, reason = "execution context is wide")]
pub(crate) async fn execute(
    context: &ContextGuard,
    module: calimero_runtime::Module,
    // One argument, not two adjacent 32-byte ids that the compiler cannot tell
    // apart. See `principal::Principal` for which layer consumes which half.
    principal: Principal,
    method: Cow<'static, str>,
    input: Cow<'static, [u8]>,
    mut storage: ContextStorage,
    // `None` for a run on an account's behalf, which has no private store
    // (see `internal_execute`); the runtime then refuses a private host call.
    mut private_storage: Option<ContextPrivateStorage>,
    node_client: NodeClient,
    is_read_only_call: bool,
    xcall_origin: Option<ContextId>,
    tee_trigger: bool,
    sealing: calimero_runtime::logic::SealingContext,
    // Only ever `Some` for a read-only run (see `internal_execute`).
    search: Option<std::sync::Arc<dyn calimero_runtime::logic::SearchHost>>,
) -> eyre::Result<(Outcome, ContextStorage, Option<ContextPrivateStorage>)> {
    let context_id = **context;

    global_runtime()
        .spawn_blocking(move || {
            let private = private_storage
                .as_mut()
                .map(|p| p as &mut dyn calimero_runtime::store::Storage);
            let outcome = if is_read_only_call {
                // Wrap shared storage in a read-only view: `set`/`remove` host
                // calls are silenced so a method holding a shared read guard
                // cannot mutate shared state. The post-exec assertion on
                // outcome.root_hash / artifact catches any method that
                // nonetheless produced a mutation.
                //
                // `with_local_index` rather than `new`: the ordered secondary
                // index is node-local and rebuilt lazily *by* an ordered read,
                // so silencing it does not make the read safer, it makes it
                // return nothing. See `ReadOnlyContextStorage`'s own docs.
                let mut ro_storage = ReadOnlyContextStorage::with_local_index(&mut storage);
                // The private plane is NOT wrapped. It is node-local: a separate
                // column, never hashed into the root, never gossiped, and
                // committed only under `outcome.root_hash.is_some()` — which a
                // read-only call discards before the commit block is reached. So
                // nothing here can escape the transaction, while a first read of
                // a private collection can still create the root element it
                // hangs off. Wrapping it bought no safety and cost exactly that:
                // `my_secrets()` panicked with `CannotCreateOrphan`.
                module.run_with_origin(
                    context_id,
                    principal.account,
                    principal.device,
                    &method,
                    &input,
                    &mut ro_storage,
                    private,
                    Some(node_client),
                    xcall_origin,
                    tee_trigger,
                    sealing,
                    search,
                )?
            } else {
                module.run_with_origin(
                    context_id,
                    principal.account,
                    principal.device,
                    &method,
                    &input,
                    &mut storage,
                    private,
                    Some(node_client),
                    xcall_origin,
                    tee_trigger,
                    sealing,
                    None,
                )?
            };
            Ok((outcome, storage, private_storage))
        })
        .await
        .wrap_err("failed to receive execution response")?
}

/// A run's events as they ride in its delta, handlers included, or `None` if it
/// emitted none. The delta id commits to exactly these bytes.
fn events_payload(events: &[calimero_runtime::logic::Event]) -> Option<Vec<u8>> {
    if events.is_empty() {
        return None;
    }
    let events: Vec<ExecutionEvent> = events
        .iter()
        .map(|e| ExecutionEvent {
            kind: e.kind.clone(),
            data: e.data.clone(),
            handler: e.handler.clone(),
        })
        .collect();
    Some(ExecutionEvent::encode_all(&events))
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

/// The `#[app::handler]` methods a module's embedded ABI declares; empty when
/// the manifest is absent or unparseable, so such an app runs no handler.
fn extract_handler_set(bytecode: &[u8]) -> Arc<HashSet<String>> {
    let methods = calimero_wasm_abi::embed::read_embedded_state_schema(bytecode)
        .map(|manifest| manifest.methods)
        .unwrap_or_default();
    Arc::new(
        methods
            .into_iter()
            .filter(|m| m.handler)
            .map(|m| m.name)
            .collect(),
    )
}

/// Decides whether an xcall to `method` is denied, given the target module's
/// declared entry points (`policies`, `None` if unknown),
/// the caller's application id (`source_app`, `None` if it couldn't be
/// resolved), and the target's application id (`target_app`).
///
/// Denied when the entry points are unknown, the method is not a declared
/// `#[app::xcall]` entry point, or its policy excludes the caller. A `SameApp`
/// entry point with an unresolved caller is denied (fail closed).
fn xcall_caller_denied(
    policies: Option<&crate::XCallPolicyMap>,
    method: &str,
    source_app: Option<ApplicationId>,
    target_app: ApplicationId,
) -> bool {
    match policies.and_then(|policies| policies.get(method)) {
        None => true,
        Some(XCallCallers::AnyInNamespace) => false,
        Some(XCallCallers::SameApp) => source_app != Some(target_app),
    }
}

/// The `#[app::xcall]` entry points declared in a module's embedded ABI mapped
/// to their caller policy; empty if the manifest is absent/unparseable or
/// declares none, so every xcall into the module is denied.
fn extract_xcall_policies(bytecode: &[u8]) -> Arc<crate::XCallPolicyMap> {
    let map = calimero_wasm_abi::embed::read_embedded_state_schema(bytecode)
        .map(|manifest| {
            manifest
                .methods
                .into_iter()
                .filter(|m| m.xcall_callable)
                .map(|m| (m.name, m.xcall_callers))
                .collect()
        })
        .unwrap_or_default();
    Arc::new(map)
}

/// Whether `source` and `target` share the SAME directly-owning group, and are
/// therefore permitted to cross-call each other. Callers treat `Err` or an
/// unregistered (`None`) resolution as a denial.
///
/// The xcall boundary used to be the shared *namespace root*, but that let any
/// sibling context anywhere in the namespace — including a different, even
/// `Restricted`, subgroup — call in and execute as a member of the target
/// (possibly its admin). Requiring the same owning group keeps a cross-call
/// inside the one group whose membership both contexts already belong to, so it
/// can never punch through a subgroup boundary. There is no cross-subgroup
/// xcall grant mechanism; contexts that must call across subgroups have to
/// share an owning group.
fn xcall_same_owning_group(
    store: &calimero_store::Store,
    source: &ContextId,
    target: &ContextId,
) -> eyre::Result<bool> {
    let src = calimero_governance_store::get_group_for_context(store, source)?;
    let tgt = calimero_governance_store::get_group_for_context(store, target)?;
    Ok(matches!((src, tgt), (Some(a), Some(b)) if a == b))
}

#[cfg(test)]
mod search_tests;

#[cfg(test)]
mod shared_rotation_tests;

#[cfg(test)]
mod state_write_gate_tests;
#[cfg(test)]
mod xcall_tests;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{
        register_context_in_group, MetaRepository, NamespaceRepository,
    };
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::ContextId;
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;
    use calimero_store::key::GroupTarget;
    use calimero_store::Store;

    use std::collections::HashMap;

    use super::{
        extract_xcall_policies, resolve_producing_bytecode_id, should_block, upgrade_blocks_write,
        upgrade_rejects_committed_write, xcall_caller_denied, xcall_same_owning_group,
        XCallCallers,
    };
    use calimero_store::key::GroupUpgradeStatus;

    fn fresh_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// Construct a minimal `GroupMetaValue` with the given `bytecode_id`.
    fn group_meta_with_bytecode_id(bytecode_id: [u8; 32]) -> GroupMetaValue {
        let dummy_pk = PublicKey::from([0xAB; 32]);
        GroupMetaValue {
            target: GroupTarget {
                application_id: ApplicationId::from([0xCC; 32]),
                bytecode_id,
                package: Box::default(),
                version: Box::default(),
            },
            created_at: 1_700_000_000,
            admin_identity: crate::test_support::account_for(&dummy_pk),
            owner_identity: crate::test_support::account_for(&dummy_pk),
            migration: None,
            auto_join: false,
        }
    }

    #[test]
    fn xcall_same_owning_group_allows_same_group() {
        let store = fresh_store();
        let group = ContextGroupId::from([0xAA; 32]);
        let a = ContextId::from([0x01; 32]);
        let b = ContextId::from([0x02; 32]);

        register_context_in_group(&store, &group, &a).expect("register a");
        register_context_in_group(&store, &group, &b).expect("register b");

        assert!(xcall_same_owning_group(&store, &a, &b).expect("resolve ok"));
    }

    #[test]
    fn xcall_same_owning_group_denies_across_subgroups_of_one_namespace() {
        let store = fresh_store();
        // namespace N (root) ⊃ { sub_a ⊃ ctx_a, sub_b ⊃ ctx_b }.
        // Both contexts share the namespace root but live in DIFFERENT
        // owning subgroups, so the cross-call must be denied even though the
        // old namespace-root check would have allowed it.
        let ns = ContextGroupId::from([0xAA; 32]);
        let sub_a = ContextGroupId::from([0xB1; 32]);
        let sub_b = ContextGroupId::from([0xB2; 32]);
        let ctx_a = ContextId::from([0x0A; 32]);
        let ctx_b = ContextId::from([0x0B; 32]);

        let ns_repo = NamespaceRepository::new(&store);
        ns_repo.nest(&ns, &sub_a).expect("nest sub_a");
        ns_repo.nest(&ns, &sub_b).expect("nest sub_b");
        register_context_in_group(&store, &sub_a, &ctx_a).expect("register ctx_a");
        register_context_in_group(&store, &sub_b, &ctx_b).expect("register ctx_b");

        assert!(!xcall_same_owning_group(&store, &ctx_a, &ctx_b).expect("resolve ok"));
    }

    #[test]
    fn xcall_same_owning_group_denies_unregistered() {
        let store = fresh_store();
        let group = ContextGroupId::from([0xAA; 32]);
        let a = ContextId::from([0x01; 32]);
        let b = ContextId::from([0x02; 32]);
        register_context_in_group(&store, &group, &a).expect("register a");

        // `b` is registered in no group at all → deny.
        assert!(!xcall_same_owning_group(&store, &a, &b).expect("resolve ok"));
    }

    #[test]
    fn xcall_caller_policy_decision() {
        let app_a = ApplicationId::from([0xA1; 32]);
        let app_b = ApplicationId::from([0xB2; 32]);
        let mut policies = HashMap::new();
        policies.insert("open".to_owned(), XCallCallers::AnyInNamespace);
        policies.insert("restricted".to_owned(), XCallCallers::SameApp);

        let policies = Some(&policies);

        // A method not declared as an entry point is always denied.
        assert!(xcall_caller_denied(policies, "unknown", Some(app_a), app_a));

        // Entry points that are not known deny every method.
        assert!(xcall_caller_denied(None, "open", Some(app_a), app_a));

        // AnyInNamespace admits any caller (including an unresolved one).
        assert!(!xcall_caller_denied(policies, "open", Some(app_b), app_a));
        assert!(!xcall_caller_denied(policies, "open", None, app_a));

        // SameApp admits only a caller running the same application id.
        assert!(!xcall_caller_denied(
            policies,
            "restricted",
            Some(app_a),
            app_a
        ));
        assert!(xcall_caller_denied(
            policies,
            "restricted",
            Some(app_b),
            app_a
        ));
        // An unresolved caller is denied for SameApp — fail closed.
        assert!(xcall_caller_denied(policies, "restricted", None, app_a));
    }

    #[test]
    fn extract_xcall_policies_empty_on_non_wasm() {
        // No embedded ABI manifest ⇒ no entry points (every xcall denied).
        assert!(extract_xcall_policies(b"not a wasm module").is_empty());
        assert!(extract_xcall_policies(&[]).is_empty());
    }

    #[test]
    fn resolve_producing_bytecode_id_returns_group_meta_bytecode_id() {
        let store = fresh_store();
        let context_id = ContextId::from([0xF1; 32]);
        let group_id = ContextGroupId::from([0xF2; 32]);

        register_context_in_group(&store, &group_id, &context_id)
            .expect("register_context_in_group");
        MetaRepository::new(&store)
            .save(&group_id, &group_meta_with_bytecode_id([0x22; 32]))
            .expect("save group meta");

        assert_eq!(
            resolve_producing_bytecode_id(&store, &context_id).unwrap(),
            Some([0x22; 32])
        );
    }

    #[test]
    fn resolve_producing_bytecode_id_none_for_non_group_context() {
        let store = fresh_store();
        // context_id was never registered in any group
        let context_id = ContextId::from([0xF3; 32]);

        assert_eq!(
            resolve_producing_bytecode_id(&store, &context_id).unwrap(),
            None
        );
    }

    #[test]
    fn resolve_producing_bytecode_id_none_when_meta_absent() {
        // Context is registered under a group, but no `GroupMetaValue` was
        // ever written for that group — the resolver must return `None`
        // (no bytecode_id to stamp) rather than erroring.
        let store = fresh_store();
        let context_id = ContextId::from([0xF4; 32]);
        let group_id = ContextGroupId::from([0xF5; 32]);

        register_context_in_group(&store, &group_id, &context_id)
            .expect("register_context_in_group");

        assert_eq!(
            resolve_producing_bytecode_id(&store, &context_id).unwrap(),
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
    // group bytecode_id → None (row fallback). Two contexts sharing one
    // application id but holding different markers must resolve different
    // blobs — the coexistence invariant the module cache re-key enables.

    #[test]
    fn bound_bytecode_two_contexts_different_markers_resolve_different_blobs() {
        let store = fresh_store();
        let group_id = ContextGroupId::from([0xB0; 32]);
        let ctx_a = ContextId::from([0xB1; 32]);
        let ctx_b = ContextId::from([0xB2; 32]);

        register_context_in_group(&store, &group_id, &ctx_a).expect("register a");
        register_context_in_group(&store, &group_id, &ctx_b).expect("register b");
        MetaRepository::new(&store)
            .save(&group_id, &group_meta_with_bytecode_id([0x33; 32]))
            .expect("save group meta");

        crate::activation::record_activation(&store, &ctx_a, [0x11; 32]);
        crate::activation::record_activation(&store, &ctx_b, [0x22; 32]);

        assert_eq!(
            super::bound_bytecode_for_context(&store, &ctx_a),
            Some(([0x11; 32], super::BoundBytecodeSource::ActivationMarker))
        );
        assert_eq!(
            super::bound_bytecode_for_context(&store, &ctx_b),
            Some(([0x22; 32], super::BoundBytecodeSource::ActivationMarker))
        );
    }

    #[test]
    fn bound_bytecode_falls_back_to_group_bytecode_id_without_marker() {
        let store = fresh_store();
        let group_id = ContextGroupId::from([0xB3; 32]);
        let ctx = ContextId::from([0xB4; 32]);

        register_context_in_group(&store, &group_id, &ctx).expect("register");
        MetaRepository::new(&store)
            .save(&group_id, &group_meta_with_bytecode_id([0x44; 32]))
            .expect("save group meta");

        assert_eq!(
            super::bound_bytecode_for_context(&store, &ctx),
            Some(([0x44; 32], super::BoundBytecodeSource::GroupKey))
        );
    }

    #[test]
    fn bound_bytecode_none_for_zero_bytecode_id_or_non_group_context() {
        let store = fresh_store();
        let group_id = ContextGroupId::from([0xB5; 32]);
        let ctx = ContextId::from([0xB6; 32]);

        register_context_in_group(&store, &group_id, &ctx).expect("register");
        MetaRepository::new(&store)
            .save(&group_id, &group_meta_with_bytecode_id([0u8; 32]))
            .expect("save group meta");

        // Zero bytecode_id (legacy) carries no blob identity — row fallback.
        assert_eq!(super::bound_bytecode_for_context(&store, &ctx), None);
        // Non-group context — row fallback.
        let lone = ContextId::from([0xB7; 32]);
        assert_eq!(super::bound_bytecode_for_context(&store, &lone), None);
    }
}
