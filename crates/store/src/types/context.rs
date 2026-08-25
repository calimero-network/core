#![allow(single_use_lifetimes, reason = "borsh shenanigans")]

use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::{Borsh, Identity};
use crate::key;
use crate::slice::Slice;
use crate::types::PredefinedEntry;

pub type Hash = [u8; 32];

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextMeta {
    pub application: key::ApplicationMeta,
    pub root_hash: Hash,
    pub dag_heads: Vec<[u8; 32]>,
    pub service_name: Option<Box<str>>,
}

impl ContextMeta {
    #[must_use]
    pub const fn new(
        application: key::ApplicationMeta,
        root_hash: Hash,
        dag_heads: Vec<[u8; 32]>,
        service_name: Option<Box<str>>,
    ) -> Self {
        Self {
            application,
            root_hash,
            dag_heads,
            service_name,
        }
    }
}

impl PredefinedEntry for key::ContextMeta {
    type Codec = Borsh;
    type DataType<'a> = ContextMeta;
}

/// Value for [`key::ContextAuthoredRemaining`]: this node's owner's count of
/// identity-gated entries still below the target schema (the heartbeat's
/// `authored_remaining`; 6f). Node-local + advisory, written only by the
/// post-migrate / `migrate_my_entries` persist and read by the heartbeat —
/// kept off the hot `ContextMeta` write path so a per-write rewrite can't
/// clobber it. A brand-new key, so a missing row reads as `None` (treated as
/// 0); no on-disk back-compat shim needed.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "single advisory counter; additions would need a migration"
)]
pub struct ContextAuthoredRemaining {
    pub count: u32,
}

impl PredefinedEntry for key::ContextAuthoredRemaining {
    type Codec = Borsh;
    type DataType<'a> = ContextAuthoredRemaining;
}

/// Value for [`key::ContextMigrationFailed`]: the categorized reason this
/// context's last migration attempt did not complete, as a stable discriminant
/// (`1` = migration-check aborted, `2` = migrate apply errored). Node-local +
/// advisory; the key's presence is the signal, the byte carries the reason. A
/// brand-new key, so a missing row reads as `None` (no failure on record).
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "single advisory discriminant; additions would need a migration"
)]
pub struct ContextMigrationFailed {
    pub kind: u8,
}

impl PredefinedEntry for key::ContextMigrationFailed {
    type Codec = Borsh;
    type DataType<'a> = ContextMigrationFailed;
}

impl PredefinedEntry for key::ContextResyncRequested {
    type Codec = Borsh;
    type DataType<'a> = ();
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextConfig {
    pub application_revision: u64,
    pub members_revision: u64,
}

impl ContextConfig {
    #[must_use]
    pub const fn new(application_revision: u64, members_revision: u64) -> Self {
        Self {
            application_revision,
            members_revision,
        }
    }
}

impl PredefinedEntry for key::ContextConfig {
    type Codec = Borsh;
    type DataType<'a> = ContextConfig;
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextState<'a> {
    pub value: Slice<'a>,
}

impl PredefinedEntry for key::ContextState {
    type Codec = Identity;
    type DataType<'a> = ContextState<'a>;
}

impl<'a> From<Slice<'a>> for ContextState<'a> {
    fn from(value: Slice<'a>) -> Self {
        Self { value }
    }
}

impl AsRef<[u8]> for ContextState<'_> {
    fn as_ref(&self) -> &[u8] {
        self.value.as_ref()
    }
}

/// Node-local private storage that is NOT synchronized across nodes
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextPrivateState<'a> {
    pub value: Slice<'a>,
}

impl PredefinedEntry for key::ContextPrivateState {
    type Codec = Identity;
    type DataType<'a> = ContextPrivateState<'a>;
}

impl<'a> From<Slice<'a>> for ContextPrivateState<'a> {
    fn from(value: Slice<'a>) -> Self {
        Self { value }
    }
}

impl AsRef<[u8]> for ContextPrivateState<'_> {
    fn as_ref(&self) -> &[u8] {
        self.value.as_ref()
    }
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "This is not expected to have additional fields"
)]
pub struct ContextIdentity {
    pub private_key: Option<[u8; 32]>,
}

impl PredefinedEntry for key::ContextIdentity {
    type Codec = Borsh;
    type DataType<'a> = ContextIdentity;
}

