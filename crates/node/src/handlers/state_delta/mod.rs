//! State delta handling for BroadcastMessage::StateDelta
//!
//! **SRP**: This module has ONE job - process state deltas from peers using DAG
use calimero_context::group_store::{membership_status_at, MembershipStatus};
use calimero_context::group_store::{DenyListRepository, NamespaceRepository};
use calimero_context_client::client::ContextClient;
use calimero_context_config::types::GovernancePosition;
use calimero_crypto::Nonce;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::ExecutionEvent;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use eyre::{bail, Result};
use libp2p::PeerId;
use tracing::{debug, info, warn};

use crate::delta_store::DeltaStore;
use crate::utils::choose_stream;

mod crypto;
mod events;
mod verify;

use crypto::{decrypt_delta_actions, lookup_group_key_with_wait, STATE_DELTA_KEY_LOOKUP_WAIT};
use events::{
    emit_state_mutation_event_parsed, execute_cascaded_events, execute_event_handlers_parsed,
    parse_events_payload, CascadeOutcome,
};
pub(crate) use verify::{verify_position_group_id_matches_context, GroupIdCheck};

pub(crate) struct StateDeltaMessage {
    pub(crate) source: PeerId,
    pub(crate) context_id: ContextId,
    pub(crate) author_id: PublicKey,
    pub(crate) delta_id: [u8; 32],
    pub(crate) parent_ids: Vec<[u8; 32]>,
    pub(crate) hlc: calimero_storage::logical_clock::HybridTimestamp,
    pub(crate) root_hash: Hash,
    pub(crate) artifact: Vec<u8>,
    pub(crate) nonce: Nonce,
    pub(crate) events: Option<Vec<u8>>,
    pub(crate) governance_position: Option<GovernancePosition>,
    pub(crate) key_id: [u8; 32],
    pub(crate) delta_signature: Option<[u8; 64]>,
    /// The `GroupMeta.app_key` the sender was executing under. `None` for
    /// non-group contexts or when the sender could not resolve the meta row.
    /// Receivers use this to fence stale-schema deltas.
    pub(crate) producing_app_key: Option<[u8; 32]>,
}

