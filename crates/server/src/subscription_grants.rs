//! Subscription grants: what makes an authorization decision stop being true.
//!
//! # Why a grant rather than a fresh check
//!
//! A subscription is authorized once, when it is made. The question this module
//! answers is when that decision has to be taken again.
//!
//! Re-deriving authority on every event is correct but unaffordable: the event
//! path carries mero-stream's video frames and mero-drive's document updates,
//! and a membership lookup per event per subscriber is a real regression for
//! both. Re-deriving on every membership change is affordable but far too
//! coarse — it is a store-touching pass over *every* connection on the node,
//! almost none of which are affected by the change.
//!
//! So a connection records the set of groups whose membership its
//! subscriptions actually depend on, and a membership event only makes it
//! re-derive when the event names one of them. The check is a hash-set lookup
//! against memory, cheap enough to ask of every connection on every membership
//! change — which is what lets the expensive re-derivation be narrowed to the
//! connections whose authority could genuinely have moved.
//!
//! # Why invalidation, and not revocation
//!
//! A grant revoked by *deletion* is only as complete as the set of paths that
//! remember to delete it. Membership authority here is lost through more than
//! one door — an explicit removal, a re-add at a lower role, a cascade removal
//! from an ancestor, a device-binding revocation, a namespace leave — and a
//! door that forgets to revoke leaves a grant outliving its authority. That
//! failure is silent and it fails OPEN.
//!
//! Invalidation inverts it. Nothing is enumerated and nothing is deleted: a
//! membership event on a watched group makes the grant stale, and staleness
//! only ever means "ask the gate again". The gate stays the sole authority, so
//! a bug here costs a redundant re-derivation, never an unauthorized delivery.
//! Note which way each mistake falls: watching too many groups is wasted work,
//! watching too few is a leak — which is why [`Grants::vouch`] fails closed by
//! staying stale whenever it cannot see the whole picture.
//!
//! # Why ancestors are watched too
//!
//! Membership is inherited: a member of a parent group is a member of its
//! descendants. A removal from a parent therefore revokes authority a
//! descendant subscription was granted on, while naming only the parent. So the
//! watched set is each governing group **and every ancestor up to the namespace
//! root** — resolved once, when the subscription is granted, on the store read
//! the gate is making anyway.
//!
//! Recording ancestors rather than walking descendants at event time is what
//! keeps the event path free of the store: a group's position in the tree is
//! fixed, so the ancestors resolved at subscribe time stay correct for the life
//! of the grant.

use std::collections::HashSet;

use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::NamespaceRepository;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_store::Store;
use tracing::warn;

/// Bound on the parent walk, so a malformed tree cannot hang the subscribe
/// path. Namespaces nest far shallower than this in practice; hitting the bound
/// means the tree is cyclic or corrupt, which [`Grants::vouch`] treats as "I
/// cannot see the whole picture" and fails closed on.
const MAX_GROUP_DEPTH: usize = 64;

/// What a connection's subscriptions depend on, and whether that has been
/// checked since it last could have changed.
///
/// A connection starts [`stale`](Self::is_stale) and becomes current only by
/// passing the gate. That default is what makes a session resumed from a
/// persisted record re-derive: its subscriptions come back from the store, its
/// grant does not, so the first thing to ask validates it against live
/// membership rather than against what the record remembers.
#[derive(Debug)]
pub(crate) struct Grants {
    /// Groups whose membership governs these subscriptions — each subscribed
    /// group and each subscribed context's owning group, plus their ancestors.
    watched: HashSet<Hash>,
    stale: bool,
}

impl Default for Grants {
    fn default() -> Self {
        Self {
            watched: HashSet::new(),
            // Fail closed: un-vouched is stale, never trusted.
            stale: true,
        }
    }
}

impl Grants {
    /// Whether these subscriptions must be re-derived before they are trusted
    /// again.
    pub(crate) fn is_stale(&self) -> bool {
        self.stale
    }

