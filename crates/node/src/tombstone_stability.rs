//! When a tombstone may be collected: once every member device has applied
//! the delete.
//!
//! A tombstone is the only record that an entity was deleted. Collect it while
//! some replica still holds the entity, or an older write to it that has not
//! reached this node yet, and that replica brings the entity back: through
//! HashComparison (a child only it holds), a late delta (an `Update` that finds
//! no row and lands as new), or a snapshot it serves (core#4331). No time limit
//! is safe, since a replica can be away for any length of time. So a tombstone
//! goes only once every device that may sync the context is known to hold a
//! state that already includes the delete.
//!
//! **The evidence** is a signed [`StateBeacon`] each member sends with its
//! heartbeat: its DAG heads and the root hash they produce. A device is
//! *caught up as of* `t` when its beacon equals this node's own heads and root
//! at `t`:
//!
//! - equal heads mean the two hold the same set of deltas, so the device has
//!   applied every delete this node had applied by `t`, and it holds no delta
//!   this node lacks — none of its offline writes is still to arrive;
//! - an equal root means the two hold the same live entities, so the device
//!   has no live copy of anything deleted here, including what a tombstone
//!   learned through HashComparison (not a delta) records.
//!
//! **The rule** ([`every_member_caught_up`]): the GC sweep notes when it first
//! saw each tombstone, and collects it once every other member device is caught
//! up as of a moment [`SETTLE_NANOS`] after that. The tombstone's row existed
//! when the sweep saw it, so the delete was applied by then; the settle margin
//! covers the gap between the row write and the heads and root that record it.
//!
//! **Who must be caught up** ([`other_member_devices`]) is every device the
//! sync admission would accept for the context: for a context in a group, each
//! live device binding of its namespace that `is_admitted_to_context` admits;
//! for a context in no group, its `ContextIdentity` members. This node's own
//! keys are left out, since its own state includes every delete it holds. A
//! context with no other member collects its tombstones on the sweep after it
//! sees them.
//!
//! **Nothing here is persisted.** After a restart the sweep re-notes every
//! tombstone and waits for fresh beacons, so collection is delayed, never
//! premature.
//!
//! What this cannot protect: a device removed from the group stops counting,
//! so a delta it authored before its removal and that surfaces only later can
//! still bring back an entity collected meanwhile. And a member that signs a
//! false beacon can release tombstones it then undoes, which any member with
//! write access could do by writing anyway.
//!
//! [`StateBeacon`]: calimero_node_primitives::sync::BroadcastMessage::StateBeacon

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Mutex;

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::ContextIdentity;
use calimero_store::Store;
use eyre::Result as EyreResult;

/// Own states remembered per context, to match beacons that arrive after this
/// node has moved on. One is recorded per heartbeat, so this covers about an
/// hour; a beacon older than that matches nothing and is simply not counted.
const OWN_STATES_KEPT: usize = 128;

/// How long after a tombstone is first seen a member must be caught up for the
/// tombstone to go: the gap between the row write and the heads and root that
/// record its delete, with room to spare (1 minute).
pub(crate) const SETTLE_NANOS: u64 = 60_000_000_000;

/// One state of this node's: its sorted DAG heads and root hash at `at`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct OwnState {
    heads: Vec<[u8; 32]>,
    root: [u8; 32],
    at: u64,
}

#[derive(Debug, Default)]
struct Inner {
    /// This node's recent states, oldest first.
    own: HashMap<ContextId, VecDeque<OwnState>>,
    /// The latest moment each member device was seen caught up with this node.
    caught_up: BTreeMap<(ContextId, PublicKey), u64>,
}

/// What this node knows of how far its members have caught up. Shared by the
/// beacon handler, which writes it, and tombstone GC, which reads it.
#[derive(Debug, Default)]
pub(crate) struct TombstoneStability {
    inner: Mutex<Inner>,
}

impl TombstoneStability {
    /// Record this node's own state of `context_id` at `at`: its DAG heads, in
    /// any order, and root hash.
    pub(crate) fn record_own(
        &self,
        context_id: ContextId,
        mut heads: Vec<[u8; 32]>,
        root: [u8; 32],
        at: u64,
    ) {
        heads.sort_unstable();
        let mut inner = self.lock();
        let states = inner.own.entry(context_id).or_default();
        if let Some(last) = states.back_mut() {
            if last.heads == heads && last.root == root {
                // Unchanged: the newest moment it held is what counts.
                last.at = last.at.max(at);
                return;
            }
        }
        states.push_back(OwnState { heads, root, at });
        while states.len() > OWN_STATES_KEPT {
            let _oldest = states.pop_front();
        }
    }