#[derive(Clone)]
pub(crate) struct StateDeltaContext {
    pub(crate) node_clients: crate::NodeClients,
    pub(crate) node_state: crate::NodeState,
    pub(crate) network_client: calimero_network_primitives::client::NetworkClient,
    pub(crate) sync_timeout: std::time::Duration,
}

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
async fn drain_governance_pending(input: &StateDeltaContext, context_id: &ContextId) {
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

/// Reconstruct a [`StateDeltaMessage`] from a [`BufferedDelta`] for re-apply
/// from the governance-pending drain path. Mirrors the borsh decode in
/// [`super::network_event::handle`] — every field that the network handler
/// destructures must be reconstructable here, otherwise drained deltas
/// would replay with missing data.
/// Outcome of the gossip-fence evaluation at the state-delta apply chokepoint.
///
/// `Fall` means the delta is readable now (`FenceDecision::Apply`) and the
/// caller must continue normal processing. `Handled` means the fence consumed
/// the delta — it was either absorbed (the migration case) or dropped (the
/// non-migration case) — and the caller must `return Ok(())` without applying.
enum FenceOutcome {
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
fn fence_and_maybe_absorb(
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

fn state_delta_message_from_buffered(
    buffered: calimero_node_primitives::delta_buffer::BufferedDelta,
    context_id: ContextId,
) -> StateDeltaMessage {
    StateDeltaMessage {
        source: buffered.source_peer,
        context_id,
        author_id: buffered.author_id,
        delta_id: buffered.id,
        parent_ids: buffered.parents,
        hlc: buffered.hlc,
        root_hash: buffered.root_hash,
        artifact: buffered.payload,
        nonce: buffered.nonce,
        events: buffered.events,
        governance_position: buffered.governance_position,
        key_id: buffered.key_id,
        delta_signature: buffered.delta_signature,
        // Carry the stamped producing_app_key through so the HLC fence can still
        // act on a buffered stale-schema delta. `None` only for legacy deltas.
        producing_app_key: buffered.producing_app_key,
    }
}

pub(crate) struct ReplayBufferedDeltaInput {
    pub(crate) context_client: ContextClient,
    pub(crate) node_client: NodeClient,
    pub(crate) node_state: crate::NodeState,
    pub(crate) context_id: ContextId,
    pub(crate) our_identity: PublicKey,
    pub(crate) buffered: calimero_node_primitives::delta_buffer::BufferedDelta,
    pub(crate) sync_timeout: std::time::Duration,
    pub(crate) is_covered_by_checkpoint: bool,
}

/// Handles state delta received from a peer (DAG-based)
///
/// This processes incoming state mutations using a DAG structure.
/// No gap checking - deltas are applied when all parents are available.
///
/// # Flow
/// 1. Validates context exists
/// 2. Reconstructs CausalDelta from broadcast
/// 3. Adds to DeltaStore (applies if parents ready, pends otherwise)
/// 4. Requests missing parents if needed
/// 5. Executes event handlers
/// 6. Re-emits events to WebSocket clients
/// Apply path for an authorized state delta — runs the snapshot-sync buffer
/// check, decryption, DAG insert, handler execution, and heartbeat broadcast.
///
/// Both [`handle_state_delta`] (after the cross-DAG check passes) and
/// [`drain_governance_pending`] (when re-applying a buffered delta whose
/// status is now `Member`) call into this function. Splitting the apply
/// tail off from `handle_state_delta` lets the drain path re-apply directly
/// instead of recursing via `Box::pin(handle_state_delta(...))` — eliminates
/// async recursion, makes the call graph linear, and avoids the per-recurse
/// future allocation.
///
/// `bypass_fence` skips the HLC / absorb fence entirely. The absorb-drain
/// ([`drain_absorbed`] / [`recover_absorbed_on_startup`]) sets it `true`: it
/// has already established the delta is readable, so re-running the fence would
/// re-absorb a stale straggler instead of applying it (infinite no-op). Every
/// other caller passes `false` and keeps fencing.
pub(crate) async fn apply_authorized_state_delta(
    input: StateDeltaContext,
    message: StateDeltaMessage,
    bypass_fence: bool,
) -> Result<()> {
    let StateDeltaContext {
        node_clients,
        node_state,
        network_client,
        sync_timeout,
    } = input;
    let StateDeltaMessage {
        source,
        context_id,
        author_id,
        delta_id,
        parent_ids,
        hlc,
        root_hash,
        artifact,
        nonce,
        events,
        governance_position,
        key_id,
        delta_signature,
        producing_app_key,
    } = message;

    // Per-delta envelope signature verification. Closes the anti-
    // impersonation gap on the delta envelope: even if the sender holds
    // the current group key (so per-action signatures pass) and even if
    // `membership_status_at(author, pos)` returns `Member`, they can't
    // relabel a foreign delta as their own (or claim authorship of a
    // delta someone else wrote) without holding `author_id`'s identity
    // key. Sits BEFORE the cross-DAG check and ReadOnly check because
    // those checks key off `author_id` — there's no point asking
    // "is this author a member?" if we haven't yet established that
    // the claim of authorship is genuine. `None` is tolerated only
    // for legacy rows authored before envelope signing landed; all
    // freshly-signed deltas (every output of `internal_execute`)
    // carry `Some(_)` and MUST verify.
    if let Some(ref sig) = delta_signature {
        if let Err(err) = calimero_node_primitives::sync::delta_auth::verify_delta_signature(
            context_id,
            delta_id,
            author_id,
            governance_position.as_ref(),
            sig,
        ) {
            warn!(
                %context_id,
                %author_id,
                delta_id = ?delta_id,
                %err,
                "Rejecting state delta — envelope signature verification failed"
            );
            return Ok(());
        }
    }

    // HLC fence: fences a delta produced under a schema the receiver's loaded
    // reader can't read AND newer than the cascade boundary. The common
    // chokepoint for direct delivery and the governance-pending drain re-apply.
    // A `None` producing_app_key is unfenceable and falls through. The migration
    // case (`Buffer`) absorbs the original bytes for later verbatim replay;
    // non-migration fences (`Drop`) drop.
    if let Some(producing_app_key) = producing_app_key {
        let outcome = fence_and_maybe_absorb(
            node_clients.context.datastore(),
            &context_id,
            producing_app_key,
            delta_id,
            author_id,
            hlc,
            bypass_fence,
            || calimero_node_primitives::delta_buffer::BufferedDelta {
                id: delta_id,
                parents: parent_ids.clone(),
                hlc,
                payload: artifact.clone(),
                nonce,
                author_id,
                root_hash,
                events: events.clone(),
                source_peer: source,
                key_id,
                governance_position: governance_position.clone(),
                delta_signature,
                governance_drain_attempts: 0,
                producing_app_key: Some(producing_app_key),
            },
        )?;
        if matches!(outcome, FenceOutcome::Handled) {
            return Ok(());
        }
    }

    let Some(context) = node_clients.context.get_context(&context_id)? else {
        bail!("context '{}' not found", context_id);
    };

    // ReadOnly check: rejects authors whose materialized current role is
    // ReadOnly / ReadOnlyTee. Performed inside the apply path so the
    // governance-pending drain path — which calls this function directly
    // when re-applying a buffered delta whose status is now `Member` — gets
    // the same enforcement. Without it, a member who became ReadOnly
    // between the delta being authored and the drain could slip a write
    // through, since the cross-DAG check via `membership_status_at` returns
    // `Member(role)` with a wildcard role that the drain matches against.
    if NamespaceRepository::new(node_clients.context.datastore())
        .is_read_only_for_context(&context_id, &author_id)
        .unwrap_or(false)
    {
        warn!(
            %context_id,
            %author_id,
            "Rejecting state delta from ReadOnly member"
        );
        return Ok(());
    }

    // Check if we should buffer this delta:
    // 1. During snapshot sync (sync session active)
    // 2. When context is uninitialized (can't decrypt without sender key)
    let is_uninitialized = context.root_hash == Hash::default();
    let should_buffer = node_state.should_buffer_delta(&context_id) || is_uninitialized;

    if should_buffer {
        info!(
            %context_id,
            delta_id = ?delta_id,
            is_uninitialized,
            has_events = events.is_some(),
            "Buffering delta (sync in progress or context uninitialized)"
        );

        let buffered = calimero_node_primitives::delta_buffer::BufferedDelta {
            id: delta_id,
            parents: parent_ids.clone(),
            hlc,
            payload: artifact.clone(),
            nonce,
            author_id,
            root_hash,
            events: events.clone(),
            source_peer: source,
            key_id,
            governance_position: governance_position.clone(),
            delta_signature,
            governance_drain_attempts: 0,
            producing_app_key,
        };

        if let Some(result) = node_state.buffer_delta(&context_id, buffered) {
            // Delta was handled by the buffer (added, evicted, or duplicate)
            // Only return early if it was successfully added or was a duplicate
            if result.was_added()
                || matches!(
                    result,
                    calimero_node_primitives::delta_buffer::PushResult::Duplicate
                )
            {
                return Ok(()); // Successfully buffered, will be replayed after sync
            }
            // If dropped due to zero capacity, fall through to normal processing
        }

        // No active session - if context is uninitialized, we must
        // start a session to buffer this delta
        if is_uninitialized && !node_state.should_buffer_delta(&context_id) {
            // Start a temporary buffer session for uninitialized context
            node_state.start_sync_session(context_id, hlc.get_time().as_u64());

            let buffered = calimero_node_primitives::delta_buffer::BufferedDelta {
                id: delta_id,
                parents: parent_ids.clone(),
                hlc,
                payload: artifact.clone(),
                nonce,
                author_id,
                root_hash,
                events: events.clone(),
                source_peer: source,
                key_id,
                governance_position: governance_position.clone(),
                delta_signature,
                governance_drain_attempts: 0,
                producing_app_key,
            };

            if let Some(result) = node_state.buffer_delta(&context_id, buffered) {
                if result.was_added()
                    || matches!(
                        result,
                        calimero_node_primitives::delta_buffer::PushResult::Duplicate
                    )
                {
                    info!(
                        %context_id,
                        delta_id = ?delta_id,
                        "Started buffer session for uninitialized context"
                    );
                    return Ok(());
                }
            }
        }

        warn!(
            %context_id,
            delta_id = ?delta_id,
            "Delta buffer full or zero capacity, proceeding with normal processing (may fail)"
        );
        // Fall through to normal processing
    }

    let group_key = {
        let store = node_clients.context.datastore();
        let gid = calimero_context::group_store::get_group_for_context(store, &context_id)?;
        match gid {
            Some(g) => {
                // Issue #2256: an `Open` subgroup encrypts state deltas
                // with the parent namespace's key (see `execute/mod.rs`
                // for the publisher choice). Try the subgroup keyring
                // first (Restricted case), then fall back to the
                // namespace keyring (Open case). Same shape as the
                // governance-op decrypt fallback in
                // `namespace_governance::apply_namespace_op`.
                //
                // Issue #2299: poll the group store for up to 3s if
                // the key isn't found — closes the race widened by
                // running StateDelta on a separate Arbiter. Store
                // errors from `resolve_namespace` (cyclic parent
                // edges, missing namespace meta) still propagate.
                lookup_group_key_with_wait(
                    &node_clients.context,
                    &g,
                    &key_id,
                    STATE_DELTA_KEY_LOOKUP_WAIT,
                )
                .await?
                .ok_or_else(|| {
                    eyre::eyre!("no group key found for key_id {}", hex::encode(key_id))
                })?
            }
            None => {
                let identity = node_clients
                    .context
                    .get_identity(&context_id, &author_id)?
                    .ok_or_else(|| eyre::eyre!("no identity for author {author_id}"))?;
                identity
                    .sender_key
                    .ok_or_else(|| eyre::eyre!("no sender_key or group_key for context"))?
            }
        }
    };

    let actions = decrypt_delta_actions(artifact, nonce, group_key)?;

    let delta = calimero_dag::CausalDelta {
        id: delta_id,
        parents: parent_ids,
        payload: actions,
        hlc,
        expected_root_hash: *root_hash,
        kind: calimero_dag::DeltaKind::Regular,
    };

    let our_identity = choose_owned_identity(&node_clients.context, &context_id).await?;

    // Check if this is our own delta (gossipsub echoes back to sender).
    // Self-authored deltas are already applied locally, so we should NOT re-apply them.
    // This prevents state divergence from double-application of actions.
    let is_self_authored = author_id == our_identity;
    if is_self_authored {
        debug!(
            %context_id,
            %author_id,
            delta_id = ?delta_id,
            "Skipping self-authored delta (already applied locally)"
        );
        // Still emit events to WebSocket clients for consistency
        let events_payload = parse_events_payload(&events, &context_id);
        if let Some(payload) = events_payload {
            emit_state_mutation_event_parsed(&node_clients.node, &context_id, root_hash, payload);
        }
        return Ok(());
    }

    // Check if application is available BEFORE applying the delta.
    // If not available, bail early so the delta can be retried later when rebroadcast.
    // This prevents the scenario where we apply the delta but skip handlers because
    // the application blob hasn't finished downloading yet.
    if let Err(e) = ensure_application_available(
        &node_clients.node,
        &node_clients.context,
        &context_id,
        sync_timeout,
    )
    .await
    {
        bail!(
            "Application not available for context {} - delta will be retried on rebroadcast: {}",
            context_id,
            e
        );
    }

    let DeltaStoreSetup {
        store: delta_store_ref,
        is_uninitialized,
    } = init_delta_store(
        &node_state,
        &node_clients,
        context_id,
        our_identity,
        context.root_hash,
        sync_timeout,
    )
    .await?;

    // Thread the envelope's author + governance position into the
    // delta store so the persisted `ContextDagDelta` row carries the
    // claim. Subsequent DAG-catchup serves from this node will then
    // include the author info, letting the receiving initiator run
    // the same `membership_status_at` check the gossip path ran here.
    let governance_position_blob = governance_position
        .as_ref()
        .and_then(|gp| borsh::to_vec(gp).ok());
    // Persist the wire-received `delta_signature` (verified above)
    // so subsequent DAG-catchup serves from this node include the
    // envelope signature. Without this, the anti-impersonation
    // property the signature provides only holds for the originating
    // node — every relay would drop the signature and downstream
    // peers couldn't verify.
    let add_result = delta_store_ref
        .add_delta_with_events(
            delta,
            events.clone(),
            Some(author_id),
            governance_position_blob.clone(),
            delta_signature,
        )
        .await?;
    let mut applied = add_result.applied;
    let mut handlers_already_executed = false;

    if !applied {
        let missing_result = delta_store_ref.get_missing_parents().await;

        // `execute_cascaded_events` folds every failure into a `warn!` + `Ok`
        // (see its doc comment), so this match-and-log is the policy, not the
        // exception path. Crucially it is NOT `?`: a cascade error must never
        // unwind `handle_state_delta` after the DAG has already been mutated —
        // failed handlers keep their events in the DB for replay on next init.
        let cascade_outcome = match execute_cascaded_events(
            &missing_result.cascaded_events,
            &node_clients.node,
            &node_clients.context,
            &context_id,
            &our_identity,
            sync_timeout,
            "missing parent check",
            Some(&delta_id),
            &delta_store_ref,
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(e) => {
                warn!(
                    ?e,
                    %context_id,
                    "Cascade handler execution failed during missing-parent check; events stay in DB for next init"
                );
                CascadeOutcome::default()
            }
        };
        applied |= cascade_outcome.applied_current;
        handlers_already_executed |= cascade_outcome.handlers_executed_for_current;

        // Events-less deltas that the cascade applied to the DAG are not
        // present in `cascade_outcome.cascaded_events` (that collector only
        // surfaces deltas with persisted events to run handlers for), so
        // `applied_current` stays false even though the DAG state reflects
        // a successful apply. Check `missing_result.cascaded_ids` (the
        // full set of cascaded deltas produced by `get_missing_parents`,
        // including events-less ones) instead of re-acquiring the DAG
        // read lock via `dag_has_delta_applied`.
        if !applied && missing_result.cascaded_ids.contains(&delta_id) {
            info!(
                %context_id,
                delta_id = ?delta_id,
                "Delta was applied via cascade - will execute handlers"
            );
            applied = true;

            if !handlers_already_executed && events.is_some() {
                info!(
                    %context_id,
                    delta_id = ?delta_id,
                    "Delta cascaded via alternate path - handlers will be executed in main flow"
                );
            }
        }

        if !missing_result.missing_ids.is_empty() {
            warn!(
                %context_id,
                missing_count = missing_result.missing_ids.len(),
                context_is_uninitialized = is_uninitialized,
                has_events = events.is_some(),
                "Delta pending due to missing parents - requesting them from peer"
            );

            let datastore_for_fetch = node_clients.context.datastore_handle().into_inner();
            match request_missing_deltas(
                network_client,
                sync_timeout,
                context_id,
                missing_result.missing_ids,
                source,
                our_identity,
                delta_store_ref.clone(),
                datastore_for_fetch,
            )
            .await
            {
                Ok(peer_fetch_cascaded_events) => {
                    // Peer-fetched parents can cascade pending children via
                    // `apply_pending` inside `add_delta_with_events`. Those
                    // cascaded children's events were discarded before this
                    // fix — now they ride back on `peer_fetch_cascaded_events`
                    // and go through `execute_cascaded_events` exactly like
                    // the cascade path inside `get_missing_parents`.
                    if !peer_fetch_cascaded_events.is_empty() {
                        // Same log-and-continue policy as the missing-parent
                        // cascade above: never let a cascade error propagate
                        // and abort the request after the DAG is mutated.
                        let cascade_outcome = match execute_cascaded_events(
                            &peer_fetch_cascaded_events,
                            &node_clients.node,
                            &node_clients.context,
                            &context_id,
                            &our_identity,
                            sync_timeout,
                            "peer-fetch cascade",
                            Some(&delta_id),
                            &delta_store_ref,
                        )
                        .await
                        {
                            Ok(outcome) => outcome,
                            Err(e) => {
                                warn!(
                                    ?e,
                                    %context_id,
                                    "Cascade handler execution failed during peer-fetch cascade; events stay in DB for next init"
                                );
                                CascadeOutcome::default()
                            }
                        };
                        applied |= cascade_outcome.applied_current;
                        handlers_already_executed |= cascade_outcome.handlers_executed_for_current;
                    }
                }
                Err(e) => {
                    warn!(?e, %context_id, ?source, "Failed to request missing deltas");
                }
            }

            // Some peer-fetched cascades may still apply the current delta
            // without having its events in the DB (events-less deltas are
            // never pre-persisted, so they won't show up in
            // `peer_fetch_cascaded_events`). The DAG state reflects the
            // apply regardless; check it before falling through to the
            // "still pending" path so we don't warn misleadingly.
            if !applied && delta_store_ref.dag_has_delta_applied(&delta_id).await {
                info!(
                    %context_id,
                    delta_id = ?delta_id,
                    "Delta was applied via cascade after peer-fetch of missing parents"
                );
                applied = true;
            }
        } else if !applied {
            // Parent is already in the database but `get_missing_parents`'s
            // explicit cascade didn't unblock this delta either. Rare, but
            // can happen if the DAG apply path itself returns an error for
            // the child. Left pending to retry on the next sync cycle.
            warn!(
                %context_id,
                delta_id = ?delta_id,
                has_events = events.is_some(),
                "Delta pending - parents exist but child did not apply during cascade"
            );
        }
    }

    let events_payload = parse_events_payload(&events, &context_id);

    // A present-but-undeserializable events blob will never parse on any
    // future restart. Clear it once the delta is applied so
    // `load_persisted_deltas` doesn't resurface it on every boot in a
    // permanent warn-and-skip loop — mirrors the deserialization-error
    // path in `execute_cascaded_events`. (`events == None` is the normal
    // "no events" case and is left untouched.)
    if applied && events.is_some() && events_payload.is_none() {
        warn!(
            %context_id,
            delta_id = ?delta_id,
            "Events blob failed to deserialize; clearing to prevent a permanent restart replay loop"
        );
        delta_store_ref.mark_events_executed(&delta_id);
    }

    if applied && !handlers_already_executed {
        if let Some(ref payload) = events_payload {
            let is_author = author_id == our_identity;
            info!(
                %context_id,
                %author_id,
                %our_identity,
                is_author,
                "Evaluating event handler execution for applied delta"
            );
            if !is_author {
                // Application availability was already verified at the start of this function,
                // so we can safely execute handlers without re-checking.
                let all_succeeded = execute_event_handlers_parsed(
                    &node_clients.context,
                    &context_id,
                    &our_identity,
                    payload,
                )
                .await?;

                // Clear the DB's `events` blob once every handler ran
                // successfully (#2185, #2194 review). Partial failure
                // leaves the blob for restart replay. `add_delta_internal`
                // preserves `events: Some(..)` when a delta is directly
                // applied, so this clear is the normal termination
                // point for the direct-apply path.
                if all_succeeded {
                    delta_store_ref.mark_events_executed(&delta_id);
                } else {
                    warn!(
                        %context_id,
                        delta_id = ?delta_id,
                        "One or more handlers failed on direct-apply path; keeping events in DB for restart replay"
                    );
                }
            } else {
                info!(
                    %context_id,
                    %author_id,
                    "Skipping event handler execution (we are the author node)"
                );
                // Author already ran handlers locally at authoring time,
                // so there is nothing to replay. Clear the preserved
                // `events: Some(..)` blob so `load_persisted_deltas` on
                // restart doesn't surface it as "pending handler events"
                // and mistakenly re-run handlers the author deliberately
                // skipped (#2194 review, High).
                delta_store_ref.mark_events_executed(&delta_id);
            }
        }
    } else if !applied && events_payload.is_some() {
        warn!(
            %context_id,
            delta_id = ?delta_id,
            "Delta with events buffered as pending - handlers will NOT execute when delta is applied later!"
        );
    }

    if let Some(payload) = events_payload {
        emit_state_mutation_event_parsed(&node_clients.node, &context_id, root_hash, payload);
    }

    // Same log-and-continue policy: a cascade failure here must not abort the
    // handler after the main delta has already been applied and emitted.
    if let Err(e) = execute_cascaded_events(
        &add_result.cascaded_events,
        &node_clients.node,
        &node_clients.context,
        &context_id,
        &our_identity,
        sync_timeout,
        "dag cascade",
        None,
        &delta_store_ref,
    )
    .await
    {
        warn!(
            ?e,
            %context_id,
            "Cascade handler execution failed during dag cascade; events stay in DB for next init"
        );
    }

    // After successfully applying a remote delta, immediately broadcast our
    // updated root hash so lagging peers detect the divergence without waiting
    // for the 30-second periodic heartbeat.
    if applied {
        if let Ok(Some(ctx)) = node_clients.context.get_context(&context_id) {
            if !ctx.root_hash.is_zero() {
                let _ = node_clients
                    .node
                    .broadcast_heartbeat(&context_id, ctx.root_hash, ctx.dag_heads.clone())
                    .await;
            }
        }
    }

    Ok(())
}

pub async fn handle_state_delta(
    input: StateDeltaContext,
    message: StateDeltaMessage,
) -> Result<()> {
    let StateDeltaContext {
        node_clients,
        node_state,
        network_client,
        sync_timeout,
    } = input;
    let StateDeltaMessage {
        source,
        context_id,
        author_id,
        delta_id,
        parent_ids,
        hlc,
        root_hash,
        artifact,
        nonce,
        events,
        governance_position,
        key_id,
        delta_signature,
        producing_app_key,
    } = message;

    let Some(context) = node_clients.context.get_context(&context_id)? else {
        bail!("context '{}' not found", context_id);
    };

    // Fast-path ReadOnly rejection — `apply_authorized_state_delta` also
    // performs this check (so the governance-pending drain path is
    // covered), but doing it here too avoids paying for drain plus the
    // cross-DAG membership lookup on a delta we'll reject anyway.
    if NamespaceRepository::new(node_clients.context.datastore())
        .is_read_only_for_context(&context_id, &author_id)
        .unwrap_or(false)
    {
        warn!(
            %context_id,
            %author_id,
            "Rejecting state delta from ReadOnly member"
        );
        return Ok(());
    }

    // Per-group deny-list filter. Populated when `MemberRemoved` /
    // `MemberLeft` apply locally; cleared when `MemberAdded` /
    // `MemberJoinedViaTeeAttestation` apply for the same member. This
    // is a cheap early-rejection layer in front of the cross-DAG
    // membership check — that check is still authoritative (a removed
    // member would be rejected there too), but the deny-list lookup is
    // O(1) and saves the drain + prefix-walk cost for traffic from
    // peers we've already explicitly removed.
    //
    // Skipped for non-group contexts (`is_author_denied_for_context`
    // returns `Ok(false)` when there's no owning group). Lookup
    // failures fall through to the cross-DAG check rather than
    // erroring; a transient store error here shouldn't drop a
    // legitimate delta when the authoritative check would still apply.
    // The failure is logged at warn level so storage corruption
    // affecting the deny prefix surfaces in monitoring instead of
    // silently degrading the defense-in-depth layer.
    //
    // Drain path note: `drain_governance_pending` calls
    // `apply_authorized_state_delta` directly and so bypasses this
    // entry-point filter. That's intentional. A buffered delta
    // carries the sender's pre-buffering `governance_position`; the
    // cross-DAG check inside the apply path consults that position,
    // and the forward-only invariant means pre-removal positions
    // correctly resolve to `Member`. The deny-list at the entry point
    // exists to short-circuit the buffer/drain/check pipeline for
    // post-removal traffic; the drain path operates on already-buffered
    // deltas whose authorization is decided by the cross-DAG check
    // against the original position, which is the authoritative
    // outcome.
    let denied = DenyListRepository::new(node_clients.context.datastore())
        .is_author_denied_for_context(&context_id, &author_id)
        .unwrap_or_else(|err| {
            warn!(
                %context_id,
                %author_id,
                ?err,
                "Deny-list lookup failed, falling through to cross-DAG check"
            );
            false
        });
    if denied {
        warn!(
            %context_id,
            %author_id,
            "Rejecting state delta from deny-listed member"
        );
        return Ok(());
    }

    info!(
        %context_id,
        %author_id,
        delta_id = ?delta_id,
        parent_count = parent_ids.len(),
        expected_root_hash = %root_hash,
        current_root_hash = %context.root_hash,
        governance_dag_heads = governance_position
            .as_ref()
            .map(|p| p.governance_dag_heads.len())
            .unwrap_or(0),
        "Received state delta"
    );

    // Drain governance-pending buffer for this context. Each pending
    // delta is re-evaluated against current local governance state; if the
    // signer's status is now decidable, the delta is re-applied (Member)
    // or rejected (Removed/NeverMember/Err). If still Unknown, push it
    // back. Doing this on every state-delta receive guarantees the buffer
    // self-clears as governance traffic catches up, without requiring a
    // notification path from the governance-apply code into this handler.
    let drain_input = StateDeltaContext {
        node_clients: node_clients.clone(),
        node_state: node_state.clone(),
        network_client: network_client.clone(),
        sync_timeout,
    };
    drain_governance_pending(&drain_input, &context_id).await;

    // Apply-time cross-DAG membership check. If the delta carries a
    // `governance_position`, ask `membership_status_at` whether `author_id`
    // was a member at the named cut. Reject ineligible ops; buffer when
    // governance state hasn't caught up; otherwise fall through to the
    // existing apply path.
    //
    // Anti-bypass: see [`GroupIdCheck`] for the two bypasses this match
    // closes (mismatched group_id on a signed position, and lying about
    // being a non-group context). Single store lookup covers both
    // position-present and position-absent cases.
    //
    // INVARIANT: `ContextManager` serializes governance ops, so
    // no concurrent group reassignment can interleave between this
    // check and the `membership_status_at` call below — see the
    // TOCTOU note on `verify_position_group_id_matches_context`.
    let datastore = node_clients.context.datastore();
    match verify_position_group_id_matches_context(
        datastore,
        &context_id,
        governance_position.as_ref().map(|p| p.group_id),
    ) {
        GroupIdCheck::NonGroupOk => {
            // Legacy non-group context with no claimed group. Fall through.
        }
        GroupIdCheck::Match => {
            // Position's group matches the context's owning group. Fall
            // through to the membership-status check below.
        }
        GroupIdCheck::GroupContextNoPosition { owning } => {
            warn!(
                %context_id,
                %author_id,
                owning_group = ?owning,
                delta_id = ?delta_id,
                "cross-DAG check: rejecting state delta — group context but no \
                 governance_position (likely a malicious bypass attempt)"
            );
            return Ok(());
        }
        GroupIdCheck::NonGroupContextWithPosition { claimed } => {
            warn!(
                %context_id,
                %author_id,
                claimed_group = ?claimed,
                delta_id = ?delta_id,
                "cross-DAG check: rejecting state delta — governance_position present \
                 but context is not part of any group"
            );
            return Ok(());
        }
        GroupIdCheck::Mismatch { owning, claimed } => {
            warn!(
                %context_id,
                %author_id,
                owning_group = ?owning,
                claimed_group = ?claimed,
                delta_id = ?delta_id,
                "cross-DAG check: rejecting state delta — governance_position references \
                 a different group than the context's owning group"
            );
            return Ok(());
        }
        GroupIdCheck::LookupError(err) => {
            warn!(
                %context_id,
                %author_id,
                %err,
                "cross-DAG check: get_group_for_context failed; rejecting delta to avoid silent bypass"
            );
            return Ok(());
        }
    }

    if let Some(pos) = governance_position.as_ref() {
        // Forward-only invariant — load-bearing. The argument passed
        // to `membership_status_at` is the delta's *signed* governance
        // position (carried inside the delta envelope by the author at
        // sign time), NOT the receiver's current local state. Pre-cut
        // writes from a now-removed author resolve to `Member` here
        // even on a receiver whose local DAG has already applied the
        // removal — without this property, two peers that observe the
        // `MemberRemoved` op in different orders relative to the
        // pre-removal delta would diverge. Swapping this argument for
        // current state or any post-cut heuristic reintroduces that
        // divergence and turns valid pre-removal writes into rejected
        // ones retroactively. Tests pinning this behavior live at
        // `crates/context/src/group_store/membership_status.rs`.
        match membership_status_at(datastore, &author_id, pos) {
            Ok(MembershipStatus::Member(role)) => {
                tracing::debug!(
                    %context_id,
                    %author_id,
                    role = ?role,
                    group_id = ?pos.group_id,
                    "cross-DAG check: author authorized at governance cut"
                );
                // Record the (peer, identity) pair now that we know the
                // signature verified AND the author is an authorized
                // member at the named cut. Consumed by anchor-preferred
                // sync peer selection. See `NodeState::peer_identities`.
                node_state.observe_peer_identity(source, author_id);
            }
            Ok(MembershipStatus::Removed { last_role }) => {
                warn!(
                    %context_id,
                    %author_id,
                    last_role = ?last_role,
                    group_id = ?pos.group_id,
                    "cross-DAG check: rejecting state delta — author was removed from group at governance cut"
                );
                return Ok(());
            }
            Ok(MembershipStatus::NeverMember) => {
                warn!(
                    %context_id,
                    %author_id,
                    group_id = ?pos.group_id,
                    "cross-DAG check: rejecting state delta — author is not a member of the group at governance cut"
                );
                return Ok(());
            }
            Ok(MembershipStatus::Unknown { needed }) => {
                info!(
                    %context_id,
                    %author_id,
                    group_id = ?pos.group_id,
                    needed_count = needed.len(),
                    "cross-DAG check: governance state behind position; buffering delta until catchup"
                );
                let buffered = calimero_node_primitives::delta_buffer::BufferedDelta {
                    id: delta_id,
                    parents: parent_ids.clone(),
                    hlc,
                    payload: artifact.clone(),
                    nonce,
                    author_id,
                    root_hash,
                    events: events.clone(),
                    source_peer: source,
                    key_id,
                    governance_position: governance_position.clone(),
                    delta_signature,
                    governance_drain_attempts: 0,
                    producing_app_key,
                };
                node_state.buffer_governance_pending(context_id, buffered);
                return Ok(());
            }
            Err(err) => {
                warn!(
                    %context_id,
                    %author_id,
                    group_id = ?pos.group_id,
                    %err,
                    "cross-DAG check: rejecting state delta — membership lookup failed (hash mismatch / corruption)"
                );
                return Ok(());
            }
        }
    }

    // Cross-DAG check passed (or no governance_position to check). Hand off
    // to the apply path. Reassembling the input/message here lets the apply
    // path stay a flat top-level function callable directly from the
    // governance-pending drain on re-apply, instead of recursing into this
    // handler.
    apply_authorized_state_delta(
        StateDeltaContext {
            node_clients,
            node_state,
            network_client,
            sync_timeout,
        },
        StateDeltaMessage {
            source,
            context_id,
            author_id,
            delta_id,
            parent_ids,
            hlc,
            root_hash,
            artifact,
            nonce,
            events,
            governance_position,
            key_id,
            delta_signature,
            // Carry the stamped producing_app_key through to the apply path,
            // where the fence reads it. Orthogonal to the cross-DAG check above.
            producing_app_key,
        },
        // Gossip-receive path: fence as normal — never bypass.
        false,
    )
    .await
}

struct DeltaStoreSetup {
    store: DeltaStore,
    is_uninitialized: bool,
}

async fn choose_owned_identity(
    context_client: &ContextClient,
    context_id: &ContextId,
) -> Result<PublicKey> {
    let identities = context_client.get_context_members(context_id, Some(true));
    let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
        .await
        .transpose()?
    else {
        bail!("no owned identities found for context: {}", context_id);
    };

    Ok(our_identity)
}

async fn init_delta_store(
    node_state: &crate::NodeState,
    node_clients: &crate::NodeClients,
    context_id: ContextId,
    our_identity: PublicKey,
    root_hash: Hash,
    sync_timeout: std::time::Duration,
) -> Result<DeltaStoreSetup> {
    let is_uninitialized = root_hash == Hash::default();

    let (delta_store_ref, is_new_store) = {
        let mut is_new = false;
        let delta_store = node_state
            .delta_stores
            .entry(context_id)
            .or_insert_with(|| {
                is_new = true;
                DeltaStore::new(
                    [0u8; 32],
                    node_clients.context.clone(),
                    context_id,
                    our_identity,
                )
            });

        (delta_store.clone(), is_new)
    };

    if is_new_store {
        let init_result = async {
            // `load_persisted_deltas` surfaces any records with
            // `applied: true, events: Some(..)` — crash-leftovers
            // whose handlers never completed. Merged with the normal
            // cascade events below so a single handler pass covers both
            // (#2185). Share the DB scan with the DAG restore to avoid
            // a second full-table iteration (#2194 review).
            let pending_handler_events = match delta_store_ref.load_persisted_deltas().await {
                Ok(result) => {
                    if !result.pending_handler_events.is_empty() {
                        info!(
                            %context_id,
                            pending_count = result.pending_handler_events.len(),
                            "Replaying handlers interrupted by crash before events were cleared"
                        );
                    }
                    result.pending_handler_events
                }
                Err(e) => {
                    warn!(
                        ?e,
                        %context_id,
                        "Failed to load persisted deltas, starting with empty DAG"
                    );
                    Vec::new()
                }
            };

            let missing_result = delta_store_ref.get_missing_parents().await;
            if !missing_result.missing_ids.is_empty() {
                warn!(
                    %context_id,
                    missing_count = missing_result.missing_ids.len(),
                    "Missing parents after loading persisted deltas - will request from network"
                );
            }

            // The two sources are disjoint by construction:
            // `pending_handler_events` are records that were `applied:
            // true` on disk before this init ran, so they're restored
            // into the DAG as already-applied by `load_persisted_deltas`
            // and can't show up in `get_missing_parents`'s
            // pending→applied diff. Concat directly.
            let mut events_to_run = missing_result.cascaded_events;
            events_to_run.extend(pending_handler_events);

            execute_cascaded_events(
                &events_to_run,
                &node_clients.node,
                &node_clients.context,
                &context_id,
                &our_identity,
                sync_timeout,
                "initial load",
                None,
                &delta_store_ref,
            )
            .await
        }
        .await;

        if let Err(err) = init_result {
            warn!(
                %context_id,
                ?err,
                "Initial delta store setup failed - removing store to retry on next delta"
            );
            // Remove the store so the next delta triggers a fresh init with retry
            node_state.delta_stores.remove(&context_id);
            return Err(err);
        }
    }

    Ok(DeltaStoreSetup {
        store: delta_store_ref,
        is_uninitialized,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_crypto::{SharedKey, NONCE_LEN};
    use calimero_storage::delta::StorageDelta;
    use rand::thread_rng;

    #[test]
    fn parse_events_payload_success() {
        let events = vec![ExecutionEvent {
            kind: "test".to_string(),
            data: vec![1, 2, 3],
            handler: Some("handler_fn".to_string()),
        }];
        let serialized = serde_json::to_vec(&events).expect("serialization should succeed");

        // Should deserialize valid event JSON
        let parsed = parse_events_payload(&Some(serialized), &ContextId::zero())
            .expect("events should parse");

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].kind, "test");
        assert_eq!(parsed[0].handler.as_deref(), Some("handler_fn"));
    }

    #[test]
    fn parse_events_payload_invalid() {
        // Invalid JSON should be rejected gracefully
        let parsed = parse_events_payload(&Some(b"not-json".to_vec()), &ContextId::zero());
        assert!(parsed.is_none());
    }

    // ---- verify_position_group_id_matches_context ----
    //
    // Covers every variant of `GroupIdCheck` so a regression in the
    // match arms (swapping `NonGroupOk` and `Match`, mishandling
    // `(None, Some)` or `(Some, None)`, etc.) gets caught at test
    // time rather than at apply time.
    mod group_id_check_tests {
        use std::sync::Arc;

        use calimero_context_config::types::ContextGroupId;
        use calimero_store::db::InMemoryDB;
        use calimero_store::Store;

        use super::super::{verify_position_group_id_matches_context, GroupIdCheck};
        use super::*;

        fn fresh_store() -> Store {
            Store::new(Arc::new(InMemoryDB::owned()))
        }

        fn ctx(byte: u8) -> ContextId {
            ContextId::from([byte; 32])
        }

        fn gid(byte: u8) -> ContextGroupId {
            ContextGroupId::from([byte; 32])
        }

        #[test]
        fn non_group_ok_when_context_has_no_group_and_no_position() {
            let store = fresh_store();
            let context_id = ctx(0xA1);

            // No `register_context_in_group` call — context isn't
            // part of any group. With no claimed group, the legacy
            // non-group path applies.
            let result = verify_position_group_id_matches_context(&store, &context_id, None);
            assert!(matches!(result, GroupIdCheck::NonGroupOk));
        }

        #[test]
        fn match_when_context_owning_group_equals_claimed() {
            let store = fresh_store();
            let context_id = ctx(0xB1);
            let group_id = gid(0xB2);

            calimero_context::group_store::register_context_in_group(
                &store,
                &group_id,
                &context_id,
            )
            .expect("register_context_in_group");

            let result =
                verify_position_group_id_matches_context(&store, &context_id, Some(group_id));
            assert!(matches!(result, GroupIdCheck::Match));
        }

        #[test]
        fn group_context_no_position_when_context_in_group_but_position_absent() {
            let store = fresh_store();
            let context_id = ctx(0xC1);
            let group_id = gid(0xC2);

            calimero_context::group_store::register_context_in_group(
                &store,
                &group_id,
                &context_id,
            )
            .expect("register_context_in_group");

            // Group context but the delta carries no position.
            let result = verify_position_group_id_matches_context(&store, &context_id, None);
            match result {
                GroupIdCheck::GroupContextNoPosition { owning } => {
                    assert_eq!(owning, group_id);
                }
                other => panic!("expected GroupContextNoPosition, got {other:?}"),
            }
        }

        #[test]
        fn non_group_context_with_position_when_context_has_no_group_but_position_set() {
            let store = fresh_store();
            let context_id = ctx(0xD1);
            let claimed = gid(0xD2);

            // Context is not registered under any group, but the delta
            // claims one — bypass attempt shape.
            let result =
                verify_position_group_id_matches_context(&store, &context_id, Some(claimed));
            match result {
                GroupIdCheck::NonGroupContextWithPosition { claimed: c } => {
                    assert_eq!(c, claimed);
                }
                other => panic!("expected NonGroupContextWithPosition, got {other:?}"),
            }
        }

        #[test]
        fn mismatch_when_context_owning_group_differs_from_claimed() {
            let store = fresh_store();
            let context_id = ctx(0xE1);
            let owning = gid(0xE2);
            let claimed = gid(0xE3);

            calimero_context::group_store::register_context_in_group(&store, &owning, &context_id)
                .expect("register_context_in_group");

            let result =
                verify_position_group_id_matches_context(&store, &context_id, Some(claimed));
            match result {
                GroupIdCheck::Mismatch {
                    owning: o,
                    claimed: c,
                } => {
                    assert_eq!(o, owning);
                    assert_eq!(c, claimed);
                }
                other => panic!("expected Mismatch, got {other:?}"),
            }
        }
    }

    // ---- HLC fence (PR-3): the guard the receive path calls ----
    //
    // Exercises `calimero_context::hlc_fence::delta_is_fenced` against a
    // store shaped exactly as the receive path sees it after a cascade
    // migration (group meta `app_key` = current target + a completed
    // upgrade row carrying `cascade_hlc`). Mirrors `group_id_check_tests`'
    // store setup so a regression in the fence resolution (wrong app_key
    // source, missing cascade boundary read) is caught here rather than at
    // apply time.
    mod fence_drop_tests {
        use std::sync::Arc;

        use calimero_context::group_store::{
            register_context_in_group, MetaRepository, UpgradesRepository,
        };
        use calimero_context::hlc_fence::delta_is_fenced;
        use calimero_context_config::types::ContextGroupId;
        use calimero_primitives::application::ApplicationId;
        use calimero_primitives::context::{ContextId, UpgradePolicy};
        use calimero_primitives::identity::PublicKey;
        use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
        use calimero_store::db::InMemoryDB;
        use calimero_store::key::{GroupMetaValue, GroupUpgradeStatus, GroupUpgradeValue};
        use calimero_store::Store;
        use core::num::NonZeroU128;

        // App-schema keys: v1 is the *pre*-cascade schema, v2 is the schema
        // the context now targets after the migration.
        const APP_V1: [u8; 32] = [0x11; 32];
        const APP_V2: [u8; 32] = [0x22; 32];

        fn fresh_store() -> Store {
            Store::new(Arc::new(InMemoryDB::owned()))
        }

        /// Build a store whose context [0xF1;32] lives under group [0xF2;32],
        /// targets app schema v2, and — when `boundary` is `Some` — has a
        /// completed cascade upgrade recording that HLC as the fence boundary.
        fn cascaded_store(boundary: Option<HybridTimestamp>) -> (Store, ContextId) {
            let store = fresh_store();
            let context_id = ContextId::from([0xF1; 32]);
            let group_id = ContextGroupId::from([0xF2; 32]);

            register_context_in_group(&store, &group_id, &context_id)
                .expect("register_context_in_group");

            let dummy_pk = PublicKey::from([0xAB; 32]);
            MetaRepository::new(&store)
                .save(
                    &group_id,
                    &GroupMetaValue {
                        app_key: APP_V2,
                        target_application_id: ApplicationId::from([0xCC; 32]),
                        upgrade_policy: UpgradePolicy::Automatic,
                        created_at: 1_700_000_000,
                        admin_identity: dummy_pk,
                        owner_identity: dummy_pk,
                        migration: None,
                        auto_join: false,
                    },
                )
                .expect("save group meta");

            if let Some(cascade_hlc) = boundary {
                UpgradesRepository::new(&store)
                    .save(
                        &group_id,
                        &GroupUpgradeValue {
                            from_version: "1.0.0".to_owned(),
                            to_version: "2.0.0".to_owned(),
                            migration: None,
                            initiated_at: 1_700_000_000,
                            initiated_by: dummy_pk,
                            status: GroupUpgradeStatus::Completed { completed_at: None },
                            cascade_hlc: Some(cascade_hlc),
                        },
                    )
                    .expect("save group upgrade");
            }

            (store, context_id)
        }

        /// A `HybridTimestamp` strictly greater than `zero()` — a delta
        /// produced after the cascade boundary at `zero()`.
        fn hlc_after_zero() -> HybridTimestamp {
            let id = ID::from(NonZeroU128::new(1).expect("1 is non-zero"));
            HybridTimestamp::new(Timestamp::new(NTP64(1), id))
        }

        #[test]
        fn drops_stale_v1_delta_after_cascade() {
            // v1 schema (≠ current v2) + delta after boundary ⇒ fenced.
            let (store, ctx) = cascaded_store(Some(HybridTimestamp::zero()));
            assert!(delta_is_fenced(&store, &ctx, APP_V1, hlc_after_zero()).unwrap());
        }

        #[test]
        fn keeps_current_v2_delta() {
            // Delta produced under the current target schema ⇒ never fenced.
            let (store, ctx) = cascaded_store(Some(HybridTimestamp::zero()));
            assert!(!delta_is_fenced(&store, &ctx, APP_V2, hlc_after_zero()).unwrap());
        }

        #[test]
        fn keeps_delta_when_no_cascade_recorded() {
            // No cascade boundary ⇒ never fence, even a stale-schema delta.
            let (store, ctx) = cascaded_store(None);
            assert!(!delta_is_fenced(&store, &ctx, APP_V1, hlc_after_zero()).unwrap());
        }

        // ---- PR-6b Task 6b.4: absorb-don't-drop at the gossip fence ----

        use calimero_context::group_store::{AbsorbRecord, AbsorbRepository};
        use calimero_node_primitives::delta_buffer::BufferedDelta;
        use calimero_primitives::hash::Hash;

        use super::super::{fence_and_maybe_absorb, FenceOutcome};

        /// A minimal `BufferedDelta` carrying the replay fields the absorb path
        /// persists. `producing_app_key` is the schema discriminator the fence
        /// keys on.
        fn sample_buffered(delta_id: [u8; 32], producing_app_key: [u8; 32]) -> BufferedDelta {
            BufferedDelta {
                id: delta_id,
                parents: vec![],
                hlc: hlc_after_zero(),
                payload: vec![1, 2, 3],
                nonce: [0; 12],
                author_id: PublicKey::from([0xAB; 32]),
                root_hash: Hash::default(),
                events: None,
                source_peer: libp2p::PeerId::random(),
                key_id: [0; 32],
                governance_position: None,
                delta_signature: Some([7; 64]),
                governance_drain_attempts: 0,
                producing_app_key: Some(producing_app_key),
            }
        }

        /// A `Buffer`-decision delta (schema ≠ the loaded reader, after the
        /// cascade boundary) must be persisted into the AbsorbBuffer, not
        /// dropped — and the call reports `Handled` so the caller returns early.
        #[test]
        fn buffer_decision_persists_absorb_record_not_drop() {
            // Loaded reader falls back to GroupMeta.app_key = APP_V2 here, so an
            // APP_V1 delta after the boundary is unreadable now ⇒ Buffer.
            let (store, ctx) = cascaded_store(Some(HybridTimestamp::zero()));
            let bd = sample_buffered([3; 32], APP_V1);

            let outcome = fence_and_maybe_absorb(
                &store,
                &ctx,
                APP_V1,
                bd.id,
                bd.author_id,
                bd.hlc,
                false,
                || bd.clone(),
            )
            .unwrap();

            assert!(matches!(outcome, FenceOutcome::Handled));
            let pending = AbsorbRepository::new(&store)
                .enumerate_pending(&ctx)
                .unwrap();
            assert_eq!(
                pending.len(),
                1,
                "a stale-schema delta to a behind-reader node must be absorbed, not dropped"
            );
            assert_eq!((pending[0].1).id, [3; 32]);
        }

        /// REGRESSION (the SECOND PR-6b drain bug — fence re-absorb): the absorb
        /// drain re-feeds a buffered straggler through the real apply path
        /// ([`apply_authorized_state_delta`]), whose fence step is exactly
        /// [`fence_and_maybe_absorb`]. For a STALE v1 straggler that the drain
        /// already selected for replay (`producing == v1`, node advanced to
        /// `loaded == target == v2`, delta after the boundary), the *un-bypassed*
        /// fence returns [`FenceOutcome::Handled`] and RE-ABSORBS the delta —
        /// it bounces off the fence and never converges (infinite no-op /
        /// silent drop). The drain-replay call must therefore BYPASS the fence:
        /// with `bypass == true` the already-authorized, already-decided delta
        /// falls through ([`FenceOutcome::Fall`]) and is applied, NOT re-buffered.
        ///
        /// Negative-verify in the same shape: with `bypass == false` (the normal
        /// gossip-receive path) the identical stale straggler IS re-absorbed —
        /// proving the bypass, not a weakened fence, is what makes it apply.
        #[test]
        fn drain_replay_bypasses_fence_for_stale_straggler() {
            // loaded reader falls back to GroupMeta.app_key = APP_V2; the stale
            // straggler was produced under APP_V1, after the cascade boundary.
            // This is precisely the record the drain selected for verbatim replay.
            let (store, ctx) = cascaded_store(Some(HybridTimestamp::zero()));
            let bd = sample_buffered([0xB2; 32], APP_V1);

            // Un-bypassed (normal receive): the fence re-absorbs — the bug.
            let outcome = fence_and_maybe_absorb(
                &store,
                &ctx,
                APP_V1,
                bd.id,
                bd.author_id,
                bd.hlc,
                false,
                || bd.clone(),
            )
            .unwrap();
            assert!(
                matches!(outcome, FenceOutcome::Handled),
                "the un-bypassed fence re-absorbs the stale straggler (the bug)"
            );
            assert_eq!(
                AbsorbRepository::new(&store)
                    .enumerate_pending(&ctx)
                    .unwrap()
                    .len(),
                1,
                "un-bypassed: the straggler bounces off the fence and is re-buffered"
            );

            // Bypassed (drain replay): the fence is skipped — the delta falls
            // through to be applied, and NOTHING new is written to the buffer.
            // Use a fresh store so the un-bypassed half's re-absorb can't mask
            // a bypassed write.
            let (store, ctx) = cascaded_store(Some(HybridTimestamp::zero()));
            let outcome = fence_and_maybe_absorb(
                &store,
                &ctx,
                APP_V1,
                bd.id,
                bd.author_id,
                bd.hlc,
                true,
                || bd.clone(),
            )
            .unwrap();
            assert!(
                matches!(outcome, FenceOutcome::Fall),
                "drain replay must bypass the fence and fall through to apply"
            );
            assert!(
                AbsorbRepository::new(&store)
                    .enumerate_pending(&ctx)
                    .unwrap()
                    .is_empty(),
                "bypassed: the drain-replayed straggler is applied, not re-absorbed"
            );
        }

        /// An `Apply`-decision delta (schema matches the loaded reader) must
        /// fall through and must NOT land in the AbsorbBuffer.
        #[test]
        fn apply_decision_does_not_persist_absorb_record() {
            let (store, ctx) = cascaded_store(Some(HybridTimestamp::zero()));
            let bd = sample_buffered([4; 32], APP_V2);

            let outcome = fence_and_maybe_absorb(
                &store,
                &ctx,
                APP_V2,
                bd.id,
                bd.author_id,
                bd.hlc,
                false,
                || bd.clone(),
            )
            .unwrap();

            assert!(matches!(outcome, FenceOutcome::Fall));
            let pending = AbsorbRepository::new(&store)
                .enumerate_pending(&ctx)
                .unwrap();
            assert!(
                pending.is_empty(),
                "a readable delta must apply normally, not be absorbed"
            );
        }

        // ---- PR-6b Task 6b.5: drain-on-advance (verbatim replay) ----

        use super::super::drain_absorbed_records;

        /// Build a durable `AbsorbRecord` mirroring a buffered straggler delta.
        fn sample_record(delta_id: [u8; 32], producing_app_key: [u8; 32]) -> AbsorbRecord {
            AbsorbRecord::from_buffered(&sample_buffered(delta_id, producing_app_key))
        }

        /// Install a loaded reader for `context_id` resolving to `blob`, so
        /// [`loaded_reader_app_key`] returns `blob` (instead of falling back to
        /// `GroupMeta.app_key`). Lets a test model `loaded != target`.
        fn install_loaded_reader(store: &Store, context_id: &ContextId, blob: [u8; 32]) {
            use calimero_primitives::application::ApplicationId;
            use calimero_primitives::blobs::BlobId;
            use calimero_store::key;
            use calimero_store::types::{ApplicationMeta, ContextMeta};

            let app_key = key::ApplicationMeta::new(ApplicationId::from([0xCC; 32]));
            let app_meta = ApplicationMeta::new(
                key::BlobMeta::new(BlobId::from(blob)),
                0,
                "".into(),
                Box::default(),
                key::BlobMeta::new(BlobId::from([0; 32])),
                "".into(),
                "".into(),
                "".into(),
            );
            let ctx_meta = ContextMeta::new(app_key, [0; 32], vec![], None);

            let mut handle = store.handle();
            handle
                .put(&key::ContextMeta::new(*context_id), &ctx_meta)
                .expect("put ContextMeta");
            handle
                .put(&app_key, &app_meta)
                .expect("put ApplicationMeta");
        }

        /// REGRESSION (the PR-6b drain bug): a STALE v1 straggler delta —
        /// `producing_app_key == v1`, the node already advanced to
        /// `loaded == target == v2` — must be REPLAYED (verbatim) and deleted,
        /// NOT skipped forever. The drain-ready signal is "the node reached the
        /// migration target", not "producing == loaded". This test FAILS against
        /// the old `producing_app_key != Some(loaded)` skip (the stale record was
        /// dropped, losing the offline write).
        #[tokio::test]
        async fn drain_replays_stale_straggler_when_node_reached_target() {
            // Loaded reader falls back to GroupMeta.app_key = APP_V2, and the
            // migration target is also APP_V2 ⇒ loaded == target.
            let (store, ctx) = cascaded_store(Some(HybridTimestamp::zero()));
            let repo = AbsorbRepository::new(&store);

            // The stale v1 straggler buffered while this node was on v1; the node
            // has since advanced to v2 (loaded == target == v2). It is now behind
            // the loaded reader yet replayable through the current wasm.
            repo.save(&ctx, APP_V1, &sample_record([0xB2; 32], APP_V1))
                .unwrap();

            let replayed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<[u8; 32]>::new()));
            let replayed_capture = replayed.clone();

            let drained = drain_absorbed_records(&store, &ctx, move |buffered| {
                let replayed = replayed_capture.clone();
                async move {
                    // Verbatim: the replay sees the original payload bytes,
                    // never a translated re-encoding.
                    assert_eq!(buffered.payload, vec![1, 2, 3]);
                    replayed.lock().unwrap().push(buffered.id);
                    Ok::<bool, eyre::Report>(true)
                }
            })
            .await
            .unwrap();

            assert_eq!(
                drained, 1,
                "a stale straggler must drain once the node reached the target"
            );
            assert_eq!(
                *replayed.lock().unwrap(),
                vec![[0xB2; 32]],
                "the stale v1 straggler is replayed verbatim, not dropped"
            );
            assert!(
                repo.enumerate_pending(&ctx).unwrap().is_empty(),
                "the replayed straggler must be deleted, not left to leak"
            );
        }

        /// A FUTURE-schema delta — `producing == v2` (the target), node still
        /// behind on `loaded == v1 < target` — must be SKIPPED (the binary can't
        /// read it yet, never translate). Once the node advances so
        /// `loaded == target == v2`, the same record drains.
        #[tokio::test]
        async fn drain_skips_future_delta_until_node_advances() {
            let (store, ctx) = cascaded_store(Some(HybridTimestamp::zero()));
            // Node behind: loaded reader = v1, target (GroupMeta.app_key) = v2.
            install_loaded_reader(&store, &ctx, APP_V1);
            let repo = AbsorbRepository::new(&store);

            // Future delta: produced under the target schema v2.
            repo.save(&ctx, APP_V2, &sample_record([0xC3; 32], APP_V2))
                .unwrap();

            let noop = |_buffered: BufferedDelta| async move { Ok::<bool, eyre::Report>(true) };

            // While behind (loaded == v1 != target == v2, producing == v2 !=
            // loaded), the future delta must NOT be replayed.
            let drained = drain_absorbed_records(&store, &ctx, noop).await.unwrap();
            assert_eq!(drained, 0, "a future-schema delta is skipped while behind");
            let pending = repo.enumerate_pending(&ctx).unwrap();
            assert_eq!(pending.len(), 1, "the future delta stays pending");
            assert_eq!((pending[0].1).id, [0xC3; 32]);

            // The node advances to the target → loaded == target == v2 → drains.
            install_loaded_reader(&store, &ctx, APP_V2);
            let drained = drain_absorbed_records(&store, &ctx, noop).await.unwrap();
            assert_eq!(drained, 1, "the future delta drains once the node advances");
            assert!(repo.enumerate_pending(&ctx).unwrap().is_empty());
        }

        /// Leaf- and entity-shaped sync-repair records are NOT replayable deltas
        /// and must be SKIPPED by the delta drain (they are drained by the
        /// leaf/entity-replay path), even when the node has reached the target.
        #[tokio::test]
        async fn delta_drain_skips_leaf_and_entity_records() {
            // loaded == target == v2.
            let (store, ctx) = cascaded_store(Some(HybridTimestamp::zero()));
            let repo = AbsorbRepository::new(&store);

            // A sync-repair leaf and a snapshot entity, both stamped v2 (the
            // target) — the delta drain must still leave them untouched.
            repo.save(
                &ctx,
                APP_V2,
                &AbsorbRecord::from_leaf([0xD4; 32], vec![1, 2, 3], APP_V2),
            )
            .unwrap();
            repo.save(
                &ctx,
                APP_V2,
                &AbsorbRecord::from_snapshot_entity([0xE5; 32], vec![1], vec![2], APP_V2),
            )
            .unwrap();

            let replayed = std::sync::Arc::new(std::sync::Mutex::new(0usize));
            let replayed_capture = replayed.clone();
            let drained = drain_absorbed_records(&store, &ctx, move |_buffered| {
                let replayed = replayed_capture.clone();
                async move {
                    *replayed.lock().unwrap() += 1;
                    Ok::<bool, eyre::Report>(true)
                }
            })
            .await
            .unwrap();

            assert_eq!(drained, 0, "the delta drain replays no leaf/entity records");
            assert_eq!(
                *replayed.lock().unwrap(),
                0,
                "leaf/entity records are never fed to the delta replay path"
            );
            assert_eq!(
                repo.enumerate_pending(&ctx).unwrap().len(),
                2,
                "leaf/entity records are left for the leaf/entity drain path"
            );
        }

        /// Re-running the drain after a successful pass is a no-op (idempotent
        /// via delta_id key): the deleted record does not re-replay.
        #[tokio::test]
        async fn drain_is_idempotent_after_success() {
            let (store, ctx) = cascaded_store(Some(HybridTimestamp::zero()));
            let repo = AbsorbRepository::new(&store);
            repo.save(&ctx, APP_V2, &sample_record([0xA1; 32], APP_V2))
                .unwrap();

            let noop = |_buffered: BufferedDelta| async move { Ok::<bool, eyre::Report>(true) };

            let first = drain_absorbed_records(&store, &ctx, noop).await.unwrap();
            assert_eq!(first, 1);
            let second = drain_absorbed_records(&store, &ctx, noop).await.unwrap();
            assert_eq!(second, 0, "no records survive a successful drain");
            assert!(repo.enumerate_pending(&ctx).unwrap().is_empty());
        }

        /// A record whose replay fails is NOT deleted — it survives for the
        /// next drain pass (delete-after-success only).
        #[tokio::test]
        async fn failed_replay_leaves_record_pending() {
            let (store, ctx) = cascaded_store(Some(HybridTimestamp::zero()));
            let repo = AbsorbRepository::new(&store);
            repo.save(&ctx, APP_V2, &sample_record([0xA1; 32], APP_V2))
                .unwrap();

            let drained = drain_absorbed_records(&store, &ctx, |_buffered| async move {
                Err::<bool, eyre::Report>(eyre::eyre!("replay failed"))
            })
            .await
            .unwrap();

            assert_eq!(drained, 0, "a failed replay drains nothing");
            let pending = repo.enumerate_pending(&ctx).unwrap();
            assert_eq!(pending.len(), 1, "a failed-replay record must survive");
            assert_eq!((pending[0].1).id, [0xA1; 32]);
        }

        // ---- PR-6b Task 6b.6: startup recovery scan ----

        use super::super::recover_absorbed_records;

        /// On node startup the AbsorbBuffer is durable (RocksDB CF), so any
        /// straggler delta persisted before a restart must be re-considered for
        /// drain. With the loaded reader at the migration target, both a
        /// target-schema record and a STALE v1 straggler (behind the loaded
        /// reader) are now replayable and must drain — a restart mid-window must
        /// not lose buffered deltas. A genuinely future record (target not yet
        /// reached) is left behind; that path is exercised below.
        #[tokio::test]
        async fn startup_recovery_drains_records_once_target_reached() {
            // The store's loaded reader falls back to GroupMeta.app_key = APP_V2,
            // and the target is APP_V2 ⇒ loaded == target.
            let (store, ctx) = cascaded_store(Some(HybridTimestamp::zero()));
            let repo = AbsorbRepository::new(&store);

            // Persisted-before-restart, target-schema: must drain on startup.
            repo.save(&ctx, APP_V2, &sample_record([0xA1; 32], APP_V2))
                .unwrap();
            // Persisted-before-restart, stale v1 straggler (behind the loaded
            // reader but the node reached the target): must ALSO drain — the
            // current wasm verbatim-replays it.
            repo.save(&ctx, APP_V1, &sample_record([0xB2; 32], APP_V1))
                .unwrap();

            let replayed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<[u8; 32]>::new()));
            let replayed_capture = replayed.clone();

            let drained = recover_absorbed_records(&store, move |context_id, buffered| {
                let replayed = replayed_capture.clone();
                async move {
                    // The recovery threads the right context to the replay.
                    assert_eq!(context_id, ctx);
                    // Verbatim: the recovery replay sees the original bytes.
                    assert_eq!(buffered.payload, vec![1, 2, 3]);
                    replayed.lock().unwrap().push(buffered.id);
                    Ok::<bool, eyre::Report>(true)
                }
            })
            .await
            .unwrap();

            assert_eq!(
                drained, 2,
                "both the target-schema and the stale straggler drain on startup"
            );
            let mut seen = replayed.lock().unwrap().clone();
            seen.sort_unstable();
            assert_eq!(
                seen,
                vec![[0xA1; 32], [0xB2; 32]],
                "both records are replayed verbatim once the node reached the target"
            );

            assert!(
                repo.enumerate_pending(&ctx).unwrap().is_empty(),
                "no record is left stranded once the node reached the target"
            );
        }

        /// A node restarting *still behind* the target (loaded reader < target)
        /// leaves the unreadable future record pending across the startup scan.
        #[tokio::test]
        async fn startup_recovery_keeps_future_record_while_behind() {
            let (store, ctx) = cascaded_store(Some(HybridTimestamp::zero()));
            // Node behind: loaded reader = v1, target (GroupMeta.app_key) = v2.
            install_loaded_reader(&store, &ctx, APP_V1);
            let repo = AbsorbRepository::new(&store);

            // Future delta (target schema) the behind-reader node can't read yet.
            repo.save(&ctx, APP_V2, &sample_record([0xC3; 32], APP_V2))
                .unwrap();

            let noop = |_ctx: ContextId, _buffered: BufferedDelta| async move {
                Ok::<bool, eyre::Report>(true)
            };
            let drained = recover_absorbed_records(&store, noop).await.unwrap();

            assert_eq!(
                drained, 0,
                "a behind node drains no future record on startup"
            );
            let pending = repo.enumerate_pending(&ctx).unwrap();
            assert_eq!(
                pending.len(),
                1,
                "the future record survives the startup scan"
            );
            assert_eq!((pending[0].1).id, [0xC3; 32]);
        }

        /// The startup scan is idempotent across two recovery calls (e.g. a
        /// double-init or a quick restart): an already-drained record does not
        /// re-replay, and a record the node is still too far behind to read
        /// stays put.
        #[tokio::test]
        async fn startup_recovery_is_idempotent_across_two_calls() {
            let (store, ctx) = cascaded_store(Some(HybridTimestamp::zero()));
            // Node behind: loaded == v1 < target == v2, so the future record is
            // a stable survivor across both scans.
            install_loaded_reader(&store, &ctx, APP_V1);
            let repo = AbsorbRepository::new(&store);
            // Readable now (matches the loaded reader v1): drains on the 1st scan.
            repo.save(&ctx, APP_V1, &sample_record([0xA1; 32], APP_V1))
                .unwrap();
            // Future (target schema v2): the behind node leaves it pending.
            repo.save(&ctx, APP_V2, &sample_record([0xB2; 32], APP_V2))
                .unwrap();

            let noop = |_ctx: ContextId, _buffered: BufferedDelta| async move {
                Ok::<bool, eyre::Report>(true)
            };

            let first = recover_absorbed_records(&store, noop).await.unwrap();
            assert_eq!(first, 1);
            let second = recover_absorbed_records(&store, noop).await.unwrap();
            assert_eq!(second, 0, "a second startup scan re-drains nothing");

            let pending = repo.enumerate_pending(&ctx).unwrap();
            assert_eq!(pending.len(), 1, "the future record persists");
            assert_eq!((pending[0].1).id, [0xB2; 32]);
        }

        // NOTE: startup drain of buffered snapshot-**entity** records is not
        // unit-tested via a standalone recovery function any more. The entity
        // arm of `drain_absorbed_leaves` (the live startup hook runs it over
        // every context with a pending absorb) already drains both leaf- and
        // entity-shaped records with the identical `schema_app_key == loaded`
        // gate and the same `persist_buffered_snapshot_entity` path; the entity
        // persist/redrive/pending logic is covered directly by
        // `sync::snapshot::tests::test_persist_buffered_snapshot_entity_*`, and
        // `delta_drain_skips_leaf_and_entity_records` pins that the delta drain
        // leaves entity records for that path.

        /// With nothing buffered (the common case) the startup scan is a cheap
        /// no-op and never panics.
        #[tokio::test]
        async fn startup_recovery_is_noop_when_nothing_buffered() {
            let (store, _ctx) = cascaded_store(Some(HybridTimestamp::zero()));
            let noop = |_ctx: ContextId, _buffered: BufferedDelta| async move {
                Ok::<bool, eyre::Report>(true)
            };
            let drained = recover_absorbed_records(&store, noop).await.unwrap();
            assert_eq!(
                drained, 0,
                "no contexts with pending absorbs ⇒ nothing drains"
            );
        }
    }
}

