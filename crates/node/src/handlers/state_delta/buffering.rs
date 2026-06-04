//! Governance-pending buffering: draining buffered/absorbed state deltas,
//! re-evaluating their authorization against current governance state, and the
//! fence-and-absorb decision for stale/uninitialized deltas.
//!
//! Extracted from the state-delta handler. The drain/recover entry points are
//! invoked by the apply path, the sync manager, and namespace network-event
//! handlers once governance state advances or on startup.

use calimero_context::group_store::{membership_status_at, MembershipStatus};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use eyre::Result;
use tracing::{debug, info, warn};

use super::{
    apply_authorized_state_delta, choose_owned_identity, state_delta_message_from_buffered,
    verify_position_group_id_matches_context, GroupIdCheck, StateDeltaContext,
};

/// Drain the governance-pending buffer for `context_id`, re-evaluating each
/// delta's authorization status against current local governance state and
/// dispatching by outcome.
///
/// Outcomes per drained delta:
/// * `Member` — the referenced governance heads are now known and the author
///   is authorized. The buffered delta is reconstructed into a
///   [`StateDeltaMessage`] and applied directly via
///   [`apply_authorized_state_delta`]. Gossipsub does *not* auto-rebroadcast
///   already-delivered messages, so dropping here would lose the delta
///   permanently — recovery would only happen via hash-heartbeat divergence
///   detection triggering snapshot sync.
/// * `Removed` / `NeverMember` / `Err` — author is permanently not
///   authorized at this position; drop with a warn log.
/// * `Unknown { needed }` — governance still hasn't caught up; push the
///   delta back into the pending buffer.
///
/// Calls `apply_authorized_state_delta` directly (not `handle_state_delta`)
/// so the call graph stays linear — no async recursion, no per-recurse
/// future allocation. The cross-DAG check we just performed via
/// `membership_status_at` is the same check `handle_state_delta` would have
/// performed; skipping back through the entry handler would also re-drain
/// the (now-empty) pending buffer, wasted work.
pub(super) async fn drain_governance_pending(input: &StateDeltaContext, context_id: &ContextId) {
    // Pop-then-process pattern: drain one delta at a time so that if
    // `apply_authorized_state_delta` panics or the actor task is killed
    // mid-iteration, the rest of the queue stays in the buffer and the
    // next drain pass picks them up. Bulk-drain-then-process would lose
    // every still-unprocessed delta on panic.
    //
    // Iteration is capped at the snapshot length we observe at entry —
    // a delta re-buffered as still-Unknown during this drain pass must
    // not be re-evaluated until the *next* drain pass (a fresh trigger
    // signal), otherwise drain could loop forever on a permanently
    // unresolvable delta. The per-delta `governance_drain_attempts`
    // counter is the deeper guard; this snapshot cap is the cheap
    // pre-check.
    let snapshot_len = input.node_state.governance_pending_len(context_id);
    if snapshot_len == 0 {
        return;
    }
    debug!(
        %context_id,
        count = snapshot_len,
        "governance-pending drain: draining governance-pending buffer"
    );
    for _ in 0..snapshot_len {
        let Some(buffered) = input.node_state.pop_governance_pending(context_id) else {
            break;
        };
        let Some(pos) = buffered.governance_position.as_ref() else {
            warn!(
                %context_id,
                delta_id = ?buffered.id,
                "governance-pending drain: pending delta has no governance_position; dropping"
            );
            crate::node_metrics::record_governance_drain_outcome("no_governance_position");
            continue;
        };
        let datastore = input.node_clients.context.datastore();
        // Anti-bypass: see `GroupIdCheck` for the bypasses this match
        // closes. The drain path only sees `Some` positions (the
        // governance-pending buffer wouldn't accept `None`), so the
        // `NonGroupOk` / `GroupContextNoPosition` variants are
        // unreachable here; we still spell them out for exhaustive-
        // match safety against future buffer shape changes.
        //
        // INVARIANT: `ContextManager` serializes governance ops, so
        // no concurrent group reassignment can interleave between
        // this check and the `membership_status_at` call below — see
        // the TOCTOU note on `verify_position_group_id_matches_context`.
        match verify_position_group_id_matches_context(datastore, context_id, Some(pos.group_id)) {
            GroupIdCheck::Match => {}
            // `NonGroupOk` and `GroupContextNoPosition` both require
            // `claimed_group_id == None` per the helper's match table.
            // The drain always passes `Some(pos.group_id)` here (we
            // bound `pos` via let-else above), so both variants are
            // structurally unreachable from this call site.
            //
            // `debug_assert!` catches a future refactor that breaks
            // the call-site contract (e.g. swapping to pass `None`)
            // in test/dev builds. Release builds fall through to a
            // defensive `continue` rather than panic — a single
            // anomalous delta shouldn't crash the actor; the metric
            // counter is the operator signal.
            GroupIdCheck::NonGroupOk | GroupIdCheck::GroupContextNoPosition { .. } => {
                debug_assert!(
                    false,
                    "GroupIdCheck::{{NonGroupOk, GroupContextNoPosition}} require \
                     claimed_group_id=None, but drain always passes Some(pos.group_id) — \
                     the call-site contract has been broken"
                );
                warn!(
                    %context_id,
                    delta_id = ?buffered.id,
                    author = %buffered.author_id,
                    "governance-pending drain: dropping pending delta — \
                     verify_position_group_id_matches_context returned an outcome that \
                     requires claimed_group_id=None despite drain passing Some \
                     (call-site contract violated; investigate)"
                );
                crate::node_metrics::record_governance_drain_outcome("helper_contract_violation");
                continue;
            }
            GroupIdCheck::Mismatch { owning, claimed } => {
                warn!(
                    %context_id,
                    delta_id = ?buffered.id,
                    author = %buffered.author_id,
                    owning_group = ?owning,
                    claimed_group = ?claimed,
                    "governance-pending drain: rejecting pending delta — governance_position \
                     references a different group than the context's owning group"
                );
                crate::node_metrics::record_governance_drain_outcome("group_mismatch");
                continue;
            }
            GroupIdCheck::NonGroupContextWithPosition { claimed } => {
                // The context was in a group when this delta was
                // accepted into the buffer, but is no longer — either
                // it was detached, or a store inconsistency caused a
                // transient `Ok(None)` here. Distinct metric label
                // from `group_mismatch` so operators can tell the two
                // cases apart in dashboards.
                warn!(
                    %context_id,
                    delta_id = ?buffered.id,
                    author = %buffered.author_id,
                    claimed_group = ?claimed,
                    "governance-pending drain: rejecting pending delta — governance_position \
                     present but context is not part of any group (group disappeared since buffering?)"
                );
                crate::node_metrics::record_governance_drain_outcome("group_disappeared");
                continue;
            }
            GroupIdCheck::LookupError(err) => {
                warn!(
                    %context_id,
                    delta_id = ?buffered.id,
                    author = %buffered.author_id,
                    %err,
                    "governance-pending drain: get_group_for_context failed; dropping delta to avoid silent bypass"
                );
                crate::node_metrics::record_governance_drain_outcome("group_lookup_failed");
                continue;
            }
        }
        // Forward-only invariant — see the gossip-receive site in
        // `apply_authorized_state_delta` for the full contract. The
        // governance-pending drain MUST use the buffered delta's
        // signed `governance_position`, not the receiver's current
        // state — the whole point of buffering was that the author
        // signed against a cut the receiver wasn't caught up to. By
        // the time we drain, the receiver's local DAG may have
        // advanced past the signed cut (including a `MemberRemoved`
        // for this author); forward-only resolves pre-removal writes
        // to `Member` so the deferred apply is correct.
        let status = membership_status_at(datastore, &buffered.author_id, pos);
        match status {
            Ok(MembershipStatus::Member(_)) => {
                debug!(
                    %context_id,
                    delta_id = ?buffered.id,
                    author = %buffered.author_id,
                    "governance-pending drain: pending delta now authorized; re-applying"
                );
                crate::node_metrics::record_governance_drain_outcome("applied");
                let reconstructed = state_delta_message_from_buffered(buffered, *context_id);
                // NOT an absorb-drain — the delta may still be stale-schema, so
                // it must keep going through the fence (bypass = false).
                if let Err(err) =
                    apply_authorized_state_delta(input.clone(), reconstructed, false).await
                {
                    warn!(
                        %context_id,
                        %err,
                        "governance-pending drain: re-apply of authorized buffered delta failed"
                    );
                }
            }
            Ok(MembershipStatus::Removed { last_role }) => {
                warn!(
                    %context_id,
                    delta_id = ?buffered.id,
                    author = %buffered.author_id,
                    last_role = ?last_role,
                    "governance-pending drain: pending delta from removed author; dropping"
                );
                crate::node_metrics::record_governance_drain_outcome("removed");
            }
            Ok(MembershipStatus::NeverMember) => {
                warn!(
                    %context_id,
                    delta_id = ?buffered.id,
                    author = %buffered.author_id,
                    "governance-pending drain: pending delta from non-member; dropping"
                );
                crate::node_metrics::record_governance_drain_outcome("never_member");
            }
            Ok(MembershipStatus::Unknown { needed }) => {
                let mut buffered = buffered;
                buffered.governance_drain_attempts =
                    buffered.governance_drain_attempts.saturating_add(1);
                if buffered.governance_drain_attempts
                    >= calimero_node_primitives::delta_buffer::MAX_GOVERNANCE_DRAIN_ATTEMPTS
                {
                    warn!(
                        %context_id,
                        delta_id = ?buffered.id,
                        attempts = buffered.governance_drain_attempts,
                        "governance-pending drain: dropping pending delta after exhausting drain attempts \
                         (governance heads still unknown — likely permanently missing)"
                    );
                    crate::node_metrics::record_governance_drain_outcome("dropped_max_attempts");
                } else {
                    debug!(
                        %context_id,
                        delta_id = ?buffered.id,
                        needed_count = needed.len(),
                        attempts = buffered.governance_drain_attempts,
                        "governance-pending drain: still pending governance catchup; re-buffering"
                    );
                    crate::node_metrics::record_governance_drain_outcome("rebuffered");
                    input
                        .node_state
                        .buffer_governance_pending(*context_id, buffered);
                }
            }
            Err(err) => {
                warn!(
                    %context_id,
                    delta_id = ?buffered.id,
                    %err,
                    "governance-pending drain: membership lookup failed for pending delta; dropping"
                );
                crate::node_metrics::record_governance_drain_outcome("lookup_error");
            }
        }
    }
}