    /// A verified beacon from `signer`: its sorted DAG heads and root hash. If
    /// they equal a state this node held, `signer` is caught up as of the
    /// latest moment it held it. Returns whether the beacon matched.
    pub(crate) fn observe(
        &self,
        context_id: ContextId,
        signer: PublicKey,
        heads: &[[u8; 32]],
        root: [u8; 32],
    ) -> bool {
        let mut inner = self.lock();
        let Some(at) = inner.own.get(&context_id).and_then(|states| {
            states
                .iter()
                .rev()
                .find(|state| state.heads == heads && state.root == root)
                .map(|state| state.at)
        }) else {
            return false;
        };
        let latest = inner.caught_up.entry((context_id, signer)).or_insert(at);
        *latest = (*latest).max(at);
        true
    }

    /// The latest moment `signer` was seen caught up with this node in
    /// `context_id`, if ever since this node started.
    pub(crate) fn caught_up(&self, context_id: ContextId, signer: &PublicKey) -> Option<u64> {
        self.lock().caught_up.get(&(context_id, *signer)).copied()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A poisoned lock only means a panic elsewhere mid-update; every update
        // here leaves the maps consistent, so keep using them.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Wall-clock nanoseconds since the Unix epoch, or `0` for a clock before it.
///
/// Every moment this module compares comes from this one clock on this one
/// node: when the sweep saw a tombstone, and when this node held a state a
/// member matched. A clock that steps back can only make a member look caught
/// up earlier than a tombstone was seen, which delays collection.
#[must_use]
pub(crate) fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_nanos()).unwrap_or(0))
}

/// Whether a tombstone first seen at `seen_at` may be collected: every one of
/// `members` was caught up as of a moment at least [`SETTLE_NANOS`] after it.
#[must_use]
pub(crate) fn every_member_caught_up(
    members: &[PublicKey],
    caught_up: impl Fn(&PublicKey) -> Option<u64>,
    seen_at: u64,
) -> bool {
    let after = seen_at.saturating_add(SETTLE_NANOS);
    members
        .iter()
        .all(|member| caught_up(member).is_some_and(|at| at >= after))
}

/// Every device other than this node's own that the sync admission would
/// accept for `context_id`: the ones that must be caught up before a
/// tombstone there goes.
///
/// For a context in a group, each live device binding of its namespace that
/// [`calimero_governance_store::is_admitted_to_context`] admits; for one in no
/// group, its `ContextIdentity` members. Either way this node's own signing keys
/// are left out.
///
/// # Errors
/// Propagates a store read failure. The caller must then collect nothing.
pub(crate) fn other_member_devices(
    store: &Store,
    context_id: &ContextId,
) -> EyreResult<Vec<PublicKey>> {
    let own = calimero_governance_store::find_local_signing_identities(store, context_id)?;
    let mut devices = match calimero_governance_store::get_group_for_context(store, context_id)? {
        Some(group) => {
            let namespace =
                calimero_governance_store::NamespaceRepository::new(store).resolve(&group)?;
            let mut admitted = Vec::new();
            for binding in calimero_governance_store::AccountBindingRepository::new(store)
                .live_bindings(&namespace)?
            {
                if calimero_governance_store::is_admitted_to_context(
                    store,
                    context_id,
                    &binding.sign_pk,
                )? == Some(true)
                {
                    admitted.push(binding.sign_pk);
                }
            }
            admitted
        }
        None => context_identities(store, context_id)?,
    };
    devices.sort_unstable();
    devices.dedup();
    devices.retain(|device| !own.contains(device));
    Ok(devices)
}

