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
/// self-healing: the function is idempotent and the apply path that
/// calls it re-runs on every receipt of the governance op (including
/// catch-up sync re-application), so the next apply completes the
/// remaining rows. The symmetric `cascade_remove_member` uses the
/// same one-`handle`-loop pattern; if either is ever made
/// transactional, both should be.
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
    let namespace_id = super::resolve_namespace(store, group_id)?;
    let Some((local_pk, private_key, _sender_key)) =
        super::get_namespace_identity(store, &namespace_id)?
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