/// Tombstone value for `key::ContextLeftMarker`. Stores when the user explicitly
/// left this context on this node (millis since epoch). Presence of the row is
/// what matters for the auto-follow gate; the timestamp is for diagnostics.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "Tombstone value — additions would need a migration"
)]
pub struct ContextLeftMarker {
    pub left_at_ms: u64,
}

impl PredefinedEntry for key::ContextLeftMarker {
    type Codec = Borsh;
    type DataType<'a> = ContextLeftMarker;
}

/// DAG delta data (persisted)
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug)]
pub struct ContextDagDelta {
    pub delta_id: [u8; 32],
    pub parents: Vec<[u8; 32]>,
    pub actions: Vec<u8>, // Serialized actions
    pub hlc: calimero_storage::logical_clock::HybridTimestamp,
    pub applied: bool,
    /// Root hash of the snapshot a CHECKPOINT delta marks the boundary of;
    /// `None` for every regular delta.
    ///
    /// This replaced a blanket `expected_root_hash` carried on every row. That
    /// value was sender-asserted — absent from the `compute_id` preimage, so no
    /// receiver could verify it, and settable at will by a DAG-catchup responder
    /// — and the node has always stored the root hash it COMPUTED rather than
    /// this one. A checkpoint's root hash is different in kind: it is derived
    /// locally from a snapshot this node took.
    pub checkpoint_root_hash: Option<[u8; 32]>,
    pub events: Option<Vec<u8>>,
    /// Signing identity of the node that authored this delta. Populated
    /// from the gossip envelope on receive; populated from the local
    /// node identity on local apply. Used by the DAG-catchup responder
    /// to advertise the author on the wire so initiator-side membership
    /// checks can reject revoked-author deltas at apply time (parity
    /// with the gossip-receive cross-DAG check).
    pub author_id: Option<calimero_primitives::identity::PublicKey>,
    /// Serialized `calimero_context_config::types::GovernanceParentEdge`
    /// (borsh bytes) at sign time. Stored as a blob to avoid pulling
    /// `calimero-context-config` into `calimero-store` — matches the
    /// existing pattern for `actions` / `events`. Initiator-side
    /// DAG-catchup deserializes this and runs `membership_status_at`
    /// against it. `None` for legacy deltas authored before this
    /// field was added.
    pub governance_position_blob: Option<Vec<u8>>,
    /// Ed25519 signature by `author_id`'s identity key over the
    /// canonical `DeltaSignaturePayload`. Closes the anti-impersonation
    /// gap on the delta envelope: a current group-key holder can't
    /// relabel a foreign delta as their own (or vice versa). Served
    /// alongside `author_id` on the wire; verified by every receive
    /// path before applying. `None` for snapshot checkpoints / genesis
    /// rows that have no author signature to record.
    pub delta_signature: Option<[u8; 64]>,
    /// The author's consent and the two certificates behind it, for a delta
    /// produced by an executor on the author's behalf. `None` for a
    /// self-authored delta, which is every delta today.
    ///
    /// Persisted rather than left on the gossip envelope for the reason
    /// `producing_bytecode_id` still cannot be bound into a signature: the
    /// delegated preimage embeds the warrant, so a catchup or parent-fetch
    /// responder that did not store it could not serve a delta any initiator
    /// could verify. A field that is signed over has to survive every path the
    /// delta can arrive by.
    pub delegation: Option<calimero_account::Delegation>,
}

impl ContextDagDelta {
    /// Deserialize actions from the serialized byte array
    ///
    /// # Errors
    ///
    /// Returns an error if the actions cannot be deserialized
    pub fn deserialize_actions(
        &self,
    ) -> Result<Vec<calimero_storage::action::Action>, borsh::io::Error> {
        borsh::from_slice(&self.actions)
    }

    /// Deserialize events from the serialized byte array (if present)
    ///
    /// # Errors
    ///
    /// Returns an error if the events cannot be deserialized
    #[cfg(feature = "serde")]
    pub fn deserialize_events(&self) -> Result<Option<Vec<serde_json::Value>>, eyre::Report> {
        if let Some(ref events_bytes) = self.events {
            let events: Vec<serde_json::Value> = serde_json::from_slice(events_bytes)
                .map_err(|e| eyre::eyre!("Failed to deserialize events: {}", e))?;
            Ok(Some(events))
        } else {
            Ok(None)
        }
    }
}