/// Drain governance-pending buffers for **every** context that currently
/// holds at least one entry. Called from the namespace-governance apply
/// path on `Applied` outcome — a governance op that just applied may
/// unblock state deltas previously buffered as `Unknown`. Without this
/// hook, the lazy on-state-delta drain alone deadlocks when the only
/// state delta in flight is the one waiting for that very governance op
/// (the e2e 3-node test reproduced this: node-1 broadcasts a single state
/// delta, node-2 buffers it for missing governance heads, no further
/// state delta arrives to trigger drain, never converges).
///
/// Per-context drain still happens lazily on incoming state-deltas; this
/// hook is the *active* path that converges in the absence of fresh
/// state-delta traffic.
pub(crate) async fn drain_all_governance_pending(input: &StateDeltaContext) {
    let context_ids = input.node_state.governance_pending_context_ids();
    if context_ids.is_empty() {
        return;
    }
    debug!(
        count = context_ids.len(),
        "governance-pending drain: governance-apply hook draining pending buffers across contexts"
    );
    for context_id in context_ids {
        drain_governance_pending(input, &context_id).await;
    }
}

/// Drain the absorb buffer for `context_id` by replaying each now-readable
/// straggler delta's original signed bytes verbatim.
///
/// Called when the context's loaded binary advances. Each pending
/// [`AbsorbRecord`] readable by the node's loaded reader is reconstructed into
/// its byte-identical [`BufferedDelta`] and re-applied via
/// [`apply_authorized_state_delta`]; bytes are never translated (that would
/// break each `Action`'s `payload_for_signing` signature). The record is
/// deleted only after a successful replay (idempotent via the `delta_id` key).
///
/// Replays with `bypass_fence == true` so the fence isn't re-evaluated: the
/// drain has already decided the delta is readable, and re-fencing a stale
/// straggler would re-absorb it instead of applying it (infinite no-op).
pub(crate) async fn drain_absorbed(input: &StateDeltaContext, context_id: &ContextId) {
    let store = input.node_clients.context.datastore();
    let drained = drain_absorbed_records(store, context_id, |buffered| {
        let input = input.clone();
        let context_id = *context_id;
        async move {
            let reconstructed = state_delta_message_from_buffered(buffered, context_id);
            // Bypass the fence: the drain already decided this straggler is
            // replayable, and re-fencing would re-absorb it (infinite no-op).
            apply_authorized_state_delta(input, reconstructed, true).await?;
            // Ok(()) covers both an applied and a soft-declined (e.g. ReadOnly)
            // delta — either way it's consumed, so report success and delete.
            Ok::<bool, eyre::Report>(true)
        }
    })
    .await;

    match drained {
        Ok(0) => {}
        Ok(n) => info!(
            %context_id,
            drained = n,
            "absorb drain: replayed buffered straggler deltas verbatim after binary advance"
        ),
        Err(err) => warn!(
            %context_id,
            %err,
            "absorb drain: failed to enumerate absorb buffer"
        ),
    }

    // Buffered sync-repair leaves/entities drain on a separate tag.
    if let Err(err) = drain_absorbed_leaves(input, context_id).await {
        warn!(%context_id, %err, "absorb drain: leaf drain failed");
    }
}

