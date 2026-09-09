//! Which connected peers host a context's availability nodes.
//!
//! This is the node-side implementation of
//! [`calimero_node_primitives::client::MemberRoles`], the seam blob discovery
//! uses to probe availability nodes first and to address blob announcements.
//!
//! It deliberately reuses the plumbing sync peer-selection already runs on —
//! `crate::sync::availability_device_keys` (the `ReadOnlyTee`-only sibling of
//! `anchor_device_keys`) plus `NodeState::peer_identities` — rather than
//! opening a second, independently-drifting path to the same governance rows.

use std::collections::BTreeSet;

use calimero_governance_store::get_group_for_context;
use calimero_node_primitives::client::MemberRoles;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use libp2p::PeerId;

use crate::state::NodeState;

/// Resolves a context's availability peers from local governance state and the
/// observed peer→identity reverse view.
#[derive(Clone, Debug)]
pub(crate) struct GovernanceAvailabilityPeers {
    datastore: Store,
    node_state: NodeState,
}

impl GovernanceAvailabilityPeers {
    #[must_use]
    pub(crate) const fn new(datastore: Store, node_state: NodeState) -> Self {
        Self {
            datastore,
            node_state,
        }
    }

    /// The `ReadOnlyTee` device keys for the group owning `context_id`.
    /// Empty when the context is not registered to any group.
    fn availability_keys(&self, context_id: &ContextId) -> BTreeSet<PublicKey> {
        let Ok(Some(group_id)) = get_group_for_context(&self.datastore, context_id) else {
            return BTreeSet::new();
        };
        crate::sync::availability_device_keys(&self.datastore, &group_id)
    }
}

impl MemberRoles for GovernanceAvailabilityPeers {
    fn anchors_for_context(&self, context_id: &ContextId) -> Vec<PeerId> {
        let keys = self.availability_keys(context_id);
        if keys.is_empty() {
            return Vec::new();
        }

        // The reverse view is a `DashMap`, whose iteration order is not
        // stable, so the result is sorted: two probes of the same context
        // must ask the same peer first, or a retry re-rolls the search.
        let mut peers: Vec<PeerId> = self
            .node_state
            .peer_identities
            .iter()
            .filter(|entry| entry.value().iter().any(|id| keys.contains(id)))
            .map(|entry| *entry.key())
            .collect();
        peers.sort_unstable();
        peers.dedup();
        peers
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::AccountId;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{register_context_in_group, MembershipRepository};
    use calimero_primitives::context::GroupMemberRole;
    use calimero_store::db::InMemoryDB;

    use super::*;

    fn peer(byte: u8) -> PeerId {
        let keypair =
            libp2p::identity::Keypair::ed25519_from_bytes([byte; 32]).expect("valid ed25519 seed");
        PeerId::from_public_key(&keypair.public())
    }

    /// A namespace with `context` registered under it, `tee` enrolled as a
    /// `ReadOnlyTee` member and `plain` as an ordinary `Member`. Returns the
    /// store and the two device keys.
    fn namespace_with_tee_and_member() -> (Store, ContextId, PublicKey, PublicKey) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let group = ContextGroupId::from([0xA0; 32]);
        let context = ContextId::from([0xC0; 32]);

        let tee_key = PublicKey::from([0x11; 32]);
        let member_key = PublicKey::from([0x22; 32]);
        let tee_account = calimero_context::test_support::enrol(&store, &group, &tee_key);
        let member_account = calimero_context::test_support::enrol(&store, &group, &member_key);

        let members = MembershipRepository::new(&store);
        members
            .add_member(&group, &tee_account, GroupMemberRole::ReadOnlyTee)
            .expect("add tee member");
        members
            .add_member(&group, &member_account, GroupMemberRole::Member)
            .expect("add plain member");
        register_context_in_group(&store, &group, &context).expect("register context");

        (store, context, tee_key, member_key)
    }

    fn state_with(entries: impl IntoIterator<Item = (PeerId, PublicKey)>) -> NodeState {
        let state = NodeState::new();
        for (peer, key) in entries {
            let _replaced = state
                .peer_identities
                .insert(peer, [key].into_iter().collect());
        }
        state
    }

    #[test]
    fn only_read_only_tee_peers_are_returned() {
        let (store, context, tee_key, member_key) = namespace_with_tee_and_member();
        let state = state_with([(peer(1), member_key), (peer(2), tee_key)]);

        let resolver = GovernanceAvailabilityPeers::new(store, state);
        assert_eq!(resolver.anchors_for_context(&context), vec![peer(2)]);
    }

    /// The account→device expansion is the point: one availability ACCOUNT
    /// running two machines must be preferred at both, because governance rows
    /// are account-keyed while peers present device keys.
    #[test]
    fn an_availability_account_is_preferred_on_every_device_it_runs() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let group = ContextGroupId::from([0xA0; 32]);
        let context = ContextId::from([0xC0; 32]);

        let device_a = PublicKey::from([0x11; 32]);
        let device_b = PublicKey::from([0x12; 32]);
        let account: AccountId = calimero_context::test_support::enrol(&store, &group, &device_a);
        let same_account = calimero_context::test_support::enrol(&store, &group, &device_b);
        assert_ne!(
            account, same_account,
            "fixture sanity: distinct keys enrol as distinct accounts here"
        );

        let members = MembershipRepository::new(&store);
        members
            .add_member(&group, &account, GroupMemberRole::ReadOnlyTee)
            .expect("add device a");
        members
            .add_member(&group, &same_account, GroupMemberRole::ReadOnlyTee)
            .expect("add device b");
        register_context_in_group(&store, &group, &context).expect("register context");

        let state = state_with([(peer(1), device_a), (peer(2), device_b)]);
        let resolver = GovernanceAvailabilityPeers::new(store, state);

        let mut expected = vec![peer(1), peer(2)];
        expected.sort_unstable();
        assert_eq!(resolver.anchors_for_context(&context), expected);
    }

    #[test]
    fn a_context_with_no_group_binding_has_no_availability_peers() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let state = state_with([(peer(1), PublicKey::from([0x11; 32]))]);
        let resolver = GovernanceAvailabilityPeers::new(store, state);
        assert!(resolver
            .anchors_for_context(&ContextId::from([0xC1; 32]))
            .is_empty());
    }

    /// An availability member whose device has not been observed on any peer
    /// yields nothing — the lookup answers in peers, and inventing one would
    /// waste a probe slot inside the 32-candidate cap.
    #[test]
    fn an_unobserved_availability_member_yields_no_peer() {
        let (store, context, _tee_key, member_key) = namespace_with_tee_and_member();
        let state = state_with([(peer(1), member_key)]);
        let resolver = GovernanceAvailabilityPeers::new(store, state);
        assert!(resolver.anchors_for_context(&context).is_empty());
    }
}