/// Requests missing parent deltas from a peer
///
/// Opens a stream to the peer and requests each missing delta sequentially.
/// Adds successfully retrieved deltas back to the DAG for processing.
/// Fetch missing ancestor deltas from a peer and add them to the DAG in
/// topological order.
///
/// Returns the aggregated `cascaded_events` from every `add_delta_with_events`
/// call. Each peer-fetched parent that resolves a pending child carries that
/// child's stored events along in its `AddDeltaResult`; callers *must* run
/// `execute_cascaded_events` on the returned list, otherwise handler execution
/// for cascaded children silently never happens.
async fn request_missing_deltas(
    network_client: calimero_network_primitives::client::NetworkClient,
    sync_timeout: std::time::Duration,
    context_id: ContextId,
    missing_ids: Vec<[u8; 32]>,
    source: PeerId,
    our_identity: PublicKey,
    delta_store: DeltaStore,
    datastore: calimero_store::Store,
) -> Result<Vec<([u8; 32], Vec<u8>)>> {
    use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};

    // Metric: number of missing-parent IDs the caller is about to fetch.
    // Recorded *before* the stream open so a peer-stream failure doesn't
    // hide the demand signal in dashboards.
    crate::node_metrics::record_missing_parents_request(missing_ids.len());

    // Open stream to peer
    let mut stream = network_client.open_stream(source).await?;

    // Fetch all missing ancestors, then add them in topological order (oldest first).
    // The tuple carries the wire-received author + governance position
    // + envelope signature so the persist step writes them to the
    // `ContextDagDelta` row (next DAG-catchup serves can pass them on)
    // and the cross-DAG check + envelope verification fire before apply.
    let mut to_fetch = missing_ids;
    type ParentFetch = (
        calimero_dag::CausalDelta<Vec<Action>>,
        [u8; 32], // delta_id (redundant with .id but kept for log clarity)
        // `None` for genesis (matches what `create_context` persists
        // — the row's existence + parents=[[0;32]] is what the
        // responder's carve-out keys off, NOT the sentinel author id).
        // `Some(author)` for every other delta.
        Option<PublicKey>,
        Option<Vec<u8>>,  // governance_position_blob from wire
        Option<[u8; 64]>, // delta_signature from wire
    );
    let mut fetched_deltas: Vec<ParentFetch> = Vec::new();
    let mut fetch_count = 0;
    // Accumulated (delta_id, events_data) pairs from any cascades that
    // happen while adding peer-fetched parents below. Passed back to the
    // caller so handlers can run.
    let mut cascaded_events: Vec<([u8; 32], Vec<u8>)> = Vec::new();

    // Phase 1: Fetch ALL missing deltas recursively
    // No artificial limit - DAG is acyclic so this will naturally terminate at genesis
    while !to_fetch.is_empty() {
        let current_batch = to_fetch.clone();
        to_fetch.clear();

        for missing_id in current_batch {
            fetch_count += 1;

            info!(
                %context_id,
                delta_id = ?missing_id,
                total_fetched = fetch_count,
                "Requesting missing parent delta from peer"
            );

            // Send request
            let request_msg = StreamMessage::Init {
                context_id,
                party_id: our_identity,
                payload: InitPayload::DeltaRequest {
                    context_id,
                    delta_id: missing_id,
                },
                next_nonce: {
                    use rand::Rng;
                    rand::thread_rng().gen()
                },
            };

            crate::sync::stream::send(&mut stream, &request_msg, None).await?;

            // Wait for response
            let timeout_budget = sync_timeout / 3;
            match crate::sync::stream::recv(&mut stream, None, timeout_budget).await? {
                Some(StreamMessage::Message {
                    payload:
                        MessagePayload::DeltaResponse {
                            delta,
                            author_id: response_author,
                            governance_position_blob,
                            delta_signature: response_delta_signature,
                        },
                    ..
                }) => {
                    // Deserialize storage delta
                    let storage_delta: calimero_storage::delta::CausalDelta =
                        borsh::from_slice(&delta)?;

                    info!(
                        %context_id,
                        delta_id = ?missing_id,
                        author = %response_author,
                        action_count = storage_delta.actions.len(),
                        "Received missing parent delta"
                    );

                    // Genesis carve-out: the responder serves the
                    // genesis delta with the all-zeros sentinel
                    // `author_id` because the wire requires an author
                    // but genesis predates any governance op. Skip
                    // every author-keyed check, persist directly with
                    // `None` author info so subsequent serves see it
                    // as the genesis row and use the same sentinel
                    // dispatch.
                    if crate::sync::delta_request::is_genesis_author_sentinel(&response_author) {
                        debug!(
                            %context_id,
                            delta_id = ?missing_id,
                            "parent-fetch: accepting genesis delta via author sentinel"
                        );
                        let dag_delta = calimero_dag::CausalDelta {
                            id: storage_delta.id,
                            parents: storage_delta.parents.clone(),
                            payload: storage_delta.actions,
                            hlc: storage_delta.hlc,
                            expected_root_hash: storage_delta.expected_root_hash,
                            kind: calimero_dag::DeltaKind::Regular,
                        };
                        // Persist with `author_id: None` so when this
                        // node later serves the genesis row, the
                        // responder's existing genesis carve-out
                        // (`stored_delta.author_id is None &&
                        // parents == [[0;32]]`) fires and re-wraps
                        // with the sentinel for the next hop. Matches
                        // what `create_context` originally persists.
                        fetched_deltas.push((dag_delta, missing_id, None, None, None));
                        continue;
                    }

                    // Decode governance_position once for both the
                    // envelope-signature verification and the cross-
                    // DAG membership check below.
                    let governance_position: Option<
                        calimero_context_config::types::GovernancePosition,
                    > = match governance_position_blob
                        .as_deref()
                        .map(
                            borsh::from_slice::<calimero_context_config::types::GovernancePosition>,
                        )
                        .transpose()
                    {
                        Ok(pos) => pos,
                        Err(err) => {
                            warn!(
                                %context_id,
                                delta_id = ?missing_id,
                                %err,
                                "parent-fetch: failed to decode governance_position from \
                                 peer; skipping this delta to avoid silent bypass"
                            );
                            continue;
                        }
                    };

                    // Envelope-signature verification (parity with the
                    // gossip + DAG-catchup paths in
                    // `apply_authorized_state_delta` / `request_dag_heads_and_sync`).
                    // `None` is only tolerated for legacy rows
                    // authored before envelope signing landed; any
                    // present signature MUST verify.
                    if let Some(ref sig) = response_delta_signature {
                        if let Err(err) =
                            calimero_node_primitives::sync::delta_auth::verify_delta_signature(
                                context_id,
                                storage_delta.id,
                                response_author,
                                governance_position.as_ref(),
                                sig,
                            )
                        {
                            warn!(
                                %context_id,
                                delta_id = ?missing_id,
                                author = %response_author,
                                %err,
                                "parent-fetch: envelope signature verification failed, dropping"
                            );
                            continue;
                        }
                    }

                    // Sanity check: peer returned the delta we
                    // requested. A malicious or buggy peer could send
                    // a different delta's body in response to our
                    // request; the envelope signature we verified
                    // above bound `storage_delta.id`, not
                    // `missing_id`, so a body-id mismatch would slip
                    // an unrelated authorized delta into our DAG.
                    if storage_delta.id != missing_id {
                        warn!(
                            %context_id,
                            requested = ?missing_id,
                            received = ?storage_delta.id,
                            "parent-fetch: peer returned a different delta id than requested, dropping"
                        );
                        continue;
                    }

                    // Group-id parity check: same as the gossip apply
                    // path. Without this, a delta whose author signed
                    // a position citing a *different* group's
                    // governance could pass the membership check
                    // (`membership_status_at` walks the cited group's
                    // DAG, not this context's owning group) and slip
                    // through. Match the `GroupIdCheck` branches
                    // `verify_position_group_id_matches_context`
                    // returns to apply identical reject/skip rules.
                    match verify_position_group_id_matches_context(
                        &datastore,
                        &context_id,
                        governance_position.as_ref().map(|p| p.group_id),
                    ) {
                        GroupIdCheck::NonGroupOk | GroupIdCheck::Match => {
                            // ok — fall through to membership check
                        }
                        GroupIdCheck::GroupContextNoPosition { owning } => {
                            warn!(
                                %context_id,
                                delta_id = ?missing_id,
                                author = %response_author,
                                owning_group = ?owning,
                                "parent-fetch: group context but no governance_position, dropping"
                            );
                            continue;
                        }
                        GroupIdCheck::NonGroupContextWithPosition { claimed } => {
                            warn!(
                                %context_id,
                                delta_id = ?missing_id,
                                author = %response_author,
                                claimed_group = ?claimed,
                                "parent-fetch: non-group context but position claims a group, dropping"
                            );
                            continue;
                        }
                        GroupIdCheck::Mismatch { owning, claimed } => {
                            warn!(
                                %context_id,
                                delta_id = ?missing_id,
                                author = %response_author,
                                owning_group = ?owning,
                                claimed_group = ?claimed,
                                "parent-fetch: governance_position cites a different group than context owns, dropping"
                            );
                            continue;
                        }
                        GroupIdCheck::LookupError(err) => {
                            warn!(
                                %context_id,
                                delta_id = ?missing_id,
                                %err,
                                "parent-fetch: get_group_for_context failed, dropping to avoid silent bypass"
                            );
                            continue;
                        }
                    }

                    // ReadOnly check — parity with the gossip apply
                    // path in `apply_authorized_state_delta`.
                    // `membership_status_at` treats ReadOnly as
                    // `Member(ReadOnly)`, so without a separate
                    // `is_read_only_for_context` gate a delta authored
                    // by a ReadOnly / ReadOnlyTee identity passes the
                    // membership check on the catchup path even
                    // though gossip rejects the same envelope.
                    if NamespaceRepository::new(&datastore)
                        .is_read_only_for_context(&context_id, &response_author)
                        .unwrap_or(false)
                    {
                        warn!(
                            %context_id,
                            delta_id = ?missing_id,
                            author = %response_author,
                            "parent-fetch: rejecting delta from ReadOnly member"
                        );
                        continue;
                    }

                    // Cross-DAG membership check: same as the
                    // request_dag_heads_and_sync path. Reject deltas
                    // whose author was removed at the cited cut.
                    if let Some(ref pos) = governance_position {
                        use calimero_context::group_store::{
                            membership_status_at, MembershipStatus,
                        };
                        match membership_status_at(&datastore, &response_author, pos) {
                            Ok(MembershipStatus::Member(_)) => {}
                            Ok(MembershipStatus::Removed { last_role }) => {
                                warn!(
                                    %context_id,
                                    delta_id = ?missing_id,
                                    author = %response_author,
                                    last_role = ?last_role,
                                    "parent-fetch: author was removed at cited cut, dropping"
                                );
                                continue;
                            }
                            Ok(MembershipStatus::NeverMember) => {
                                warn!(
                                    %context_id,
                                    delta_id = ?missing_id,
                                    author = %response_author,
                                    "parent-fetch: author never a member at cited cut, dropping"
                                );
                                continue;
                            }
                            Ok(MembershipStatus::Unknown { needed }) => {
                                warn!(
                                    %context_id,
                                    delta_id = ?missing_id,
                                    author = %response_author,
                                    needed = ?needed,
                                    "parent-fetch: governance cut not locally known, skipping"
                                );
                                continue;
                            }
                            Err(err) => {
                                warn!(
                                    %context_id,
                                    delta_id = ?missing_id,
                                    author = %response_author,
                                    %err,
                                    "parent-fetch: membership_status_at failed, dropping to \
                                     avoid silent bypass"
                                );
                                continue;
                            }
                        }
                    }

                    // Convert to DAG delta
                    let dag_delta = calimero_dag::CausalDelta {
                        id: storage_delta.id,
                        parents: storage_delta.parents.clone(),
                        payload: storage_delta.actions,
                        hlc: storage_delta.hlc,
                        expected_root_hash: storage_delta.expected_root_hash,
                        kind: calimero_dag::DeltaKind::Regular,
                    };

                    // Store for later (don't add to DAG yet!) — carry
                    // the verified wire fields so the persist step
                    // can write them to the row.
                    fetched_deltas.push((
                        dag_delta,
                        missing_id,
                        Some(response_author),
                        governance_position_blob.as_ref().map(|c| c.to_vec()),
                        response_delta_signature,
                    ));

                    // Check what parents THIS delta needs
                    for parent_id in &storage_delta.parents {
                        // Skip genesis
                        if *parent_id == [0; 32] {
                            continue;
                        }
                        // Skip if we already have it or are about to fetch it
                        if !delta_store.has_delta(parent_id).await
                            && !to_fetch.contains(parent_id)
                            && !fetched_deltas
                                .iter()
                                .any(|(d, _, _, _, _)| d.id == *parent_id)
                        {
                            to_fetch.push(*parent_id);
                        }
                    }
                }
                Some(StreamMessage::Message {
                    payload: MessagePayload::DeltaNotFound,
                    ..
                }) => {
                    // `DeltaNotFound` is overloaded (compacted away #2026, a
                    // not-yet-persisted post-broadcast race, or an unverifiable
                    // row), so we just skip this id and keep fetching the rest.
                    // A genuinely pruned ancestor leaves descendants pending and
                    // the next sync round converges via HashComparison without
                    // the delta log; no explicit abort/fallback is needed.
                    warn!(%context_id, delta_id = ?missing_id, "Peer doesn't have requested delta");
                }
                other => {
                    warn!(%context_id, delta_id = ?missing_id, ?other, "Unexpected response to delta request");
                }
            }
        }
    }

    // Phase 2: Add all fetched deltas to DAG in topological order (oldest first)
    // We fetched breadth-first, so reversing gives us depth-first (ancestors before descendants)
    if !fetched_deltas.is_empty() {
        info!(
            %context_id,
            total_fetched = fetched_deltas.len(),
            "Adding fetched deltas to DAG in topological order"
        );

        // Reverse so oldest ancestors are added first
        fetched_deltas.reverse();

        for (dag_delta, delta_id, author_id, governance_position_blob, delta_signature) in
            fetched_deltas
        {
            // Use the events-aware entry point so we can forward any events
            // attached to *cascaded children* to the caller. The peer-fetched
            // parent itself has no events (the wire protocol doesn't carry
            // them on `DeltaResponse`) — hence `None` for the second arg —
            // but `add_delta_internal`'s internal `apply_pending` can cascade
            // children that were pre-persisted with events, and those need
            // to reach `execute_cascaded_events` at the caller.
            //
            // The wire-received author + governance position + envelope
            // signature are persisted on the row so subsequent
            // DAG-catchup serves from this node include the claim
            // (responder filters out rows without an author claim, see
            // `crates/node/src/sync/delta_request.rs`).
            match delta_store
                .add_delta_with_events(
                    dag_delta,
                    None,
                    author_id,
                    governance_position_blob,
                    delta_signature,
                )
                .await
            {
                Ok(result) => {
                    if !result.cascaded_events.is_empty() {
                        info!(
                            %context_id,
                            parent_delta_id = ?delta_id,
                            cascaded_count = result.cascaded_events.len(),
                            "Peer-fetched parent cascaded pending children with events"
                        );
                        cascaded_events.extend(result.cascaded_events);
                    }
                }
                Err(e) => {
                    warn!(?e, %context_id, delta_id = ?delta_id, "Failed to add fetched delta to DAG");
                }
            }
        }

        // Log warning for very large syncs (informational, not a hard limit)
        if fetch_count > 1000 {
            warn!(
                %context_id,
                total_fetched = fetch_count,
                "Large sync detected - fetched many deltas from peer (context has deep history)"
            );
        }
    }

    Ok(cascaded_events)
}