/// Drain buffered sync-repair leaves once the loaded reader has advanced to
/// their schema.
///
/// Sibling of [`drain_absorbed`] for the leaf-shaped [`AbsorbRecord`]s that the
/// HashComparison / LevelSync / snapshot apply gate buffered (a receiver on an
/// older reader buffers a future-schema leaf rather than LWW-storing unreadable
/// bytes). On binary advance the leaf becomes readable, so its original
/// `TreeLeafData` bytes are re-applied verbatim through
/// [`apply_leaf_with_crdt_merge`] (the same convergent CRDT-merge the live sync
/// path uses); the record is deleted only on success (idempotent on the key).
async fn drain_absorbed_leaves(input: &StateDeltaContext, context_id: &ContextId) -> Result<()> {
    use borsh::BorshDeserialize;
    use calimero_context::group_store::AbsorbRepository;
    use calimero_context::hlc_fence::loaded_reader_app_key;
    use calimero_node_primitives::sync::storage_bridge::create_runtime_env;
    use calimero_node_primitives::sync::TreeLeafData;

    let store = input.node_clients.context.datastore();

    // The schema this node can read right now. `None` ⇒ can't tell readability;
    // leave every leaf record pending.
    let Some(loaded) = loaded_reader_app_key(store, context_id)? else {
        return Ok(());
    };

    let repo = AbsorbRepository::new(store);
    let pending = repo.enumerate_pending(context_id)?;
    // Nothing leaf- or entity-shaped to do? Avoid building a runtime env /
    // resolving an identity for the common (delta-only) case.
    if !pending
        .iter()
        .any(|(_, r)| r.leaf.is_some() || r.entity.is_some())
    {
        return Ok(());
    }

    let identity = choose_owned_identity(&input.node_clients.context, context_id).await?;
    let runtime_env = create_runtime_env(store, *context_id, identity);

    let mut drained = 0usize;
    for ((producing_app_key, delta_id), record) in pending {
        // Snapshot-entity-shaped records: re-verify + persist the raw `entry` +
        // `index` blobs via `handle.put` (the snapshot apply path deliberately
        // bypasses CRDT merge), once the loaded reader matches the schema.
        if let Some(entity_absorb) = record.entity {
            if entity_absorb.schema_app_key != loaded {
                continue;
            }
            let mut handle = input.node_clients.context.datastore_handle();
            match crate::sync::snapshot::persist_buffered_snapshot_entity(
                &mut handle,
                *context_id,
                entity_absorb.id,
                &entity_absorb.entry,
                &entity_absorb.index,
            ) {
                Ok(crate::sync::snapshot::SnapshotEntityDrainOutcome::Persisted) => {
                    repo.delete(context_id, producing_app_key, delta_id)?;
                    drained += 1;
                }
                // SharedMember is re-applied via the snapshot pass-2 re-drive.
                // Delete the orphaned buffer record so it stops blocking the
                // drain early-exit and wasting a runtime env per apply.
                Ok(crate::sync::snapshot::SnapshotEntityDrainOutcome::RedrivenElsewhere) => {
                    repo.delete(context_id, producing_app_key, delta_id)?;
                }
                Ok(crate::sync::snapshot::SnapshotEntityDrainOutcome::Pending) => {
                    /* left pending — verify/parse failed */
                }
                Err(err) => warn!(
                    %context_id,
                    delta_id = ?delta_id,
                    %err,
                    "absorb entity drain: persist failed — leaving record pending for retry"
                ),
            }
            continue;
        }

        let Some(leaf_absorb) = record.leaf else {
            continue; // delta record — handled by `drain_absorbed_records`.
        };
        // Only re-apply once the loaded reader matches the leaf's schema.
        if leaf_absorb.schema_app_key != loaded {
            continue;
        }

        let leaf = match TreeLeafData::try_from_slice(&leaf_absorb.leaf_bytes) {
            Ok(l) => l,
            Err(err) => {
                warn!(
                    %context_id,
                    delta_id = ?delta_id,
                    %err,
                    "absorb leaf drain: corrupt buffered leaf bytes — skipping"
                );
                continue;
            }
        };

        let ctx = *context_id;
        let apply = calimero_storage::env::with_runtime_env(runtime_env.clone(), || {
            crate::sync::helpers::apply_leaf_with_crdt_merge(ctx, &leaf)
        });
        match apply {
            Ok(()) => {
                repo.delete(context_id, producing_app_key, delta_id)?;
                drained += 1;
            }
            Err(err) => warn!(
                %context_id,
                delta_id = ?delta_id,
                %err,
                "absorb leaf drain: re-apply failed — leaving record pending for retry"
            ),
        }
    }

    if drained > 0 {
        info!(
            %context_id,
            drained,
            "absorb drain: re-applied buffered sync-repair leaves/entities after binary advance"
        );
    }
    Ok(())
}

