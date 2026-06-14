use crate::{
    CapabilitiesRepository, DenyListRepository, MembershipRepository, MetaRepository,
    MetadataRepository, SigningKeysRepository, UpgradeLadderRepository, UpgradesRepository,
};
use calimero_context_client::local_governance::SignedGroupOp;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{
    GroupLocalGovNonce, GroupLocalGovNonceWindow, GroupLocalGovNonceWindowValue,
    GroupMemberContext, GroupOpHead, GroupOpHeadValue, GroupOpLog, NamespaceGovHead,
    NamespaceGovOp, NamespaceIdentity, GROUP_MEMBER_CONTEXT_PREFIX, GROUP_OP_LOG_PREFIX,
    NAMESPACE_GOV_OP_PREFIX,
};
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::collect_keys_with_prefix;
use crate::nonce_window::NonceWindow;

/// Read the contiguous applied-nonce floor for a (group, signer), or `None` if
/// nothing has ever been persisted.
///
/// Reads the floor out of the authoritative [`GroupLocalGovNonceWindow`],
/// falling back to the legacy [`GroupLocalGovNonce`] high-water mark for
/// databases written before the window existed. It returns the FLOOR ONLY —
/// callers that need to know whether a specific (possibly out-of-order) nonce
/// was applied must use [`load_nonce_window`] + [`NonceWindow::contains`].
pub fn get_local_gov_nonce(
    store: &Store,
    group_id: &ContextGroupId,
    signer: &PublicKey,
) -> EyreResult<Option<u64>> {
    let handle = store.handle();
    let gid = group_id.to_bytes();
    if let Some(window) = handle.get(&GroupLocalGovNonceWindow::new(gid, *signer))? {
        return Ok(Some(window.floor));
    }
    // Legacy fallback: a pre-window database only has the single-`u64` key.
    Ok(handle.get(&GroupLocalGovNonce::new(gid, *signer))?)
}

/// Force the persisted window to a bare floor with an empty above-set.
///
/// This is a coarse, floor-only seed — it CLOBBERS any out-of-order above-floor
/// nonces — so it is NOT for the apply path; production advances go through
/// [`store_nonce_window`], which persists the recorded window. It survives for
/// migration seeding and for tests that need to force a specific floor (e.g.
/// rolling it back to simulate a window that lost an entry). Writes the
/// authoritative [`GroupLocalGovNonceWindow`] so [`get_local_gov_nonce`] and
/// [`load_nonce_window`] observe it.
pub fn set_local_gov_nonce(
    store: &Store,
    group_id: &ContextGroupId,
    signer: &PublicKey,
    nonce: u64,
) -> EyreResult<()> {
    store_nonce_window(store, group_id, signer, &NonceWindow::new(nonce, []))
}

/// Load the applied-nonce window for a (group, signer).
///
/// Reads the authoritative [`GroupLocalGovNonceWindow`] (floor + above-set in
/// one value). If it is absent, migrates from the legacy [`GroupLocalGovNonce`]
/// floor — a pre-window database loads as `floor` with an empty above-set and
/// dedups exactly like the old `nonce <= last` high-water-mark guard. Both
/// absent → a fresh `(0, {})` window.
pub fn load_nonce_window(
    store: &Store,
    group_id: &ContextGroupId,
    signer: &PublicKey,
) -> EyreResult<NonceWindow> {
    let handle = store.handle();
    let gid = group_id.to_bytes();
    if let Some(window) = handle.get(&GroupLocalGovNonceWindow::new(gid, *signer))? {
        return Ok(NonceWindow::new(window.floor, window.above));
    }
    let floor = handle
        .get(&GroupLocalGovNonce::new(gid, *signer))?
        .unwrap_or(0);
    Ok(NonceWindow::new(floor, []))
}

