//! Pending forward-secrecy key rotations — the worklist a self-leave leaves behind.
//!
//! # Why a worklist exists at all
//!
//! Key rotation is minted by whoever PUBLISHES the op: an admin removing a member
//! generates a fresh group key, wraps it for everyone who remains (skipping the
//! removed member), and ships it as a sidecar on the same op. That works because
//! the publisher stays in the group.
//!
//! A self-leave inverts it. The publisher IS the departing member, and they cannot
//! rotate for themselves twice over:
//!
//! 1. They would have to mint the very key they are supposed to be cut off from —
//!    and they would retain it. That is not forward secrecy, it is theatre.
//! 2. Peers would reject the rotation anyway: the receive gate accepts a rotation
//!    only from an admin of the group.
//!
//! So the leave and the rotation must be performed by different nodes. This module
//! is the hand-off: the `MemberLeft` apply records what is owed, and a remaining
//! admin discharges it.
//!
//! # Why this is safe without an election or a quorum
//!
//! The rows are written inside the deterministic, replicated apply, so every node
//! derives the SAME worklist — no coordination, no leader, nothing to agree on.
//!
//! Any remaining admin may then rotate. If several do so concurrently, they mint
//! DIFFERENT keys — and that is fine, because the keyring already converges: the
//! current key is the one with the highest epoch, ties broken by the larger key id,
//! which is a total order over a hash and therefore identical on every node. Safety
//! holds under the race because every competing rotation excludes the leaver:
//! whichever key wins, the leaver holds none of them. The only cost of a race is
//! redundant envelopes on the wire.
//!
//! That is why there is no "who rotates" rule here. Trying to elect one would add a
//! coordination problem the crypto already solves.
//!
//! # Liveness
//!
//! The rows are durable, so an admin that was offline at the moment of the leave
//! still finds the outstanding work when it comes back, and a node that crashes
//! mid-rotation retries. The worklist drains; it does not evaporate.

use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{GroupPendingKeyRotation, GROUP_PENDING_KEY_ROTATION_PREFIX};
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::collect_keys_with_prefix;
use crate::{CapabilitiesRepository, NamespaceRepository};

/// Does a departure from `group_id` require a key rotation?
///
/// Only for a group that encrypts under its OWN key. That is:
///
/// - a **Restricted** subgroup (or any subgroup behind a Restricted ancestor) — it
///   has a per-subgroup key, and the departing member holds it;
/// - the **namespace root** — it holds the namespace key, which also decrypts every
///   Open subgroup beneath it. A member who leaves the namespace entirely keeps that
///   key unless it is rotated, so they would go on reading the root and every Open
///   subgroup. `is_open_chain_to_namespace` returns `false` for the root against
///   itself, so the root falls out of this predicate as rotating, which is correct.
///
/// An **Open** subgroup beneath a fully-Open chain is encrypted with the NAMESPACE
/// key, which the departing member still holds by virtue of their namespace
/// membership. Minting a fresh per-subgroup key there would revoke nothing — it would
/// just produce a key nobody uses. Leaving such a subgroup revokes authorization, not
/// read access; closing that gap is what leaving the NAMESPACE does.
pub fn group_rotates_on_departure(store: &Store, group_id: &ContextGroupId) -> EyreResult<bool> {
    let namespace_id = NamespaceRepository::new(store).resolve(group_id)?;
    Ok(!CapabilitiesRepository::new(store).is_open_chain_to_namespace(group_id, &namespace_id)?)
}

/// Typed repository over the pending-key-rotation worklist.
///
/// A row means: `group_id` owes a rotation because `departed` left it, and none has
/// landed yet. Presence of the key IS the marker; the value is `()`.
pub struct PendingRotationRepository<'a> {
    store: &'a Store,
}