/// Core drain mechanics, factored out so the decision/delete logic is unit-
/// testable without a live WASM executor: `replay` is the injection seam (the
/// production hook passes [`apply_authorized_state_delta`]; tests pass a
/// recording mock).
///
/// For each pending [`AbsorbRecord`] in `context_id`'s absorb buffer:
/// - skip it while the node has not reached the migration target AND the
///   record's `producing_app_key` differs from the loaded reader (binary hasn't
///   caught up — leave it for a later pass);
/// - otherwise reconstruct the verbatim [`BufferedDelta`], hand it to `replay`,
///   and — only on `Ok(true)` — `delete` the record.
///
/// Returns the number of records drained. A replay that errors or returns
/// `Ok(false)` leaves the record in place for the next pass (delete-after-
/// success).
pub(crate) async fn drain_absorbed_records<F, Fut>(
    store: &calimero_store::Store,
    context_id: &ContextId,
    replay: F,
) -> Result<usize>
where
    F: Fn(calimero_node_primitives::delta_buffer::BufferedDelta) -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    use calimero_context::group_store::AbsorbRepository;
    use calimero_context::hlc_fence::{loaded_reader_app_key, target_reader_app_key};

    // The schema this node can read *right now*. `None` (non-group context /
    // unresolvable meta) means we cannot tell whether any record is readable,
    // so we drain nothing and leave everything pending.
    let Some(loaded) = loaded_reader_app_key(store, context_id)? else {
        return Ok(0);
    };
    // The migration target (replicated `GroupMeta.app_key`). When the loaded
    // reader has caught up to the target, every record buffered for this
    // migration is verbatim-replayable, including a stale straggler whose schema
    // is behind the loaded reader. Falls back to `loaded` when no group meta is
    // resolvable (then `loaded == target`).
    let target = target_reader_app_key(store, context_id)?.unwrap_or(loaded);

    let repo = AbsorbRepository::new(store);
    let pending = repo.enumerate_pending(context_id)?;

    let mut drained = 0usize;
    for ((producing_app_key, delta_id), record) in pending {
        // Leaf- and snapshot-entity-shaped records have no
        // `__calimero_sync_next` payload — they are not replayable deltas.
        // Skip them; `drain_absorbed_leaves` handles them.
        if record.leaf.is_some() || record.entity.is_some() {
            continue;
        }
        // Drain-ready signal: replay once the node has caught up to the
        // migration TARGET (or the record's schema already matches the loaded
        // reader). Within one migration every buffered delta's schema is
        // <= target, so `loaded == target` means the current wasm can replay
        // all of them, including a stale straggler whose schema is behind the
        // loaded reader. Skip only when not at target AND schema != loaded.
        if loaded != target && Some(loaded) != record.producing_app_key {
            continue;
        }

        let buffered = match record.into_buffered() {
            Ok(b) => b,
            Err(err) => {
                warn!(
                    %context_id,
                    delta_id = ?delta_id,
                    %err,
                    "absorb drain: corrupt AbsorbRecord — cannot reconstruct buffered delta; skipping"
                );
                continue;
            }
        };

        match replay(buffered).await {
            Ok(true) => {
                // Delete only after a successful verbatim replay. Idempotent:
                // the `delta_id` is part of the key, so a crash before this
                // delete just re-replays the survivor (replay is convergent).
                repo.delete(context_id, producing_app_key, delta_id)?;
                drained += 1;
            }
            Ok(false) => {
                debug!(
                    %context_id,
                    delta_id = ?delta_id,
                    "absorb drain: replay declined to consume delta — leaving record pending"
                );
            }
            Err(err) => {
                warn!(
                    %context_id,
                    delta_id = ?delta_id,
                    %err,
                    "absorb drain: verbatim replay failed — leaving record pending for retry"
                );
            }
        }
    }

    Ok(drained)
}