/// Ensures the application blob is available for a context before handler execution.
///
/// This fixes the race condition where gossipsub state deltas arrive before the
/// WASM application blob has finished downloading. Without this check, handler
/// execution would fail with "ApplicationNotInstalled" errors.
///
/// The function polls for blob availability with exponential backoff, up to the
/// specified timeout. If the blob becomes available, it returns Ok(()); otherwise
/// it returns an error.
async fn ensure_application_available(
    node_client: &calimero_node_primitives::client::NodeClient,
    context_client: &calimero_context_client::client::ContextClient,
    context_id: &ContextId,
    timeout: std::time::Duration,
) -> Result<()> {
    use std::time::Duration;
    use tokio::time::{sleep, Instant};

    let context = context_client
        .get_context(context_id)?
        .ok_or_else(|| eyre::eyre!("context not found"))?;

    let application_id = context.application_id;

    // Check if application is already installed and blob available
    if let Ok(Some(app)) = node_client.get_application(&application_id) {
        // Application exists, check if bytecode blob is available
        if node_client.has_blob(&app.blob.bytecode)? {
            debug!(
                %context_id,
                %application_id,
                "Application blob already available"
            );
            return Ok(());
        }
    }

    // Blob not yet available - poll with backoff
    let start = Instant::now();
    let mut delay = Duration::from_millis(50);
    let max_delay = Duration::from_millis(500);

    info!(
        %context_id,
        %application_id,
        timeout_ms = timeout.as_millis(),
        "Waiting for application blob to become available..."
    );

    while start.elapsed() < timeout {
        sleep(delay).await;

        // Re-check application and blob
        if let Ok(Some(app)) = node_client.get_application(&application_id) {
            if node_client.has_blob(&app.blob.bytecode)? {
                info!(
                    %context_id,
                    %application_id,
                    elapsed_ms = start.elapsed().as_millis(),
                    "Application blob now available"
                );
                return Ok(());
            }
        }

        // Exponential backoff
        delay = std::cmp::min(delay * 2, max_delay);
    }

    // Timeout reached
    warn!(
        %context_id,
        %application_id,
        elapsed_ms = start.elapsed().as_millis(),
        "Timeout waiting for application blob"
    );

    Err(eyre::eyre!(
        "Application blob not available after {:?}",
        timeout
    ))
}