impl PredefinedEntry for key::ContextDagDelta {
    type Codec = Borsh;
    type DataType<'a> = ContextDagDelta;
}

/// The warrant nonces this node has already accepted from one author device in
/// one context, as a sliding replay window.
///
/// **Not a high-water mark, and that is the whole design.** A
/// strictly-increasing rule is delivery-order dependent: a peer that sees nonce
/// 7 before nonce 5 would refuse the 5, while a peer that saw them the other way
/// round accepted both — the two peers then hold different state for the same
/// history, which is divergence rather than replay protection. Gossip gives no
/// ordering between two warrants from the same device, so the rule has to be
/// order-independent.
///
/// A window is the standard answer and is bounded: 16 bytes per active author
/// device per context, whatever the nonce values are.
///
/// # Why not `calimero_governance_store::NonceWindow`
///
/// That type solves the same shape for governance-op nonces and is strictly
/// better there: a contiguous floor plus a sparse set of applied nonces above
/// it, so a late nonce is never wrongly refused however far behind it is. It can
/// afford the sparse set because a signer's governance ops are DAG ancestors of
/// each other, so gaps fill and the set collapses back into the floor.
///
/// Warrant nonces have no such guarantee, and the reason is adversarial rather
/// than incidental: a relay withholding one of a member's requests is an
/// explicit part of what this ledger exists to detect. A withheld nonce is a gap
/// that never fills, so the set above the floor grows without bound — one entry
/// per warrant the relay chose to drop, per author device, per context. That is
/// a cheap remote memory-growth primitive handed to exactly the party the
/// warrant protects against.
///
/// A fixed window pays for that bound with a real cost, stated on
/// [`Self::WINDOW`]: a warrant delayed past the window is refused. The trade is
/// deliberate and the two types should not be merged without revisiting it.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextWarrantNonce {
    /// The highest nonce accepted so far. Always a member of the accepted set.
    pub high_water: u64,
    /// Bitmap of the 64 nonces below [`Self::high_water`]: bit `i` set means
    /// `high_water - 1 - i` has been accepted.
    pub window: u64,
}

impl ContextWarrantNonce {
    /// Window width. A nonce further below the high-water mark than this is
    /// refused because this node can no longer tell whether it was already
    /// spent, and guessing in the accepting direction is what a replay wants.
    ///
    /// The residual is that a delta delayed by more than 64 of its author's own
    /// warrants is refused, and refused only on the peers that fell behind. That
    /// is a real cost and it is the bounded end of the trade: the alternative is
    /// an unbounded set of seen nonces per device.
    pub const WINDOW: u64 = 64;

    /// The state after accepting `nonce`, or `None` if it must be refused.
    ///
    /// Pure, so the rule can be tested without a store — and it is the whole
    /// safety property, so it is worth testing directly.
    #[must_use]
    pub const fn accept(self, nonce: u64) -> Option<Self> {
        if nonce > self.high_water {
            let advance = nonce - self.high_water;
            // The old high-water mark becomes a set bit, and everything shifts
            // down by the distance moved. `checked_shl` handles a jump wider
            // than the window: nothing below the new mark is known any more.
            let shifted = match advance {
                d if d >= Self::WINDOW => 0,
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "guarded above: advance < WINDOW == 64"
                )]
                d => (self.window << (d as u32)) | (1u64 << (d as u32 - 1)),
            };
            return Some(Self {
                high_water: nonce,
                window: shifted,
            });
        }

        let below = self.high_water - nonce;
        if below == 0 || below > Self::WINDOW {
            // Already the mark, or too old to judge.
            return None;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "guarded above: 0 < below <= WINDOW == 64"
        )]
        let bit = 1u64 << (below as u32 - 1);
        if self.window & bit != 0 {
            return None;
        }
        Some(Self {
            high_water: self.high_water,
            window: self.window | bit,
        })
    }

    /// The state after accepting `nonce` as the first one ever seen.
    #[must_use]
    pub const fn first(nonce: u64) -> Self {
        Self {
            high_water: nonce,
            window: 0,
        }
    }
}

impl PredefinedEntry for key::ContextWarrantNonce {
    type Codec = Borsh;
    type DataType<'a> = ContextWarrantNonce;
}