/// Persist an applied-nonce window under [`GroupLocalGovNonceWindow`] as a
/// SINGLE value, so the whole window (floor + above-set) lands in ONE atomic
/// `put` — there is no cross-key crash window. A crash either leaves the prior
/// value intact or commits the new one; it can never observe a half-written
/// floor-without-above state, so neither an already-applied nonce can be
/// resurrected as unapplied nor an unapplied one skipped.
///
/// The legacy [`GroupLocalGovNonce`] key is intentionally NOT written: it is
/// read-only migration state, superseded by the window value on first store.
pub fn store_nonce_window(
    store: &Store,
    group_id: &ContextGroupId,
    signer: &PublicKey,
    window: &NonceWindow,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupLocalGovNonceWindow::new(group_id.to_bytes(), *signer);
    handle.put(
        &key,
        &GroupLocalGovNonceWindowValue {
            floor: window.floor(),
            above: window.above().collect(),
        },
    )?;
    Ok(())
}

fn delete_local_gov_nonce_for_signer(
    store: &Store,
    group_id: &ContextGroupId,
    signer: &PublicKey,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let gid = group_id.to_bytes();
    handle.delete(&GroupLocalGovNonceWindow::new(gid, *signer))?;
    // Drop the legacy floor key too, so a future signer reusing this
    // (group, signer) doesn't inherit a stale migration floor.
    handle.delete(&GroupLocalGovNonce::new(gid, *signer))?;
    Ok(())
}

/// Delete all [`GroupLocalGovNonce`] rows for current group members (best-effort; ignores missing).
fn delete_local_gov_nonces_for_listed_members(
    store: &Store,
    group_id: &ContextGroupId,
    members: &[(PublicKey, GroupMemberRole)],
) -> EyreResult<()> {
    for (pk, _) in members {
        if let Err(err) = delete_local_gov_nonce_for_signer(store, group_id, pk) {
            tracing::debug!(
                group_id = %hex::encode(group_id.to_bytes()),
                member = %pk,
                ?err,
                "best-effort nonce cleanup failed"
            );
        }
    }
    Ok(())
}