impl<'a> PendingRotationRepository<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Record that `group_id` owes a rotation because `departed` left.
    ///
    /// Idempotent. Called from the `MemberLeft` apply on every node, so the worklist
    /// is replicated rather than gossiped.
    pub fn mark(&self, group_id: &ContextGroupId, departed: &PublicKey) -> EyreResult<()> {
        let key = GroupPendingKeyRotation::new(group_id.to_bytes(), *departed);
        let mut handle = self.store.handle();
        handle
            .put(&key, &())
            .map_err(|e| eyre::eyre!("PendingRotationRepository::mark: {e}"))?;
        Ok(())
    }

    /// Discharge the rotation `group_id` owed for `departed`.
    ///
    /// Idempotent — clearing an absent row is a no-op, which is what makes a
    /// concurrent double-rotation harmless: the second `GroupKeyRotated` to apply
    /// simply finds nothing left to clear.
    pub fn clear(&self, group_id: &ContextGroupId, departed: &PublicKey) -> EyreResult<()> {
        let key = GroupPendingKeyRotation::new(group_id.to_bytes(), *departed);
        let mut handle = self.store.handle();
        handle
            .delete(&key)
            .map_err(|e| eyre::eyre!("PendingRotationRepository::clear: {e}"))?;
        Ok(())
    }

    /// Does `group_id` still owe a rotation for `departed`?
    pub fn is_pending(&self, group_id: &ContextGroupId, departed: &PublicKey) -> EyreResult<bool> {
        let key = GroupPendingKeyRotation::new(group_id.to_bytes(), *departed);
        let handle = self.store.handle();
        handle
            .has(&key)
            .map_err(|e| eyre::eyre!("PendingRotationRepository::is_pending: {e}"))
    }

    /// Every identity `group_id` still owes a rotation for.
    ///
    /// Usually zero or one. More than one means several members left before any
    /// rotation landed — a single rotation discharges them all (it excludes every
    /// non-member), so the caller clears each row it covers.
    pub fn departed_for_group(&self, group_id: &ContextGroupId) -> EyreResult<Vec<PublicKey>> {
        let gid = group_id.to_bytes();
        // Seek from the lexicographic minimum of the identity space so a forward
        // iterator visits every row under this group — the same scan-from-minimum
        // convention the deny-list uses.
        let keys = collect_keys_with_prefix(
            self.store,
            GroupPendingKeyRotation::new(gid, PublicKey::from([0u8; 32])),
            GROUP_PENDING_KEY_ROTATION_PREFIX,
            |k| k.group_id() == gid,
        )?;
        Ok(keys
            .iter()
            .map(|k| PublicKey::from(*k.departed()))
            .collect())
    }

    /// The node's entire rotation backlog, as `(group_id, departed)` pairs.
    ///
    /// This is what a rotator drains on startup: a node that was offline when the
    /// leave applied still finds the outstanding work here, which is the whole point
    /// of persisting the worklist rather than reacting only to a live event.
    pub fn all_pending(&self) -> EyreResult<Vec<(ContextGroupId, PublicKey)>> {
        let keys = collect_keys_with_prefix(
            self.store,
            GroupPendingKeyRotation::new([0u8; 32], PublicKey::from([0u8; 32])),
            GROUP_PENDING_KEY_ROTATION_PREFIX,
            |_| true,
        )?;
        Ok(keys
            .iter()
            .map(|k| {
                (
                    ContextGroupId::from(k.group_id()),
                    PublicKey::from(*k.departed()),
                )
            })
            .collect())
    }

    /// Drop every pending row under `group_id`. Used during group teardown so the
    /// worklist doesn't outlive the group it describes.
    pub fn clear_all_for_group(&self, group_id: &ContextGroupId) -> EyreResult<()> {
        let gid = group_id.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupPendingKeyRotation::new(gid, PublicKey::from([0u8; 32])),
            GROUP_PENDING_KEY_ROTATION_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let mut handle = self.store.handle();
        for key in keys {
            handle
                .delete(&key)
                .map_err(|e| eyre::eyre!("PendingRotationRepository::clear_all_for_group: {e}"))?;
        }
        Ok(())
    }
}