/// Raw-bytes value for a unified causal-log op row (cutover C2): the
/// borsh-serialized `calimero_op::Op`. Kept opaque (`Identity` codec) because
/// `calimero_store` cannot depend on `calimero_op` (the dependency points the
/// other way) — the `calimero-context` layer borsh-codes the `Op` and stores the
/// bytes here, the same shape as [`ContextState`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ScopeUnifiedOp<'a> {
    pub value: Slice<'a>,
}

impl PredefinedEntry for key::ScopeUnifiedOp {
    type Codec = Identity;
    type DataType<'a> = ScopeUnifiedOp<'a>;
}

impl<'a> From<Slice<'a>> for ScopeUnifiedOp<'a> {
    fn from(value: Slice<'a>) -> Self {
        Self { value }
    }
}

impl AsRef<[u8]> for ScopeUnifiedOp<'_> {
    fn as_ref(&self) -> &[u8] {
        self.value.as_ref()
    }
}

#[cfg(test)]
mod context_authored_remaining_tests {
    use borsh::BorshDeserialize;

    use super::ContextAuthoredRemaining;

    // The dedicated counter value round-trips through borsh.
    #[test]
    fn authored_remaining_roundtrips() {
        let v = ContextAuthoredRemaining { count: 5 };
        let bytes = borsh::to_vec(&v).expect("serialize");
        let back = ContextAuthoredRemaining::try_from_slice(&bytes).expect("deserialize");
        assert_eq!(back.count, 5);
    }
}

#[cfg(test)]
mod context_local_key_isolation_tests {
    use std::sync::Arc;

    use calimero_primitives::context::ContextId;

    use crate::db::InMemoryDB;
    use crate::key;
    use crate::types::{ContextAuthoredRemaining, ContextMigrationFailed};
    use crate::Store;

    // ContextMigrationFailed lives in its own column, so its context_id-only key
    // must not share a KV row with the same-shaped ContextAuthoredRemaining key.
    // (Sharing ContextLocal collided: a failure write clobbered the count, and a
    // count of 1 misdecoded as `check_aborted`.) Writing/clearing one must leave
    // the other untouched.
    #[test]
    fn migration_failed_does_not_collide_with_authored_remaining() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ctx = ContextId::from([7u8; 32]);
        let ar = key::ContextAuthoredRemaining::new(ctx);
        let mf = key::ContextMigrationFailed::new(ctx);

        let mut h = store.handle();
        h.put(&ar, &ContextAuthoredRemaining { count: 5 }).unwrap();
        h.put(&mf, &ContextMigrationFailed { kind: 2 }).unwrap();

        // Independent rows — neither write clobbered the other.
        assert_eq!(h.get(&ar).unwrap().unwrap().count, 5);
        assert_eq!(h.get(&mf).unwrap().unwrap().kind, 2);

        // Clearing the failure marker must NOT delete the authored-remaining row.
        h.delete(&mf).unwrap();
        assert_eq!(h.get(&ar).unwrap().unwrap().count, 5);
        assert!(h.get(&mf).unwrap().is_none());
    }
}

/// Value for [`key::ContextExecutingBytecode`]: the bytecode blob this context's
/// committed state executes under, when it differs from the application
/// row's (version-stable bundle id, row already overwritten in place by a
/// newer version). Written on logical migration abort; deleted when a
/// migrate succeeds. Node-local; a missing row means "execute the row's
/// bytecode" (today's behavior).
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "single pin value; additions would need a migration"
)]
pub struct ContextExecutingBytecode {
    pub blob: [u8; 32],
}

impl PredefinedEntry for key::ContextExecutingBytecode {
    type Codec = Borsh;
    type DataType<'a> = ContextExecutingBytecode;
}

/// Value for [`key::ContextActivatedBytecode`]: the bytecode blob this context
/// last ACTIVATED — set when a migration commits or a code-only swap is
/// applied, moved forward only. The single up-to-date check everywhere is
/// `marker == group.bytecode_id`; it replaces the legacy method-name and
/// `blob:`-string markers (which are folded forward on first read).
/// Node-local; a missing row means "never activated by v2 machinery".
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "single marker value; additions would need a migration"
)]
pub struct ContextActivatedBytecode {
    pub blob: [u8; 32],
}

impl PredefinedEntry for key::ContextActivatedBytecode {
    type Codec = Borsh;
    type DataType<'a> = ContextActivatedBytecode;
}