/// Drain the absorb buffer for **every** context that currently holds at least
/// one absorbed straggler delta.
///
/// The active convergence path, mirroring [`drain_all_governance_pending`].
/// Since we can't cheaply tell which context just advanced, we re-evaluate
/// every context with pending absorbs; `drain_absorbed` skips records still
/// behind the loaded reader, so the pass is a no-op for contexts that haven't
/// caught up. Returns immediately when nothing is buffered.
pub(crate) async fn drain_all_absorbed(input: &StateDeltaContext) {
    use calimero_context::group_store::AbsorbRepository;

    let store = input.node_clients.context.datastore();
    let context_ids = match AbsorbRepository::new(store).enumerate_all_contexts() {
        Ok(ids) => ids,
        Err(err) => {
            warn!(%err, "absorb drain: failed to enumerate contexts with pending absorbs");
            return;
        }
    };
    if context_ids.is_empty() {
        return;
    }
    debug!(
        count = context_ids.len(),
        "absorb drain: binary-advance hook draining absorb buffers across contexts"
    );
    for context_id in context_ids {
        drain_absorbed(input, &context_id).await;
    }
}

/// Core startup-recovery mechanics, factored out so the enumerate-then-drain
/// scan is unit-testable without a live WASM executor: `replay` is the same
/// injection seam as [`drain_absorbed_records`] (production passes
/// [`apply_authorized_state_delta`]; tests pass a recording mock).
///
/// The AbsorbBuffer is durable, so straggler deltas persisted before a restart
/// survive. This enumerates every context with a pending absorb and runs
/// [`drain_absorbed_records`] on each. Returns the total drained across all
/// contexts.
pub(crate) async fn recover_absorbed_records<F, Fut>(
    store: &calimero_store::Store,
    replay: F,
) -> Result<usize>
where
    F: Fn(ContextId, calimero_node_primitives::delta_buffer::BufferedDelta) -> Fut + Clone,
    Fut: std::future::Future<Output = Result<bool>>,
{
    use calimero_context::group_store::AbsorbRepository;

    let context_ids = AbsorbRepository::new(store).enumerate_all_contexts()?;
    if context_ids.is_empty() {
        return Ok(0);
    }

    let mut total = 0usize;
    for context_id in context_ids {
        let replay = replay.clone();
        total += drain_absorbed_records(store, &context_id, move |buffered| {
            replay(context_id, buffered)
        })
        .await?;
    }
    Ok(total)
}

