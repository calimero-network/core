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

/// Inverse of [`cascade_remove_member_from_group_tree`]: re-create the local
/// rejoiner's `ContextIdentity` **membership marker** under every context
/// registered directly beneath `group_id`.
///
/// The marker is *keyless* (`private_key: None`). Its presence is what tells the
/// signing path "this node is a member here"; the actual signing key is resolved
/// live from the node's namespace identity at read time (see the client-side
/// `resolve_owned_namespace_signer`), so no per-context key copy is stored. A
/// prior `MemberRemoved` / `MemberLeft` cascade deletes the marker; this restores
/// it so the rejoiner can author again the moment the member row is back.
///
/// **Scoped to the local rejoiner.** Only re-create the marker on the node whose
/// namespace identity *is* `member` — on every other peer this resolves to a
/// different identity (or `None`) and the function is a no-op. Peers re-learn a
/// member's marker through the ordinary sync/registration paths, not here. Both
/// apply-path call sites (`MemberAdded` in `mod.rs`, `MemberJoinedOpen` in
/// `namespace_governance.rs`) invoke this unconditionally and rely on the gate.
///
/// An existing row is left untouched: a standalone context's *keyed* row must
/// not be clobbered, and an already-present marker needs no rewrite.
///
/// **Crash-consistency.** Rows are written one `put` at a time with no batch
/// transaction, so a crash mid-loop leaves a partial restore. This is
/// self-healing because of the apply-pipeline *ordering*: both call sites run
/// this as part of the op mutation, and the governance nonce / DAG head only
/// advances *after* the mutation returns. A crash that left markers unwritten
/// therefore crashed before the nonce/head advanced, so the op is not yet
/// nonce-deduplicated and re-applies on the next receipt, filling the rest. The
/// symmetric `cascade_remove_member` uses the same one-`handle`-loop pattern; if
/// either is ever made transactional, both should be.
///
/// **No concurrent-registration gap.** The enumerate and the write loop use
/// separate store handles, but a context cannot be registered between them:
/// governance ops for a namespace apply sequentially through a single actor, so
/// no `ContextRegistered` can interleave with this apply.
///
/// **Why `enumerate_group_contexts(.., 0, usize::MAX)` is fine here.** The number
/// of contexts directly registered under a single `ContextGroupId` is bounded by
/// application use (typically 1, rarely a handful). The same unbounded-enumerate
/// pattern is used by `cascade_remove_member`; if a future use case pushes tens
/// of contexts into one subgroup, both paths should be paginated together.
pub fn restore_member_context_identities(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<()> {
    // Scope gate (see doc comment). Only the local rejoiner's own node holds the
    // namespace identity for `member`; on every other peer this resolves to a
    // different pk (or `None`) and the function is a no-op.
    let namespace_id = NamespaceRepository::new(store).resolve(group_id)?;
    let Some((local_pk, _private_key, _sender_key)) =
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
        let identity_key = calimero_store::key::ContextIdentity::new(*context_id, *member);
        // Only write when there is no row at all. A prior cascade deleted the
        // marker, so re-create it keyless. Leave any existing row untouched — a
        // standalone keyed row or an already-present marker must not be clobbered.
        if handle.get(&identity_key)?.is_none() {
            handle.put(
                &identity_key,
                &calimero_store::types::ContextIdentity {
                    private_key: None,
                    sender_key: None,
                },
            )?;
            tracing::info!(
                group_id = %hex::encode(group_id.to_bytes()),
                context_id = %hex::encode(context_id.as_ref()),
                member = %member,
                "rejoin: restored ContextIdentity membership marker for local rejoiner"
            );
        }
    }
    Ok(())
}

/// The node's namespace identity `PublicKey` for `context_id`, if it holds a
/// membership marker row in that context.
///
/// Namespace-backed contexts store a *keyless* `ContextIdentity` marker per
/// membership; the signing key is resolved live from the node's namespace
/// identity. A key scan (`private_key.is_some()`) misses these markers, so the
/// signer-finders below consult this to include the node's namespace identity.
/// Gated on the marker row's presence — the same row-presence membership signal
/// the client uses — so a removed member (whose marker the cascade deleted) is
/// not returned.
fn owned_namespace_marker(store: &Store, context_id: &ContextId) -> EyreResult<Option<PublicKey>> {
    let Some(group_id) = get_group_for_context(store, context_id)? else {
        return Ok(None); // standalone context — no namespace identity
    };
    let namespace_id = NamespaceRepository::new(store).resolve(&group_id)?;
    let Some((local_pk, _private_key, _sender_key)) =
        NamespaceRepository::new(store).identity(&namespace_id)?
    else {
        return Ok(None); // this node holds no identity for the namespace
    };
    let marker = calimero_store::key::ContextIdentity::new(*context_id, local_pk);
    if store.handle().has(&marker)? {
        Ok(Some(local_pk))
    } else {
        Ok(None)
    }
}