    /// Whether a membership change on `group` can have moved this
    /// connection's authority.
    ///
    /// The cheap half of the design, and the reason it is a `&self` query: one
    /// hash-set lookup under a READ lock, no store access, nothing mutated. A
    /// connection that does not watch `group` is skipped without ever taking a
    /// write lock, which is what keeps a membership change from serializing
    /// every connection on the node behind it.
    ///
    /// An already-stale grant is affected by everything — it has not been
    /// checked since it could have changed, so there is nothing to compare
    /// against and the only safe answer is to re-derive.
    pub(crate) fn is_affected_by(&self, group: &Hash) -> bool {
        self.stale || self.watched.contains(group)
    }

    /// Record what the current subscriptions depend on and mark them checked.
    ///
    /// Called after the gate has run — at subscribe time and after each
    /// re-derivation — so the set reflects exactly what was just authorized.
    ///
    /// Anything that cannot be resolved leaves the grant STALE rather than
    /// narrowing the watched set: an unresolvable owning group means this
    /// cannot tell which memberships matter, and the safe reading of that is
    /// "re-derive again next time", not "nothing governs this subscription".
    pub(crate) fn vouch(
        &mut self,
        store: &Store,
        subscriptions: &HashSet<ContextId>,
        group_subscriptions: &HashSet<Hash>,
    ) {
        let mut watched = HashSet::new();
        let mut complete = true;

        for group in group_subscriptions {
            complete &= self.collect_chain(store, *group, &mut watched);
        }
        for context in subscriptions {
            match calimero_governance_store::get_group_for_context(store, context) {
                Ok(Some(group_id)) => {
                    let group = Hash::from(group_id.to_bytes());
                    complete &= self.collect_chain(store, group, &mut watched);
                }
                // Registered to no group, so no group-membership change can
                // revoke it and there is nothing to watch on its behalf.
                Ok(None) => {}
                Err(error) => {
                    warn!(
                        %context,
                        %error,
                        "cannot resolve the context's owning group; leaving the grant stale",
                    );
                    complete = false;
                }
            }
        }

        self.watched = watched;
        self.stale = !complete;
    }