/// The public keys of every `ContextIdentity` row of `context_id`.
fn context_identities(store: &Store, context_id: &ContextId) -> EyreResult<Vec<PublicKey>> {
    let handle = store.handle();
    let mut iter = handle.iter::<ContextIdentity>()?;
    let first = iter
        .seek(ContextIdentity::new(*context_id, [0u8; 32].into()))
        .transpose();
    let mut keys = Vec::new();
    for key in first.into_iter().chain(iter.keys()) {
        let key = key?;
        if key.context_id() != *context_id {
            break;
        }
        keys.push(key.public_key());
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_store::db::InMemoryDB;
    use calimero_store::types::ContextIdentity as ContextIdentityValue;

    use super::*;

    fn ctx() -> ContextId {
        ContextId::from([1; 32])
    }

    fn member(byte: u8) -> PublicKey {
        PublicKey::from([byte; 32])
    }

    /// A beacon equal to a state this node held counts as of the latest moment
    /// it held it, even once this node has moved on; the newest moment wins.
    #[test]
    fn a_beacon_counts_as_of_the_state_it_matches() {
        let stability = TombstoneStability::default();
        stability.record_own(ctx(), vec![[2; 32], [1; 32]], [9; 32], 10);
        stability.record_own(ctx(), vec![[1; 32], [2; 32]], [9; 32], 20);
        stability.record_own(ctx(), vec![[3; 32]], [8; 32], 30);

        assert!(stability.observe(ctx(), member(5), &[[1; 32], [2; 32]], [9; 32]));
        assert_eq!(
            stability.caught_up(ctx(), &member(5)),
            Some(20),
            "the same state recorded twice holds as of its latest record"
        );

        assert!(stability.observe(ctx(), member(5), &[[3; 32]], [8; 32]));
        assert_eq!(stability.caught_up(ctx(), &member(5)), Some(30));
        // An older state seen later never moves it back.
        assert!(stability.observe(ctx(), member(5), &[[1; 32], [2; 32]], [9; 32]));
        assert_eq!(stability.caught_up(ctx(), &member(5)), Some(30));
    }

    /// Equal heads with a different root, or a state never held here, count
    /// for nothing.
    #[test]
    fn a_beacon_matching_no_held_state_counts_for_nothing() {
        let stability = TombstoneStability::default();
        stability.record_own(ctx(), vec![[1; 32]], [9; 32], 10);

        assert!(!stability.observe(ctx(), member(5), &[[1; 32]], [7; 32]));
        assert!(!stability.observe(ctx(), member(5), &[[4; 32]], [9; 32]));
        assert!(!stability.observe(ContextId::from([2; 32]), member(5), &[[1; 32]], [9; 32]));
        assert_eq!(stability.caught_up(ctx(), &member(5)), None);
    }

    /// Only the most recent states are kept to match against.
    #[test]
    fn only_recent_states_are_matched() {
        let stability = TombstoneStability::default();
        for at in 0..=OWN_STATES_KEPT as u64 {
            stability.record_own(ctx(), vec![[at as u8; 32]], [at as u8; 32], at);
        }
        assert!(!stability.observe(ctx(), member(5), &[[0; 32]], [0; 32]));
        assert!(stability.observe(ctx(), member(5), &[[1; 32]], [1; 32]));
    }

    /// Every member must be caught up as of `SETTLE_NANOS` after the tombstone
    /// was seen; with no other member, nothing is waited on.
    #[test]
    fn every_member_must_be_caught_up_past_the_settle_margin() {
        let seen_at = 1_000;
        let at = |times: Vec<(PublicKey, u64)>| {
            move |member: &PublicKey| times.iter().find(|(m, _)| m == member).map(|(_, t)| *t)
        };
        assert!(every_member_caught_up(&[], at(vec![]), seen_at));

        let members = [member(1), member(2)];
        let settled = seen_at + SETTLE_NANOS;
        assert!(every_member_caught_up(
            &members,
            at(vec![(member(1), settled), (member(2), settled + 5)]),
            seen_at
        ));
        assert!(!every_member_caught_up(
            &members,
            at(vec![(member(1), settled), (member(2), settled - 1)]),
            seen_at
        ));
        assert!(!every_member_caught_up(
            &members,
            at(vec![(member(1), settled)]),
            seen_at
        ));
    }

    /// For a context in a group, the members are the namespace's devices the
    /// sync admission admits, whatever their role: not a device the namespace
    /// bound for an account outside the group, nor this node's own.
    #[test]
    fn a_context_in_a_group_waits_on_its_admitted_devices() {
        use calimero_context_config::types::ContextGroupId;
        use calimero_governance_store::test_fixtures::{
            enrol_member, sample_meta_with_admin, test_store,
        };
        use calimero_governance_store::{
            register_context_in_group, MembershipRepository, MetaRepository,
        };
        use calimero_primitives::context::GroupMemberRole;

        let store = test_store();
        let namespace = ContextGroupId::from([0xB7u8; 32]);
        let (admin, reader, stranger, own) =
            (member(0xB9), member(0xBA), member(0xBB), member(0xBC));
        let admin_account = enrol_member(&store, &namespace, &admin);
        for device in [reader, own] {
            let account = enrol_member(&store, &namespace, &device);
            MembershipRepository::new(&store)
                .add_member(&namespace, &account, GroupMemberRole::ReadOnly)
                .unwrap();
        }
        let _outsider = enrol_member(&store, &namespace, &stranger);
        MetaRepository::new(&store)
            .save(&namespace, &sample_meta_with_admin(admin_account))
            .unwrap();
        register_context_in_group(&store, &namespace, &ctx()).unwrap();
        store
            .handle()
            .put(
                &ContextIdentity::new(ctx(), own),
                &ContextIdentityValue {
                    private_key: Some([7; 32]),
                },
            )
            .unwrap();

        let mut expected = vec![admin, reader];
        expected.sort_unstable();
        assert_eq!(other_member_devices(&store, &ctx()).unwrap(), expected);
    }

    /// For a context in no group, the members are its `ContextIdentity` rows,
    /// less the ones this node signs with.
    #[test]
    fn a_context_in_no_group_waits_on_its_other_identities() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let mut handle = store.handle();
        let other_context = ContextId::from([2; 32]);
        for (context, key, private_key) in [
            (ctx(), member(1), None),
            (ctx(), member(2), Some([7; 32])),
            (ctx(), member(3), None),
            (other_context, member(4), None),
        ] {
            handle
                .put(
                    &ContextIdentity::new(context, key),
                    &ContextIdentityValue { private_key },
                )
                .unwrap();
        }

        assert_eq!(
            other_member_devices(&store, &ctx()).unwrap(),
            vec![member(1), member(3)]
        );
    }
}