/// Value for [`key::ContextActivatedStateVersion`]: the ABI state version the
/// activated bytecode declares, recorded beside the activation marker because
/// the install row is per-`ApplicationId` and a same-id bundle upgrade never
/// moves it. Node-local; a missing row means the version was unresolvable.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "single marker value; additions would need a migration"
)]
pub struct ContextActivatedStateVersion {
    pub state_version: u32,
}

impl PredefinedEntry for key::ContextActivatedStateVersion {
    type Codec = Borsh;
    type DataType<'a> = ContextActivatedStateVersion;
}

#[cfg(test)]
mod warrant_nonce_tests {
    use super::ContextWarrantNonce as W;

    /// The base case: one warrant, then the next.
    #[test]
    fn a_fresh_nonce_above_the_mark_is_accepted_once() {
        let w = W::first(7);
        let w = w.accept(8).expect("8 is above the mark");
        assert_eq!(w.high_water, 8);
        assert!(w.accept(8).is_none(), "8 must not be spendable twice");
        assert!(w.accept(7).is_none(), "7 was the previous mark");
    }

    /// The property a high-water mark cannot give: arrival order must not change
    /// the verdict, or two peers that saw the same warrants in different orders
    /// end up in different states.
    #[test]
    fn the_verdict_is_independent_of_arrival_order() {
        let forward = {
            let mut w = W::first(1);
            for n in [2, 3, 5, 8, 13] {
                w = w.accept(n).expect("each is new");
            }
            w
        };
        let backward = {
            let mut w = W::first(13);
            for n in [8, 5, 3, 2, 1] {
                w = w.accept(n).expect("each is new, arriving late");
            }
            w
        };
        assert_eq!(
            forward, backward,
            "the same set of nonces must leave the same state whichever order it arrived in"
        );

        // And in both, every one of them is now spent.
        for n in [1, 2, 3, 5, 8, 13] {
            assert!(forward.accept(n).is_none(), "{n} must be spent");
            assert!(backward.accept(n).is_none(), "{n} must be spent");
        }
    }

    /// A late warrant inside the window is still accepted — this is the case a
    /// strictly-increasing rule got wrong.
    #[test]
    fn a_late_nonce_inside_the_window_is_accepted() {
        let w = W::first(100);
        let w = w
            .accept(100 - W::WINDOW + 1)
            .expect("just inside the window");
        assert_eq!(w.high_water, 100, "a late nonce must not move the mark");
    }

    /// And the boundary, both sides of it. `WINDOW` counts the nonces BELOW the
    /// mark, so `high_water - WINDOW` is the oldest one still judgeable and
    /// `high_water - WINDOW - 1` is the first that is not.
    #[test]
    fn the_window_boundary_is_where_it_says_it_is() {
        let w = W::first(100);
        assert!(
            w.accept(100 - W::WINDOW).is_some(),
            "the oldest nonce inside the window must still be accepted"
        );
        assert!(
            w.accept(100 - W::WINDOW - 1).is_none(),
            "the first nonce past the window must be refused"
        );
        assert!(
            w.accept(1).is_none(),
            "far below the window must be refused"
        );
    }

    /// A jump wider than the window clears it: nothing below the new mark is
    /// known any more, so nothing below it may be accepted on a guess.
    #[test]
    fn a_jump_past_the_window_forgets_what_it_can_no_longer_judge() {
        let w = W::first(1);
        let w = w.accept(2).expect("2 is new");
        let jumped = w.accept(1_000).expect("a big jump is still a new nonce");
        assert_eq!(jumped.window, 0, "the window must not carry stale bits");
        assert!(
            jumped.accept(2).is_none(),
            "a nonce below the window must be refused, not re-accepted"
        );
    }

    /// Saturation: a full window must not wrap around and start accepting.
    #[test]
    fn a_full_window_keeps_refusing() {
        let mut w = W::first(W::WINDOW + 1);
        for n in 2..=W::WINDOW {
            w = w.accept(n).expect("filling the window");
        }
        for n in 2..=(W::WINDOW + 1) {
            assert!(w.accept(n).is_none(), "{n} is spent and must stay refused");
        }
        assert!(
            w.accept(W::WINDOW + 2).is_some(),
            "the next one still works"
        );
    }
}