/// Startup recovery scan for the durable AbsorbBuffer, run once at boot.
///
/// A node that restarted mid-migration may hold persisted straggler deltas;
/// this re-considers each so none is stranded across the restart. Records the
/// loaded reader can now read are replayed verbatim and deleted; still-behind
/// records are left for the live binary-advance hooks ([`drain_all_absorbed`]).
/// The `context_id` for each record comes from the enumeration key (a
/// `BufferedDelta` does not carry it), so [`recover_absorbed_records`]
/// enumerates contexts first and threads each into the replay. No-op when empty.
pub(crate) async fn recover_absorbed_on_startup(input: &StateDeltaContext) {
    let store = input.node_clients.context.datastore();
    let recovered = recover_absorbed_records(store, |context_id, buffered| {
        let input = input.clone();
        async move {
            let reconstructed = state_delta_message_from_buffered(buffered, context_id);
            // Bypass the fence (same rationale as the live drain) so a stale
            // straggler persisted before the restart applies on startup.
            apply_authorized_state_delta(input, reconstructed, true).await?;
            // Ok(()) covers applied and soft-declined alike — either way it's
            // consumed, so report success and let the record be deleted.
            Ok::<bool, eyre::Report>(true)
        }
    })
    .await;

    match recovered {
        Ok(0) => {}
        Ok(n) => info!(
            recovered = n,
            "absorb recovery: replayed buffered straggler deltas verbatim on startup"
        ),
        Err(err) => warn!(
            %err,
            "absorb recovery: failed to scan absorb buffer on startup"
        ),
    }

    // The delta-replay recovery above skips leaf/entity-shaped records, so a
    // buffered future-schema leaf/entity persisted before a restart would be
    // stranded without this. `drain_absorbed_leaves` handles both leaf- and
    // entity-shaped records, so run it over every context with a pending
    // absorb; it is a no-op for contexts holding only delta records.
    let leaf_contexts = match calimero_context::group_store::AbsorbRepository::new(store)
        .enumerate_all_contexts()
    {
        Ok(ids) => ids,
        Err(err) => {
            warn!(%err, "absorb recovery: failed to enumerate contexts for leaf drain on startup");
            return;
        }
    };
    for context_id in leaf_contexts {
        if let Err(err) = drain_absorbed_leaves(input, &context_id).await {
            warn!(%context_id, %err, "absorb recovery: leaf drain failed on startup");
        }
    }
}

