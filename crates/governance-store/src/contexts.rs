use crate::{MembershipRepository, NamespaceRepository};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::context_tree::ContextTreeService;

pub fn register_context_in_group(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<()> {
    ContextTreeService::new(store, *group_id).register_context(context_id)
}

pub fn unregister_context_from_group(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<()> {
    ContextTreeService::new(store, *group_id).unregister_context(context_id)
}

pub fn get_group_for_context(
    store: &Store,
    context_id: &ContextId,
) -> EyreResult<Option<ContextGroupId>> {
    ContextTreeService::new(store, ContextGroupId::from([0u8; 32])).group_for_context(context_id)
}

/// Returns `true` if `author` is currently an authorized **writer** for
/// `context_id`'s owning group, or if `context_id` is not registered to any
/// group (no group-membership constraint applies). The check includes the
/// namespace-creator admin-identity carve-out, mirroring `membership_status_at`.
///
/// Read-only roles (`ReadOnly`, `ReadOnlyTee`) are rejected here for parity
/// with the gossip path's `is_read_only_for_context` filter in
/// `state_delta::handle_state_delta` — without that filter, a read-only
/// identity could route a state mutation through HashComparison / LevelWise /
/// EntityPush (which call this helper) and have it merged on the receiver,
/// bypassing the role boundary that gossip enforces. The asymmetry between
/// "gossip rejects read-only writes" and "HC accepts read-only writes" was
/// a privilege-escalation surface; the read-only check below removes it.
///
/// ## Why current state, not membership-at-author-time
///
/// Used by sync apply paths (HashComparison EntityPush, snapshot apply,
/// LevelWise reconcile) that operate on per-leaf entities, not on the
/// signed delta envelope. The envelope carries `governance_position` —
/// the cited cut for `membership_status_at` — and we use it in the gossip
/// and DAG-catchup paths. Per-leaf HC entities are NOT signed individually
/// and the wire format deliberately does NOT attach a per-leaf governance
/// position (would balloon the per-entity overhead by an order of
/// magnitude for typical sync sessions). With no per-leaf cut to cite,
/// the only governance state the receiver can check against is its own
/// *current* view — there is no historical anchor on the wire to pin to.
///
/// This is a **defensible design choice**, not a known limitation:
///
/// * It mirrors the local-execute check in `is_authorized_for_context` —
///   both are "does this identity have current write permission?" at the
///   point the receiver makes a decision. The two checks (local write,
///   remote HC merge) use the same membership snapshot, so a node never
///   contradicts itself between "I wrote this" and "I'd accept this from
///   a peer."
/// * It's strict in the right direction: a removed-then-re-added author's
///   intermediate-history entities replay successfully on HC; a removed-
///   and-still-removed author's leaves do not. The gossip path's
///   `membership_status_at` is a *richer* signal — it can distinguish
///   "removed today but valid at sign time" from "never a member" — but
///   that richness depends on per-delta envelope metadata that doesn't
///   exist for HC leaves. We use what we have.
/// * The TOCTOU window between this check and the actual entity write is
///   the same window the local-execute path uses; if a member is removed
///   mid-merge, both paths converge on the post-removal view on the next
///   tick. Per-leaf signature replay isn't on the table — those leaves
///   are already authenticated against the per-entity `signature_data`
///   covered by `Interface::apply_action`.
///
/// Two nodes with identical DAG state but divergent local governance state
/// CAN reject different HC leaves — this is a real behaviour. But that's
/// the same behaviour the local-write path has (which is what HC is the
/// "receive mirror" of), and divergent governance state is itself
/// converged through gossip (governance ops use the same delivery
/// machinery as state deltas). The window is bounded by the same heartbeat
/// that bounds every other gossip-converged invariant.
pub fn is_currently_authorized_for_context(
    store: &Store,
    context_id: &ContextId,
    author: &PublicKey,
) -> EyreResult<bool> {
    let Some(group_id) = get_group_for_context(store, context_id)? else {
        return Ok(true);
    };
    // Namespace creator carve-out: the creator does not emit a self-
    // `MemberJoined` op at namespace genesis, so their membership lives in
    // `GroupMeta::admin_identity` rather than a `GroupMember` row. Without
    // this short-circuit, `check_group_membership` returns false for the
    // creator and HC would drop their legitimately-authored entities.
    if MembershipRepository::new(store).is_admin(&group_id, author)? {
        return Ok(true);
    }
    // Reject read-only roles up-front — `check_group_membership` returns
    // true for any `GroupMember` row regardless of role, so without this
    // gate a ReadOnly / ReadOnlyTee identity would author-launder a
    // state mutation through HC/LevelWise/EntityPush. The gossip path's
    // `is_read_only_for_context` filter (in `handle_state_delta`) is what
    // we're mirroring here.
    if NamespaceRepository::new(store).is_read_only_for_context(context_id, author)? {
        return Ok(false);
    }
    MembershipRepository::new(store).is_member(&group_id, author)
}

pub fn enumerate_group_contexts(
    store: &Store,
    group_id: &ContextGroupId,
    offset: usize,
    limit: usize,
) -> EyreResult<Vec<ContextId>> {
    ContextTreeService::new(store, *group_id).enumerate_contexts(offset, limit)
}

/// Internal helper intended to be used only from authorization-checked paths.
/// Callers must enforce the relevant governance permissions.
pub fn cascade_remove_member_from_group_tree(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<()> {
    ContextTreeService::new(store, *group_id).cascade_remove_member(member)
}

/// Inverse of [`cascade_remove_member_from_group_tree`]: re-create
/// `ContextIdentity` rows for the rejoiner under every context registered
/// directly beneath `group_id`.
///
/// Idempotent on rows that already carry a usable `private_key: Some(_)`
/// (e.g., a first-time join via the `join_context` handler beat the
/// apply-path call to here). A rejoiner who never had a `ContextIdentity`
/// row for a given context gets a freshly-written row with
/// `private_key: Some(_)` and `sender_key: None` — the same shape
/// `join_context` writes — so KeyDelivery can then populate `sender_key`.
/// A pre-existing row with `private_key: None` is repaired in place
/// (`private_key` filled, `sender_key` preserved) rather than skipped,
/// since a keyless row would leave the rejoiner unable to sign.
///
/// **Anti-spoof gate is enforced inside this function.** Writing a
/// `private_key: Some(_)` row for `member` would let the writing node
/// author state-DAG ops as `member`. The function therefore resolves
/// the namespace for `group_id`, reads THIS node's namespace identity,
/// and returns early (a no-op) unless the local identity *is*
/// `member` — i.e. this node genuinely owns the private key. The
/// private key is derived internally from that identity; callers
/// cannot pass in an arbitrary key. Both apply-path call sites
/// (`MemberAdded` in `mod.rs`, `MemberJoinedOpen` in
/// `namespace_governance.rs`) invoke this unconditionally and rely on
/// the internal gate — a future call site cannot accidentally omit it.
///
/// **Crash-consistency.** Rows are written one `put` at a time with
/// no batch transaction, so a crash mid-loop leaves a partial restore
/// (identity present for some contexts, absent for others). This is
/// self-healing, and the reason is the *ordering* of the apply
/// pipeline — not blind re-application. Both call sites run this
/// function as part of the op mutation, and the governance nonce /
/// DAG head only advances *after* the entire mutation returns:
/// `apply_local_signed_group_op` calls `apply_group_op_mutations`
/// (which contains this loop) and only then `persist_group_governance_progress`
/// (which advances the nonce); the `MemberJoinedOpen` path advances
/// the namespace-DAG head likewise after `apply_signed_op` completes.
/// So a crash that left rows unwritten necessarily crashed *before*
/// the nonce/head advanced — the op is therefore NOT yet
/// nonce-deduplicated and is re-applied on the next receipt, and the
/// idempotent loop here fills the remaining rows. (Conversely, once
/// the nonce advances and re-receipt becomes a no-op, the loop had
/// already completed — there is nothing left to heal.) Worst case if
/// that reasoning is ever broken by a refactor: the member calls
/// `join_context` for the affected context, which writes the row
/// directly. The symmetric `cascade_remove_member` uses the same
/// one-`handle`-loop pattern; if either is ever made transactional,
/// both should be.
///
/// **No concurrent-registration gap.** The enumerate and the write
/// loop use separate store handles, but a context cannot be
/// registered between them: governance ops for a namespace apply
/// sequentially through a single actor, so no `ContextRegistered`
/// can interleave with this `MemberAdded` / `MemberJoinedOpen`
/// apply. A context registered by a *later* governance op is a
/// no-op for membership — the rejoiner's row already exists by then,
/// and `register_context` does not touch `ContextIdentity`.
///
/// **Why `enumerate_group_contexts(.., 0, usize::MAX)` is fine here.**
/// The hot-path concern is unbounded reads. In this codebase the
/// number of contexts directly registered under a single
/// `ContextGroupId` is the count of contexts in one channel
/// (subgroup), which is bounded by application-level use — typically
/// 1, rarely more than a handful. The same unbounded-enumerate
/// pattern is used by `cascade_remove_member_from_group_tree` /
/// `ContextTreeService::cascade_remove_member` (see this file) and
/// has not surfaced as a memory or latency hotspot. If a future use
/// case starts pushing tens of contexts into a single subgroup, both
/// paths should be paginated together — they share the same
/// invariant.
pub fn restore_member_context_identities(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<()> {
    // Internal anti-spoof gate (see doc comment). Only the local
    // rejoiner's own node holds the namespace identity bytes for
    // `member`; on every other peer this resolves to a different pk
    // (or `None`) and the function is a no-op.
    let namespace_id = NamespaceRepository::new(store).resolve(group_id)?;
    let Some((local_pk, private_key, _sender_key)) =
        NamespaceRepository::new(store).identity(&namespace_id)?
    else {
        return Ok(());
    };
    if local_pk != *member {
        return Ok(());
    }

    let contexts = enumerate_group_contexts(store, group_id, 0, usize::MAX)?;
    let mut handle = store.handle();
    for context_id in &contexts {
        let identity_key = calimero_store::key::ContextIdentity::new(*context_id, (*member).into());
        // Three cases:
        //   * No row              → write a fresh `Some(private_key)` row.
        //   * Row, private_key None → repair it: the rejoiner can't sign
        //     with a `None` key. Overwrite `private_key` but PRESERVE
        //     `sender_key` so an already-delivered key isn't clobbered.
        //   * Row, private_key Some → leave untouched (idempotent — a
        //     prior `join_context` already wrote a usable row).
        // The `None` case shouldn't arise on the local rejoiner's own
        // store today (the cascade deletes the whole row, and the
        // anti-spoof gate above means peers never write a `None` row
        // for a member they don't own), but repairing it rather than
        // skipping keeps the restore robust against any future path
        // that leaves a keyless row behind.
        let existing = handle.get(&identity_key)?;
        let needs_write = match &existing {
            None => true,
            Some(row) => row.private_key.is_none(),
        };
        if needs_write {
            let sender_key = existing.and_then(|row| row.sender_key);
            handle.put(
                &identity_key,
                &calimero_store::types::ContextIdentity {
                    private_key: Some(private_key),
                    sender_key,
                },
            )?;
            tracing::info!(
                group_id = %hex::encode(group_id.to_bytes()),
                context_id = %hex::encode(context_id.as_ref()),
                member = %member,
                "rejoin: restored ContextIdentity row for local rejoiner"
            );
        }
    }
    Ok(())
}

/// Scans the ContextIdentity column for the given context and returns the first
/// `PublicKey` for which the node holds a local private key. Used to find a
/// valid signer when performing group upgrades on behalf of a context that the
/// group admin may not be a member of.
pub fn find_local_signing_identity(
    store: &Store,
    context_id: &ContextId,
) -> EyreResult<Option<PublicKey>> {
    ContextTreeService::new(store, ContextGroupId::from([0u8; 32]))
        .find_local_signing_identity(context_id)
}