pub fn get_op_head(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<GroupOpHeadValue>> {
    let handle = store.handle();
    let key = GroupOpHead::new(group_id.to_bytes());
    handle.get(&key).map_err(Into::into)
}

#[cfg(test)]
pub(crate) fn set_op_head(
    store: &Store,
    group_id: &ContextGroupId,
    sequence: u64,
    dag_heads: Vec<[u8; 32]>,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupOpHead::new(group_id.to_bytes());
    handle.put(
        &key,
        &GroupOpHeadValue {
            sequence,
            dag_heads,
        },
    )?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn append_op_log_entry(
    store: &Store,
    group_id: &ContextGroupId,
    sequence: u64,
    op_bytes: &[u8],
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupOpLog::new(group_id.to_bytes(), sequence);
    handle.put(&key, &op_bytes.to_vec())?;
    Ok(())
}

/// Append an op to the group op-log and advance the op head, WITHOUT
/// touching the per-signer nonce.
///
/// Used by the namespace-governance apply path
/// (`namespace_governance::apply_group_op_inner`), which manages the nonce
/// itself via `set_local_gov_nonce`. The authoring path uses
/// [`persist_group_governance_progress`] instead, which also writes the
/// nonce in the same batch. Keeping these separate avoids the
/// namespace-governance path double-writing the nonce.
///
/// CRASH-SAFETY INVARIANT (this path writes two keys non-atomically):
/// the two `Store::handle()` puts below write straight through to the
/// backend, so a crash can land between them. An atomic alternative now
/// exists — `Store::apply` (and `StoreBatch`) commit a multi-key
/// `Transaction` as one `WriteBatch` — but migrating this path onto it is
/// deferred (see the closing note); the ordered-write reasoning below still
/// governs the current code. The write ORDER is therefore chosen to be
/// crash-safe: the op-log ENTRY is written first, the `GroupOpHead` second.
///
/// - Crash after entry, before head: an ORPHAN log entry exists at
///   `sequence` while the head still points at `sequence - 1`. This is
///   benign — every reader scans the log directly (`read_op_log_after`,
///   `read_tee_admission_policy`, `is_quote_hash_used`), so the entry is
///   already visible; and the replica apply path derives `next_seq` from
///   [`max_op_log_sequence`] (the actual max persisted sequence), NOT from
///   this possibly-stale head, so the next op lands strictly above the orphan
///   and never overwrites it. (The authoring side still derives `next_seq`
///   from the head, but a crash there leaves an entry this node authored with
///   its nonce un-advanced, so the next authored op re-derives the identical
///   op and an overwrite is an idempotent self-replay.)
/// - The reverse order (head first) would be UNSAFE: a crash would leave a
///   head whose `sequence` references a log entry that was never written,
///   so `read_op_log_after` would silently skip the gap and the op-head's
///   `dag_heads` would advertise a frontier op nobody can read back.
///
/// This mirrors the entry-then-head ordering the authoring side uses
/// (`persist_group_governance_progress` below) and the head-advance /
/// store-operation ordering note in
/// `namespace_governance::apply_signed_op`. Migrating this to a single
/// atomic batch — now that `Store::apply` / `StoreBatch` provide one — would
/// remove the orphan-entry window entirely; it is left as a follow-up so the
/// crash-safety reasoning here can be retired rather than merely relocated.
pub(crate) fn persist_group_op_log_entry(
    store: &Store,
    group_id: &ContextGroupId,
    sequence: u64,
    dag_heads: Vec<[u8; 32]>,
    op_bytes: &[u8],
) -> EyreResult<()> {
    let gid = group_id.to_bytes();
    let mut handle = store.handle();

    // Entry first (see CRASH-SAFETY INVARIANT above): an orphan entry is
    // benign; a head referencing a missing entry is not.
    let op_log_key = GroupOpLog::new(gid, sequence);
    handle.put(&op_log_key, &op_bytes.to_vec())?;

    let head_key = GroupOpHead::new(gid);
    handle.put(
        &head_key,
        &GroupOpHeadValue {
            sequence,
            dag_heads,
        },
    )?;

    Ok(())
}

/// Authoring-side variant of [`persist_group_op_log_entry`] that ALSO advances
/// the per-(group, signer) nonce in the same call.
///
/// The two paths share the op-log entry + head write (delegated to
/// [`persist_group_op_log_entry`]) but differ in nonce handling: the authoring
/// path owns the nonce here, whereas the namespace-governance apply path
/// manages it separately via `set_local_gov_nonce` (it advances the nonce only
/// AFTER the full op apply succeeds — see the invariant comment in
/// `apply_group_op_inner`). The nonce `put` runs LAST so the same crash-safety
/// ordering holds: entry → head → nonce. An un-advanced nonce after a crash
/// just replays the (idempotent) op; it never skips one.
pub(crate) fn persist_group_governance_progress(
    store: &Store,
    group_id: &ContextGroupId,
    sequence: u64,
    signer: &PublicKey,
    window: &NonceWindow,
    dag_heads: Vec<[u8; 32]>,
    op_bytes: &[u8],
) -> EyreResult<()> {
    persist_group_op_log_entry(store, group_id, sequence, dag_heads, op_bytes)?;

    // Nonce window written LAST so the crash-safety ordering holds:
    // entry → head → floor → above. An un-advanced window after a crash just
    // replays the (idempotent) op; it never skips one.
    store_nonce_window(store, group_id, signer, window)?;

    Ok(())
}

pub fn read_op_log_after(
    store: &Store,
    group_id: &ContextGroupId,
    after_sequence: u64,
    limit: usize,
) -> EyreResult<Vec<(u64, Vec<u8>)>> {
    let gid = group_id.to_bytes();
    let start_seq = after_sequence.saturating_add(1);
    let keys = collect_keys_with_prefix(
        store,
        GroupOpLog::new(gid, start_seq),
        GROUP_OP_LOG_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let handle = store.handle();
    let mut results = Vec::new();

    for key in keys.into_iter().take(limit) {
        let Some(op_bytes) = handle.get(&key)? else {
            continue;
        };
        results.push((key.sequence(), op_bytes));
    }

    Ok(results)
}

/// The highest sequence number present in the group's op-log, or `None` if the
/// log is empty.
///
/// Derived by scanning the persisted op-log rather than reading
/// `GroupOpHeadValue.sequence`, so it is correct even when the head is stale
/// relative to the log — e.g. after a crash that landed between the entry `put`
/// and the head `put` in [`persist_group_op_log_entry`] (see the CRASH-SAFETY
/// INVARIANT there). The replica apply path uses this to derive `next_seq` so a
/// new op never reuses a sequence already occupied by an orphan entry, which
/// would silently overwrite it.
///
/// Keys are big-endian on the sequence component, so the op-log iterates in
/// ascending order and the last entry carries the max; cost is the same O(n)
/// governance-only scan the other log readers already pay.
pub(crate) fn max_op_log_sequence(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<u64>> {
    let gid = group_id.to_bytes();
    let keys =
        collect_keys_with_prefix(store, GroupOpLog::new(gid, 1), GROUP_OP_LOG_PREFIX, |k| {
            k.group_id() == gid
        })?;
    Ok(keys.last().map(GroupOpLog::sequence))
}

/// Whether the group op-log already holds an entry whose op has the given
/// `content_hash`.
///
/// This is the durable dedup signal for the replica apply path
/// (`namespace_governance::apply_group_op_inner`). It scans the persisted
/// op-log — the same column the readers (`read_tee_admission_policy`,
/// `is_quote_hash_used`, `is_tee_admitted_identity`) scan — rather than
/// consulting the op-head's `dag_heads`. `dag_heads` only tracks the CURRENT
/// frontier: once a later op supersedes an earlier one, the earlier op's
/// content hash is pruned from the head set, so a head-based check would
/// wrongly report a superseded-then-re-received op as "not yet logged" and
/// append a second copy — skewing every log scan. Keying the check on the
/// persisted log makes it monotonic: an op that was ever logged stays logged.
///
/// Cost is an O(n) scan over the group's governance op-log (governance ops
/// only — not state-DAG traffic), matching what the readers already pay; the
/// per-(group, signer) nonce guard in `apply_group_op_inner` short-circuits
/// the common re-receive before this is reached, so this is the backstop for
/// the retry/backfill path that re-applies without having advanced the nonce.
pub(crate) fn op_log_contains_content_hash(
    store: &Store,
    group_id: &ContextGroupId,
    content_hash: &[u8; 32],
) -> EyreResult<bool> {
    let entries = read_op_log_after(store, group_id, 0, usize::MAX)?;
    for (_seq, bytes) in &entries {
        let Ok(op) = borsh::from_slice::<SignedGroupOp>(bytes) else {
            continue;
        };
        if op
            .content_hash()
            .map(|h| h == *content_hash)
            .unwrap_or(false)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn delete_op_log_and_head(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    const BATCH_SIZE: usize = 1000;
    let mut after_sequence = 0u64;
    loop {
        let batch = read_op_log_after(store, group_id, after_sequence, BATCH_SIZE)?;
        if batch.is_empty() {
            break;
        }
        let mut handle = store.handle();
        for (seq, _) in batch {
            let key = GroupOpLog::new(group_id.to_bytes(), seq);
            handle.delete(&key)?;
            after_sequence = seq;
        }
    }
    let mut handle = store.handle();
    let head_key = GroupOpHead::new(group_id.to_bytes());
    handle.delete(&head_key)?;
    Ok(())
}

pub fn track_member_context_join(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
    context_id: &ContextId,
    context_identity: [u8; 32],
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupMemberContext::new(group_id.to_bytes(), *member, *context_id);
    handle.put(&key, &context_identity)?;
    Ok(())
}

pub fn get_member_context_joins(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<Vec<(ContextId, [u8; 32])>> {
    let gid = group_id.to_bytes();
    let member_pk = *member;
    let keys = collect_keys_with_prefix(
        store,
        GroupMemberContext::new(gid, member_pk, ContextId::from([0u8; 32])),
        GROUP_MEMBER_CONTEXT_PREFIX,
        |k| k.group_id() == gid && k.member() == member_pk,
    )?;
    let handle = store.handle();
    let mut results = Vec::new();

    for key in keys {
        let Some(ctx_identity) = handle.get(&key)? else {
            continue;
        };
        results.push((key.context_id(), ctx_identity));
    }

    Ok(results)
}

pub fn remove_all_member_context_joins(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<Vec<(ContextId, [u8; 32])>> {
    let joins = get_member_context_joins(store, group_id, member)?;
    let mut handle = store.handle();
    for (context_id, _) in &joins {
        let key = GroupMemberContext::new(group_id.to_bytes(), *member, *context_id);
        handle.delete(&key)?;
    }
    Ok(joins)
}

/// Remove all local rows for a group (group meta, members, caps, metadata
/// records, ...).
/// Caller must enforce admin authorization and `count_group_contexts == 0`.
pub fn delete_group_local_rows(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let members_snapshot = MembershipRepository::new(store).list(group_id, 0, usize::MAX)?;
    delete_local_gov_nonces_for_listed_members(store, group_id, &members_snapshot)?;

    for (pk, _) in &members_snapshot {
        if let Err(err) = remove_all_member_context_joins(store, group_id, pk) {
            tracing::debug!(
                group_id = %hex::encode(group_id.to_bytes()),
                member = %pk,
                ?err,
                "best-effort member-context cleanup failed"
            );
        }
    }

    for (identity, _) in &members_snapshot {
        MembershipRepository::new(store).remove_member(group_id, identity)?;
    }

    CapabilitiesRepository::new(store).delete_all_member_caps(group_id)?;
    MetadataRepository::new(store).delete_all_members(group_id)?;
    CapabilitiesRepository::new(store).delete_default(group_id)?;
    CapabilitiesRepository::new(store).delete_subgroup_visibility(group_id)?;
    MetadataRepository::new(store).delete_group(group_id)?;
    UpgradesRepository::new(store).delete(group_id)?;
    UpgradeLadderRepository::new(store).delete(group_id)?;
    SigningKeysRepository::new(store).delete_all_for_group(group_id)?;
    DenyListRepository::new(store).clear_all_for_group(group_id)?;
    delete_op_log_and_head(store, group_id)?;
    MetaRepository::new(store).delete(group_id)?;
    Ok(())
}

/// Remove this node's namespace-level state: the signing identity, the
/// governance DAG head, and every stored governance op for the namespace.
///
/// Complements [`delete_group_local_rows`], which handles per-group rows.
/// The namespace root is itself a group, so a full namespace teardown calls
/// `delete_group_local_rows` for every group in the subtree (including the
/// root) and then this function to remove the namespace-scoped rows.
///
/// Ops are swept in batches to avoid materializing a large namespace log at
/// once. Each batch opens its own store handle so the iterator sees the
/// previous deletes committed.
pub fn delete_namespace_local_state(
    store: &Store,
    namespace_id: &ContextGroupId,
) -> EyreResult<()> {
    const BATCH_SIZE: usize = 1000;
    let ns_bytes = namespace_id.to_bytes();

    loop {
        let batch = super::collect_keys_with_prefix_paginated::<NamespaceGovOp>(
            store,
            NamespaceGovOp::new(ns_bytes, [0u8; 32]),
            NAMESPACE_GOV_OP_PREFIX,
            |k| k.namespace_id() == ns_bytes,
            0,
            BATCH_SIZE,
        )?;
        if batch.is_empty() {
            break;
        }
        let mut handle = store.handle();
        for key in batch {
            handle.delete(&key)?;
        }
    }

    let mut handle = store.handle();
    handle.delete(&NamespaceGovHead::new(ns_bytes))?;
    handle.delete(&NamespaceIdentity::new(ns_bytes))?;
    Ok(())
}