/// The private signing key this node holds for `(context_id, public_key)`, or
/// `None` if it holds none here.
///
/// A stored key wins (standalone / `new_identity` contexts). Otherwise, when a
/// keyless membership marker is present and `public_key` is the node's namespace
/// identity, the key is resolved live from that namespace identity. Mirrors the
/// client-side `ContextClient::get_identity` key resolution, for callers in
/// crates that read the store directly rather than through the context client
/// (e.g. sync proof-of-possession, migration authorization). Row presence is the
/// membership gate: no marker row → `None`.
pub fn resolve_local_signing_key(
    store: &Store,
    context_id: &ContextId,
    public_key: &PublicKey,
) -> EyreResult<Option<[u8; 32]>> {
    let marker = calimero_store::key::ContextIdentity::new(*context_id, *public_key);
    let Some(row) = store.handle().get(&marker)? else {
        return Ok(None); // no membership marker → not a local identity here
    };
    if let Some(sk) = row.private_key {
        return Ok(Some(sk));
    }
    // Keyless marker: resolve the key from the namespace identity when this pk is it.
    let Some(group_id) = get_group_for_context(store, context_id)? else {
        return Ok(None);
    };
    let namespace_id = NamespaceRepository::new(store).resolve(&group_id)?;
    match NamespaceRepository::new(store).identity(&namespace_id)? {
        Some((ns_pk, ns_sk, _sender)) if ns_pk == *public_key => Ok(Some(ns_sk)),
        _ => Ok(None),
    }
}

/// Returns a `PublicKey` this node can sign with for `context_id`. Prefers a
/// `ContextIdentity` row that carries a stored private key (standalone contexts
/// keep their own), then falls back to the node's namespace identity when a
/// keyless membership marker is present. Used to find a valid signer when
/// performing group upgrades on behalf of a context that the group admin may not
/// be a member of.
pub fn find_local_signing_identity(
    store: &Store,
    context_id: &ContextId,
) -> EyreResult<Option<PublicKey>> {
    if let Some(pk) = ContextTreeService::new(store, ContextGroupId::from([0u8; 32]))
        .find_local_signing_identity(context_id)?
    {
        return Ok(Some(pk));
    }
    owned_namespace_marker(store, context_id)
}