/// Outcome of the gossip-fence evaluation at the state-delta apply chokepoint.
///
/// `Fall` means the delta is readable now (`FenceDecision::Apply`) and the
/// caller must continue normal processing. `Handled` means the fence consumed
/// the delta — it was either absorbed (the migration case) or dropped (the
/// non-migration case) — and the caller must `return Ok(())` without applying.
pub(super) enum FenceOutcome {
    /// Fall through to normal apply.
    Fall,
    /// Fence consumed the delta (absorbed or dropped) — return early.
    Handled,
}

/// Resolve the store-aware [`FenceDecision`] for `producing_app_key` and act on
/// it:
///
/// - [`FenceDecision::Apply`] → [`FenceOutcome::Fall`] (caller applies normally).
/// - [`FenceDecision::Buffer`] (schema differs from the loaded reader after a
///   cascade boundary) → persist the original signed [`BufferedDelta`] into the
///   [`AbsorbBuffer`] for verbatim replay once the binary advances, record the
///   `absorbed_for_migration` metric, return [`FenceOutcome::Handled`].
///   Idempotent: the `delta_id` keys the record, so a re-delivery overwrites.
/// - [`FenceDecision::Drop`] (non-migration fences) → record
///   `fenced_stale_schema` and return [`FenceOutcome::Handled`] without
///   persisting (genuinely unrecoverable).
///
/// `build_buffered` is invoked only on the `Buffer` arm, so the replay-field
/// clone is paid only when an absorb actually happens.
///
/// [`AbsorbBuffer`]: calimero_store::key::AbsorbBufferKey
#[allow(clippy::too_many_arguments)]
pub(super) fn fence_and_maybe_absorb(
    store: &calimero_store::Store,
    context_id: &ContextId,
    producing_app_key: [u8; 32],
    delta_id: [u8; 32],
    author_id: PublicKey,
    delta_hlc: calimero_storage::logical_clock::HybridTimestamp,
    bypass: bool,
    build_buffered: impl FnOnce() -> calimero_node_primitives::delta_buffer::BufferedDelta,
) -> Result<FenceOutcome> {
    use calimero_context::group_store::{AbsorbRecord, AbsorbRepository};
    use calimero_context::hlc_fence::{delta_fence_decision, FenceDecision};

    // Drain-replay bypass: an absorb-drain re-feeds an already-decided straggler
    // through the apply path once the node reached the migration target. The
    // fence must NOT re-evaluate it — a stale straggler would otherwise re-fence
    // to `Buffer` and be re-absorbed instead of applied, never converging.
    // `bypass` short-circuits to `Fall`; the gossip-receive path passes
    // `false` and keeps fencing (the fence is never weakened there).
    if bypass {
        return Ok(FenceOutcome::Fall);
    }

    match delta_fence_decision(store, context_id, producing_app_key, delta_hlc)? {
        FenceDecision::Apply => Ok(FenceOutcome::Fall),
        FenceDecision::Buffer => {
            // Migration case: the receiver's loaded binary can't read this
            // schema yet. Absorb the original signed bytes durably for verbatim
            // replay once the binary advances — never drop, never translate.
            let buffered = build_buffered();
            let record = AbsorbRecord::from_buffered(&buffered);
            AbsorbRepository::new(store).save(context_id, producing_app_key, &record)?;
            info!(
                %context_id,
                %author_id,
                delta_id = ?delta_id,
                producing_app_key = %hex::encode(producing_app_key),
                "Absorbing state delta — loaded reader behind incoming schema; buffered for verbatim replay"
            );
            crate::node_metrics::record_delta_outcome("absorbed_for_migration");
            Ok(FenceOutcome::Handled)
        }
        FenceDecision::Drop => {
            warn!(
                %context_id,
                %author_id,
                delta_id = ?delta_id,
                producing_app_key = %hex::encode(producing_app_key),
                "Dropping state delta — HLC fence: stale schema after cascade migration"
            );
            crate::node_metrics::record_delta_outcome("fenced_stale_schema");
            Ok(FenceOutcome::Handled)
        }
    }
}