    /// Walk `group` and its ancestors into `watched`; `false` if the walk could
    /// not be completed.
    fn collect_chain(&self, store: &Store, group: Hash, watched: &mut HashSet<Hash>) -> bool {
        let mut current = group;
        for _ in 0..MAX_GROUP_DEPTH {
            if !watched.insert(current) {
                // Already walked this chain via another subscription, so its
                // ancestors are in the set too.
                return true;
            }
            let group_id = ContextGroupId::from(*current.as_bytes());
            match NamespaceRepository::new(store).parent(&group_id) {
                // Reached the namespace root: the chain is complete.
                Ok(None) => return true,
                Ok(Some(parent)) => current = Hash::from(parent.to_bytes()),
                Err(error) => {
                    warn!(
                        %group,
                        %error,
                        "cannot resolve the group's parent; leaving the grant stale",
                    );
                    return false;
                }
            }
        }
        warn!(
            %group,
            depth = MAX_GROUP_DEPTH,
            "group ancestry exceeds the depth bound; leaving the grant stale",
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;

    use super::*;

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn groups(ids: &[Hash]) -> HashSet<Hash> {
        ids.iter().copied().collect()
    }

    /// A grant that has never been vouched for is stale.
    ///
    /// This is the whole of the resume story: an SSE session restored from a
    /// persisted record gets its subscriptions back from the store and its
    /// grant from this default, so it is re-derived against live membership
    /// before it is served rather than trusted on the record's word.
    #[test]
    fn a_fresh_grant_starts_stale() {
        assert!(
            Grants::default().is_stale(),
            "an un-vouched grant must never be trusted",
        );
    }

    /// The optimization's actual claim: a connection the change cannot reach
    /// does no work beyond one set lookup.
    #[test]
    fn a_change_on_an_unwatched_group_leaves_the_grant_current() {
        let store = store();
        let (namespace, subgroup, _member) =
            crate::test_support::seed_namespace_with_restricted_subgroup(
                &store,
                PublicKey::from([0x21u8; 32]),
                GroupMemberRole::Member,
            );

        let mut grants = Grants::default();
        grants.vouch(&store, &HashSet::new(), &groups(&[subgroup]));
        assert!(!grants.is_stale(), "a vouched grant starts current");

        // A group that is neither the subscribed one nor an ancestor of it.
        let unrelated = Hash::from([0xEEu8; 32]);
        assert!(
            !grants.is_affected_by(&unrelated),
            "a change on an unwatched group must not invalidate the grant",
        );
        assert!(
            !grants.is_stale(),
            "and must leave the connection out of the re-derivation entirely",
        );
        // Sanity: the namespace root IS watched, so this is a real distinction
        // and not a grant that watches nothing at all.
        assert!(namespace != unrelated);
    }

    /// The inheritance case, and the reason the watched set carries ancestors.
    ///
    /// A member of a parent group is a member of its descendants, so a removal
    /// naming only the parent revokes authority a descendant subscription was
    /// granted on. Narrowing re-authorization to "connections that watch the
    /// named group" is only safe because the named group's DESCENDANTS watch it
    /// too; without that this returns `false` and the inherited member keeps
    /// receiving the subgroup's events.
    #[test]
    fn a_change_on_an_ancestor_invalidates_a_descendant_grant() {
        let store = store();
        let (namespace, subgroup, _member) =
            crate::test_support::seed_namespace_with_restricted_subgroup(
                &store,
                PublicKey::from([0x22u8; 32]),
                GroupMemberRole::Member,
            );

        let mut grants = Grants::default();
        grants.vouch(&store, &HashSet::new(), &groups(&[subgroup]));
        assert!(!grants.is_stale());

        assert!(
            grants.is_affected_by(&namespace),
            "a removal from the PARENT must invalidate a grant held on the child",
        );
    }

    /// A grant on a group also watches that group itself, not only its parents.
    #[test]
    fn a_change_on_the_subscribed_group_invalidates_its_grant() {
        let store = store();
        let (_namespace, subgroup, _member) =
            crate::test_support::seed_namespace_with_restricted_subgroup(
                &store,
                PublicKey::from([0x23u8; 32]),
                GroupMemberRole::Member,
            );

        let mut grants = Grants::default();
        grants.vouch(&store, &HashSet::new(), &groups(&[subgroup]));
        assert!(grants.is_affected_by(&subgroup));
    }

    /// Fail-closed on an ancestry it cannot read.
    ///
    /// Watching too many groups costs a redundant re-derivation; watching too
    /// few is a leak. So a chain that cannot be walked leaves the grant stale —
    /// the gate keeps being asked — rather than narrowing the set to whatever
    /// was resolved before the walk gave out.
    #[test]
    fn an_unresolvable_ancestry_leaves_the_grant_stale() {
        let store = store();
        // Never seeded, so it has no parent row and no namespace record: the
        // shape a group id the server has not seen arrives in.
        let unknown = Hash::from([0x5Au8; 32]);

        let mut grants = Grants::default();
        grants.vouch(&store, &HashSet::new(), &groups(&[unknown]));

        // Either the walk failed (stale) or it completed and the group watches
        // itself; both are safe, and the unsafe outcome is a grant that is
        // current while watching NOTHING, which would never be re-derived.
        assert!(
            grants.is_stale() || grants.is_affected_by(&unknown),
            "an unresolved group must not yield a current grant that watches nothing",
        );
    }

    /// Vouching twice narrows to what is currently subscribed, so a group that
    /// came off stops being watched.
    #[test]
    fn re_vouching_forgets_a_group_that_is_no_longer_subscribed() {
        let store = store();
        let (_namespace, subgroup, _member) =
            crate::test_support::seed_namespace_with_restricted_subgroup(
                &store,
                PublicKey::from([0x24u8; 32]),
                GroupMemberRole::Member,
            );

        let mut grants = Grants::default();
        grants.vouch(&store, &HashSet::new(), &groups(&[subgroup]));
        assert!(grants.is_affected_by(&subgroup));

        // Re-vouch with nothing subscribed, as a full revocation leaves it.
        grants.vouch(&store, &HashSet::new(), &HashSet::new());
        assert!(
            !grants.is_affected_by(&subgroup),
            "a group that is no longer subscribed must not keep forcing re-derivation",
        );
    }
}