/// Returns EVERY `PublicKey` this node can sign with for `context_id`: every
/// `ContextIdentity` row with a stored private key, plus the node's namespace
/// identity when a keyless membership marker is present. Used by `leave_context`,
/// which must tombstone all of the node's identities in a context, not just the
/// first one.
pub fn find_local_signing_identities(
    store: &Store,
    context_id: &ContextId,
) -> EyreResult<Vec<PublicKey>> {
    let mut identities = ContextTreeService::new(store, ContextGroupId::from([0u8; 32]))
        .find_local_signing_identities(context_id)?;
    if let Some(pk) = owned_namespace_marker(store, context_id)? {
        if !identities.contains(&pk) {
            identities.push(pk);
        }
    }
    Ok(identities)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_primitives::context::ContextId;
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::{key, types, Store};

    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::{MemberCapabilities, VisibilityMode};
    use calimero_primitives::context::GroupMemberRole;

    use super::{
        find_local_signing_identities, find_local_signing_identity,
        is_currently_authorized_for_context, register_context_in_group,
    };
    use crate::test_fixtures::{nest_for_test, sample_meta_with_admin};
    use crate::{CapabilitiesRepository, MembershipRepository, MetaRepository};

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn put_identity(store: &Store, context: &ContextId, member: &PublicKey, has_private: bool) {
        let mut handle = store.handle();
        handle
            .put(
                &key::ContextIdentity::new(*context, *member),
                &types::ContextIdentity {
                    private_key: has_private.then_some([0x77; 32]),
                    sender_key: None,
                },
            )
            .expect("put identity");
    }

    #[test]
    fn returns_all_identities_holding_a_private_key() {
        let store = store();
        let context = ContextId::from([0x11; 32]);
        let a = PublicKey::from([0x01; 32]);
        let b = PublicKey::from([0x02; 32]);
        let keyless = PublicKey::from([0x03; 32]);
        // Row belonging to a DIFFERENT context — must be excluded.
        let other_ctx = ContextId::from([0x22; 32]);
        let other_member = PublicKey::from([0x04; 32]);

        put_identity(&store, &context, &a, true);
        put_identity(&store, &context, &b, true);
        put_identity(&store, &context, &keyless, false);
        put_identity(&store, &other_ctx, &other_member, true);

        let mut got = find_local_signing_identities(&store, &context).expect("enumerate");
        got.sort();
        assert_eq!(got, vec![a, b]);

        // The singular helper still returns just one of them (the first).
        let one = find_local_signing_identity(&store, &context).expect("single");
        assert!(one == Some(a) || one == Some(b));
    }

    #[test]
    fn returns_empty_when_no_local_key() {
        let store = store();
        let context = ContextId::from([0x11; 32]);
        put_identity(&store, &context, &PublicKey::from([0x01; 32]), false);
        assert!(find_local_signing_identities(&store, &context)
            .expect("enumerate")
            .is_empty());
    }

    // -----------------------------------------------------------------
    // Open -> Restricted flip-back at the *authorization* surface
    // (PR #3267 review comments 3593200482 / 3593200484).
    //
    // `membership::tests::flip_back_*` pin that `check_path` stops
    // resolving an inherited-only member. These pin what actually gates
    // apply: `is_currently_authorized_for_context`, which HC/LevelWise
    // reach through `is_leaf_currently_authorized`.
    // -----------------------------------------------------------------

    /// Root-admitted inherited-only member with a context registered
    /// under an `Open` subgroup. Returns (root, sub, context, member).
    fn seed_inherited_context_open(
        store: &Store,
    ) -> (ContextGroupId, ContextGroupId, ContextId, PublicKey) {
        let root = ContextGroupId::from([0xB0; 32]);
        let sub = ContextGroupId::from([0xB1; 32]);
        let context = ContextId::from([0xB2; 32]);
        let admin = PublicKey::from([0xEE; 32]);
        let tee = PublicKey::from([0x01; 32]);

        // Distinct admin so the member is never short-circuited by the
        // `is_admin` creator carve-out in the function under test.
        MetaRepository::new(store)
            .save(&root, &sample_meta_with_admin(admin))
            .expect("save root meta");
        MetaRepository::new(store)
            .save(&sub, &sample_meta_with_admin(admin))
            .expect("save sub meta");
        nest_for_test(store, &root, &sub);
        MembershipRepository::new(store)
            .add_member(&root, &tee, GroupMemberRole::Member)
            .expect("add root member");
        CapabilitiesRepository::new(store)
            .set_member_capability(
                &root,
                &tee,
                MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
            )
            .expect("grant join cap");
        CapabilitiesRepository::new(store)
            .set_subgroup_visibility(&sub, VisibilityMode::Open)
            .expect("open sub");
        register_context_in_group(store, &sub, &context).expect("register context");
        (root, sub, context, tee)
    }

    /// The authorization surface that actually gates apply must deny an
    /// inherited-only member once the subgroup walls back off.
    #[test]
    fn flip_back_to_restricted_revokes_context_authorization() {
        let store = store();
        let (_root, sub, context, tee) = seed_inherited_context_open(&store);

        assert!(
            is_currently_authorized_for_context(&store, &context, &tee).unwrap(),
            "precondition: an Open subgroup authorizes the inherited member"
        );

        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&sub, VisibilityMode::Restricted)
            .unwrap();

        assert!(
            !is_currently_authorized_for_context(&store, &context, &tee).unwrap(),
            "flip-back must deny the inherited member at the apply gate"
        );
    }

    /// The residue the TODO is really about: the flip-back revokes
    /// *authorization* but leaves the local `ContextIdentity` join row
    /// behind. Nothing prunes it — this is the documented gap, and it is
    /// a cleanup concern, NOT an authorization bypass, because the gate
    /// above denies regardless of the row's presence.
    #[test]
    fn flip_back_to_restricted_leaves_stale_context_identity_row() {
        let store = store();
        let (_root, sub, context, tee) = seed_inherited_context_open(&store);
        // The join that happened while the subgroup was Open.
        put_identity(&store, &context, &tee, true);

        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&sub, VisibilityMode::Restricted)
            .unwrap();

        // The row survives...
        assert!(
            store
                .handle()
                .has(&key::ContextIdentity::new(context, tee))
                .unwrap(),
            "the join row is not pruned on flip-back — the real gap"
        );
        // ...but it confers nothing: authorization is resolved live.
        assert!(
            !is_currently_authorized_for_context(&store, &context, &tee).unwrap(),
            "a surviving join row must not confer authorization after the wall is back up"
        );
    }
}