/// Replay a buffered delta after snapshot sync completes.
///
/// This function processes a delta that was buffered because the context was
/// uninitialized when it arrived. Now that the context is initialized (after
/// snapshot sync), we can decrypt it, apply it, and execute any event handlers.
///
/// The `is_covered_by_checkpoint` parameter indicates whether this delta is an
/// ancestor of an existing checkpoint. If true, the delta's state is already
/// present via the snapshot, and handlers should be executed even if the delta
/// can't be applied to the DAG (due to missing intermediate parents).
///
/// Returns Ok(true) if delta was applied, Ok(false) if pending (missing parents).
pub async fn replay_buffered_delta(input: ReplayBufferedDeltaInput) -> Result<bool> {
    let ReplayBufferedDeltaInput {
        context_client,
        node_client,
        node_state,
        context_id,
        our_identity,
        buffered,
        sync_timeout,
        is_covered_by_checkpoint,
    } = input;

    let delta_id = buffered.id;

    info!(
        %context_id,
        delta_id = ?delta_id,
        author = %buffered.author_id,
        has_events = buffered.events.is_some(),
        "Replaying buffered delta"
    );

    // Skip if this is our own delta
    if buffered.author_id == our_identity {
        debug!(
            %context_id,
            delta_id = ?delta_id,
            "Skipping replay of self-authored delta"
        );
        return Ok(false);
    }

    // Get context (should exist now after snapshot sync)
    let _context = context_client
        .get_context(&context_id)?
        .ok_or_else(|| eyre::eyre!("context not found after snapshot sync"))?;

    // Per-delta envelope signature verification, parity with the
    // gossip + DAG-catchup + parent-fetch paths. The `BufferedDelta`
    // carries the signature through snapshot-sync buffering precisely
    // so a replayed delta is re-verified against the same payload the
    // original sender signed (Wave 5). Without this gate, snapshot-
    // sync replay would silently accept envelope-forged buffered
    // deltas — the very class of attack the envelope signature
    // exists to prevent.
    if let Some(ref sig) = buffered.delta_signature {
        if let Err(err) = calimero_node_primitives::sync::delta_auth::verify_delta_signature(
            context_id,
            delta_id,
            buffered.author_id,
            buffered.governance_position.as_ref(),
            sig,
        ) {
            warn!(
                %context_id,
                delta_id = ?delta_id,
                author = %buffered.author_id,
                %err,
                "Rejecting buffered state delta — envelope signature verification failed"
            );
            return Ok(false);
        }
    }

    // ReadOnly check, parallel to `handle_state_delta` and
    // `apply_authorized_state_delta`. Snapshot-sync replay must enforce the
    // same per-context role gate; otherwise a peer that became ReadOnly
    // between authoring and replay slips a write through.
    if NamespaceRepository::new(context_client.datastore())
        .is_read_only_for_context(&context_id, &buffered.author_id)
        .unwrap_or(false)
    {
        warn!(
            %context_id,
            author = %buffered.author_id,
            "Rejecting buffered state delta from ReadOnly member"
        );
        return Ok(false);
    }

    // Apply-time cross-DAG membership check, parallel to `handle_state_delta`.
    // Snapshot sync establishes a context-state baseline but says nothing
    // about governance state, so a delta buffered during sync must still
    // pass the membership check before its actions are applied. Without
    // this, every delta arriving inside the sync window bypasses cross-DAG
    // authorization.
    //
    // Anti-bypass: see [`GroupIdCheck`] for the two bypasses this match
    // closes (mismatched group_id on a signed position, and lying about
    // being a non-group context). Single store lookup covers both
    // position-present and position-absent cases — same shape as
    // `handle_state_delta`.
    //
    // INVARIANT: `ContextManager` serializes governance ops, so
    // no concurrent group reassignment can interleave between this
    // check and the `membership_status_at` call below — see the
    // TOCTOU note on `verify_position_group_id_matches_context`.
    let datastore = context_client.datastore();
    match verify_position_group_id_matches_context(
        datastore,
        &context_id,
        buffered.governance_position.as_ref().map(|p| p.group_id),
    ) {
        GroupIdCheck::NonGroupOk => {
            // Legacy non-group context with no claimed group. Fall through.
        }
        GroupIdCheck::Match => {
            // Position's group matches the context's owning group. Fall
            // through to the membership-status check below.
        }
        GroupIdCheck::GroupContextNoPosition { owning } => {
            warn!(
                %context_id,
                author = %buffered.author_id,
                owning_group = ?owning,
                delta_id = ?delta_id,
                "cross-DAG check (replay): rejecting buffered delta — group context but no \
                 governance_position (likely a malicious bypass attempt)"
            );
            return Ok(false);
        }
        GroupIdCheck::NonGroupContextWithPosition { claimed } => {
            warn!(
                %context_id,
                author = %buffered.author_id,
                claimed_group = ?claimed,
                delta_id = ?delta_id,
                "cross-DAG check (replay): rejecting buffered delta — governance_position \
                 present but context is not part of any group"
            );
            return Ok(false);
        }
        GroupIdCheck::Mismatch { owning, claimed } => {
            warn!(
                %context_id,
                author = %buffered.author_id,
                owning_group = ?owning,
                claimed_group = ?claimed,
                delta_id = ?delta_id,
                "cross-DAG check (replay): rejecting buffered delta — governance_position \
                 references a different group than the context's owning group"
            );
            return Ok(false);
        }
        GroupIdCheck::LookupError(err) => {
            warn!(
                %context_id,
                author = %buffered.author_id,
                %err,
                "cross-DAG check (replay): get_group_for_context failed; rejecting buffered \
                 delta to avoid silent bypass"
            );
            return Ok(false);
        }
    }

    if let Some(pos) = buffered.governance_position.as_ref() {
        let datastore = context_client.datastore();
        // Forward-only invariant — same contract as the gossip-receive
        // and drain sites. Snapshot-sync establishes a context-state
        // baseline that may be at-or-ahead of the buffered delta's
        // governance cut; resolving against the buffered (signed) cut,
        // not local state, is what preserves pre-removal write
        // validity on the replay path. See
        // `apply_authorized_state_delta` site for full prose.
        match membership_status_at(datastore, &buffered.author_id, pos) {
            Ok(MembershipStatus::Member(role)) => {
                debug!(
                    %context_id,
                    author = %buffered.author_id,
                    role = ?role,
                    group_id = ?pos.group_id,
                    "cross-DAG check (replay): author authorized at governance cut"
                );
            }
            Ok(MembershipStatus::Removed { last_role }) => {
                warn!(
                    %context_id,
                    author = %buffered.author_id,
                    last_role = ?last_role,
                    group_id = ?pos.group_id,
                    "cross-DAG check (replay): rejecting buffered delta — author was removed at governance cut"
                );
                return Ok(false);
            }
            Ok(MembershipStatus::NeverMember) => {
                warn!(
                    %context_id,
                    author = %buffered.author_id,
                    group_id = ?pos.group_id,
                    "cross-DAG check (replay): rejecting buffered delta — author is not a member at governance cut"
                );
                return Ok(false);
            }
            Ok(MembershipStatus::Unknown { needed }) => {
                // After snapshot sync the receiver is at-or-ahead of any
                // legitimate authoring cut; persistent Unknown here means
                // the position references heads we provably do not have.
                // Re-buffering into governance_pending would amount to a
                // permanent leak — drop with a warn. A future delta
                // referencing the same now-known position can still
                // re-deliver via gossip if it was legitimate.
                warn!(
                    %context_id,
                    author = %buffered.author_id,
                    group_id = ?pos.group_id,
                    needed_count = needed.len(),
                    "cross-DAG check (replay): governance heads still unknown after sync — dropping"
                );
                return Ok(false);
            }
            Err(err) => {
                warn!(
                    %context_id,
                    author = %buffered.author_id,
                    group_id = ?pos.group_id,
                    %err,
                    "cross-DAG check (replay): rejecting buffered delta — membership lookup failed"
                );
                return Ok(false);
            }
        }
    }

    // HLC fence (PR-3), parallel to `apply_authorized_state_delta`. The
    // snapshot-sync replay path does NOT funnel through that chokepoint —
    // it carries its own duplicated verification chain — so the fence is
    // applied here too. `BufferedDelta` now carries the stamped
    // `producing_app_key` through snapshot-sync buffering, so a stale-schema
    // delta buffered across a cascade migration is dropped on replay rather
    // than silently applied. `None` is unfenceable and falls through.
    if let Some(producing_app_key) = buffered.producing_app_key {
        if calimero_context::hlc_fence::delta_is_fenced(
            context_client.datastore(),
            &context_id,
            producing_app_key,
            buffered.hlc,
        )? {
            warn!(
                %context_id,
                author = %buffered.author_id,
                delta_id = ?delta_id,
                producing_app_key = %hex::encode(producing_app_key),
                "Dropping buffered state delta — HLC fence: stale schema after cascade migration"
            );
            crate::node_metrics::record_delta_outcome("fenced_stale_schema");
            return Ok(false);
        }
    }

    let group_key = {
        let store = context_client.datastore();
        let gid = calimero_context::group_store::get_group_for_context(store, &context_id)?;
        match gid {
            Some(g) => {
                // Issue #2256 — Open-subgroup namespace-key fallback,
                // mirroring the live-apply path above.
                //
                // Issue #2299 — DO NOT wait here. The buffered-replay
                // path is invoked by `SyncManager` in a sequential
                // loop after snapshot sync settles; by then any
                // legitimate `KeyDelivery` has already been applied.
                // A 3s wait per delta would multiply into multi-second
                // sync recovery stalls when replaying many deltas
                // whose keys were genuinely lost. Single-shot lookup
                // here, fall back to the existing rebroadcast/sync
                // recovery path on miss.
                lookup_group_key_with_wait(
                    &context_client,
                    &g,
                    &buffered.key_id,
                    std::time::Duration::ZERO,
                )
                .await?
                .ok_or_else(|| eyre::eyre!("no group key found for buffered delta"))?
            }
            None => {
                let identity = context_client
                    .get_identity(&context_id, &buffered.author_id)?
                    .ok_or_else(|| eyre::eyre!("no identity for buffered author"))?;
                identity
                    .sender_key
                    .ok_or_else(|| eyre::eyre!("no sender_key or group_key"))?
            }
        }
    };

    let actions = decrypt_delta_actions(buffered.payload, buffered.nonce, group_key)?;

    let delta = calimero_dag::CausalDelta {
        id: buffered.id,
        parents: buffered.parents,
        payload: actions,
        hlc: buffered.hlc,
        expected_root_hash: *buffered.root_hash,
        kind: calimero_dag::DeltaKind::Regular,
    };

    // Get or create delta store - use [0u8; 32] as genesis hash placeholder
    // The actual genesis doesn't matter much for replay since the DAG already has
    // checkpoints from snapshot sync
    let delta_store = node_state
        .delta_stores
        .entry(context_id)
        .or_insert_with(|| {
            crate::delta_store::DeltaStore::new(
                [0u8; 32],
                context_client.clone(),
                context_id,
                our_identity,
            )
        })
        .clone();

    // Load any persisted deltas first. If this is the first time this
    // context's DeltaStore has been created in the process (post-crash
    // restart, buffered-replay hits before a live delta), the load
    // also surfaces any handler events whose execution was interrupted
    // before they were cleared (#2185, #2194 review). Replay them
    // *before* processing the buffered delta so causal order is
    // preserved.
    let pending_from_load = match delta_store.load_persisted_deltas().await {
        Ok(result) => result.pending_handler_events,
        Err(e) => {
            warn!(
                ?e,
                %context_id,
                "Failed to load persisted deltas during buffered-delta replay"
            );
            Vec::new()
        }
    };
    if !pending_from_load.is_empty() {
        info!(
            %context_id,
            pending_count = pending_from_load.len(),
            "Replaying crash-interrupted handlers before buffered delta"
        );
        if let Err(e) = execute_cascaded_events(
            &pending_from_load,
            &node_client,
            &context_client,
            &context_id,
            &our_identity,
            sync_timeout,
            "buffered-replay crash recovery",
            None,
            &delta_store,
        )
        .await
        {
            warn!(
                ?e,
                %context_id,
                "Crash-recovery replay failed during buffered-delta replay; events stay in DB for next init"
            );
        }
    }

    // If this delta is covered by checkpoint (ancestor of checkpoint) but is NOT the checkpoint
    // itself, skip adding it to the DAG. Its state is already present via snapshot, and adding
    // it would just put it in the pending queue forever (since its parents don't exist).
    let is_checkpoint_match = delta_store.dag_has_delta_applied(&delta_id).await;

    let add_result = if is_covered_by_checkpoint && !is_checkpoint_match {
        // Skip DAG addition for covered ancestor deltas
        // Return a "not applied" result since we're not adding to DAG
        debug!(
            %context_id,
            delta_id = ?delta_id,
            "Skipping DAG addition for ancestor delta (state covered by checkpoint)"
        );
        crate::delta_store::AddDeltaResult {
            applied: false,
            cascaded_events: vec![],
        }
    } else {
        // Normal case: add delta to DAG with events for handler execution.
        // The buffered envelope carries author + governance_position
        // captured at the original gossip receive; persist them with the
        // row so subsequent DAG-catchup serves include the claim.
        let buffered_gov_blob = buffered
            .governance_position
            .as_ref()
            .and_then(|gp| borsh::to_vec(gp).ok());
        delta_store
            .add_delta_with_events(
                delta.clone(),
                buffered.events.clone(),
                Some(buffered.author_id),
                buffered_gov_blob,
                buffered.delta_signature,
            )
            .await?
    };

    // Re-check is_checkpoint_match after potential DAG add (for the case where we did add)
    let is_checkpoint_match =
        !add_result.applied && delta_store.dag_has_delta_applied(&delta_id).await;

    // Execute handlers if:
    // 1. Delta was applied (normal case), OR
    // 2. Delta matches a checkpoint (state exists via snapshot but handlers not yet run), OR
    // 3. Delta is covered by checkpoint (ancestor of checkpoint, state already in snapshot)
    //
    // Do NOT execute handlers if delta went to pending AND is NOT covered by checkpoint
    let should_execute_handlers =
        add_result.applied || is_checkpoint_match || is_covered_by_checkpoint;

    if should_execute_handlers {
        if let Some(events_data) = &buffered.events {
            let events_payload: Option<Vec<ExecutionEvent>> =
                match serde_json::from_slice(events_data) {
                    Ok(events) => Some(events),
                    Err(e) => {
                        warn!(
                            %context_id,
                            delta_id = ?delta_id,
                            error = %e,
                            "Failed to parse buffered events"
                        );
                        None
                    }
                };

            if let Some(events) = events_payload {
                // Check if we are the author (shouldn't be, but check anyway)
                let is_author = buffered.author_id == our_identity;
                if !is_author {
                    info!(
                        %context_id,
                        delta_id = ?delta_id,
                        events_count = events.len(),
                        applied_via_dag = add_result.applied,
                        is_checkpoint_match,
                        is_covered_by_checkpoint,
                        "Executing handlers for replayed buffered delta"
                    );

                    let all_succeeded = execute_event_handlers_parsed(
                        &context_client,
                        &context_id,
                        &our_identity,
                        &events,
                    )
                    .await?;

                    // Same clear-on-success contract as the other two
                    // caller sites: keep `events: Some(..)` if any
                    // handler failed so the next restart replays.
                    if all_succeeded {
                        delta_store.mark_events_executed(&delta_id);
                    } else {
                        warn!(
                            %context_id,
                            delta_id = ?delta_id,
                            "One or more handlers failed on buffered-replay path; keeping events in DB for restart replay"
                        );
                    }
                } else {
                    // Author path: handlers already ran locally at
                    // authoring time; clear the preserved blob so
                    // restart replay doesn't mistakenly run them
                    // again (#2194 review, High).
                    delta_store.mark_events_executed(&delta_id);
                }

                // Emit to WebSocket clients
                emit_state_mutation_event_parsed(
                    &node_client,
                    &context_id,
                    buffered.root_hash,
                    events,
                );
            }
        }
    } else {
        debug!(
            %context_id,
            delta_id = ?delta_id,
            has_events = buffered.events.is_some(),
            "Skipping handler execution for pending delta (will execute when delta is applied)"
        );
    }

    // Execute any cascaded handlers.
    // Same log-and-continue policy: a cascade failure must not mask an
    // otherwise-applied buffered delta. Failed handlers keep their events in
    // the DB for replay on the next init.
    if let Err(e) = execute_cascaded_events(
        &add_result.cascaded_events,
        &node_client,
        &context_client,
        &context_id,
        &our_identity,
        sync_timeout,
        "buffered delta replay",
        None,
        &delta_store,
    )
    .await
    {
        warn!(
            ?e,
            %context_id,
            "Cascade handler execution failed during buffered delta replay; events stay in DB for next init"
        );
    }

    Ok(add_result.applied)
}
