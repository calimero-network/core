//! Storage interface implementing a repository pattern for CRDT-based data.
//!
//! This module provides the primary API for interacting with the storage system,
//! handling entity persistence, hierarchy management, and distributed synchronization.
//!
//! # Architecture
//!
//! Calimero uses a **hybrid CRDT model**:
//! - **Operation-based (CmRDT)**: Local changes emit [`Action`]s propagated to peers
//! - **State-based (CvRDT)**: Merkle tree comparison for catch-up/reconciliation
//!
//! Each element maintains two Merkle hashes (own data, and full including descendants)
//! enabling efficient tree comparison—only subtrees with differing hashes need examination.
//!
//! # API Entry Points
//!
//! **Direct Operations:**
//! - [`save()`](Interface::save()) - Save/update entities
//! - [`add_child_to()`](Interface::add_child_to()) - Add to collections
//! - [`remove_child_from()`](Interface::remove_child_from()) - Remove from collections
//! - [`find_by_id()`](Interface::find_by_id()) - Direct lookup
//!
//! **Synchronization:**
//! - [`apply_action()`](Interface::apply_action()) - Execute remote changes
//!
//! # Conflict Resolution
//!
//! - Last-write-wins based on timestamps
//! - Orphaned children (from out-of-order ops) stored temporarily
//! - Future comparison reconciles inconsistencies
//!
//! See the [crate README](../README.md) for detailed design documentation.

#[cfg(test)]
#[path = "tests/interface.rs"]
mod tests;

use core::fmt::Debug;
use core::marker::PhantomData;
use std::collections::BTreeMap;

use borsh::{from_slice, to_vec};
use calimero_account::AccountId;
use calimero_primitives::identity::PublicKey;
use sha2::{Digest, Sha256};
use tracing::{debug, info, trace, warn};

use crate::address::Id;
use crate::constants;
use crate::entities::{ChildInfo, Data, Metadata, OpMask, SignatureData, StorageType};
use crate::env::time_now;
use crate::index::Index;
use crate::store::{Key, MainStorage, StorageAdaptor};

// Re-export types for convenience
pub use crate::action::Action;
pub use crate::error::StorageError;

/// Convenient type alias for the main storage system.
pub type MainInterface = Interface<MainStorage>;

/// Whether a root entity's stored `crdt_type` marks it as *opaque* — a root
/// with no application-defined `Mergeable` merge dispatch.
///
/// An opaque root is one whose metadata carries no `crdt_type` (`None`). This
/// is how the app-state container is stored for a JS app (written locally via
/// the host `persist_root_state` → `save_raw` with `Metadata::new`, which
/// leaves `crdt_type: None`) and for any app that does not use `#[app::state]`.
/// A `#[app::state]` root registers a field-by-field `Mergeable` via the WASM
/// module, and its root merge succeeds through the registry rather than being
/// treated as opaque — so this predicate must never turn such a root into an
/// LWW fallback. It is consulted only after the merge registry reports no
/// registered function, so a registered merger always wins first.
///
/// The synthetic `LwwRegister { inner_type: "Opaque" }` marker the node sync
/// layer attaches to opaque leaves *on the wire* is deliberately NOT matched
/// here: that marker is a HashComparison wire-format concern owned by the node
/// crate and never reaches a local `save_raw`/`save_internal` write (the sync
/// apply path persists opaque roots with `crdt_type: None`, not the marker).
/// The local write path this predicate guards only ever observes `None`.
#[inline]
fn is_opaque_root_crdt_type(crdt_type: &Option<crate::collections::crdt_meta::CrdtType>) -> bool {
    crdt_type.is_none()
}

/// Apply-time context passed to [`Interface::apply_action`].
///
/// Centralizes apply-time metadata so the call signature doesn't accumulate
/// positional parameters. Per #2266 (DAG-causal Shared verifier), the node
/// sync layer pre-resolves the writer set for a delta via
/// `rotation_log_reader::writers_at(parents, happens_before)` and passes
/// it here as `effective_writers`; storage no longer needs DAG ancestry
/// knowledge. The closure-typed `happens_before` and `causal_parents`
/// fields the P1/P3 design carried have been removed.
///
/// # Field semantics
///
/// - `effective_writers: Some(set)` → caller pre-resolved the
///   ADR-0001-compliant writer set as of the delta's causal point.
///   The Shared verifier MUST validate against this set.
///
///   This set is the authorization decision itself, so storage takes it on
///   trust and the caller owes it two properties. It must be resolved from
///   the *local* rotation log — a set chosen by whoever authored the delta
///   would let a peer name itself a writer of any Shared object and then
///   satisfy the signature check against its own set (a signature proves
///   possession of a key, never whose key it is). And it must be resolved
///   with `writers_at_authenticated`, not `writers_at`, because the
///   rotation log rides ordinary sync: each entry earns its place only when
///   its signer held ADMIN in the set resolved just before it. The node's
///   `ContextStorageApplier::apply` is the only production caller that
///   passes `Some`, and it does both — which is also why the sync paths
///   refuse a wire-supplied `StorageDelta::CausalActions` outright rather
///   than forwarding the writer set it carries.
/// - `effective_writers: None` → caller has no DAG context (snapshot
///   leaf push, local apply, tests). The verifier falls back to the
///   entity's currently-stored `metadata.storage_type.writers` (v2
///   semantics, preserved for these known-safe paths).
/// - `delta_id` / `delta_hlc` carry the originating `CausalDelta`'s
///   identity. Both populated together: the rotation-log write hook
///   appends an entry only when both are `Some`.
#[derive(Clone, Debug)]
pub struct ApplyContext {
    /// Pre-resolved authoritative writer set for `Shared` actions. When
    /// `Some`, the verifier validates the signature against this set and
    /// skips the v2 stored-writers fallback. Resolved by the applying
    /// node's own sync layer from its own rotation log; see the trust
    /// contract on the type docs before adding a caller that passes `Some`.
    pub effective_writers: Option<BTreeMap<AccountId, OpMask>>,

    /// Hash of the `CausalDelta` containing the action being applied. Used
    /// by the rotation-log write hook to record the originating delta on
    /// detected rotations. `None` for local apply / snapshot leaf push.
    pub delta_id: Option<[u8; 32]>,

    /// Hybrid timestamp of the containing `CausalDelta`. Used by the
    /// rotation-log write hook (sibling tiebreak per ADR 0001). `None` for
    /// callers without a `CausalDelta` in scope.
    pub delta_hlc: Option<crate::logical_clock::HybridTimestamp>,

    /// The account the action's signing key speaks for, resolved by the node at
    /// the action's causal cut.
    ///
    /// The writer set names accounts; a signature names a key. This is the bridge,
    /// and it is resolved by the caller because that resolution needs the device
    /// bindings folded to this action's cut — neither of which this crate has.
    /// Resolving it live instead would let two nodes at different fold depths
    /// disagree about who may write, which splits the root rather than rejecting a
    /// write.
    ///
    /// `None` means "could not be resolved", and every consumer treats that as a
    /// refusal. It must never fall back to the locally executing account: a remote
    /// action would then authorize itself.
    ///
    /// **Contract the caller owes, which this crate cannot check.** This must be
    /// the resolution of *this action's own* `signature_data.signer`. Storage
    /// verifies two things — that the signature is valid under the key the action
    /// names, and that this account is in the writer set — but it has no bindings
    /// with which to confirm the two describe the same principal. Hand it an
    /// authorized account beside an unrelated key's valid signature and the write
    /// is accepted. Production cannot produce that pair (one delta has one author,
    /// and the account is resolved *from* that author's key), which is exactly why
    /// the resolution must stay in one place instead of being assembled from two.
    pub signer_account: Option<AccountId>,
}

impl ApplyContext {
    /// Construct an empty context (no DAG-causal resolution available).
    /// Used by snapshot-leaf push, local apply, and tests that don't
    /// exercise the verifier swap. Verifier behavior is identical to v2
    /// (validate against stored writers).
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            effective_writers: None,
            delta_id: None,
            delta_hlc: None,
            signer_account: None,
        }
    }
}

// ----- Test-only hook: bypass the v2 monotonic-nonce check -----------------
//
// The v2 nonce check rejects out-of-order delivery of concurrent deltas —
// exactly the cases #2233's DAG-causal verifier is designed to accept. Per
// the epic exit criterion the nonce check is removed only after 4 weeks of
// production telemetry confirming DAG-causal subsumes it. Tests that need
// to exercise the v3 target behavior (post-removal) can opt out here.
//
// Gated on `cfg(any(test, feature = "testing"))` so dependent crates' tests
// (notably `calimero-node`'s migrated P3/P5 partition scenarios — see
// #2266 step 5) can opt into the bypass via the `testing` feature on the
// storage dev-dependency. Production builds (no `testing` feature, no
// `cfg(test)`) compile out the toggle entirely so the nonce check stays
// live — `nonce_check_disabled_for_testing` reduces to `const false`.
//
// SECURITY: the `testing` feature disables replay protection for Shared
// storage actions. The compile-error below blocks any release build that
// accidentally activates it — the typical path is a downstream crate
// declaring `calimero-storage = { ..., features = ["testing"] }` as a
// regular dependency rather than `[dev-dependencies]`. Cargo's feature
// unification would then propagate it into the production binary. The
// guard fires only in release-without-test, so dev builds and `cargo test`
// (with or without `--release` on test profile) keep working. Per #2272
// review.

#[cfg(all(feature = "testing", not(test), not(debug_assertions)))]
compile_error!(
    "calimero-storage `testing` feature enables `disable_nonce_check_for_testing`, \
     which turns off replay protection for Shared storage actions. \
     This must NEVER be enabled in a release build. \
     If you see this error: a dependency declared `features = [\"testing\"]` \
     outside `[dev-dependencies]` and Cargo's feature unification leaked \
     it into the release graph. Move it into `[dev-dependencies]` or drop \
     the feature."
);

#[cfg(any(test, feature = "testing"))]
thread_local! {
    static SKIP_NONCE_CHECK: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

/// Disable the v2 monotonic-nonce check on this thread. Returns a guard
/// that re-enables on drop, so a single test can scope the bypass without
/// leaking it to the next test on the same thread.
///
/// # Security
///
/// **This disables replay protection for Shared storage actions.** Use
/// it **only** when validating the v3 target behavior (post-#2266
/// telemetry-soak nonce-check removal). Never call this from production
/// code paths — the `testing` feature it depends on is rejected at
/// compile time in release builds, but a stray call from a non-test code
/// path inside a debug build would still create a window.
///
/// Tests of the nonce check itself (or of behavior expected to hold
/// under the v2 regime) should NOT bypass.
#[cfg(any(test, feature = "testing"))]
#[must_use]
pub fn disable_nonce_check_for_testing() -> NonceCheckGuard {
    SKIP_NONCE_CHECK.with(|c| c.set(true));
    NonceCheckGuard
}

/// RAII guard returned by [`disable_nonce_check_for_testing`].
#[cfg(any(test, feature = "testing"))]
pub struct NonceCheckGuard;

#[cfg(any(test, feature = "testing"))]
impl Drop for NonceCheckGuard {
    fn drop(&mut self) {
        SKIP_NONCE_CHECK.with(|c| c.set(false));
    }
}

#[cfg(any(test, feature = "testing"))]
fn nonce_check_disabled_for_testing() -> bool {
    SKIP_NONCE_CHECK.with(core::cell::Cell::get)
}

#[cfg(not(any(test, feature = "testing")))]
const fn nonce_check_disabled_for_testing() -> bool {
    false
}

/// Why a child is being removed from its collection, selecting whether the
/// Frozen-deletion guard applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoveMode {
    /// A genuine semantic deletion: reject `Frozen` children.
    Delete,
    /// A deterministic re-key relocation: the child is immediately re-inserted
    /// under a new id, so `Frozen` children are relocated rather than rejected.
    Relocate,
}

/// A resolved local `Shared` stamp authorization: the writer set to persist
/// paired with the signer to record. Produced by
/// [`Interface::authorize_local_shared_stamp`].
type SharedStampAuthorization = (BTreeMap<AccountId, OpMask>, PublicKey);

/// The primary interface for the storage system.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct Interface<S: StorageAdaptor = MainStorage>(PhantomData<S>);

impl<S: StorageAdaptor> Interface<S> {
    /// Resolve a [`SharedMember`](StorageType::SharedMember)'s writer set from
    /// its `anchor`'s **locally verified** state, mirroring
    /// `SharedStorage::current_writers`:
    ///
    /// 1. the anchor's rotation log (latest entry, then its compacted
    ///    snapshot) — only ever written by a signature-verified rotation apply
    ///    or the originating node's own committed rotation; and
    /// 2. the anchor's index metadata (`Shared { writers }`).
    ///
    /// Returns the empty set when the anchor has neither — i.e. the anchor has
    /// not synced to this node yet. The caller treats the empty set as "cannot
    /// verify this member yet" (fail closed / buffer), never as "no writers".
    /// This is the local-execution / settled-state resolver; the
    /// causal-cut-accurate resolution at merge is the node layer's
    /// `writers_at(anchor_log, delta.parents)`, passed in via
    /// `effective_writers`.
    ///
    /// Resolution uses [`rotation_log::resolve_local`](crate::rotation_log::resolve_local):
    /// the live entry that is max by `(delta_hlc, signer)`, or the compaction
    /// snapshot when there are no live entries. This is **not** a full causal
    /// cut (it has no `happens_before`), so it is reserved for the
    /// **local-execution / settled-state** gate, where "current writers" is the
    /// right answer. The **merge-path** security boundary is the causal
    /// `writers_at(anchor_log, delta.parents)` set passed as `effective_writers`.
    /// Because the HLC is causally monotonic since #2635, the `(delta_hlc,
    /// signer)` max coincides with the causal latest for a well-formed log, and
    /// — unlike the prior `entries.last()` — it is insertion-order invariant, so
    /// it converges across nodes under concurrent rotations (core#2673).
    ///
    /// As of the DAG-causal rotation completion (P4), every node records the
    /// genesis writer set **and its own rotations** in the log (the originator
    /// via `add_local_applied_delta`'s self-log, receivers via
    /// `maybe_append_rotation_log`, cold-joiners via a seeded floor). With a
    /// complete log on every node, `writers_at` is **total** — it never returns
    /// `None` for a causal cut, so the node always supplies `effective_writers`
    /// and this non-causal fallback is no longer reached on the merge path for
    /// anchors created post-P4. It remains for local execution (correct there)
    /// and for legacy anchors whose log predates P4 (a vanishing set after a
    /// state reset).
    pub(crate) fn resolve_anchor_writers(anchor: Id) -> BTreeMap<AccountId, OpMask> {
        // core#2716 P3: the rotation log is a real `UnorderedMap` child of the
        // anchor (see `rotation_log_map`) and is THE authoritative, synced
        // source — it converges identically on every node via HashComparison's
        // structural add-wins merge. Resolve the latest writer set from it.
        //
        // Fall back to the anchor's stored `metadata.storage_type.writers` only
        // for an anchor with no collection yet (legacy/bootstrap, or a cold join
        // that hasn't materialised it — that stored set is the last-applied
        // writers, correct for those non-causal paths).
        if let Some(child_log) = Self::load_rotation_log_child(anchor) {
            if let Some(writers) = crate::rotation_log::resolve_local(&child_log) {
                return writers;
            }
        }
        if let Ok(Some(metadata)) = <Index<S>>::get_metadata(anchor) {
            if let StorageType::Shared { writers, .. } = metadata.storage_type {
                return writers;
            }
        }
        BTreeMap::new()
    }

    /// Resolve an anchor's writer set **as of** the causal point of a write at
    /// storage-HLC `at` (core#2716/#2673), rather than the latest set
    /// ([`Self::resolve_anchor_writers`]).
    ///
    /// Used to verify a `SharedMember`/`Shared` value whose causal DAG position
    /// is unavailable — a HashComparison-pushed leaf carries no delta parents,
    /// so the node can't run the exact `writers_at(parents)` the gossip path
    /// uses. Verifying such a value against the LATEST writers wrongly rejects a
    /// value authored under an earlier rotation whose writer a later rotation
    /// removed (the residual concurrent-rotation split-brain). Resolving as of
    /// the value's own HLC authorizes it against the set that was in effect when
    /// it was written.
    ///
    /// Reads the authoritative rotation-log collection child
    /// ([`Self::load_rotation_log_child`]) and applies
    /// [`rotation_log::resolve_local_as_of`]. Falls back to the anchor's stored
    /// writers (last-applied) for a value authored before any signed rotation,
    /// or a legacy anchor with no collection.
    pub(crate) fn resolve_anchor_writers_as_of(anchor: Id, at: u64) -> BTreeMap<AccountId, OpMask> {
        if let Some(child_log) = Self::load_rotation_log_child(anchor) {
            if let Some(writers) = crate::rotation_log::resolve_local_as_of(&child_log, at) {
                return writers;
            }
        }
        if let Ok(Some(metadata)) = <Index<S>>::get_metadata(anchor) {
            if let StorageType::Shared { writers, .. } = metadata.storage_type {
                return writers;
            }
        }
        BTreeMap::new()
    }

    /// Originator-side rotation logging: for each `Shared` rotation in this
    /// delta's `actions`, append it to the anchor's hashed rotation-log
    /// collection (idempotent on `delta_id`, and only when the writer set
    /// actually changed). Returns `true` if any anchor changed, so the caller
    /// can recompute the context root.
    ///
    /// The local write path persists the anchor and its children *during* WASM
    /// execution — before the delta (hence `delta_id`) exists — so the
    /// originator's own rotation isn't in its log yet. The execute pipeline
    /// calls this once the delta is built (and signed) to record the
    /// originator's OWN rotation, so it converges with peers that apply the
    /// rotation as a delta. The `insert` into the rotation-log collection
    /// propagates the new child's hash into the anchor's `full_hash` (and up to
    /// the root) on its own — no separate anchor rehash is needed.
    ///
    /// # Errors
    /// Propagates rotation-log / index read/write failures.
    pub fn self_log_own_rotations(
        actions: &[crate::action::Action],
        delta_id: [u8; 32],
        delta_hlc: crate::logical_clock::HybridTimestamp,
    ) -> Result<bool, StorageError> {
        use crate::action::Action;

        let mut changed = false;
        for action in actions {
            let (id, metadata) = match action {
                Action::Add { id, metadata, .. } | Action::Update { id, metadata, .. } => {
                    (*id, metadata)
                }
                Action::DeleteRef { .. } => continue,
            };
            let StorageType::Shared {
                writers,
                signature_data,
            } = &metadata.storage_type
            else {
                continue;
            };

            // Read the authoritative rotation-log collection for the
            // prior-writer check + dedup.
            let existing = Self::load_rotation_log_child(id)
                .unwrap_or_else(crate::rotation_log::RotationLog::empty);
            // Append only on an actual rotation (writers changed from the latest
            // logged set). `resolve_local` picks the causally-latest entry; a
            // value-write that re-stamps the same writers is a no-op here.
            if crate::rotation_log::resolve_local(&existing).as_ref() == Some(writers) {
                continue;
            }
            // Dedup on delta_id (idempotent replay).
            if existing.entries.iter().any(|e| e.delta_id == delta_id) {
                continue;
            }

            let entry = crate::rotation_log::RotationLogEntry {
                delta_id,
                delta_hlc,
                signer: signature_data.as_ref().and_then(|s| s.signer),
                signature: signature_data.as_ref().map(|s| s.signature),
                signed_payload: signature_data
                    .as_ref()
                    .map(|_| action.payload_for_signing()),
                new_writers: writers.clone(),
                writers_nonce: signature_data.as_ref().map(|s| s.nonce).unwrap_or(0),
            };
            // Write the entry into the hashed child collection (authoritative).
            // The originator builds it from its own signed action; a receiver
            // builds the identical entry from the same delta via
            // `build_rotation_entry` — byte-identical, so the per-`delta_id`
            // children converge across nodes under the add-wins collection merge.
            // (Unsigned/bootstrap entries are skipped by `append_rotation_to_child`.)
            Self::append_rotation_to_child(id, &entry)?;
            changed = true;
        }
        Ok(changed)
    }

    /// Field key for the rotation-log **collection** parent under a `Shared`
    /// anchor (P3 of core#2716). The rotation log is an `UnorderedMap`-shaped
    /// child: a parent entity here, with one child PER `delta_id` (each holding
    /// a single-entry `RotationLog`). Per-entry children are separate
    /// content-addressed Merkle leaves, so HashComparison reconciles them
    /// individually via the proven structural add-wins path — a single blob
    /// child does NOT converge under HC (its custom merge re-runs every round).
    pub(crate) const ROTATION_LOG_CHILD_KEY: &'static [u8] = b"__calimero_rotation_log__";

    /// Id of the rotation-log collection PARENT for `anchor`.
    pub fn rotation_log_child_id(anchor: Id) -> Id {
        crate::collections::compute_id(anchor, Self::ROTATION_LOG_CHILD_KEY)
    }

    /// Open a handle to `anchor`'s rotation-log map (P3 of core#2716).
    ///
    /// The rotation log is a real
    /// [`UnorderedMap<[u8; 32], RotationLogEntry>`](crate::collections::UnorderedMap)
    /// child of the `Shared` anchor, keyed by `delta_id`. Using the genuine
    /// collection type — rather than the previous hand-rolled per-`delta_id`
    /// children stamped `CrdtType::RotationLog` — means each entry rides the
    /// proven structural add-wins collection merge: `insert` routes through
    /// `Interface::add_child_to`, which seeds the entry's REAL `own_hash` into
    /// the parent's `ChildInfo` (the hand-rolled path seeded `[0u8; 32]` and
    /// relied on a later `update_hash_for` to backfill it, which did not
    /// propagate into the parent's child list — so HashComparison saw equal
    /// subtree hashes and never reconciled the per-`delta_id` children).
    ///
    /// This only OPENS a handle at the deterministic id; the parent entity
    /// itself must already be linked under the anchor (see
    /// [`Self::ensure_rotation_log_parent`]).
    fn rotation_log_map(
        anchor: Id,
    ) -> crate::collections::UnorderedMap<[u8; 32], crate::rotation_log::RotationLogEntry, S> {
        crate::collections::UnorderedMap::open_existing(Self::rotation_log_child_id(anchor))
    }

    /// Read the rotation log by collecting every `delta_id → RotationLogEntry`
    /// value of the [`UnorderedMap`](Self::rotation_log_map) child (P3). `None`
    /// if no rotation has been recorded yet (the parent map does not exist).
    pub fn load_rotation_log_child(anchor: Id) -> Option<crate::rotation_log::RotationLog> {
        let map_id = Self::rotation_log_child_id(anchor);
        S::storage_read(Key::Entry(map_id))?;
        let map = Self::rotation_log_map(anchor);
        let mut entries: Vec<crate::rotation_log::RotationLogEntry> = map
            .entries()
            .ok()?
            .map(|(_delta_id, entry)| entry)
            .collect();
        // Canonical order so resolution is insertion-order invariant.
        entries.sort_by(|a, b| a.delta_id.cmp(&b.delta_id));
        Some(crate::rotation_log::RotationLog {
            snapshot: None,
            entries,
        })
    }

    /// Persist a whole log into the collection: insert each entry under its
    /// `delta_id` key (idempotent). Used by the side-store mirror; the apply
    /// paths prefer [`Self::append_rotation_to_child`] for a single entry.
    ///
    /// # Errors
    /// Propagates serialization / storage failures.
    pub fn save_rotation_log_child(
        anchor: Id,
        log: &crate::rotation_log::RotationLog,
    ) -> Result<(), StorageError> {
        for entry in &log.entries {
            Self::append_rotation_to_child(anchor, entry)?;
        }
        Ok(())
    }

    /// Ensure the rotation-log collection PARENT (an [`UnorderedMap`] entity)
    /// exists and is linked under `anchor`, returning its id.
    ///
    /// The parent is stamped `CrdtType::UnorderedMap` so the merge dispatch and
    /// HashComparison treat it — and its per-`delta_id` children — exactly like
    /// any other map: the parent value-merge returns incoming (structural;
    /// entries are separate child entities), and the children converge by the
    /// add-wins union of the parent's child list. Its own value is the empty
    /// serialized collection (deterministic across nodes — `Element` serializes
    /// only its id, metadata is `#[borsh(skip)]`); only its children carry
    /// rotation entries. `add_child_to` before the value write avoids the
    /// `CannotCreateOrphan` reject; `save_raw`'s `update_hash_for` then sets the
    /// real hash and propagates it into the anchor's `full_hash`.
    fn ensure_rotation_log_parent(anchor: Id) -> Result<Id, StorageError> {
        use crate::collections::crdt_meta::CrdtType;
        let map_id = Self::rotation_log_child_id(anchor);
        if S::storage_read(Key::Entry(map_id)).is_none() {
            let crdt = CrdtType::unordered_map(
                core::any::type_name::<[u8; 32]>(),
                core::any::type_name::<crate::rotation_log::RotationLogEntry>(),
            );
            let meta = Metadata::with_crdt_type(0, 0, crdt);
            <Index<S>>::add_child_to(anchor, ChildInfo::new(map_id, [0u8; 32], meta.clone()))?;
            // Byte-identical to a genuinely-created empty `UnorderedMap` at this
            // id (`Collection` serializes only its `Element`, which serializes
            // only its id), so every node that materialises the parent stores
            // the same bytes and the same `own_hash`.
            let empty = to_vec(&Self::rotation_log_map(anchor))
                .map_err(StorageError::SerializationError)?;
            let _ = Self::save_raw(map_id, empty, meta)?;
        }
        Ok(map_id)
    }

    /// The [`OpMask`] an action requires of its signer to be authorized.
    /// `Add`/`Update` are a single `WRITE` capability for now (INSERT vs UPDATE
    /// is not split — see the OpMask design); `DeleteRef` requires `DELETE`.
    fn required_op_mask(action: &crate::action::Action) -> OpMask {
        use crate::action::Action;
        match action {
            Action::Add { .. } | Action::Update { .. } => OpMask::WRITE,
            Action::DeleteRef { .. } => OpMask::DELETE,
        }
    }

    /// Enforce that the verified `signer` holds `required` in the resolved
    /// capability map. Runs **after** signature verification, so the signer is
    /// known to be a current writer; this is the operation-granularity gate. An
    /// absent signer (shouldn't happen post-verify) fails closed.
    fn enforce_op_mask(
        signer_account: &AccountId,
        required: OpMask,
        writers: &BTreeMap<AccountId, OpMask>,
    ) -> Result<(), StorageError> {
        let granted = writers.get(signer_account).copied().unwrap_or(OpMask::NONE);
        if granted.contains(required) {
            Ok(())
        } else {
            Err(StorageError::ActionNotAllowed(
                "Signer is a writer but lacks the required operation capability".to_owned(),
            ))
        }
    }

    /// Resolve which writer produced `sig_data`'s signature over `payload`,
    /// returning that writer's key on success.
    ///
    /// **A write must name its signer.** One `ed25519_verify` against the named
    /// key, and the key must hold an entry in `writers`. `None` means the write
    /// named nobody, named a non-writer, or the signature does not verify —
    /// callers map all three to `InvalidSignature`.
    ///
    /// The hint used to be optional, falling back to a linear scan that tried
    /// every writer's key until one verified. Two reasons that had to go, and the
    /// second is why this function is shaped the way it is now:
    ///
    /// - **It was verification amplification.** A signature that verifies under
    ///   nobody costs one `ed25519_verify` per writer, on a path any peer can
    ///   drive by sending a malformed delta. The `[0; 64]` placeholder bail only
    ///   covered the trivial case.
    /// - **A scan cannot exist once a writer set names accounts rather than
    ///   keys.** There would be no keys to scan. Requiring the signature to name
    ///   its author is what makes "resolve the author, then ask whether that
    ///   principal is a writer" expressible at all — and those are two separable
    ///   questions, which the scan conflated into one.
    ///
    /// This is the single source of truth for that check, shared by every signed
    /// `Shared`/`SharedMember` arm (upsert and delete) and the snapshot verifiers.
    /// Callers needing only a yes/no answer use `.is_some()`; callers needing the
    /// signer for the operation-granularity gate use the returned key.
    ///
    /// The caller is responsible for the `[0; 64]` placeholder reject before
    /// calling this.
    fn resolve_signer(
        writers: &BTreeMap<AccountId, OpMask>,
        sig_data: &crate::entities::SignatureData,
        payload: &[u8],
        signer_account: Option<AccountId>,
    ) -> Option<AccountId> {
        // The two questions, in order. Authentication is about a KEY: only the
        // named signing key can produce this signature, and only a device holds a
        // key. Authorization is about an ACCOUNT: the writer set names people, so a
        // person's second device passes without being granted separately.
        //
        // `signer_account` is the node's resolution of `sig_data.signer` to the
        // account it speaks for, taken at the write's causal cut. It is a parameter
        // rather than something looked up here because this crate has no store and
        // no cut — and resolving live would reintroduce the divergence class the
        // account plane spent its review on: two nodes that folded different amounts
        // of the device-binding history would disagree about who may write, which is
        // a split root rather than a rejected write.
        //
        // `None` is a hard reject, never a fallback to the locally executing
        // account. A remote delta whose signer this node cannot yet resolve must be
        // refused (and retried once the binding folds), because defaulting to the
        // local account would let any delta authorize itself.
        let signer = sig_data.signer?;
        let account = signer_account?;

        // Cheap check before the expensive one — this ordering is what stage 1
        // bought: a signature that verifies under nobody used to cost one
        // `ed25519_verify` per writer, on a path any peer can drive.
        if !writers.contains_key(&account) {
            return None;
        }
        crate::env::ed25519_verify(&sig_data.signature, signer.digest(), payload).then_some(account)
    }

    /// Whether `sig_data`'s signature over `payload` verifies under the key it
    /// names — the whole of what a snapshot leaf can prove.
    ///
    /// **Deliberately does not ask whether that signer was a writer.** That is an
    /// at-cut question, and a snapshot leaf has no cut: it is state, not an op, so
    /// there are no parents to resolve "was this account a writer *then*" against.
    /// Resolving against the receiver's *current* bindings instead answers a
    /// different question and answers it wrongly in both directions — a leaf
    /// written by a since-revoked device would be refused even though the sender's
    /// root hash includes it, leaving the receiver unable to match the root it just
    /// accepted and sending HashComparison to repair an entity it will refuse
    /// again.
    ///
    /// What still holds a snapshot together: the sender is a member, the delivered
    /// contents hash to the root the sender claims, and every subsequent *op* is
    /// authorized at its own cut. What this check adds on top is that no leaf
    /// carries a forged or placeholder signature. The writer half resumes the
    /// moment the entity is next written by a delta.
    fn snapshot_signature_verifies(
        sig_data: &crate::entities::SignatureData,
        payload: &[u8],
    ) -> bool {
        let Some(signer) = sig_data.signer else {
            return false;
        };
        if sig_data.signature == [0u8; 64] {
            return false;
        }
        crate::env::ed25519_verify(&sig_data.signature, signer.digest(), payload)
    }

    /// Verify a [`User`](StorageType::User) action: the signature under the key
    /// it names, and then that key's account against the stored `owner`.
    ///
    /// Two questions, deliberately separate, because `owner` stopped being a key
    /// the moment it became an account — a content hash verifies nothing. The
    /// signature is checked against [`SignatureData::signer`], and whether that
    /// device speaks for `owner` is answered by `signer_account`, which the node
    /// resolved at this action's causal cut.
    ///
    /// `signer_account` is never defaulted to the locally executing account: a
    /// remote action would otherwise authorize itself. It is the same bridge,
    /// and the same contract, that the `Shared` writer-set check runs on — see
    /// [`ApplyContext::signer_account`].
    ///
    /// **`None` defers the ownership half; it does not satisfy it.** The sync
    /// repair paths (HashComparison, snapshot, level-wise) apply through an
    /// [`ApplyContext::empty`] because they carry no cut to resolve a signer's
    /// account at, and resolving against whatever this receiver has folded would
    /// answer a different question than the author did. Refusing there would
    /// drop every legitimately repaired `User` entity instead; the `Shared` and
    /// `SharedMember` arms take exactly this deferral, for exactly this reason.
    ///
    /// What makes the deferral safe is that it is not the only gate. The node
    /// resolves a repaired leaf's author against the bindings — which live there
    /// and not here — before handing it to this crate;
    /// `calimero-node`'s `is_leaf_currently_authorized` checks that author's
    /// membership AND, for a `User` leaf, that the author's account is the
    /// entry's `owner`. Signature authenticity is still enforced here, on every
    /// path, because that needs no bindings at all.
    fn user_action_authorized(
        sig_data: &crate::entities::SignatureData,
        payload: &[u8],
        owner: &AccountId,
        signer_account: Option<&AccountId>,
    ) -> bool {
        if !Self::snapshot_signature_verifies(sig_data, payload) {
            return false;
        }
        match signer_account {
            Some(account) => account == owner,
            // Refused, matching the `Shared` and `SharedMember` arms, which bail
            // on an unnameable writer via `resolve_signer`.
            //
            // This arm used to accept, from when nothing could name a signer on
            // any path that reaches here. Both now can: a local apply states the
            // executing account (`Root::sync`), and a repair resolves the leaf's
            // signer (`calimero-node`'s `repair_signer_account`). So `None` no
            // longer means "nobody asked" — it means the binding has not folded
            // here yet, which is a retryable timing gap, not authority.
            //
            // Accepting it was the divergence the refusal exists to prevent: a
            // peer that HAS folded the binding refuses the same leaf, and the two
            // keep different state. Refusing converges them, because the leaf is
            // re-driven once the binding lands.
            None => false,
        }
    }

    /// Verify the writer's signature on a snapshot-supplied entity
    /// against the access-control rules in its metadata.
    ///
    /// Snapshot sync bypasses the
    /// [`apply_action`](Self::apply_action) verification pipeline
    /// (it writes data + metadata directly to storage from a chosen
    /// peer). To close the peer-trust gap documented in issue
    /// #2387, the snapshot receiver invokes this helper per-entity
    /// before persisting:
    ///
    /// * `Public` / `Frozen` — accept unconditionally (Public has
    ///   no signature; Frozen is content-addressed and verified
    ///   elsewhere).
    /// * `User` with `signature_data: Some(_)` — compute
    ///   `payload_for_signing` from a synthetic `Action::Add { id,
    ///   data, ancestors: vec![], metadata }` and `ed25519_verify`
    ///   against the owner.
    /// * `Shared` / `SharedMember` with `signature_data: Some(_)` — the same
    ///   payload, verified under the key the signature NAMES. The writer set is
    ///   not consulted; see
    ///   [`snapshot_signature_verifies`](Self::snapshot_signature_verifies) for
    ///   why it cannot be, and where the writer check happens instead.
    /// * `User` / `Shared` with `signature_data: None` — rejected as
    ///   `InvalidSignature`. After the bootstrap-signing fix
    ///   (`persist_signed_signatures` in
    ///   `crates/context/src/handlers/execute/mod.rs`), no locally
    ///   stored entity should carry `None` past `sign_authorized_actions`,
    ///   so a snapshot record with `None` is from a buggy or hostile
    ///   peer.
    ///
    /// Returns `Ok(())` if the entity is verified or doesn't require
    /// verification; `Err(StorageError::InvalidSignature)` otherwise. Does not
    /// write to storage.
    ///
    /// # Errors
    /// `InvalidSignature` if the `signature_data` is `None`, names no signer,
    /// carries the `[0; 64]` placeholder, or fails ed25519 verification under the
    /// key it names.
    pub fn verify_snapshot_entity_signature(
        id: crate::address::Id,
        data: &[u8],
        metadata: &crate::entities::Metadata,
    ) -> Result<(), StorageError> {
        let verdict = Self::verify_snapshot_entity_signature_inner(id, data, metadata);
        if verdict.is_err() {
            // Name the storage type and the entity, because the error variant
            // cannot. One `InvalidSignature` is returned by three different arms,
            // and on core#3376 every rejection was a `SharedMember` while the
            // error text said "user-owned data" — the investigation went to the
            // wrong arm twice. A rejection here aborts the whole HashComparison
            // session, so it is worth a line: without it the only way to learn
            // which entity failed is to download the node-log artifact and read
            // the DEBUG line that happens to precede the warning.
            //
            // `signature_shape` is the discriminator, and it is the field worth
            // having: "rejected" alone left core#3376 guessing which sub-case it
            // was, and six hypotheses were spent deciding that from code instead
            // of from the one place that already knows. `absent` /
            // `placeholder` / `signed-but-unnamed-signer` each point at a
            // different producer; `signed` means the bytes and a real signature
            // genuinely disagree, which is a different investigation entirely.
            tracing::warn!(
                %id,
                storage_type = crate::entities::storage_type_name(&metadata.storage_type),
                signature = crate::entities::signature_shape(&metadata.storage_type),
                crdt_type = ?metadata.crdt_type,
                data_len = data.len(),
                "snapshot entity signature rejected; the HashComparison session \
                 using this leaf cannot complete"
            );
        }
        verdict
    }

    /// Refuse an action's signature, naming WHICH of the apply path's checks
    /// fired.
    ///
    /// Twelve distinct rejections in `apply_action` all return the same
    /// `InvalidSignature`, whose Display names three storage types and no arm —
    /// so a failure in the field says only "one of a dozen things went wrong
    /// with one of three storage types". `verify_snapshot_entity_signature` was
    /// given a discriminator for exactly this reason, but it sits on the
    /// SNAPSHOT path; HashComparison applies through `apply_action` and never
    /// reaches it, which is why reading that field yields nothing on the
    /// failure everyone has actually been chasing.
    ///
    /// `reason` is the field to read first. It names the check, not the
    /// symptom: `signer-not-in-writer-set` and `storage-type-changed` are
    /// different investigations that were previously indistinguishable.
    fn reject_action_signature(
        reason: &'static str,
        id: &crate::address::Id,
        metadata: &crate::entities::Metadata,
    ) -> StorageError {
        tracing::warn!(
            %id,
            reason,
            storage_type = crate::entities::storage_type_name(&metadata.storage_type),
            signature = crate::entities::signature_shape(&metadata.storage_type),
            crdt_type = ?metadata.crdt_type,
            "action signature rejected while applying"
        );
        StorageError::InvalidSignature
    }

    /// The verdict itself. Split out so [`Self::verify_snapshot_entity_signature`]
    /// can log every rejection in one place — several arms bail early, so a tail
    /// log on the public function would miss them.
    fn verify_snapshot_entity_signature_inner(
        id: crate::address::Id,
        data: &[u8],
        metadata: &crate::entities::Metadata,
    ) -> Result<(), StorageError> {
        use crate::action::Action;
        use crate::entities::StorageType;

        // P3 (core#2716): the hashed rotation-log child is internal book-keeping
        // stamped `crdt_type: RotationLog`, written via `save_raw` with the
        // anchor's *default* (User) storage type and NO entity-level signature —
        // so the User arm below would reject it and a cold-joiner would never
        // receive it (the broad root-bootstrap-converge / cold-sync divergence).
        // By design these entries are UNTRUSTED IN TRANSIT and authenticated at
        // RESOLVE time: each `RotationLogEntry` carries its own signature, and
        // `writers_at`/`resolve_local` verify it against the writer set at its
        // causal cut. So the child entity itself is transit-exempt here; its
        // security comes from per-entry resolve-time verification, not a
        // signature on the aggregate child blob.
        if matches!(
            metadata.crdt_type,
            Some(crate::collections::crdt_meta::CrdtType::RotationLog)
        ) {
            return Ok(());
        }

        // Public / Frozen don't require signature verification.
        match &metadata.storage_type {
            StorageType::Public | StorageType::Frozen => return Ok(()),
            StorageType::User { .. }
            | StorageType::Shared { .. }
            | StorageType::SharedMember { .. } => {}
        }

        // Reconstruct the authorization payload the writer signed.
        // Snapshot doesn't carry ancestors and the verification is
        // tree-shape-independent (v2 design — see
        // `Action::payload_for_signing`), so an empty ancestor list
        // is correct here.
        let action = Action::Add {
            id,
            data: data.to_vec(),
            ancestors: vec![],
            metadata: metadata.clone(),
        };
        let payload = action.payload_for_signing();

        match &metadata.storage_type {
            StorageType::User {
                owner,
                signature_data: Some(sig_data),
            } => {
                // Explicit placeholder reject. `ed25519_verify` would
                // also reject `[0; 64]` cryptographically, but
                // bailing in O(1) before invoking the crypto library
                // matches `update_signature_in_place`'s contract and
                // avoids burning CPU on a known-bad value from a
                // misbehaving peer. Defense-in-depth.
                if sig_data.signature == [0u8; 64] {
                    return Err(StorageError::InvalidSignature);
                }
                // Signature only, like the two arms below. `owner` is an account
                // and an account is a content hash, so it is not a key anything
                // can verify against — the signature is checked against the
                // device key the action names, and whether that device speaks
                // for `owner` is a separate question this path cannot ask. A
                // snapshot leaf carries no cut to resolve the signer's account
                // at, and resolving it against whatever this receiver has folded
                // would ask a different question than the author answered.
                // Authorship is re-established the moment a delta writes it.
                let _ = owner;
                if Self::snapshot_signature_verifies(sig_data, &payload) {
                    Ok(())
                } else {
                    Err(StorageError::InvalidSignature)
                }
            }
            StorageType::User {
                signature_data: None,
                ..
            } => Err(StorageError::InvalidSignature),
            StorageType::Shared {
                writers,
                signature_data: Some(sig_data),
            } => {
                // Same `[0; 64]` placeholder reject as the User arm
                // above: an all-zero signature is never valid, so
                // refuse it here rather than paying an ed25519 verify
                // to learn the same thing.
                if sig_data.signature == [0u8; 64] {
                    return Err(StorageError::InvalidSignature);
                }
                // The signature must name its signer — one verify, no scan, as
                // `apply_action` does. What is NOT asked here is whether that
                // signer is a writer: see `snapshot_signature_verifies`. `writers`
                // is still carried on the leaf and still gates every later
                // delta-borne write.
                let _ = writers;
                if Self::snapshot_signature_verifies(sig_data, &payload) {
                    Ok(())
                } else {
                    Err(StorageError::InvalidSignature)
                }
            }
            StorageType::Shared {
                signature_data: None,
                ..
            } => Err(StorageError::InvalidSignature),
            StorageType::SharedMember {
                anchor,
                signature_data: Some(sig_data),
            } => {
                if sig_data.signature == [0u8; 64] {
                    return Err(StorageError::InvalidSignature);
                }
                // Signature only, as above. The anchor's writer set is not
                // consulted: resolving it "as of" this leaf's HLC was the closest
                // a snapshot could get to a cut, and it still asks the writer
                // question against whatever the receiver has folded rather than
                // against the author's own ancestry. A member's authorization is
                // re-established the moment a delta writes it.
                let _ = anchor;
                if Self::snapshot_signature_verifies(sig_data, &payload) {
                    Ok(())
                } else {
                    Err(StorageError::InvalidSignature)
                }
            }
            StorageType::SharedMember {
                signature_data: None,
                ..
            } => Err(StorageError::InvalidSignature),
            // Unreachable: handled at the top of the function.
            StorageType::Public | StorageType::Frozen => Ok(()),
        }
    }

    /// Verify a snapshot-supplied [`SharedMember`](StorageType::SharedMember)
    /// leaf against an **explicitly provided** writer set.
    ///
    /// [`verify_snapshot_entity_signature`](Self::verify_snapshot_entity_signature)
    /// resolves a member's writers via `resolve_anchor_writers`, which reads
    /// through `MainStorage` — only valid inside the WASM `RUNTIME_ENV`. The
    /// snapshot apply path runs **outside** that env, and a member's anchor may
    /// arrive in a later page anyway, so the node instead resolves the writers
    /// from the anchor's own snapshot record (itself signature-verified) and
    /// passes them here. Otherwise identical to the member arm above:
    /// placeholder reject, then one verify against the signature's named
    /// signer, which must appear in `writers`.
    ///
    /// `metadata.storage_type` must be `SharedMember`; any other variant is a
    /// caller error and is rejected as `InvalidData`.
    ///
    /// # Errors
    /// `InvalidSignature` if `signature_data` is `None`, names no signer, carries
    /// the `[0; 64]` placeholder, or fails ed25519 verification under the key it
    /// names; `InvalidData` if `metadata.storage_type` is not `SharedMember`.
    ///
    /// Does not ask whether the signer was a writer — see
    /// [`snapshot_signature_verifies`](Self::snapshot_signature_verifies).
    pub fn verify_snapshot_member_signature(
        id: crate::address::Id,
        data: &[u8],
        metadata: &crate::entities::Metadata,
    ) -> Result<(), StorageError> {
        use crate::action::Action;
        use crate::entities::StorageType;

        let StorageType::SharedMember { signature_data, .. } = &metadata.storage_type else {
            return Err(StorageError::InvalidData(
                "verify_snapshot_member_signature: storage_type is not SharedMember".to_owned(),
            ));
        };
        let Some(sig_data) = signature_data.as_ref() else {
            return Err(StorageError::InvalidSignature);
        };
        if sig_data.signature == [0u8; 64] {
            return Err(StorageError::InvalidSignature);
        }
        let action = Action::Add {
            id,
            data: data.to_vec(),
            ancestors: vec![],
            metadata: metadata.clone(),
        };
        let payload = action.payload_for_signing();
        if Self::snapshot_signature_verifies(sig_data, &payload) {
            Ok(())
        } else {
            Err(StorageError::InvalidSignature)
        }
    }

    /// Persist the signed `signature_data` produced by the runtime's
    /// `sign_authorized_actions` step back to the local index entry.
    ///
    /// The runtime signs actions in-place on the broadcast artifact,
    /// but the entity persisted by [`save_raw`](Self::save_raw)
    /// carries the placeholder signature (`[0; 64]`) emitted at WASM
    /// save time — `save_raw` runs synchronously inside the WASM host
    /// function and has no access to the identity private key. Without
    /// this re-persist step, the locally stored entity keeps the
    /// placeholder and HashComparison sync would ship that placeholder
    /// to peers, breaking signature verification on receivers and
    /// silently downgrading the entity's authorization commitment.
    ///
    /// Validates that the signed `storage_type`:
    ///
    /// * Is `Shared` or `User` — `Public`/`Frozen` carry no signature.
    /// * Carries a real signature: `signature_data` is `Some` AND its
    ///   `signature` field is not the `[0; 64]` placeholder. This
    ///   guards against a caller accidentally passing back an
    ///   unsigned action and clobbering a previously-stored real
    ///   signature. The contract is structural: the function name
    ///   says "signed", and the API now enforces it rather than
    ///   trusting every caller to filter beforehand.
    /// * Matches the stored entity's access-control triple (same
    ///   writers set for `Shared`, same owner for `User`) — this is
    ///   a signature-patch operation, not a writer-set rotation.
    ///
    /// Returns `Ok(false)` if the entity no longer exists locally
    /// (raced a delete); `Ok(true)` on successful update; or an
    /// error on any of the validation failures above.
    ///
    /// **Hash invariance**: `own_hash` is computed over the entity's
    /// data bytes (see `save_internal`'s `Sha256::digest(&data)`), not
    /// metadata, so patching `signature_data` does not invalidate the
    /// merkle tree. No ancestor recomputation needed.
    ///
    /// # Errors
    /// - `InvalidData` if the input is `Public`/`Frozen`, missing
    ///   `signature_data`, carries the `[0; 64]` placeholder, or
    ///   differs from the stored access-control triple.
    pub fn update_signature_in_place(
        id: Id,
        signed_storage_type: crate::entities::StorageType,
    ) -> Result<bool, StorageError> {
        use crate::entities::StorageType;

        // Contract guard: the input MUST be a Shared/User with a
        // non-placeholder signature. Without this check, a caller
        // could pass a `Some(SignatureData { signature: [0; 64], .. })`
        // and silently overwrite a previously-stored real signature
        // with the placeholder — a strict regression of the very
        // bug this function exists to fix.
        let incoming_sig_data = match &signed_storage_type {
            StorageType::Shared {
                signature_data: Some(sd),
                ..
            }
            | StorageType::User {
                signature_data: Some(sd),
                ..
            }
            | StorageType::SharedMember {
                signature_data: Some(sd),
                ..
            } => sd,
            StorageType::Shared {
                signature_data: None,
                ..
            }
            | StorageType::User {
                signature_data: None,
                ..
            }
            | StorageType::SharedMember {
                signature_data: None,
                ..
            } => {
                return Err(StorageError::InvalidData(
                    "update_signature_in_place: signature_data is None (input must \
                     carry a real signature; bootstrap-unsigned actions should not \
                     reach this API)"
                        .to_owned(),
                ));
            }
            StorageType::Public | StorageType::Frozen => {
                return Err(StorageError::InvalidData(
                    "update_signature_in_place: storage_type is Public/Frozen (only \
                     Shared/User carry a signature to patch)"
                        .to_owned(),
                ));
            }
        };
        if incoming_sig_data.signature == [0u8; 64] {
            return Err(StorageError::InvalidData(
                "update_signature_in_place: signature is the [0; 64] placeholder \
                 (caller must replace the save_raw placeholder with a real ed25519 \
                 signature before calling)"
                    .to_owned(),
            ));
        }

        // RMW on this entity's index entry (read → patch storage_type → save).
        // Serialize against a concurrent index mutation on the same entry so the
        // signature patch and a concurrent `add_child_to` can't clobber each
        // other (core#2571).
        let _mutation_guard = crate::index::index_mutation_guard();
        let Some(mut index) = <Index<S>>::get_index(id)? else {
            return Ok(false);
        };
        match (&index.metadata.storage_type, &signed_storage_type) {
            (
                StorageType::Shared {
                    writers: stored_writers,
                    ..
                },
                StorageType::Shared {
                    writers: new_writers,
                    ..
                },
            ) => {
                if stored_writers != new_writers {
                    return Err(StorageError::InvalidData(
                        "update_signature_in_place: writer set mismatch".to_owned(),
                    ));
                }
            }
            (
                StorageType::User {
                    owner: stored_owner,
                    ..
                },
                StorageType::User {
                    owner: new_owner, ..
                },
            ) => {
                if stored_owner != new_owner {
                    return Err(StorageError::InvalidData(
                        "update_signature_in_place: owner mismatch".to_owned(),
                    ));
                }
            }
            (
                StorageType::SharedMember {
                    anchor: stored_anchor,
                    ..
                },
                StorageType::SharedMember {
                    anchor: new_anchor, ..
                },
            ) => {
                // A member's access control is its anchor pointer; patching the
                // signature must not re-anchor it (that would silently move it
                // to a different writer domain).
                if stored_anchor != new_anchor {
                    return Err(StorageError::InvalidData(
                        "update_signature_in_place: anchor mismatch".to_owned(),
                    ));
                }
            }
            _ => {
                return Err(StorageError::InvalidData(
                    "update_signature_in_place: storage-type variant mismatch (expected \
                     Shared/User, with the same access-control triple as stored)"
                        .to_owned(),
                ));
            }
        }
        index.metadata.storage_type = signed_storage_type;
        <Index<S>>::save_index(&index)?;
        Ok(true)
    }

    /// Adds a child entity to a parent's collection.
    ///
    /// Updates Merkle hashes and generates sync actions automatically.
    ///
    /// # Errors
    /// - `SerializationError` if child can't be encoded
    /// - `IndexNotFound` if parent doesn't exist
    pub fn add_child_to<D: Data>(parent_id: Id, child: &mut D) -> Result<bool, StorageError> {
        if !child.element().is_dirty() {
            return Ok(false);
        }

        let data = to_vec(child).map_err(StorageError::SerializationError)?;

        let own_hash = Sha256::digest(&data).into();

        // ENTRY-BEFORE-PARENT: pre-write Key::Entry so the parent's
        // children list never advertises an id that has no backing
        // entry. The matching `add_child_to` in `apply_action`'s
        // delta-apply path already pre-writes the entry; this is the
        // local-write path (`CollectionMut::insert`, i.e. every
        // WASM-side `chars.insert`) and needs the same order, otherwise
        // a reader iterating the parent's children between the index
        // update and the entry write sees the id but `find_by_id`
        // returns `None`, silently dropping the child.
        //
        // Signature on the pre-written bytes: `data` here is the
        // borsh-encoded entity *before* `save_raw` re-stamps the metadata
        // (signature placeholder / nonce for User and Shared storage), so
        // the entry briefly carries a placeholder/stale signature.
        // `save_raw` → `save_internal` below overwrites the bytes with
        // the freshly-stamped version.
        //
        // Why this is safe locally: no local read path verifies entity
        // signatures. `Interface::find_by_id` (line ~1750) reads bytes
        // and the index entry without invoking
        // `verify_snapshot_entity_signature`; signature checks live
        // exclusively in `apply_action`'s remote-apply path (Action::Add
        // / Action::Update verification at lines ~611-1196), which never
        // sees these bytes because they're not shipped to peers
        // (`save_raw` emits the post-stamp Action). The invariant to
        // preserve: any future caller that wants to verify a signature
        // must do so via `apply_action`'s gate or by re-reading
        // `Key::Entry` *after* `save_raw` returns. A direct
        // signature-check on a `find_by_id` result would observe this
        // window's placeholder; don't add one.
        let _ignored = S::storage_write(Key::Entry(child.id()), &data);

        <Index<S>>::add_child_to(
            parent_id,
            ChildInfo::new(child.id(), own_hash, child.element().metadata.clone()),
        )?;

        let Some(hash) = Self::save_raw(child.id(), data, child.element().metadata.clone())? else {
            return Ok(false);
        };

        child.element_mut().is_dirty = false;
        child.element_mut().merkle_hash = hash;

        Ok(true)
    }

    /// Persists the application root document as a **leaf** entry
    /// (`ROOT_ENTRY_ID`), linked as a child of the context root
    /// (`Id::root()`).
    ///
    /// # Why a leaf and not `Id::root()`
    ///
    /// A JS app's root document has no in-WASM `Mergeable`, so it can converge
    /// only via the HashComparison **deferred-merge** path
    /// (`hash_comparison_protocol.rs`): an entry that
    /// `is_app_root_entry(id) && !is_opaque` is deferred to the guest
    /// `__calimero_merge_root_state`. That defer fires **only for leaf
    /// entries**. Written to `Id::root()` the document lived on the context
    /// root node itself — which, once the app owns any CRDT collections (each a
    /// child of `Id::root()`), is an **internal** Merkle node, never a leaf, so
    /// it was never deferred and concurrent JS writers never converged.
    ///
    /// Storing it at `ROOT_ENTRY_ID` — a pure leaf child of `Id::root()`,
    /// sibling of the collections — makes the existing leaf-defer path fire.
    /// `is_app_root_entry(ROOT_ENTRY_ID)` is already true, so the `JsRoot`
    /// stamping and the `crdt_type` re-stamp on updates apply unchanged.
    ///
    /// # `Id::root()` must have a backing entry (not just an index)
    ///
    /// In the JS model `Id::root()` parents BOTH this doc leaf and the app's
    /// CRDT collections — so once any collection exists it has an index
    /// (children) but, now that nothing writes its own value, NO
    /// `Key::Entry`. That is an *orphan index without an entry*, which the
    /// snapshot generator drops (`orphan_index_without_entry`) and sync then
    /// never converges. (The Rust SDK never hits this: its `ROOT_ID` is a real
    /// `Collection<T>` shell created with its own entry via
    /// `Collection::new(Some(*ROOT_ID))` → `Interface::save`; JS has no such
    /// shell.)
    ///
    /// So on the FIRST write (guarded on `Key::Entry(Id::root())` being absent)
    /// we give `Id::root()` a stable EMPTY, OPAQUE container blob
    /// (`crdt_type: None` — this is the container, NOT the merged doc). The blob
    /// is empty and identical on every node, so its `own_hash` is stable and
    /// converges trivially; the child hashes (this doc leaf plus the
    /// collections) fold into `Id::root()`'s `full_hash` via the normal
    /// ancestor-hash recalculation. The real, merge-dispatched document lives
    /// in the `ROOT_ENTRY_ID` leaf. Only the first call writes the container so
    /// later writes don't churn it.
    ///
    /// # Linkage
    ///
    /// Modeled on `add_child_to` above (ENTRY-BEFORE-PARENT ordering) but for a
    /// fixed id and idempotent linkage. `Index::add_child_to` dedupes by child
    /// id (replacing any existing same-id `ChildInfo` in place rather than
    /// appending a second one), so it is safe to call on **every** write: the
    /// first write links the leaf into `Id::root()`'s children list, and later
    /// writes update the leaf's advertised hash in place without duplicating
    /// the child (a duplicate would hash the child twice and diverge the root).
    /// The entry bytes are pre-written before the link so a reader that iterates
    /// `Id::root()`'s children between the index update and the entry write
    /// never sees an id with no backing entry (`save_raw` below overwrites the
    /// bytes with the freshly-stamped version).
    ///
    /// # Errors
    /// - `SerializationError` / index errors propagated from `add_child_to`
    /// - any error from `save_raw`
    pub fn save_root_entry(
        payload: Vec<u8>,
        metadata: Metadata,
    ) -> Result<Option<[u8; 32]>, StorageError> {
        let id = crate::collections::ROOT_ENTRY_ID;

        // Ensure `Id::root()` is a well-formed container (has a backing entry),
        // not an orphan index, BEFORE linking/writing the doc leaf. Guard on the
        // entry being absent so only the first call writes the empty opaque blob;
        // later writes leave it untouched. Uses the incoming timestamps so the
        // container's index metadata is consistent with the doc write.
        if S::storage_read(Key::Entry(Id::root())).is_none() {
            let _root_hash = Self::save_raw(
                Id::root(),
                Vec::new(),
                Metadata::new(metadata.created_at, *metadata.updated_at),
            )?;
        }

        let own_hash: [u8; 32] = Sha256::digest(&payload).into();
        let _ignored = S::storage_write(Key::Entry(id), &payload);
        <Index<S>>::add_child_to(Id::root(), ChildInfo::new(id, own_hash, metadata.clone()))?;

        Self::save_raw(id, payload, metadata)
    }

    /// Reads the raw bytes of the application root document from its leaf entry
    /// (`ROOT_ENTRY_ID`). Mirrors `env::read_committed_root_entry`. Returns
    /// `None` if nothing has been persisted yet.
    #[must_use]
    pub fn read_root_entry() -> Option<Vec<u8>> {
        S::storage_read(Key::Entry(crate::collections::ROOT_ENTRY_ID))
    }

    /// Verify the action's claimed ancestors against the receiver's local
    /// tree state.
    ///
    /// Replaces the cryptographic commitment to `ancestor.merkle_hash` that
    /// the v1 signed payload carried. The check is explicit + unsigned:
    /// for each ancestor in the action, look up the local entity's full
    /// merkle hash and compare. Currently **warn-only**: mismatches are
    /// logged at `debug` and the function returns `()` — see the rationale
    /// block below. Ancestors that don't exist locally are skipped
    /// (auto-vivification happens during apply); the v1 signed binding
    /// didn't provide any stronger check on this case either — the
    /// receiver had no local merkle hash to compare against.
    ///
    /// **Skip on sync-reconcile.** The HashComparison apply path constructs
    /// actions with `ancestors: vec![]`, which makes this check a no-op
    /// (correctly — sync runs precisely when tree shapes have drifted;
    /// asserting they haven't would reject every legitimate divergence
    /// repair). The delta-replay path carries the signer's ancestor list
    /// in the envelope; that's where this check actually fires.
    ///
    /// **Warn-only on mismatch.** The delta-replay path runs inside the
    /// SDK's auto-generated `__calimero_sync_next` (see
    /// `crates/sdk/macros/src/logic/method.rs::method` — line 189), which
    /// `.expect("fatal: sync failed")`s any `Err` from `Root::sync`. A
    /// hard `TreeStateMismatch` rejection there turns into a WASM
    /// "unreachable" trap that aborts the entire merge — wiping out the
    /// receiver's ability to converge any in-flight delta from a peer
    /// whose tree has legitimately drifted (which is precisely when CRDT
    /// merge is supposed to be doing its job). Until the SDK macro
    /// surfaces sync errors instead of panicking — or we thread a
    /// "this is a merge" flag through `ApplyContext` so the check can
    /// fire only on truly-sequential deltas — log the mismatch and
    /// accept. The CRDT merge logic at `save_internal` resolves the
    /// divergence regardless of ancestor-hash agreement.
    ///
    /// Single responsibility: tree-shape integrity only. Does not touch
    /// signature verification, nonce checking, or mutation. Composable.
    ///
    /// Returns `()` rather than `Result<()>` because the function never
    /// fails for the caller's purposes: mismatches are debug-logged
    /// (warn-only relax, see above), and storage-read errors during the
    /// ancestor lookup are also debug-logged and treated as "no local
    /// hash to compare" — which is the same outcome as the
    /// auto-vivification path above. Returning `Result` was the original
    /// intent (strict-reject mode) but turned out to break the SDK
    /// macro; the [`StorageError::TreeStateMismatch`] variant is kept
    /// in the error enum for the eventual strict-mode restoration but
    /// currently unconstructed.
    fn verify_ancestor_integrity(ancestors: &[ChildInfo]) {
        for ancestor in ancestors {
            // `get_hashes_for` returns `(full_hash, own_hash)`. We
            // bind the first element (`full_hash`) and compare it
            // against `ancestor.merkle_hash()` — which despite the
            // name returns the FULL subtree hash (entity + all
            // descendants), not the data-only `own_hash`. Both sides
            // are the "subtree merkle root for this entity", so the
            // comparison is correct. If a future addition needs to
            // compare data-only hashes, use `own_hash` (the second
            // element of `get_hashes_for`) and `ChildInfo::own_hash`
            // — don't conflate `merkle_hash` with "data hash".
            let lookup = match <Index<S>>::get_hashes_for(ancestor.id()) {
                Ok(opt) => opt,
                Err(e) => {
                    tracing::debug!(
                        ancestor_id = %ancestor.id(),
                        error = ?e,
                        "ancestor lookup failed; skipping integrity check for this ancestor"
                    );
                    continue;
                }
            };
            let Some((local_hash, _)) = lookup else {
                // Ancestor doesn't exist locally yet. Apply will
                // auto-vivify it from the action's claimed hash. The v1
                // signed binding had no local hash to verify against
                // here either; nothing to enforce.
                continue;
            };
            if local_hash != ancestor.merkle_hash() {
                tracing::debug!(
                    ancestor_id = %ancestor.id(),
                    "ancestor merkle hash mismatch — receiver state diverged from signer's \
                     view (accepting; CRDT merge resolves divergence). See \
                     `verify_ancestor_integrity` doc."
                );
            }
        }
    }

    /// Applies a synchronization action from a remote node.
    ///
    /// Handles Add/Update/DeleteRef actions, creating missing ancestors if needed.
    ///
    /// `ctx` carries apply-time metadata. For `Shared`-storage actions
    /// (#2266), if `ctx.effective_writers` is `Some`, the signature is
    /// validated against that pre-resolved set (the node sync layer
    /// resolves it via `writers_at(delta.parents)` per ADR 0001). When
    /// `None`, the verifier falls back to the entity's currently-stored
    /// writer set (v2 semantics). On a successful apply that changes the
    /// writer set, the rotation-log write hook appends a
    /// [`RotationLogEntry`](crate::rotation_log::RotationLogEntry) when
    /// `ctx.delta_id`/`delta_hlc` are populated.
    ///
    /// # Errors
    /// - `DeserializationError` if action data is invalid
    /// - `ActionNotAllowed` if the action violates storage-type access rules
    ///   (e.g. deleting `Frozen` data, or an unauthorized `Shared`/`User` write)
    ///
    pub fn apply_action(action: Action, ctx: &ApplyContext) -> Result<(), StorageError> {
        // Verify that the action timestamp is not too far in the future
        // to prevent LWW Time Drift attacks.
        verify_action_timestamp(&action)?;

        // P3 (core#2716): a `Shared` rotation is recorded in the anchor's hashed
        // rotation-log child, but only AFTER the anchor's own `save_internal`
        // runs in the apply pass below (the child links under an EXISTING anchor
        // — appending before the anchor exists would synthesise a placeholder).
        // The verification pass is where the entry's inputs are in scope (the
        // pre-apply writer set, the signed payload), so it builds the entry and
        // stashes it here; the apply pass drains it once the anchor is written.
        // The stale-nonce path returns inside verification (it never reaches the
        // apply pass), so it appends its own entry directly — the anchor already
        // exists there.
        let mut pending_rotation: Option<crate::rotation_log::RotationLogEntry> = None;

        // TODO: refactor to a separate function.
        // Run verification logic before applying
        match &action {
            Action::Add {
                metadata, data, id, ..
            }
            | Action::Update {
                metadata, data, id, ..
            } => {
                Self::verify_action_update(&action)?;

                match &metadata.storage_type {
                    StorageType::User {
                        owner,
                        signature_data,
                    } => {
                        debug!(
                            %id,
                            created_at = metadata.created_at,
                            updated_at = metadata.updated_at(),
                            %owner,
                            ?owner,
                            data_len = data.len(),
                            "Interface::apply_action received upsert user action"
                        );
                        let sig_data = signature_data.as_ref().ok_or(StorageError::InvalidData(
                            "Remote User action must be signed".to_owned(),
                        ))?;

                        debug!(
                            %id,
                            ?id,
                            created_at = metadata.created_at,
                            updated_at = metadata.updated_at(),
                            %owner,
                            ?owner,
                            data_len = data.len(),
                            ?sig_data.signature,
                            sig_data.nonce,
                            "Interface::apply_action received upsert user action: sig data"
                        );

                        let payload = action.payload_for_signing();

                        // Replay protection check.
                        //
                        // * `new_nonce < last_nonce` — stale action,
                        //   reject as `NonceReplay`.
                        // * `new_nonce == last_nonce` — byte-identical
                        //   re-apply. The signature commits to
                        //   `(id, data, nonce, storage_type)`, so
                        //   equal nonce + valid signature ⇒ equal
                        //   payload. We verify the signature (to
                        //   confirm the action is genuine, not just
                        //   reusing a stored nonce) and then short-
                        //   circuit with `Ok(())` — skipping
                        //   `save_internal` avoids hitting the
                        //   "equal updated_at" branch which would
                        //   call into CRDT merge and fail for
                        //   non-CRDT entities. This is critical for
                        //   the HashComparison
                        //   "recurse-into-common-children" path
                        //   that can re-deliver leaves which already
                        //   match locally (when the parent's
                        //   `full_hash` differs e.g. via
                        //   post-divergence CRDT merge).
                        // * `new_nonce > last_nonce` — normal apply.
                        let new_nonce = sig_data.nonce;
                        let last_nonce = <Index<S>>::get_metadata(*id)?
                            .map(|m| *m.updated_at)
                            .unwrap_or(0);
                        // `nonce_check_disabled_for_testing` is the explicit
                        // test escape hatch; `in_merge_mode` covers the
                        // production case where this very action is being
                        // re-evaluated as part of a CRDT merge (e.g. the
                        // host-side deferred-root-merge dispatch hands the
                        // root-state bytes back into the WASM Mergeable,
                        // which re-runs each sub-action including the
                        // upserts already-applied on the local side).
                        // Without the merge-mode bypass, the second pass
                        // hits `new_nonce == last_nonce`, skips the apply,
                        // and the merged children references / RGA edits
                        // never land — exactly the
                        // shared-storage / scaffolding-e2e regression on
                        // PR #2465. Skipping is safe in merge mode because:
                        // (1) the signature still verifies (so the bytes
                        // are authentic), and (2) merge is by definition
                        // idempotent — re-applying the same action is the
                        // expected, deterministic behaviour.
                        let skip_nonce =
                            nonce_check_disabled_for_testing() || crate::env::in_merge_mode();

                        // Verify signature FIRST, before deciding whether
                        // to skip. We need to know the action is
                        // authentic before we drop it as stale — an
                        // unauthenticated stale action should still
                        // reject as `InvalidSignature`, not silently
                        // disappear.
                        let verification_result = Self::user_action_authorized(
                            sig_data,
                            &payload,
                            owner,
                            ctx.signer_account.as_ref(),
                        );

                        if !verification_result {
                            return Err(Self::reject_action_signature(
                                "stale-action-unauthenticated",
                                id,
                                metadata,
                            ));
                        }

                        // Strictly stale: signature verified, but our
                        // local state is already AHEAD of this nonce.
                        // Drop silently — the action is authentic, just
                        // older than what we already have, the normal
                        // post-divergence sync case (HashComparison can
                        // re-deliver leaves whose newer twin already
                        // landed via gossipsub; DAG-causal catchup can
                        // hand us an older delta after a newer one).
                        // Treating this as a hard `NonceReplay` Err
                        // propagates through `Root::sync().expect("fatal:
                        // sync failed")` and aborts the whole sync batch,
                        // blocking convergence.
                        //
                        // The `==` (equal-nonce) case is deliberately NOT
                        // skipped — kept symmetric with the Shared arm so
                        // an equal-HLC write reaches `save_internal`, whose
                        // equal-timestamp branch resolves the tie
                        // deterministically by content hash
                        // (`try_merge_non_root`'s `lww_pick`). A
                        // byte-identical re-delivery is then a no-op
                        // (equal hash), while genuinely-different concurrent
                        // content converges identically on every replica.
                        // Security is unaffected: a forged
                        // different-data-same-nonce action fails the
                        // signature check above (the signature commits to
                        // the data), so only authentic writes fall through.
                        //
                        // Gated by the same `nonce_check_disabled_for_testing`
                        // bypass as the Shared arm. When the bypass is active
                        // (`skip_nonce = true`), stale actions fall through to
                        // `save_internal`, whose LWW-by-HLC guard
                        // (`last_metadata.updated_at > metadata.updated_at`
                        // ⇒ `Ok(None)`, no write) keeps state from being
                        // downgraded regardless of which path executes.
                        //
                        // Logged at WARN, not DEBUG: silent-skip on a
                        // signature-verified-but-stale action is an
                        // audit-relevant event (could be a captured-
                        // signature replay attempt, or just a benign
                        // sync redelivery). Surface enough information
                        // for downstream monitoring to distinguish the
                        // two.
                        if !skip_nonce && new_nonce < last_nonce {
                            tracing::warn!(
                                %id,
                                %owner,
                                new_nonce,
                                last_nonce,
                                "User upsert: stale nonce, signature verified \
                                 — skipping save_internal (authentic but no-op)"
                            );
                            return Ok(());
                        }
                    }
                    StorageType::Frozen => {
                        debug!(
                            %id,
                            created_at = metadata.created_at,
                            updated_at = metadata.updated_at(),
                            data_len = data.len(),
                            "Interface::apply_action received upsert frozen action"
                        );
                        verify_frozen_action_upsert(&action, data)?;
                    }
                    StorageType::Shared {
                        writers,
                        signature_data,
                    } => {
                        debug!(
                            %id,
                            created_at = metadata.created_at,
                            updated_at = metadata.updated_at(),
                            writer_count = writers.len(),
                            data_len = data.len(),
                            "Interface::apply_action received upsert shared action"
                        );
                        let sig_data = signature_data.as_ref().ok_or(StorageError::InvalidData(
                            "Remote Shared action must be signed".to_owned(),
                        ))?;

                        // Snapshot of stored state. Used both for the v2-style
                        // bootstrap fallback below and for the rotation-log write
                        // hook (post-apply, in the Add/Update branch).
                        let stored_metadata = <Index<S>>::get_metadata(*id)?;
                        let stored_writers = match stored_metadata.as_ref().map(|m| &m.storage_type)
                        {
                            Some(StorageType::Shared {
                                writers: stored_w, ..
                            }) => Some(stored_w.clone()),
                            _ => None,
                        };

                        // #2266: the node sync layer pre-resolves the
                        // ADR-0001-compliant writer set via
                        // writers_at(delta.parents) and passes it as
                        // effective_writers. Storage no longer carries
                        // DAG-ancestry knowledge.
                        //
                        // When effective_writers is None (snapshot leaf
                        // push, local apply), fall back to the entity's
                        // currently-stored writers, then to the action's
                        // claim for bootstrap. These paths are
                        // already-verified state from a peer, so
                        // stored-writers semantics are safe for them.
                        let authoritative_writers = match ctx.effective_writers.as_ref() {
                            Some(effective) => effective.clone(),
                            None => stored_writers.clone().unwrap_or_else(|| writers.clone()),
                        };

                        // Replay protection (per-entity monotonic nonce). Done BEFORE
                        // signature verification so replays are O(1)-rejected without
                        // iterating Ed25519 verifies over each writer (matches User arm).
                        //
                        // Tests that need to validate behavior under
                        // out-of-order delivery can opt out via the
                        // test-only [`disable_nonce_check_for_testing`]
                        // hook.
                        //
                        // Source asymmetry vs the signature check above:
                        // signature uses `authoritative_writers` (the
                        // pre-resolved causal writer set when callers
                        // supply it); the nonce baseline below reads from
                        // stored metadata regardless of causal context.
                        // Intentional — the two checks answer different
                        // questions:
                        // * Signature: WHO can write at this causal point
                        //   (authorization boundary).
                        // * Nonce: WHEN this write happened relative to
                        //   local state — same baseline `save_internal`
                        //   reads for its LWW-by-HLC guard, so the two
                        //   layers never disagree.
                        //
                        // `ApplyContext` deliberately does not carry an
                        // `effective_last_nonce`: computing one would
                        // require scanning the DAG for this entity's most
                        // recent prior write at the causal point. The
                        // HLC's `max(local, last_seen_remote) + 1`
                        // monotonicity rule means a post-rotation writer
                        // who has observed the rotation has also observed
                        // all writes ancestral to it, so its HLC must
                        // exceed the stored baseline — ruling out the
                        // "fresh writer at lower HLC than stored" case.
                        let new_nonce = sig_data.nonce;
                        let last_nonce =
                            stored_metadata.as_ref().map(|m| *m.updated_at).unwrap_or(0);
                        // See the User arm for the merge-mode bypass
                        // rationale — applies symmetrically here.
                        let skip_nonce =
                            nonce_check_disabled_for_testing() || crate::env::in_merge_mode();

                        // Verify signature first — see the User arm
                        // above for the full "verify-before-skip"
                        // rationale: an authentic stale action is a
                        // no-op; an unauthenticated stale action must
                        // still reject as InvalidSignature.
                        //
                        // Identify the signer: the action must name it,
                        // that name must appear in the authoritative
                        // set, and its signature must verify — one
                        // ed25519 verify, no scan over the writer set.
                        // A write that names nobody is refused.
                        //
                        // The named signer is checked against the
                        // *causal* writer set, not the stored one:
                        // `authoritative_writers` is the DAG-causal
                        // answer whenever the caller supplied one.
                        let payload = action.payload_for_signing();
                        let Some(signer) = Self::resolve_signer(
                            &authoritative_writers,
                            sig_data,
                            &payload,
                            ctx.signer_account,
                        ) else {
                            return Err(Self::reject_action_signature(
                                "shared-signer-not-in-writer-set",
                                id,
                                metadata,
                            ));
                        };
                        // Operation-granularity gate: the signer is a current
                        // writer, but must also hold the capability for THIS op.
                        Self::enforce_op_mask(
                            &signer,
                            Self::required_op_mask(&action),
                            &authoritative_writers,
                        )?;

                        // P3: build the rotation-log entry from THIS delta's
                        // metadata (identical on every node, so the child's
                        // order-invariant union converges). It is appended to the
                        // hashed child either here (stale path, anchor present) or
                        // by the apply pass after `save_internal` (non-stale).
                        let rotation_entry = Self::build_rotation_entry(
                            metadata,
                            ctx,
                            stored_writers.as_ref(),
                            Some(payload),
                        );

                        if !skip_nonce && new_nonce < last_nonce {
                            // Strictly stale: signature verified, but our
                            // local state is already AHEAD of this nonce.
                            // Drop the DATA write silently — an authentic but
                            // older write whose newer twin already landed
                            // (HashComparison re-delivery / DAG-catchup
                            // out-of-order). A hard NonceReplay here would
                            // propagate through `Root::sync().expect()` and
                            // abort the sync batch, blocking convergence.
                            //
                            // The rotation's writer set was already recorded in
                            // the log above (#2716), so dropping the data write
                            // here never loses the writer-set fact — only the
                            // (stale, no-op) value bytes.
                            //
                            // NOTE: the `==` (equal-nonce) case is
                            // deliberately NOT skipped here. Two distinct
                            // writers in a `Shared` set can stamp the same
                            // HLC nonce on DIFFERENT content (e.g. after a
                            // writer-set rotation); skipping the equal case
                            // dropped the second writer's genuinely-new write
                            // and left the cluster diverged on the same DAG
                            // heads (the shared-storage post-rotation
                            // split-brain). Equal nonce now falls through to
                            // `save_internal`, whose equal-HLC branch resolves
                            // the tie deterministically by content hash (see
                            // `try_merge_non_root`'s `lww_pick`), so a
                            // byte-identical re-delivery is a no-op while a
                            // different-content concurrent write converges.
                            //
                            // Logged at WARN — same audit rationale as the
                            // User arm.
                            tracing::warn!(
                                %id,
                                new_nonce,
                                last_nonce,
                                "Shared upsert: stale nonce, signature verified \
                                 — skipping save_internal (authentic but no-op)"
                            );
                            // We skip `save_internal` here (the value write is a
                            // stale no-op), but the rotation is still a causal
                            // FACT for the writer set: a peer's rotation whose
                            // nonce is below ours (because our own rotation bumped
                            // the anchor nonce) must still enter this node's
                            // rotation-log collection, or the originator of the
                            // higher-nonce rotation never converges (the
                            // concurrent-rotation split-brain). The `insert`
                            // below moves the rotation-log child's hash into the
                            // anchor's `full_hash`, so the context root reflects
                            // this rotation even though the value write was skipped.
                            //
                            // Write the rotation into the hashed child
                            // collection (authoritative) even on the stale-skip
                            // path — the anchor exists (stale means it was
                            // already present), and the writer-set FACT must be
                            // recorded regardless of the value-write LWW outcome.
                            // The `insert` propagates the child's hash into the
                            // anchor's `full_hash` on its own.
                            if let Some(entry) = &rotation_entry {
                                Self::append_rotation_to_child(*id, entry)?;
                            }
                            return Ok(());
                        }

                        // Non-stale rotation: the anchor write happens in the
                        // apply pass below; stash the entry for it to append once
                        // the anchor exists.
                        pending_rotation = rotation_entry;
                    }
                    StorageType::SharedMember {
                        anchor,
                        signature_data,
                    } => {
                        debug!(
                            %id,
                            created_at = metadata.created_at,
                            updated_at = metadata.updated_at(),
                            %anchor,
                            data_len = data.len(),
                            "Interface::apply_action received upsert shared-member action"
                        );
                        let sig_data = signature_data.as_ref().ok_or(StorageError::InvalidData(
                            "Remote SharedMember action must be signed".to_owned(),
                        ))?;

                        // A member carries NO writer set. The authoritative set
                        // is the anchor's, resolved by the node at the delta's
                        // causal cut (`writers_at(anchor_log, delta.parents)`)
                        // and passed in `effective_writers`. With no causal
                        // context (snapshot leaf push / local apply) fall back
                        // to the anchor's settled local state. There is NO
                        // inline-writers fallback — that is the whole point of
                        // the member design.
                        //
                        // An empty set here means the anchor has not synced to
                        // this node yet: verification fails closed and the node
                        // buffers the member delta until the anchor arrives,
                        // rather than trusting an unverifiable member. (Buffering
                        // lives in the node sync layer; storage just rejects.)
                        let authoritative_writers = match ctx.effective_writers.as_ref() {
                            Some(effective) => effective.clone(),
                            // No causal context (HashComparison-pushed leaf has no
                            // delta parents): resolve the writers AS OF this value's
                            // own HLC, not the latest set, so a value authored under
                            // an earlier rotation whose writer a later rotation
                            // removed still verifies (core#2716/#2673). `sig_data.nonce`
                            // is this write's storage HLC, the same clock the rotation
                            // entries' `writers_nonce` records.
                            None => Self::resolve_anchor_writers_as_of(*anchor, sig_data.nonce),
                        };

                        // Replay protection — identical baseline to the Shared
                        // arm (stored monotonic nonce; type-agnostic).
                        let stored_metadata = <Index<S>>::get_metadata(*id)?;
                        let new_nonce = sig_data.nonce;
                        let last_nonce =
                            stored_metadata.as_ref().map(|m| *m.updated_at).unwrap_or(0);
                        let skip_nonce =
                            nonce_check_disabled_for_testing() || crate::env::in_merge_mode();

                        // Verify signature first (same hint-fast-path / scan as
                        // Shared), against the anchor-resolved set.
                        let payload = action.payload_for_signing();
                        let Some(signer) = Self::resolve_signer(
                            &authoritative_writers,
                            sig_data,
                            &payload,
                            ctx.signer_account,
                        ) else {
                            return Err(Self::reject_action_signature(
                                "sharedmember-signer-not-in-writer-set",
                                id,
                                metadata,
                            ));
                        };
                        // Operation-granularity gate (member resolves the anchor's masks).
                        Self::enforce_op_mask(
                            &signer,
                            Self::required_op_mask(&action),
                            &authoritative_writers,
                        )?;

                        if !skip_nonce && new_nonce < last_nonce {
                            tracing::warn!(
                                %id,
                                new_nonce,
                                last_nonce,
                                "SharedMember upsert: stale nonce, signature verified \
                                 — skipping save_internal (authentic but no-op)"
                            );
                            return Ok(());
                        }

                        // NB: no rotation-log hook. A member owns no rotation
                        // log; rotations live only at its anchor.
                    }
                    StorageType::Public => {
                        // No signature verification for Public.
                        //
                        // `Action::payload_for_signing` produces a minimal
                        // payload for `Public` (type tag only) — see the doc
                        // on `hash_authorization_for_payload`. That payload
                        // is NOT load-bearing because this arm never runs an
                        // `ed25519_verify`; it just falls through to the
                        // upsert below.
                        //
                        // Storage-type-downgrade prevention:
                        // - `Update` of an entity with a different stored
                        //   storage type is rejected by
                        //   `verify_action_update` above.
                        // - `Add` of `Public` for an entity that already
                        //   exists locally as `Shared`/`User` is not
                        //   synthesized by the sync apply path
                        //   (`apply_leaf_with_crdt_merge` produces
                        //   `Action::Update` whenever the entity exists),
                        //   and a forged `Action::Add` from the gossipsub
                        //   delta path requires forging a signed
                        //   `CausalDelta` (ed25519 over the artifact).
                        //
                        // If a future refactor of `apply_action` ever adds
                        // a code path that lets a Public action reach
                        // `save_internal` for an entity stored as
                        // `Shared`/`User`, the downgrade protection breaks
                        // silently — add an explicit storage-type-match
                        // check here instead of relying on the upstream
                        // guards.
                    }
                }
            }
            Action::DeleteRef { id, metadata, .. } => {
                // Get the metadata of the item being deleted to check its domain
                let existing_metadata = <Index<S>>::get_metadata(*id)?
                    .ok_or_else(|| StorageError::IndexNotFound(*id))?;

                match existing_metadata.storage_type {
                    StorageType::Frozen => {
                        debug!(
                            %id,
                            created_at = metadata.created_at,
                            updated_at = metadata.updated_at(),
                            "Interface::apply_action received delete frozen action"
                        );
                        return Err(StorageError::ActionNotAllowed(
                            "Frozen data cannot be deleted".to_owned(),
                        ));
                    }
                    StorageType::User {
                        owner: existing_owner,
                        ..
                    } => {
                        // Verify the action's metadata, which contains the signature
                        match &metadata.storage_type {
                            StorageType::User {
                                owner,
                                signature_data,
                            } => {
                                // Check it matches the owner on record
                                if *owner != existing_owner {
                                    return Err(Self::reject_action_signature(
                                        "user-owner-mismatch",
                                        id,
                                        metadata,
                                    ));
                                }

                                let sig_data =
                                    signature_data.as_ref().ok_or(StorageError::InvalidData(
                                        "Remote User delete must be signed".to_owned(),
                                    ))?;

                                // Verify signature FIRST, then check nonce.
                                // Consistent with the upsert arms:
                                // an unauthenticated stale delete
                                // should reject as `InvalidSignature`
                                // (the more informative error)
                                // rather than `NonceReplay` (which
                                // leaks current-nonce state to
                                // unauthenticated probers).
                                //
                                // DeleteRef rejects only STRICTLY stale
                                // nonces (`<`) with a hard `Err` — unlike
                                // upsert's silent skip — because a stale
                                // delete dropped vs accepted carries
                                // different semantics than a stale upsert,
                                // and rare-by-design deletes don't drive
                                // the post-divergence convergence problem
                                // the upsert silent-skip fixes.
                                //
                                // The EQUAL-nonce case (`==`) is NOT
                                // rejected here: it falls through to
                                // `apply_delete_ref_action`, whose tiebreak
                                // (`deleted_at < updated_at` ⇒ delete loses,
                                // so equal ⇒ delete WINS) resolves the
                                // delete-vs-update tie deterministically and
                                // identically for every storage type. The
                                // previous `<=` rejected equal-HLC deletes
                                // for signed types only, while `Public` (no
                                // nonce gate) and the apply path both let
                                // equal through — so signed vs `Public`
                                // diverged on the equal-HLC delete-vs-update
                                // tie. Using `<` here unifies the tiebreak
                                // across all storage types.
                                let payload = action.payload_for_signing();
                                let verification_result = Self::user_action_authorized(
                                    sig_data,
                                    &payload,
                                    owner,
                                    ctx.signer_account.as_ref(),
                                );
                                if !verification_result {
                                    return Err(Self::reject_action_signature(
                                        "user-signature-absent",
                                        id,
                                        metadata,
                                    ));
                                }

                                // Replay protection: nonce is the
                                // `deleted_at` time, checked against
                                // the last `updated_at` stored in
                                // the index.
                                let new_nonce = sig_data.nonce;
                                let last_nonce = *existing_metadata.updated_at;
                                if new_nonce < last_nonce {
                                    return Err(StorageError::NonceReplay(Box::new((
                                        *owner.as_bytes(),
                                        new_nonce,
                                    ))));
                                }
                            }
                            _ => {
                                // Action metadata is not User, but existing is.
                                return Err(Self::reject_action_signature(
                                    "storage-type-changed-to-user",
                                    id,
                                    metadata,
                                ));
                            }
                        }
                    }
                    StorageType::Shared {
                        writers: ref existing_writers,
                        ..
                    } => {
                        // Verify the action's metadata, which contains the signature
                        match &metadata.storage_type {
                            StorageType::Shared {
                                writers: action_writers,
                                signature_data,
                                ..
                            } => {
                                // Action's claimed writers must match stored — delete is
                                // not a rotation channel.
                                if action_writers != existing_writers {
                                    return Err(Self::reject_action_signature(
                                        "shared-rotation-channel-misuse",
                                        id,
                                        metadata,
                                    ));
                                }

                                let sig_data =
                                    signature_data.as_ref().ok_or(StorageError::InvalidData(
                                        "Remote Shared delete must be signed".to_owned(),
                                    ))?;

                                // Verify signature FIRST, then check
                                // nonce — consistent with the upsert
                                // arms and the User DeleteRef arm
                                // above. An unauthenticated stale
                                // delete now rejects as
                                // `InvalidSignature` rather than
                                // `NonceReplay` (which leaks
                                // current-nonce state).
                                //
                                // DeleteRef rejects only STRICTLY stale
                                // nonces (`<`) with a hard `Err` (unlike
                                // upsert's silent skip); the equal-nonce
                                // case falls through to the apply-path
                                // tiebreak so the equal-HLC delete-vs-update
                                // resolution is identical across storage
                                // types — see the User DeleteRef arm for
                                // the full rationale.
                                //
                                // Identify the signer: the delete must name it,
                                // that name must be a writer, and its signature
                                // must verify — one verify, no scan (matches
                                // the Add/Update arm).
                                let payload = action.payload_for_signing();
                                let Some(signer) = Self::resolve_signer(
                                    existing_writers,
                                    sig_data,
                                    &payload,
                                    ctx.signer_account,
                                ) else {
                                    return Err(Self::reject_action_signature(
                                        "shared-signature-absent",
                                        id,
                                        metadata,
                                    ));
                                };
                                // Operation-granularity gate: deletes need DELETE.
                                Self::enforce_op_mask(&signer, OpMask::DELETE, existing_writers)?;

                                // Replay protection (per-entity monotonic nonce).
                                //
                                // Strict `<` Err, symmetric with the
                                // User DeleteRef arm above and matching
                                // the rationale documented there: stale
                                // delete semantics differ from upsert
                                // silent-skip, the equal-HLC case falls
                                // through to the unified apply-path
                                // tiebreak, and DeleteRef tests do not
                                // opt into the test-only bypass. Removing
                                // the previously-speculative
                                // `nonce_check_disabled_for_testing`
                                // guard here so the two delete arms
                                // behave identically.
                                let new_nonce = sig_data.nonce;
                                let last_nonce = *existing_metadata.updated_at;
                                if new_nonce < last_nonce {
                                    let placeholder = existing_writers
                                        .keys()
                                        .copied()
                                        .next()
                                        .unwrap_or_else(|| AccountId::from([0u8; 32]));
                                    return Err(StorageError::NonceReplay(Box::new((
                                        *placeholder.as_bytes(),
                                        new_nonce,
                                    ))));
                                }
                            }
                            _ => {
                                // Action metadata is not Shared, but existing is.
                                return Err(Self::reject_action_signature(
                                    "storage-type-changed-to-shared",
                                    id,
                                    metadata,
                                ));
                            }
                        }
                    }
                    StorageType::SharedMember {
                        anchor: existing_anchor,
                        ..
                    } => {
                        // Verify the action's metadata, which contains the signature
                        match &metadata.storage_type {
                            StorageType::SharedMember {
                                anchor: action_anchor,
                                signature_data,
                                ..
                            } => {
                                // The action's claimed anchor must match stored —
                                // delete is not a re-anchor channel.
                                if *action_anchor != existing_anchor {
                                    return Err(Self::reject_action_signature(
                                        "shared-delete-reanchor-misuse",
                                        id,
                                        metadata,
                                    ));
                                }

                                let sig_data =
                                    signature_data.as_ref().ok_or(StorageError::InvalidData(
                                        "Remote SharedMember delete must be signed".to_owned(),
                                    ))?;

                                // Writers: prefer the node-resolved causal set
                                // (`writers_at(anchor_log, delta.parents)`, keyed
                                // by this member id) exactly like the upsert arm,
                                // so a delete is authorized against the same set
                                // a concurrent rotation would resolve. Only fall
                                // back to the anchor's settled local state when
                                // no causal set was supplied (snapshot/local
                                // apply). An unsynced anchor → empty set → signer
                                // scan fails → InvalidSignature (fail closed).
                                let existing_writers =
                                    ctx.effective_writers.clone().unwrap_or_else(|| {
                                        // As-of THIS delete's HLC — same rationale as
                                        // the upsert arm (core#2716/#2673).
                                        Self::resolve_anchor_writers_as_of(
                                            existing_anchor,
                                            sig_data.nonce,
                                        )
                                    });

                                let payload = action.payload_for_signing();
                                let Some(signer) = Self::resolve_signer(
                                    &existing_writers,
                                    sig_data,
                                    &payload,
                                    ctx.signer_account,
                                ) else {
                                    return Err(Self::reject_action_signature(
                                        "sharedmember-signature-absent",
                                        id,
                                        metadata,
                                    ));
                                };
                                // Operation-granularity gate: deletes need DELETE.
                                Self::enforce_op_mask(&signer, OpMask::DELETE, &existing_writers)?;

                                // Replay protection (strict `<` Err, as Shared:
                                // equal-HLC falls through to the unified
                                // apply-path delete-vs-update tiebreak).
                                let new_nonce = sig_data.nonce;
                                let last_nonce = *existing_metadata.updated_at;
                                if new_nonce < last_nonce {
                                    let placeholder = existing_writers
                                        .keys()
                                        .copied()
                                        .next()
                                        .unwrap_or_else(|| AccountId::from([0u8; 32]));
                                    return Err(StorageError::NonceReplay(Box::new((
                                        *placeholder.as_bytes(),
                                        new_nonce,
                                    ))));
                                }
                            }
                            _ => {
                                // Action metadata is not SharedMember, but existing is.
                                return Err(Self::reject_action_signature(
                                    "storage-type-changed-to-sharedmember",
                                    id,
                                    metadata,
                                ));
                            }
                        }
                    }
                    StorageType::Public => { /* No special checks */ }
                }
            }
        }

        match action {
            Action::Add {
                id,
                data,
                // Note: We track both parent and collection for full metadata,
                // though parent_id alone would suffice for tree structure
                ancestors,
                metadata,
            }
            | Action::Update {
                id,
                data,
                ancestors,
                metadata,
            } => {
                trace!(
                    %id,
                    ancestor_ids = ?ancestors.iter().map(|a| a.id()).collect::<Vec<_>>(),
                    created_at = metadata.created_at,
                    updated_at = metadata.updated_at(),
                    data_len = data.len(),
                    "Interface::apply_action preparing to upsert entity"
                );
                // Tree-shape integrity check. Replaces the v1 signed
                // commitment to ancestor merkle hashes — same coverage,
                // separate concern (signature checks authorization;
                // this checks tree-state agreement). HashComparison
                // sync supplies `ancestors: vec![]`, which makes this a
                // no-op there (correct — sync runs precisely when tree
                // shapes have drifted).
                Self::verify_ancestor_integrity(&ancestors);
                let mut parent = None;
                for this in ancestors.iter().rev() {
                    let parent = parent.replace(this);

                    if <Index<S>>::has_index(this.id()) {
                        debug!(
                            ancestor = %this.id(),
                            "Ancestor already present in index - skipping creation"
                        );
                        continue;
                    }

                    let Some(parent) = parent else {
                        debug!(
                            ancestor = %this.id(),
                            "Creating ancestor as root index entry (no parent yet)"
                        );
                        <Index<S>>::add_root(this.clone())?;

                        continue;
                    };

                    // Set up parent-child relationship
                    debug!(
                        parent = %parent.id(),
                        child = %this.id(),
                        "Linking ancestor to parent in index"
                    );
                    <Index<S>>::add_child_to(parent.id(), this.clone())?;
                }

                // For new entities, create a minimal index entry first to avoid orphan errors.
                //
                // ENTRY-BEFORE-PARENT ordering (#2319 root cause): the
                // `add_child_to` call below inserts `id` into the
                // parent's `children` list. A reader that iterates the
                // parent's children (`UnorderedMap::entries()` etc.)
                // would then see `id` and try `find_by_id(id)` →
                // `storage_read(Key::Entry(id))` → `None` (entry not
                // yet written by `save_internal` below). The collection
                // iterator's `.flatten().fuse()` silently drops the
                // `NotFound` Err, producing a partial child list — the
                // "Hello Wor" rga flake. PR #2470 swapped the order
                // inside `save_internal` (entry-then-index) but missed
                // this `apply_action` pre-creation path, which
                // advertises the child in the parent BEFORE
                // `save_internal` is reached at all.
                //
                // Fix: write `Key::Entry(id)` here, before the
                // placeholder `add_child_to`, so by the time the
                // parent advertises this child, the entry already
                // exists. `save_internal` below will go through the
                // "concurrent update" path (`last_metadata.updated_at
                // == metadata.updated_at` since the placeholder we
                // create carries the same metadata) and produce the
                // same final bytes for non-root non-merging cases — a
                // redundant overwrite that's the price of closing the
                // window.
                if !<Index<S>>::has_index(id) {
                    if id.is_root() {
                        debug!(%id, "Creating root index entry for entity");
                        <Index<S>>::add_root(ChildInfo::new(id, [0; 32], metadata.clone()))?;
                    } else if let Some(parent) = parent {
                        // Pre-write the entry bytes so the parent's
                        // children list never advertises an id without
                        // a backing `Key::Entry`. See the
                        // ENTRY-BEFORE-PARENT comment above.
                        let _ignored = S::storage_write(Key::Entry(id), &data);
                        // Create minimal index entry with placeholder hash
                        let placeholder_hash = Sha256::digest(&data).into();
                        debug!(
                            %id,
                            parent = %parent.id(),
                            placeholder_hash = ?placeholder_hash,
                            "Creating placeholder child entry pending save"
                        );
                        <Index<S>>::add_child_to(
                            parent.id(),
                            ChildInfo::new(id, placeholder_hash, metadata.clone()),
                        )?;
                    } else {
                        // ORPHAN_ADD diagnostic: brand-new non-root entity
                        // with empty `ancestors`. Sync senders now carry
                        // the full ancestor chain on the wire, so this
                        // path is only hit by legacy peers that ship just
                        // an immediate parent id. `save_internal` still
                        // writes `Key::Entry(id)` but the parent's
                        // `children` list never learns about it — the read
                        // path skips the entry because it isn't
                        // advertised. Warn loudly so the next reproduction
                        // names the entity and the sending peer is
                        // identifiable as legacy.
                        tracing::warn!(
                            target: "calimero_storage::orphan_add",
                            %id,
                            created_at = metadata.created_at,
                            updated_at = metadata.updated_at(),
                            "ORPHAN_ADD: brand-new non-root entity with empty ancestors — legacy peer or pre-ancestor-chain sync path"
                        );
                    }
                }

                // Invalidate-on-sync: this apply links `id` as a child of
                // `parent` outside `Collection::insert`, so it mutates the
                // parent's enumerable child set without touching the parent's
                // node-local ordered index or its validity marker. If `parent`
                // is a `SortedSet`/`SortedMap`, a marker left stamped to a
                // `full_hash` that ran ahead of the child list would let the next
                // ordered read serve a stale subset forever (sdk-js#87). Clear
                // the parent's marker unconditionally so the next ordered read
                // rebuilds the index once from the converged child set. This is
                // done BEFORE `save_internal` so it also fires on the idempotent
                // re-delivery path below (where `save_internal` returns `None`
                // and this arm returns early). Clearing a non-sorted parent's
                // marker is a no-op, so no "is this sorted?" check is needed.
                if let Some(parent) = parent {
                    tracing::trace!(
                        target: "calimero_storage::sorted_index_dbg",
                        child = %id,
                        parent = %parent.id(),
                        "APPLY_ADD clearing parent marker"
                    );
                    let _ = S::index_meta_clear(parent.id());
                } else {
                    tracing::trace!(
                        target: "calimero_storage::sorted_index_dbg",
                        child = %id,
                        "APPLY_ADD parent=None, marker clear SKIPPED"
                    );
                }

                // Save data (might merge, producing different hash)
                let Some((_, _full_hash)) = Self::save_internal(id, &data, metadata.clone())?
                else {
                    debug!(
                        %id,
                        "Remote action produced no storage change (save_internal returned None)"
                    );
                    // The data write lost LWW (stored.updated_at >=
                    // incoming.updated_at), but a `Shared` ROTATION is still a
                    // causal FACT that must be recorded regardless of which value
                    // bytes win — otherwise a concurrent sibling rotation whose
                    // value lost LWW would never enter this node's rotation-log
                    // collection and `writers_at` could never grant the writer it
                    // added (the #2716 split-brain). Append it before returning
                    // (the anchor exists — `save_internal` returning `None` means
                    // it was already present). Same principle as the stale-nonce
                    // skip path above.
                    if let Some(entry) = pending_rotation.take() {
                        Self::append_rotation_to_child(id, &entry)?;
                    }
                    // save_internal short-circuited because stored.updated_at >
                    // incoming.updated_at: nothing changed locally, but the
                    // apply still "happened" from the network's perspective —
                    // we received and acknowledged this delta. Merkle-root
                    // convergence for concurrently-merged entities is handled by
                    // the HashComparison/level-wise sync protocols, not by this
                    // apply path, so there is nothing further to emit here.
                    return Ok(());
                };

                debug!(
                    %id,
                    ancestor_count = ancestors.len(),
                    "Applied Add/Update action to storage"
                );

                // A non-stale Shared rotation stashed its entry during the
                // verification pass; now that `save_internal` has written the
                // anchor, append it to the hashed child (the anchor exists, so
                // `add_child_to` can't synthesise a placeholder). This is a
                // DIRECT write of the canonical `build_rotation_entry` entry —
                // identical on every node for a given delta — so the
                // per-`delta_id` children converge via the normal add-wins
                // collection merge, and the `insert` propagates the child's
                // hash into the anchor's `full_hash` on its own.
                if let Some(entry) = pending_rotation.take() {
                    Self::append_rotation_to_child(id, &entry)?;
                }

                // Receiver-side signature/data COUPLING (mirror of the
                // originator's `persist_signed_signatures`). `save_internal`
                // (→ `update_hash_for`: hashes + `updated_at`) and `add_child_to`
                // (refreshes the PARENT's child list + this entity's hashes, but
                // for an already-present entity keeps its stored `metadata`) never
                // rewrite the entity's OWN stored `signature_data`. So a receiver
                // that LWW-accepts the INCOMING data kept the PREVIOUS write's
                // signature, then shipped a decoupled `{new data, old signature}`
                // leaf (HashComparison ships the entity's own index
                // `storage_type` as the wire authorization) that NO peer — nor the
                // node itself on a pushed-back leaf — could verify: the residual
                // concurrent-rotation `SharedMember` `InvalidSignature`
                // split-brain (two writers stamp the same HLC on
                // different content; the equal-HLC `lww_pick` keeps one side's
                // bytes while the signature stayed the other's).
                //
                // Re-couple them: when the stored bytes are now the INCOMING
                // write's (it won the LWW / equal-HLC tiebreak), patch the
                // entity's `signature_data` to the incoming one so
                // `{stored data, stored signature}` stays consistent. Gated on the
                // stored bytes equalling `data` so an EXISTING-data winner keeps
                // its own (already-consistent) signature. Scoped to
                // `User`/`SharedMember` — a `Shared` ANCHOR's writer set changes on
                // rotation (the in-place guard rejects that) and its consistency is
                // handled by the rotation machinery above. Best-effort: an
                // identity/placeholder mismatch returns `Err`, which means this was
                // not a same-record signed update — leave the stored signature.
                if matches!(
                    metadata.storage_type,
                    StorageType::User {
                        signature_data: Some(_),
                        ..
                    } | StorageType::SharedMember {
                        signature_data: Some(_),
                        ..
                    }
                ) && S::storage_read(Key::Entry(id)).as_deref() == Some(data.as_slice())
                {
                    let _ = Self::update_signature_in_place(id, metadata.storage_type.clone());
                }

                // Owner-driven convert (PR-6c): persist the incoming
                // `schema_version` to the stored index entry. A replicated
                // convert lands here as an ordinary signed `Action::Update`
                // whose metadata carries the new schema tag; but for an existing
                // entry neither `save_internal` (→ `update_hash_for`, hashes +
                // `updated_at` only) nor `add_child_to` (sets stored metadata
                // only on first creation) rewrites it. Stamp it explicitly so a
                // receiving replica observes the converted tag — exactly as the
                // owner's local `save_raw` does on the originating node.
                // Merkle-invisible, so it cannot diverge the root hash.
                //
                // Monotonic only: advance the stored tag, never regress it
                // (`None` == version 0). A legacy/older delta that carries no (or
                // a lower) schema tag must not downgrade an already-converted
                // entry — the no-silent-downgrade rail.
                let incoming_schema = metadata.schema_version.unwrap_or(0);
                let stored_schema = <Index<S>>::get_metadata(id)?
                    .and_then(|m| m.schema_version)
                    .unwrap_or(0);
                if incoming_schema > stored_schema {
                    <Index<S>>::set_schema_version(id, metadata.schema_version)?;
                }

                // ALWAYS update parent with correct hash after save (handles merging)
                // save_internal calls update_hash_for which updates child_index.own_hash
                if let Some(parent) = parent {
                    let (_, own_hash) =
                        <Index<S>>::get_hashes_for(id)?.ok_or(StorageError::IndexNotFound(id))?;

                    // Update parent relationship with the actual hash after any merging
                    debug!(
                        %id,
                        parent = %parent.id(),
                        own_hash = ?own_hash,
                        "Updating parent child info with final hash"
                    );
                    <Index<S>>::add_child_to(
                        parent.id(),
                        ChildInfo::new(id, own_hash, metadata.clone()),
                    )?;
                }
            }
            Action::DeleteRef { id, deleted_at, .. } => {
                Self::apply_delete_ref_action(id, deleted_at)?;
            }
        };

        Ok(())
    }

    /// Build the [`RotationLogEntry`](crate::rotation_log::RotationLogEntry) for
    /// a `Shared` apply, or `None` if this write isn't a loggable rotation.
    ///
    /// Returns `None` when: the entity isn't `Shared`; the writer set is
    /// unchanged from `pre_apply_writers` (a plain value-write — `None` prior
    /// means bootstrap, which always logs); or the apply carries no causal
    /// identity (`ctx.delta_id`/`delta_hlc` absent — snapshot leaf push / local
    /// apply / non-causal `StorageDelta::Actions`).
    ///
    /// This is the single source of the rotation entry, shared by the receive
    /// path and (via [`Self::append_rotation_to_child`]) every originator leg,
    /// so the entry every node records for a given rotation delta is identical —
    /// the precondition for the child's order-invariant union to converge.
    pub fn build_rotation_entry(
        metadata: &Metadata,
        ctx: &ApplyContext,
        pre_apply_writers: Option<&BTreeMap<AccountId, OpMask>>,
        signed_payload: Option<[u8; 32]>,
    ) -> Option<crate::rotation_log::RotationLogEntry> {
        let StorageType::Shared {
            writers,
            signature_data,
        } = &metadata.storage_type
        else {
            return None;
        };
        let is_rotation = pre_apply_writers != Some(writers);
        if !is_rotation {
            return None;
        }
        let (delta_id, delta_hlc) = (ctx.delta_id?, ctx.delta_hlc?);
        let signer = signature_data.as_ref().and_then(|s| s.signer);
        let nonce = signature_data.as_ref().map(|s| s.nonce).unwrap_or(0);
        let signature = signature_data.as_ref().map(|s| s.signature);
        Some(crate::rotation_log::RotationLogEntry {
            delta_id,
            delta_hlc,
            signer,
            signature,
            signed_payload: signature.and(signed_payload),
            new_writers: writers.clone(),
            writers_nonce: nonce,
        })
    }

    /// Append `entry` to the anchor's rotation-log [`UnorderedMap`] (P3), the
    /// synced source of truth for its writer-set history. Inserts under the
    /// `delta_id` key, so it is idempotent on replay (same key + byte-identical
    /// value) and convergent on a same-`delta_id`/different-bytes collision (the
    /// per-entry child LWW-merges to a node-independent winner — no hard
    /// `DuplicateRotationInDelta` error, which the old blob would have raised;
    /// the collection just converges).
    ///
    /// The anchor MUST already exist (callers append only after the anchor's own
    /// `save_internal`, or on the stale-skip path where it was present), so
    /// `ensure_rotation_log_parent`'s `add_child_to` can't synthesise a
    /// placeholder.
    ///
    /// # Errors
    /// Propagates child read/write failures.
    pub fn append_rotation_to_child(
        anchor: Id,
        entry: &crate::rotation_log::RotationLogEntry,
    ) -> Result<(), StorageError> {
        // Only SIGNED rotations belong in the hashed collection. An unsigned
        // entry (`signer == None` — the bootstrap/genesis writer set, logged by
        // the originator's self-log from an unsigned bootstrap action) carries no
        // authoritative writer-set fact: `writers_at_authenticated` and
        // `resolve_local_as_of` both IGNORE unsigned entries (they can't be
        // verified), and the genesis writer set is already available via the
        // anchor's stored `metadata.storage_type.writers` and, when it actually
        // rotated, via the first SIGNED entry. So an unsigned entry has ZERO
        // effect on resolution — but if it lands in the collection it diverges
        // the collection's Merkle hash across nodes: the ORIGINATOR self-logs it
        // while peers (which receive the anchor via sync, not via that unsigned
        // bootstrap action) never do, so the rotation-log map child hash splits
        // and the anchor's `full_hash` never converges (CI run 27196723799:
        // node-1 had a 4th unsigned entry `31498e5e writers=[8ae4fd15] signer=none`
        // that node-3 lacked, with the 3 SIGNED entries byte-identical on both).
        // Keep the collection to SIGNED rotations only so every node logs the
        // same set.
        if entry.signer.is_none() {
            return Ok(());
        }
        // Ensure the map parent exists + is linked under the anchor before
        // opening the handle (so `insert`'s `add_child_to(map_id, ..)` links the
        // entry into a parent that is itself in the anchor's subtree).
        let _map_id = Self::ensure_rotation_log_parent(anchor)?;
        let mut map = Self::rotation_log_map(anchor);
        map.insert(entry.delta_id, entry.clone())
            .map(|_prev| ())
            .map_err(|e| match e {
                crate::collections::error::StoreError::StorageError(se) => se,
                other => StorageError::InvalidData(other.to_string()),
            })
    }

    /// 2. Exists locally - compare timestamps (LWW)
    /// 3. Never seen - ignore (could create tombstone in future)
    ///
    /// IMPORTANT: When deletion wins, we must also update the parent's children
    /// list and recalculate ancestor hashes. This ensures convergence with nodes
    /// that performed the deletion locally.
    fn apply_delete_ref_action(id: Id, deleted_at: u64) -> Result<(), StorageError> {
        // Guard: Already deleted, check if this deletion is newer
        if <Index<S>>::is_deleted(id)? {
            // Already has tombstone, use later deletion timestamp
            let _ignored = <Index<S>>::mark_deleted(id, deleted_at);
            return Ok(());
        }

        // Guard: Entity doesn't exist, nothing to delete
        let Some(metadata) = <Index<S>>::get_metadata(id)? else {
            // Entity doesn't exist - no tombstone needed
            // CRDT rationale: Deleting non-existent entity is idempotent no-op.
            return Ok(());
        };

        // Guard: Local update is newer, deletion loses.
        //
        // This is the SINGLE canonical equal-HLC delete-vs-update tiebreak
        // for every storage type. The strict `<` means a STRICTLY older
        // delete loses, while an equal-HLC delete (`deleted_at ==
        // updated_at`) WINS. The signed-type verify arms (User/Shared/
        // SharedMember) reject only strictly-stale nonces (`<`) and let
        // equal-HLC deletes fall through to here, and `Public` has no nonce
        // gate at all — so all four types resolve the equal-HLC tie
        // identically rather than signed types rejecting it earlier.
        if deleted_at < *metadata.updated_at {
            // Local update wins, ignore older deletion
            return Ok(());
        }

        // Get parent ID BEFORE deleting - we need it to update the Merkle tree
        let parent_id = <Index<S>>::get_parent_id(id)?;

        // Tombstone the subtree BEFORE removing this entity, while its
        // `children` are still readable. The originating node tombstoned the
        // whole subtree under `id`; replay the same recursion here, with the
        // same `deleted_at`, so this replica reclaims the descendant rows too
        // and converges (otherwise they would leak as un-tombstoned orphans
        // under a tombstoned ancestor).
        <Index<S>>::tombstone_descendants_of(id, deleted_at)?;

        // Deletion wins - apply it, through the SAME helper the local delete
        // uses. Inlining the remove + tombstone here is how the two paths
        // drifted: the helper also drops the entity's child trie, and this one
        // did not, so a locally-deleted entity lost its trie while a replayed
        // delete kept it. Collection ids are deterministic, so the next
        // re-creation (or an add-wins resurrection, which recomputes
        // `full_hash_from_trie`) folded EMPTY on one replica and a ghost root
        // on the other — a different hash for the same logical state,
        // propagating to the context root with nothing reporting an error.
        // Exactly what the comment below warns about, one operation earlier.
        <Index<S>>::delete_entity_and_create_tombstone(id, deleted_at)?;

        // CRITICAL: Update parent's children list and recalculate hashes
        // Without this, the receiving node would have a different root hash than
        // the node that performed the deletion locally.
        if let Some(parent_id) = parent_id {
            // Remove child from parent's children list and recalculate hashes
            <Index<S>>::update_parent_after_child_removal(parent_id, id)?;
            <Index<S>>::recalculate_ancestor_hashes_for(parent_id)?;
            // Invalidate-on-sync (mirror of the add path in `apply_action`): a
            // synced delete unlinks a child from `parent_id` outside
            // `Collection::insert`, changing the enumerable child set without
            // touching the parent's node-local ordered index / marker. Clear the
            // marker so a `SortedSet`/`SortedMap` rebuilds its ordered index on
            // the next read rather than serving the removed element. No-op for a
            // non-sorted parent.
            tracing::trace!(
                target: "calimero_storage::sorted_index_dbg",
                child = %id,
                parent = %parent_id,
                "APPLY_DELETE clearing parent marker"
            );
            let _ = S::index_meta_clear(parent_id);
        }

        Ok(())
    }

    /// Retrieves all children in a collection.
    ///
    /// Returns deserialized child entities. Order is not guaranteed.
    ///
    /// # Errors
    /// - `IndexNotFound` if parent doesn't exist
    /// - `DeserializationError` if child data is corrupt
    ///
    pub fn children_of<D: Data>(parent_id: Id) -> Result<Vec<D>, StorageError> {
        let children_info = <Index<S>>::get_children_of(parent_id)?;
        let mut children = Vec::new();
        for child_info in children_info {
            if let Some(child) = Self::find_by_id(child_info.id())? {
                children.push(child);
            }
        }
        Ok(children)
    }

    /// Retrieves child metadata without deserializing full data.
    ///
    /// Returns IDs, hashes, and timestamps only. More efficient than [`children_of()`](Self::children_of()).
    ///
    /// # Errors
    /// Returns error if index lookup fails.
    ///
    pub fn child_info_for(parent_id: Id) -> Result<Vec<ChildInfo>, StorageError> {
        <Index<S>>::get_children_of(parent_id)
    }

    /// Finds and deserializes an entity by its unique ID.
    ///
    /// Filters out tombstoned (deleted) entities automatically.
    ///
    /// # Errors
    /// - `DeserializationError` if stored data is corrupt
    /// - `IndexNotFound` if entity exists but has no index
    ///
    pub fn find_by_id<D: Data>(id: Id) -> Result<Option<D>, StorageError> {
        // Single `EntityIndex` read serves the tombstone check AND supplies the
        // merkle_hash and metadata below. Loading it once here avoids the
        // earlier `is_deleted()` + `get_index()` pair, which read and
        // deserialized the index twice for every child of every collection scan.
        let index = <Index<S>>::get_index(id)?;

        // Check if entity is deleted (tombstone)
        if index.as_ref().and_then(|index| index.deleted_at).is_some() {
            return Ok(None); // Entity is deleted
        }

        let value = S::storage_read(Key::Entry(id));

        let Some(slice) = value else {
            return Ok(None);
        };

        let mut item = from_slice::<D>(&slice).map_err(StorageError::DeserializationError)?;

        let index = index.ok_or(StorageError::IndexNotFound(id))?;
        item.element_mut().merkle_hash = index.full_hash();
        item.element_mut().metadata = index.metadata;

        Ok(Some(item))
    }

    /// Finds an entity by ID, returning raw bytes without deserialization.
    ///
    /// Note: This does NOT filter deleted entities. Use `find_by_id` for automatic
    /// tombstone filtering.
    ///
    pub fn find_by_id_raw(id: Id) -> Option<Vec<u8>> {
        S::storage_read(Key::Entry(id))
    }

    /// Gets raw entity data by ID.
    ///
    /// This is a simple alias for `find_by_id_raw` for convenience in tests.
    ///
    /// # Errors
    /// Returns `IndexNotFound` if entity doesn't exist.
    ///
    pub fn get(id: Id) -> Result<Vec<u8>, StorageError> {
        Self::find_by_id_raw(id).ok_or(StorageError::IndexNotFound(id))
    }

    /// Checks if a collection has any children.
    ///
    /// # Errors
    /// Returns error if index lookup fails.
    ///
    pub fn has_children(parent_id: Id) -> Result<bool, StorageError> {
        <Index<S>>::has_children(parent_id)
    }

    /// Retrieves the parent entity of a child.
    ///
    /// # Errors
    /// Returns error if index lookup or deserialization fails.
    ///
    pub fn parent_of<D: Data>(child_id: Id) -> Result<Option<D>, StorageError> {
        <Index<S>>::get_parent_id(child_id)?
            .map_or_else(|| Ok(None), |parent_id| Self::find_by_id(parent_id))
    }

    /// Removes a child from a collection.
    ///
    /// Deletes the child entity and generates sync actions automatically.
    ///
    /// Rejects deletion of `Frozen` children — frozen data is immutable and
    /// every peer rejects an incoming `DeleteRef` for it (see the
    /// `StorageType::Frozen` arm in [`apply_action`](Self::apply_action)), so
    /// a local delete would diverge the deleter from the rest of the network.
    /// The re-key migration that relocates entries under new deterministic ids
    /// must use [`relocate_child_from`](Self::relocate_child_from) instead.
    ///
    /// # Errors
    /// Returns error if parent or child doesn't exist, or if the child is
    /// `Frozen`.
    ///
    pub fn remove_child_from(parent_id: Id, child_id: Id) -> Result<bool, StorageError> {
        Self::remove_child_from_inner(parent_id, child_id, RemoveMode::Delete)
    }

    /// Removes a child from a collection as part of a deterministic re-key
    /// relocation (the entry is immediately re-inserted under a new id).
    ///
    /// Unlike [`remove_child_from`](Self::remove_child_from) this does **not**
    /// reject `Frozen` children: a re-key is a local relocation, not a
    /// semantic deletion, so the frozen data is preserved (re-inserted under
    /// its new deterministic id by the caller). Used only by the collection
    /// re-key paths (`reassign_deterministic_id_*`).
    ///
    /// # Errors
    /// Returns error if parent or child doesn't exist.
    ///
    pub(crate) fn relocate_child_from(parent_id: Id, child_id: Id) -> Result<bool, StorageError> {
        Self::remove_child_from_inner(parent_id, child_id, RemoveMode::Relocate)
    }

    /// Shared implementation behind [`remove_child_from`](Self::remove_child_from)
    /// and [`relocate_child_from`](Self::relocate_child_from).
    ///
    /// `mode` selects whether the Frozen-deletion guard applies: it does for
    /// [`RemoveMode::Delete`], but not for [`RemoveMode::Relocate`] re-keys.
    fn remove_child_from_inner(
        parent_id: Id,
        child_id: Id,
        mode: RemoveMode,
    ) -> Result<bool, StorageError> {
        let child_exists = <Index<S>>::get_children_of(parent_id)?
            .iter()
            .any(|child| child.id() == child_id);
        if !child_exists {
            return Ok(false);
        }

        // This will act as our nonce
        let deleted_at = time_now();

        // Get metadata before removing index
        let mut metadata =
            <Index<S>>::get_metadata(child_id)?.ok_or(StorageError::IndexNotFound(child_id))?;

        // Reject deletion of Frozen data locally, before mutating any state.
        //
        // The receiving side rejects every `DeleteRef` for Frozen data (see
        // the `StorageType::Frozen` arm in `apply_action`). If we let the
        // local delete proceed it would tombstone the child and broadcast a
        // `DeleteRef` that every peer rejects, leaving the deleter
        // permanently diverged from the rest of the network (split-brain).
        // Refusing here keeps the deleter consistent with its peers.
        //
        // The re-key relocation path (`relocate_child_from`) passes
        // `RemoveMode::Relocate`: it removes the entry only to re-insert it
        // under a new deterministic id, so the frozen data is preserved.
        if mode == RemoveMode::Delete && matches!(metadata.storage_type, StorageType::Frozen) {
            return Err(StorageError::ActionNotAllowed(
                "Frozen data cannot be deleted".to_owned(),
            ));
        }

        // A genuine subtree delete must not strand Frozen data buried deeper in
        // the tree either. Scan descendants BEFORE mutating any state; if any
        // Frozen entity exists, reject so the operator relocates it out of the
        // subtree first. Same split-brain avoidance as the direct-child guard
        // above: we never tombstone the subtree or broadcast a `DeleteRef` that
        // would leave the frozen data detached on every peer.
        if mode == RemoveMode::Delete {
            if let Some(frozen_id) = <Index<S>>::find_frozen_descendant(child_id)? {
                return Err(StorageError::ActionNotAllowed(format!(
                    "cannot delete subtree {child_id}: it contains Frozen data at {frozen_id}; \
                     relocate the frozen entity out of the subtree before deleting"
                )));
            }
        }

        // If this is a local user action, set the nonce
        if let StorageType::User { owner, .. } = metadata.storage_type {
            if owner == AccountId::from(crate::env::account_id()) {
                // Use the deletion timestamp as the nonce
                metadata.storage_type = StorageType::User {
                    owner,
                    signature_data: Some(SignatureData {
                        signature: [0; 64], // Placeholder, added by signer
                        nonce: deleted_at,
                        // The DEVICE writing on the owner's behalf. Required
                        // now that `owner` is an account: an account is a
                        // content hash, so it is not what the signature
                        // verifies against.
                        signer: Some(crate::env::device_id().into()),
                    }),
                };
            }
        }

        // If this is a local shared action by a writer, set the nonce. Same
        // authority rule as save_raw, via the shared helper. Here `metadata`
        // was just loaded from the index above, so its writers already are the
        // stored set — pass them as `stored` so the helper skips a redundant
        // index read, and the stored ∪ claimed union collapses to the stored
        // membership check this delete requires.
        let shared_to_stamp = if let StorageType::Shared {
            writers: claimed, ..
        } = &metadata.storage_type
        {
            Self::authorize_local_shared_stamp(child_id, claimed, Some(claimed))?
        } else {
            None
        };
        if let Some((writers, signer)) = shared_to_stamp {
            metadata.storage_type = StorageType::Shared {
                writers,
                signature_data: Some(SignatureData {
                    signature: [0; 64], // Placeholder, added by signer
                    nonce: deleted_at,
                    signer: Some(signer), // O(1) verifier lookup
                }),
            };
        }

        // Same for a member delete: authorize against the ANCHOR's writers
        // (the member carries none), re-stamp the anchor pointer with a fresh
        // signature placeholder for the signer to fill in.
        let member_to_stamp =
            if let StorageType::SharedMember { anchor, .. } = &metadata.storage_type {
                // Authorize by ACCOUNT, but stamp the KEY. `SignatureData.signer` names
                // whatever will actually verify this write, and only a device holds a
                // signing key — so the gate and the stamp read different identities on
                // purpose. Collapsing them either way breaks something: an account in
                // `signer` verifies against nothing, and a device in the writer-set check
                // is the per-device gate this change exists to remove.
                let executor: AccountId = crate::env::account_id().into();
                let device: PublicKey = crate::env::device_id().into();
                let writers = Self::resolve_anchor_writers(*anchor);
                if writers.contains_key(&executor) {
                    Some((*anchor, device))
                } else {
                    None
                }
            } else {
                None
            };
        if let Some((anchor, signer)) = member_to_stamp {
            metadata.storage_type = StorageType::SharedMember {
                anchor,
                signature_data: Some(SignatureData {
                    signature: [0; 64], // Placeholder, added by signer
                    nonce: deleted_at,
                    signer: Some(signer), // O(1) verifier lookup
                }),
            };
        }

        <Index<S>>::remove_child_from(parent_id, child_id, deleted_at)?;

        // Use DeleteRef for efficient tombstone-based deletion.
        // More efficient than Delete: only sends ID + timestamp + metadata vs full ancestor tree.
        // The tombstone is created by remove_child_from, we just broadcast the deletion.
        //
        // Gated by `S::participates_in_sync()`: a `PrivateStorage` delete
        // stays local and must NOT enter the synced delta stream — same
        // reasoning as the `Compare` push at the end of `apply_action`
        // above. The remove_child_from call right above mutates only
        // `S`'s index, so the broadcast was the only sync surface for
        // this path.
        if S::participates_in_sync() {
            crate::delta::push_action(Action::DeleteRef {
                id: child_id,
                deleted_at,
                // Pass the full metadata
                metadata,
            });
        }

        Ok(true)
    }

    /// Retrieves the root entity.
    ///
    /// # Errors
    /// Returns error if deserialization fails.
    ///
    pub fn root<D: Data>() -> Result<Option<D>, StorageError> {
        Self::find_by_id(Id::root())
    }

    /// Saves the root entity and commits sync actions.
    ///
    /// Should be called at the end of each transaction. Call once per execution.
    ///
    /// # Errors
    /// - `UnexpectedId` if root ID doesn't match
    /// - `SerializationError` if encoding fails
    ///
    pub fn commit_root<D: Data>(root: Option<D>) -> Result<(), StorageError> {
        let id: Id = Id::root();

        debug!(%id, has_root = root.is_some(), "commit_root invoked");
        let hash = if let Some(root) = root {
            if root.id() != id {
                return Err(StorageError::UnexpectedId(root.id()));
            }

            if !root.element().is_dirty() {
                return Ok(());
            }

            let data = to_vec(&root).map_err(StorageError::SerializationError)?;

            Self::save_raw(id, data, root.element().metadata.clone())?
        } else {
            <Index<S>>::get_hashes_for(id)?.map(|(full_hash, _)| full_hash)
        };

        if let Some(hash) = hash {
            crate::delta::commit_root(&hash)?;
        }

        debug!(%id, ?hash, "commit_root completed");
        Ok(())
    }

    /// Saves an entity to storage, updating if it exists.
    ///
    /// Only saves if entity is dirty. Returns `false` if not saved due to:
    /// - Entity not dirty
    /// - Existing record is newer (last-write-wins guard)
    ///
    /// Automatically:
    /// - Calculates Merkle hashes
    /// - Updates timestamps
    /// - Generates sync actions
    /// - Propagates hash changes up ancestor chain
    ///
    /// **Note**: Use [`add_child_to()`](Self::add_child_to()) for new children,
    /// then `save()` for subsequent updates.
    ///
    /// # Errors
    /// - `SerializationError` if encoding fails
    /// - `CannotCreateOrphan` if entity has no parent and isn't root
    ///
    pub fn save<D: Data>(entity: &mut D) -> Result<bool, StorageError> {
        if !entity.element().is_dirty() {
            return Ok(false);
        }

        let data = to_vec(entity).map_err(StorageError::SerializationError)?;

        let Some(hash) = Self::save_raw(entity.id(), data, entity.element().metadata.clone())?
        else {
            return Ok(false);
        };

        entity.element_mut().is_dirty = false;
        entity.element_mut().merkle_hash = hash;

        Ok(true)
    }

    /// Saves raw data to the storage system.
    ///
    /// # Errors
    ///
    /// If an error occurs when serialising data or interacting with the storage
    /// system, an error will be returned.
    ///
    fn save_internal(
        id: Id,
        data: &[u8],
        metadata: Metadata,
    ) -> Result<Option<(bool, [u8; 32])>, StorageError> {
        // Serialize the WHOLE read-merge-write-rehash sequence, not just the
        // index update. The entry-value write (`storage_write(Key::Entry(id))`)
        // and the `own_hash` update (`Index::update_hash_for`) are two separate
        // store writes; `own_hash = Sha256(final_data)` is computed from THIS
        // call's merged bytes. Without a guard spanning both, a concurrent
        // writer for the same id (the execute path vs. the dedicated sync
        // apply, which run on different threads sharing one store) can land its
        // value write and its own_hash update in opposite orders, leaving the
        // stored bytes and the recorded `own_hash` from DIFFERENT writers. A
        // peer recomputing the leaf hash from the bytes then never matches this
        // node's advertised `own_hash`, so the parent collection's `full_hash`
        // can't converge and HashComparison re-merges it forever (the
        // stable-but-different root-hash split-brain). The guard is reentrant,
        // so the nested `update_hash_for` / `add_child_to` re-acquire it on this
        // thread without deadlock; on wasm it compiles out (single-threaded).
        //
        // TODO(perf): this widens the global mutation guard to span the CRDT
        // merge (not just the microsecond index update it was scoped to), so all
        // entity writes now serialize through one process-global lock for the
        // duration of a merge. Revisit whether this regresses write throughput
        // on hot collections; if so, move to a per-entity (id-keyed) lock so
        // independent entities can merge in parallel.
        let _mutation_guard = crate::index::index_mutation_guard();

        let incoming_updated_at = metadata.updated_at();

        // `incoming_hash` (Sha256 of `data`) is only consumed by the
        // root-merge trace logs below, so it's computed lazily inside those
        // branches rather than on every (hot, non-root) write.

        let last_metadata = <Index<S>>::get_metadata(id)?;
        let final_data = if let Some(last_metadata) = &last_metadata {
            if matches!(
                metadata.crdt_type,
                Some(crate::collections::crdt_meta::CrdtType::RotationLog)
            ) {
                // P3 (core#2716) per-`delta_id` rotation-log child. Merge
                // REGARDLESS of timestamp ordering (the LWW-by-HLC branches below
                // would stale-skip a concurrent same-id write). `try_merge_non_root`
                // resolves a same-`delta_id` collision via `lww_pick`'s
                // content-hash tiebreak — symmetric, so HashComparison's
                // bidirectional leaf reconciliation settles.
                //
                // First real write: `write_rotation_entry_child` links the child
                // (`add_child_to`) BEFORE writing its value, so `last_metadata`
                // is already `Some` while the stored value is still ABSENT. Treat
                // absent existing bytes as "take incoming" — `lww_pick` would
                // otherwise compare against an empty buffer and could pick it on
                // the hash tiebreak, storing an empty child (load returns nothing).
                match S::storage_read(Key::Entry(id)) {
                    None => data.to_vec(),
                    Some(existing_data) => Self::try_merge_non_root(
                        id,
                        &existing_data,
                        data,
                        &metadata,
                        *last_metadata.updated_at,
                        *metadata.updated_at,
                    )?,
                }
            } else if last_metadata.updated_at > metadata.updated_at {
                return Ok(None);
            } else if crate::collections::is_app_root_entry(id) {
                // App root state — either the canonical `ROOT_ID` or the
                // `Root<T>` entry (`ROOT_ENTRY_ID`). Both contain the
                // app's serialised root and MUST go through CRDT merge,
                // not the non-root LWW-by-HLC path. The `Mergeable` impl
                // (auto-generated by `#[app::state]`) handles each field
                // with its own CRDT semantics (Counter sums, UnorderedMap
                // per-key LWW, UnorderedSet union, etc.); the
                // bootstrap-aware default in `merge_root_state` covers
                // apps without a registered merger.
                //
                // Pre-2026-05-21 only `id.is_root()` was checked here,
                // so the `Root<T>` entry fell into the non-root LWW path
                // and silently dropped one side's writes on bootstrap
                // and on concurrent root writes. See the doc comment on
                // `is_app_root_entry` for the regression timeline.
                let incoming_hash: [u8; 32] = Sha256::digest(data).into();
                if let Some(existing_data) = S::storage_read(Key::Entry(id)) {
                    let existing_hash: [u8; 32] = Sha256::digest(&existing_data).into();
                    info!(
                        target: "storage::root_merge",
                        %id,
                        existing_len = existing_data.len(),
                        existing_hash = %hex::encode(existing_hash),
                        incoming_len = data.len(),
                        incoming_hash = %hex::encode(incoming_hash),
                        existing_created_at = last_metadata.created_at,
                        existing_updated_at = *last_metadata.updated_at,
                        incoming_updated_at,
                        "ROOT MERGE: Starting CRDT merge for root entity"
                    );
                    // An opaque root (no `crdt_type`) has no app-defined
                    // `Mergeable` to dispatch to. When no merger is registered,
                    // `try_merge_data` resolves it by LWW instead of erroring;
                    // a non-opaque root (real `crdt_type`) still errors loudly
                    // (I5). Read the opaqueness off the STORED entity — its
                    // metadata is the authoritative record of what kind of root
                    // this is.
                    //
                    // A JS-SDK root (the `JsRoot` marker, stamped when the guest
                    // called `register_js_sdk_root_merge`) resolves LOCAL writes
                    // by LWW too: a local write's incoming state always descends
                    // from the existing state, so incoming-wins is correct here.
                    // Its field-aware convergence for CONCURRENT writers runs only
                    // on the sync path, where the marker routes the root to the
                    // guest `__calimero_merge_root_state` callback. A real
                    // `#[app::state]` root still merges through the registry
                    // (checked first) and never reaches the LWW arm.
                    let is_opaque_root = is_opaque_root_crdt_type(&last_metadata.crdt_type)
                        || last_metadata
                            .crdt_type
                            .as_ref()
                            .is_some_and(|t| t.is_js_root());
                    let merged = Self::try_merge_data(
                        id,
                        &existing_data,
                        data,
                        last_metadata.created_at,
                        *last_metadata.updated_at,
                        *metadata.updated_at,
                        is_opaque_root,
                    )?;
                    let merged_hash: [u8; 32] = Sha256::digest(&merged).into();
                    info!(
                        target: "storage::root_merge",
                        %id,
                        merged_len = merged.len(),
                        merged_hash = %hex::encode(merged_hash),
                        same_as_existing = (merged_hash == existing_hash),
                        same_as_incoming = (merged_hash == incoming_hash),
                        "ROOT MERGE: Completed CRDT merge"
                    );
                    merged
                } else {
                    info!(
                        target: "storage::root_merge",
                        %id,
                        incoming_len = data.len(),
                        incoming_hash = %hex::encode(incoming_hash),
                        "ROOT MERGE: No existing data, using incoming directly"
                    );
                    data.to_vec()
                }
            } else if last_metadata.updated_at == metadata.updated_at {
                // Concurrent update (same timestamp) - try to merge
                if let Some(existing_data) = S::storage_read(Key::Entry(id)) {
                    Self::try_merge_non_root(
                        id,
                        &existing_data,
                        data,
                        &metadata,
                        *last_metadata.updated_at,
                        *metadata.updated_at,
                    )?
                } else {
                    data.to_vec()
                }
            } else {
                // Incoming is newer - try CRDT merge for non-root entities if possible
                // (Invariant I5: no silent data loss)
                if let Some(existing_data) = S::storage_read(Key::Entry(id)) {
                    Self::try_merge_non_root(
                        id,
                        &existing_data,
                        data,
                        &metadata,
                        *last_metadata.updated_at,
                        *metadata.updated_at,
                    )?
                } else {
                    data.to_vec()
                }
            }
        } else {
            if id.is_root() {
                let incoming_hash: [u8; 32] = Sha256::digest(data).into();
                info!(
                    target: "storage::root_merge",
                    %id,
                    incoming_len = data.len(),
                    incoming_hash = %hex::encode(incoming_hash),
                    "ROOT MERGE: First time creating root entity"
                );
                <Index<S>>::add_root(ChildInfo::new(id, [0_u8; 32], metadata.clone()))?;
            }
            data.to_vec()
        };

        let own_hash: [u8; 32] = Sha256::digest(&final_data).into();

        // `own_hash` is `Sha256(data)` for every storage type, including
        // `Shared` anchors. The Phase-2 ACL fold (mixing the resolved writer set
        // into a `Shared` anchor's `own_hash`) was removed once the rotation log
        // became a hashed `UnorderedMap` child of the anchor (P3): a writer-set
        // rotation is recorded as a per-`delta_id` child whose hash is part of
        // the anchor's `full_hash`, so divergent writer sets surface as divergent
        // child hashes (and divergent roots) WITHOUT folding them into `own_hash`.
        // The fold was redundant AND it was a divergence source in its own right
        // (a node could fold a stale/transient resolved set and never re-fold
        // after the collection converged via HC), so dropping it makes `own_hash`
        // identical on every write path (WASM-execute and merge alike).

        // Write the entry bytes BEFORE updating the Merkle index. The
        // index update propagates the new own_hash up the parent chain,
        // making the new state observable via the root-hash poll path
        // (`compute_root_hash`). Readers that iterate a collection's
        // children silently drop entries whose `Key::Entry` lookup
        // returns `None` (`UnorderedMap::entries` → `flatten().fuse()`
        // swallows the `NotFound` Err), so an admin-server reader hit
        // mid-write would otherwise see a converged root hash with
        // missing children — the "Hello Wor" vs "Hello World" rga
        // flake reproduced post-#2465. Writing the entry first means
        // readers see either (old hash + old entries) or
        // (new hash + new entries), never the inconsistent middle.
        //
        // `storage_write` returns `bool` meaning "evicted a previous
        // value" (true) vs "inserted a new key" (false) — not
        // success/failure. Actual write failures surface as `HostError`
        // traps from the runtime (`KeyLengthOverflow`,
        // `ValueLengthOverflow`, `InvalidMemoryAccess`), not as
        // `Ok(false)`. Discard the bool — `let _ignored = ...` matches
        // the style used at the `storage_remove` site (line 1448).
        let _ignored = S::storage_write(Key::Entry(id), &final_data);

        // If `update_hash_for` errors below after the entry write above
        // succeeded, the entry bytes remain in storage with no index
        // entry pointing at them — an "orphan." This is unavoidable
        // without a transactional storage layer, and it's the lesser
        // evil compared to the inverse (index advertising bytes that
        // aren't there) because:
        //   * `find_by_id` consults the index first (line 1689, 1702)
        //     and bails when the index entry is missing or deleted —
        //     so the read path used by collections (`Collection::get`,
        //     `Collection::entries`) silently skips the orphan.
        //   * `find_by_id_raw` does NOT consult the index — it returns
        //     raw bytes whenever `Key::Entry(id)` is present. In
        //     principle this exposes the orphan, but every production
        //     caller (the sync-layer traversals in
        //     `hash_comparison{,_protocol}.rs`, `level_sync.rs`)
        //     reaches `find_by_id_raw` only after iterating a parent's
        //     index-derived child list — and the orphan's id is, by
        //     definition, not in any parent's index.
        //   * The next successful `apply_action` for the same id
        //     overwrites the orphan bytes, so the storage cost is
        //     transient.
        // The pre-fix ordering (index-then-entry) had the symmetric
        // problem with much worse user-visible behavior — the rga
        // "Hello Wor" flake described above — because the read path
        // *does* propagate index-advertised entries through every
        // production caller, so a "hash exists, bytes don't"
        // inconsistency surfaces immediately as a wrong-content read.
        // (Re)assert the root's merge-dispatch tag on every local write. Unlike
        // creation (`add_root`), a plain hash update never persisted `crdt_type`,
        // so a root first stored opaque could never be upgraded to `JsRoot` by a
        // later `persist_root_state` — the write stamped the marker on `metadata`
        // but it was dropped here. Non-root entities pass `None` (leave unchanged).
        let root_crdt_type = if crate::collections::is_app_root_entry(id) {
            metadata.crdt_type.clone()
        } else {
            None
        };
        let full_hash =
            <Index<S>>::update_hash_for(id, own_hash, Some(metadata.updated_at), root_crdt_type)?;

        // A value write that causally follows an existing tombstone must lift it,
        // or `find_by_id` would keep hiding the bytes we just wrote (the entity's
        // `updated_at` already outran the tombstone in the LWW guard above, so
        // the write won — but the stale `deleted_at` would silently suppress it,
        // diverging replicas on delete-then-update vs update-only delivery). This
        // is a no-op unless the entity is tombstoned; `save_internal` is never on
        // the delete path (deletes go through `apply_delete_ref_action`), and the
        // `> deleted_at` guard inside `clear_deleted` keeps ties and older writes
        // from resurrecting.
        //
        // Ordering: `update_hash_for` above already persisted the new
        // `updated_at`, and both calls run inside the same reentrant
        // `index_mutation_guard`, so no concurrent writer interleaves between
        // them (`clear_deleted` also re-advances the nonce defensively).
        <Index<S>>::clear_deleted(id, *metadata.updated_at)?;

        if id.is_root() {
            info!(
                target: "storage::root_merge",
                %id,
                own_hash = %hex::encode(own_hash),
                full_hash = %hex::encode(full_hash),
                "ROOT MERGE: Final hashes after Merkle tree update"
            );
        }

        let is_new = metadata.created_at == *metadata.updated_at;

        Ok(Some((is_new, full_hash)))
    }

    /// Write a root-state byte blob that has *already* been CRDT-merged
    /// by an external dispatcher (e.g. the WASM module via
    /// `ContextClient::merge_root_state`). Bypasses the host-side
    /// merge step entirely — the caller has guaranteed the merge has
    /// happened — and just does the post-merge work: hash, Merkle
    /// index update, storage write.
    ///
    /// Necessary because host-side `merge_root_state` can't dispatch the
    /// app's typed `Mergeable::merge` (the registry it consults is only
    /// populated inside WASM). The sync paths that encounter root-entity
    /// divergence delegate the merge itself to WASM, then call this to
    /// commit the result.
    ///
    /// # Errors
    ///
    /// Returns `StorageError` if the index update fails or the storage
    /// write fails. Does NOT enforce I5 — the caller IS the source of
    /// the merged bytes and is responsible for I5 compliance.
    pub fn write_pre_merged_root_state(
        id: Id,
        merged: &[u8],
        metadata: Metadata,
    ) -> Result<[u8; 32], StorageError> {
        // Mirror the post-merge work in `save_internal` for the app
        // root: hash the merged bytes, update the Merkle index, write
        // storage. When this is the first time the receiver has seen
        // the entity, the index doesn't exist yet — create it so
        // `update_hash_for` doesn't fail with `IndexNotFound`.
        //
        // App root state covers TWO ids: `ROOT_ID` (the system root)
        // and `ROOT_ENTRY_ID` (the `Root<T>` entry). Pre-fix only
        // `id.is_root()` was checked, missing the latter — first-time
        // merges for `Root<T>` entries would fail with `IndexNotFound`
        // and the deferred WASM merge would be dropped, leaving the
        // receiver's root entity permanently divergent.
        //
        // Hold the reentrant mutation guard across the whole LWW-check →
        // entry-write → own_hash-update sequence for the same reason as
        // `save_internal`: the value write and the `own_hash` update are
        // separate store writes, and a concurrent writer for this id must not
        // interleave between them or the stored bytes and recorded `own_hash`
        // diverge.
        //
        // TODO(perf): see the matching note in `save_internal` — this holds the
        // process-global guard across the merge; revisit for a per-entity lock
        // if it regresses write throughput.
        let _mutation_guard = crate::index::index_mutation_guard();

        let last_metadata = <Index<S>>::get_metadata(id)?;

        // LWW guard — same shape as `save_internal`'s LWW-by-HLC
        // check. If the locally-stored state is already newer (e.g.
        // gossip already applied the action and stored the entity
        // with a newer `updated_at`), HC / LevelWise re-syncing the
        // same root via this path would otherwise overwrite the
        // metadata with the wire's older `updated_at` and regress
        // the Merkle parent's full_hash. Root cause of the
        // shared-storage e2e: gossip applied set_shared correctly,
        // then HC re-pushed the root entity via this LWW path and
        // the timestamp regression silently broke convergence.
        //
        // When the timestamps tie we still write — the bytes may
        // differ (concurrent writes resolved differently). Strictly
        // greater = newer here, equal = re-apply, older = no-op.
        if let Some(ref existing) = last_metadata {
            if existing.updated_at > metadata.updated_at {
                let existing_full = <Index<S>>::get_hashes_for(id)?
                    .map(|(full, _own)| full)
                    .unwrap_or([0_u8; 32]);
                tracing::debug!(
                    %id,
                    existing_ts = %*existing.updated_at,
                    incoming_ts = %*metadata.updated_at,
                    "write_pre_merged_root_state: local state is newer, skipping (LWW)"
                );
                return Ok(existing_full);
            }
        }

        if last_metadata.is_none() {
            if id.is_root() {
                <Index<S>>::add_root(ChildInfo::new(id, [0_u8; 32], metadata.clone()))?;
            } else if crate::collections::is_app_root_entry(id) {
                // `Root<T>` entry — attach as a child of the system
                // root so the index hierarchy stays consistent with
                // the layout `Root::new` produces locally.
                //
                // ENTRY-BEFORE-PARENT (#2319 follow-up): pre-write
                // Key::Entry so `Id::root()`'s children list never
                // advertises an id without a backing entry. The
                // matching `storage_write(Key::Entry(id), merged)`
                // below would otherwise leave a window in which
                // `find_by_id(id)` returns `None` for an id that the
                // root's children advertises. Same rationale as the
                // apply_action fix at line 1267.
                let _ignored = S::storage_write(Key::Entry(id), merged);
                <Index<S>>::add_child_to(
                    Id::root(),
                    ChildInfo::new(id, [0_u8; 32], metadata.clone()),
                )?;
            }
        }

        let own_hash: [u8; 32] = Sha256::digest(merged).into();
        // Entry-before-index ordering — same rationale as `save_internal`:
        // updating the Merkle index first makes the new root hash
        // observable before the entry bytes are stored, so a concurrent
        // reader can see a converged root hash with missing children
        // (the "Hello Wor" rga flake). The discarded `bool` from
        // `storage_write` is the eviction signal ("did a previous value
        // exist under this key"), not a success/failure flag — write
        // failures trap from the runtime as `HostError`, not `Ok(false)`.
        //
        // Same orphan trade-off as `save_internal` (see the longer
        // comment there): if `update_hash_for` errors below, the
        // merged bytes are persisted but the index isn't updated.
        // `find_by_id` bails on the missing index; `find_by_id_raw`
        // would expose the orphan in principle, but every production
        // caller reaches it only via an index-derived child list that
        // the orphan isn't in. The next successful merge for this id
        // overwrites the orphan bytes.
        //
        // We don't re-check the LWW guard after the entry write
        // because the only thing that could invalidate it is a
        // concurrent writer for the same id, and the storage layer
        // doesn't serialize concurrent writes anyway — re-checking
        // would just narrow the race window without closing it.
        let _ignored = S::storage_write(Key::Entry(id), merged);
        // Preserve the root's merge-dispatch tag across a sync-applied write so a
        // `JsRoot` root materialised via sync keeps routing to the guest merge
        // (see the note in `Index::update_hash_for`). Only the app root carries a
        // meaningful tag on this path; non-root entities pass `None`.
        let root_crdt_type = if crate::collections::is_app_root_entry(id) {
            metadata.crdt_type.clone()
        } else {
            None
        };
        let full_hash =
            <Index<S>>::update_hash_for(id, own_hash, Some(metadata.updated_at), root_crdt_type)?;
        Ok(full_hash)
    }

    /// Attempt to merge two versions of data using CRDT semantics.
    ///
    /// Returns the merged data, or an error if merge fails.
    /// Merge mode is enabled to prevent timestamp generation during merge operations.
    ///
    /// `is_opaque_root` is `true` when the stored root entity carries no
    /// `crdt_type` (an *opaque* root — see [`is_opaque_root_crdt_type`]). Such a
    /// root has no application-defined `Mergeable` to dispatch to, so when the
    /// merge registry reports no registered function it falls back to a direct
    /// last-writer-wins accept instead of failing (see the `# Errors` note).
    ///
    /// # Errors
    ///
    /// Returns `StorageError::MergeFailure` when the merge registry has no
    /// function for the root entity type **and** the root is not opaque
    /// (`is_opaque_root == false`). This enforces I5 (No Silent Data Loss) for a
    /// real `#[app::state]` root — a type that *should* merge field-by-field
    /// must fail loudly rather than be silently overwritten by LWW. An opaque
    /// root (`is_opaque_root == true`) never reaches this error: it resolves by
    /// LWW, mirroring what the sync path does for opaque root leaves.
    fn try_merge_data(
        _id: Id,
        existing: &[u8],
        incoming: &[u8],
        existing_created_at: u64,
        existing_timestamp: u64,
        incoming_timestamp: u64,
        is_opaque_root: bool,
    ) -> Result<Vec<u8>, StorageError> {
        use crate::collections::crdt_meta::MergeError;
        use crate::merge::merge_root_state;

        // Attempt CRDT merge with merge mode enabled
        // This prevents timestamp generation during merge to ensure deterministic hashes.
        //
        // `existing_created_at` is forwarded so the bootstrap-aware fallback in
        // `merge_root_state` can recognise an entity that was created but never
        // explicitly written (`created_at == updated_at`) and accept incoming
        // unconditionally. Without that signal, the local-clock HLC at
        // materialisation beats an earlier-written remote root on plain LWW
        // and silently drops the remote bytes — see the regression timeline
        // in `is_app_root_entry`'s doc comment.
        let result = crate::env::with_merge_mode(|| {
            merge_root_state(
                existing,
                incoming,
                existing_created_at,
                existing_timestamp,
                incoming_timestamp,
            )
        });

        match result {
            Ok(merged) => Ok(merged),
            // Opaque root (no `crdt_type`) with no registered merger. Two ways
            // to reach here for such a root, both benign:
            //   * Host production builds delete the merge registry entirely, so
            //     `merge_root_state` always reports `NoMergeFunctionRegistered`.
            //     This is the local-write path a JS app (or any app that does
            //     not use `#[app::state]`) takes via `persist_root_state`.
            //   * A registry exists (WASM/test) but nothing was registered.
            // Either way there is no `Mergeable` to dispatch to — WASM has no
            // `__calimero_merge_root_state` for a type without a `Mergeable`
            // impl — so the only convergent resolution is a direct LWW write.
            // The enclosing `save_internal` branch is only entered when
            // `incoming_timestamp >= existing_timestamp`, so incoming is the LWW
            // winner; the explicit `>=` tie-break keeps this correct if the
            // function is ever called from elsewhere and matches the opaque-root
            // direct-LWW the sync path applies. Crucially the merge registry is
            // consulted FIRST, so a real `#[app::state]` root whose merger IS
            // registered takes the `Ok` arm above and is never LWW-collapsed.
            Err(MergeError::NoMergeFunctionRegistered) if is_opaque_root => {
                tracing::debug!(
                    target: "storage::root_merge",
                    existing_ts = existing_timestamp,
                    incoming_ts = incoming_timestamp,
                    "opaque root entity with no registered merge function; \
                     resolving by LWW (incoming wins by updated_at)"
                );
                if incoming_timestamp >= existing_timestamp {
                    Ok(incoming.to_vec())
                } else {
                    Ok(existing.to_vec())
                }
            }
            // I5 Enforcement: for a NON-opaque root (a real `crdt_type`) with no
            // registered merger — and for every other merge failure — propagate
            // the error instead of falling back to LWW, preventing silent data
            // loss. The MergeError is preserved for programmatic error handling.
            Err(e) => Err(StorageError::from(e)),
        }
    }

    /// Attempt to merge two versions of non-root entity data using CRDT semantics.
    ///
    /// # Merge Dispatch by CrdtType
    ///
    /// For non-root entities, we dispatch based on `CrdtType` in metadata:
    ///
    /// **Built-in types** (all except `Custom`) - merged via [`merge_by_crdt_type`]:
    /// - `GCounter`, `PnCounter`: Semantic merge (max per executor)
    /// - `Rga`: Semantic merge (union of characters)
    /// - `LwwRegister`: Returns incoming (timestamp comparison done by caller)
    /// - `UnorderedMap`, `UnorderedSet`, `Vector`: Returns incoming (entries are
    ///   separate entities with their own `CrdtType`, merged individually)
    /// - `UserStorage`: Returns incoming (LWW per user)
    /// - `FrozenStorage`: Returns existing (first-write-wins, immutable)
    ///
    /// **Custom types** - require WASM callback (PR #1940), currently fall back to LWW
    ///
    /// **Legacy data** (no CrdtType metadata) - fall back to LWW
    ///
    /// # Invariants
    ///
    /// - **I5 (No Silent Data Loss)**: Built-in CRDT types MUST use their semantic
    ///   merge rules, not be overwritten by LWW.
    /// - **I10 (Metadata Persistence)**: Relies on `crdt_type` being persisted in
    ///   entity metadata for correct dispatch.
    ///
    /// [`merge_by_crdt_type`]: crate::merge::merge_by_crdt_type
    fn try_merge_non_root(
        id: Id,
        existing: &[u8],
        incoming: &[u8],
        metadata: &Metadata,
        existing_timestamp: u64,
        incoming_timestamp: u64,
    ) -> Result<Vec<u8>, StorageError> {
        use crate::collections::crdt_meta::{CrdtType, MergeError};
        use crate::merge::{is_builtin_crdt, merge_by_crdt_type};

        // Deterministic LWW pick. `incoming_timestamp > existing` ⇒ incoming;
        // `<` ⇒ existing. The `==` (concurrent, same-HLC) case must be
        // resolved IDENTICALLY on every replica regardless of which write it
        // applied first, or two writers stamping the same HLC nanosecond
        // (e.g. distinct writers in a `Shared` set after a rotation) leave the
        // cluster permanently diverged on the same DAG heads (the
        // shared-storage post-rotation split-brain). A plain "incoming wins"
        // is NOT order-independent — it flips symmetrically. Break exact ties
        // by content hash (higher `Sha256(data)` wins): node-independent, so
        // all replicas converge. Equal data is a true no-op (either is fine).
        let lww_pick = |existing: &[u8], incoming: &[u8]| -> Vec<u8> {
            use core::cmp::Ordering;
            match incoming_timestamp.cmp(&existing_timestamp) {
                Ordering::Greater => incoming.to_vec(),
                Ordering::Less => existing.to_vec(),
                Ordering::Equal => {
                    let inc_hash: [u8; 32] = Sha256::digest(incoming).into();
                    let exi_hash: [u8; 32] = Sha256::digest(existing).into();
                    if inc_hash >= exi_hash {
                        incoming.to_vec()
                    } else {
                        existing.to_vec()
                    }
                }
            }
        };

        // Check if we have CRDT type metadata
        let Some(crdt_type) = &metadata.crdt_type else {
            // Legacy data - no CRDT type, use LWW
            debug!(
                target: "storage::merge",
                %id,
                "No CRDT type metadata, falling back to LWW"
            );
            return Ok(lww_pick(existing, incoming));
        };

        // For built-in types, merge in storage layer
        if is_builtin_crdt(crdt_type) {
            // LwwRegister's merge_by_crdt_type always returns incoming; the
            // actual last-writer-wins comparison must happen here using the
            // HLC timestamps carried in metadata.
            //
            // RotationLog joins this LWW path (P3): a rotation-log entry now
            // lives as its OWN per-`delta_id` child holding a single entry (the
            // collection accumulates DIFFERENT deltas structurally, via the
            // parent's children list / add-wins, NOT via a value union). So the
            // per-child *value* merge is a same-`delta_id` collision, which LWW
            // resolves convergently: equal timestamps fall to `lww_pick`'s
            // content-hash tiebreak, so both nodes pick `max_hash` — symmetric,
            // so HashComparison's bidirectional leaf reconciliation SETTLES
            // (the value-union merge did not, leaving a sticky HC loop). The
            // old `merge_rotation_log` union was only needed by the abandoned
            // single-blob representation.
            let is_lww = matches!(
                crdt_type,
                CrdtType::LwwRegister { .. } | CrdtType::RotationLog
            );
            if is_lww {
                return Ok(lww_pick(existing, incoming));
            }

            let result =
                crate::env::with_merge_mode(|| merge_by_crdt_type(crdt_type, existing, incoming));

            match result {
                Ok(merged) => {
                    trace!(
                        target: "storage::merge",
                        %id,
                        crdt_type = ?crdt_type,
                        "Successfully merged non-root entity using CRDT semantics"
                    );
                    return Ok(merged);
                }
                Err(MergeError::SerializationError(msg)) => {
                    warn!(
                        target: "storage::merge",
                        %id,
                        crdt_type = ?crdt_type,
                        error = %msg,
                        "CRDT merge failed due to serialization error, falling back to LWW"
                    );
                }
                Err(e) => {
                    warn!(
                        target: "storage::merge",
                        %id,
                        crdt_type = ?crdt_type,
                        error = %e,
                        "CRDT merge failed, falling back to LWW"
                    );
                }
            }
        } else {
            // Types that need WASM callback (LwwRegister, collections, Custom)
            // For now, fall back to LWW. PR #1940 will add WASM callback support.
            debug!(
                target: "storage::merge",
                %id,
                crdt_type = ?crdt_type,
                "CRDT type requires WASM callback, falling back to LWW"
            );
        }

        // Fall back to LWW (deterministic equal-HLC tiebreak — see `lww_pick`).
        Ok(lww_pick(existing, incoming))
    }

    /// Decides whether the local executor may stamp a `Shared` mutation of
    /// `id`, returning the writer set to persist and the signer to record, or
    /// `None` if the executor is not authorized.
    ///
    /// Authority is the union of two writer sets:
    ///   - `claimed`: the writers carried in the metadata being written. On a
    ///     save this is the incoming action's own claimed set; on a delete it
    ///     is the set just loaded from the stored index (so `claimed` already
    ///     equals stored there, and the union below is a no-op).
    ///   - stored: the writers currently persisted in the index for `id`.
    ///
    /// Membership in EITHER set authorizes the stamp. The union is what lets a
    /// writer rotate itself out: it is still in the stored set though absent
    /// from the new claimed set, and the remote verifier also checks against
    /// stored, so the signature still verifies there.
    ///
    /// Both the save and delete paths route through here so the local-write
    /// authority rule lives in exactly one place and cannot drift between them;
    /// each caller keeps its own nonce and any schema re-stamp.
    ///
    /// `stored`: the caller's already-loaded stored writer set, when it has one.
    /// The delete path loads `metadata` from the index immediately before
    /// calling, so its writers ARE the stored set — it passes `Some(..)` to
    /// avoid a redundant index read. The save path's `claimed` is the incoming
    /// action's set (not stored), so it passes `None` and the stored set is
    /// looked up here.
    fn authorize_local_shared_stamp(
        id: Id,
        claimed: &BTreeMap<AccountId, OpMask>,
        stored: Option<&BTreeMap<AccountId, OpMask>>,
    ) -> Result<Option<SharedStampAuthorization>, StorageError> {
        let executor: AccountId = crate::env::account_id().into();
        let stored_has_executor = match stored {
            Some(stored) => stored.contains_key(&executor),
            None => <Index<S>>::get_metadata(id)?
                .as_ref()
                .map(|m| match &m.storage_type {
                    StorageType::Shared { writers, .. } => writers.contains_key(&executor),
                    _ => false,
                })
                .unwrap_or(false),
        };
        let authorized = stored_has_executor || claimed.contains_key(&executor);
        // Same split as the other stamp site: authorized by account, stamped with the
        // key that will verify.
        let device: PublicKey = crate::env::device_id().into();
        Ok(authorized.then(|| (claimed.clone(), device)))
    }

    /// Saves raw serialized data with orphan checking.
    ///
    /// # Errors
    /// - `CannotCreateOrphan` if entity has no parent and isn't root
    ///
    pub fn save_raw(
        id: Id,
        data: Vec<u8>,
        metadata: Metadata,
    ) -> Result<Option<[u8; 32]>, StorageError> {
        debug!(
            %id,
            data_len = data.len(),
            created_at = metadata.created_at,
            updated_at = metadata.updated_at(),
            "save_raw called"
        );
        if !id.is_root() && <Index<S>>::get_parent_id(id)?.is_none() {
            return Err(StorageError::CannotCreateOrphan(id));
        }

        let mut metadata = metadata.clone();
        // Whether THIS call is a local owner/writer write — i.e. one of the
        // three stamp branches below fired. When it does, the owner-driven
        // convert (PR-6c) re-stamps the entry's `schema_version` to the binary's
        // current target so a stale identity-gated entry migrates as the owner's
        // next ordinary signed delta. The stamp must also be persisted to the
        // stored index entry, because a re-write of an existing entry flows
        // through `update_hash_for`, which deliberately does NOT rewrite stored
        // metadata — so we persist it explicitly via `Index::set_schema_version`
        // after `save_internal` succeeds.
        let mut local_owner_schema_stamp: Option<u32> = None;
        // For a local User write, ALWAYS overwrite the incoming
        // signature_data with a fresh placeholder tied to this call's
        // nonce. We can't trust the WASM-provided value: a re-write
        // via `UnorderedMap::insert_with_storage_type` /
        // `EntryMut::drop` plumbs through the previously-stored
        // metadata verbatim — including a real ed25519 signature for
        // the prior (data, nonce) pair. Skipping the stamp in that
        // case would broadcast the new data with the old signature,
        // which receivers cannot verify (the signed payload commits
        // to data + nonce, both of which just changed). Remote
        // actions never go through `save_raw` (they apply via
        // `apply_action`), so unconditionally stamping here is safe:
        // it only fires when the executor is the owner.
        if let StorageType::User { owner, .. } = metadata.storage_type {
            if owner == AccountId::from(crate::env::account_id()) {
                let nonce = *metadata.updated_at;
                metadata.storage_type = StorageType::User {
                    owner,
                    signature_data: Some(SignatureData {
                        signature: [0; 64], // Placeholder, added by signer
                        nonce,
                        // The DEVICE writing on the owner's behalf — see the
                        // matching stamp on the delete path.
                        signer: Some(crate::env::device_id().into()),
                    }),
                };
                // Owner-driven convert (PR-6c): the owner's own write re-stamps
                // the entry at the binary's current target schema version, so a
                // stale identity-gated entry migrates as the owner's next
                // ordinary signed delta. This is exactly the local-owner stamp
                // site, so it advances on the same monotonic nonce. It MUST NOT
                // fire under merge mode: merge mode bypasses the replay-nonce
                // check (see the `skip_nonce` site above), so converting there
                // would re-shape the identity-gated entry on the idempotent
                // merge re-apply path instead of as a fresh, owner-signed,
                // monotonic delta (O4). The signature placeholder above still
                // stamps (that is about authenticity, not the convert).
                if !crate::env::in_merge_mode() {
                    let target = calimero_sdk::app::schema_version();
                    metadata.schema_version = Some(target);
                    local_owner_schema_stamp = Some(target);
                }
            }
        }

        // If this is a local shared action by a writer, set the nonce.
        // Authority (stored ∪ claimed) is decided by the shared helper; here
        // `claimed` is the incoming action's own writer set.
        //
        // Same re-stamp-always rationale as the User arm above: a
        // re-write may carry the previously-stored real signature
        // through, and broadcasting that with new data + new nonce
        // would not verify on receivers.
        let shared_to_stamp = if let StorageType::Shared {
            writers: claimed_writers,
            ..
        } = &metadata.storage_type
        {
            Self::authorize_local_shared_stamp(id, claimed_writers, None)?
        } else {
            None
        };
        if let Some((writers, signer)) = shared_to_stamp {
            let nonce = *metadata.updated_at;
            metadata.storage_type = StorageType::Shared {
                writers,
                signature_data: Some(SignatureData {
                    signature: [0; 64], // Placeholder, added by signer
                    nonce,
                    signer: Some(signer), // O(1) verifier lookup
                }),
            };
            // Owner-driven convert (PR-6c): same as the User arm — a current
            // writer's own write re-stamps the target schema version on the
            // monotonic-nonce path, and is likewise suppressed under merge mode
            // (which bypasses the replay-nonce check — see the `skip_nonce`
            // site above), so the convert only lands as a fresh signed delta.
            if !crate::env::in_merge_mode() {
                let target = calimero_sdk::app::schema_version();
                metadata.schema_version = Some(target);
                local_owner_schema_stamp = Some(target);
            }
        }

        // Member upsert: a member carries no writer set, so there is no
        // claimed-set union — authority is purely the ANCHOR's resolved writers
        // (settled local state). Stamp the anchor pointer + signer placeholder.
        let member_to_stamp =
            if let StorageType::SharedMember { anchor, .. } = &metadata.storage_type {
                let executor: AccountId = crate::env::account_id().into();
                if Self::resolve_anchor_writers(*anchor).contains_key(&executor) {
                    Some((*anchor, executor))
                } else {
                    None
                }
            } else {
                None
            };
        if let Some((anchor, _authorized_account)) = member_to_stamp {
            let nonce = *metadata.updated_at;
            // Authorize by account, stamp the key that will verify — see the sibling
            // sites. The account is what passed the gate; it is not what signs.
            let signer: PublicKey = crate::env::device_id().into();
            metadata.storage_type = StorageType::SharedMember {
                anchor,
                signature_data: Some(SignatureData {
                    signature: [0; 64], // Placeholder, added by signer
                    nonce,
                    signer: Some(signer), // O(1) verifier lookup
                }),
            };
            // Owner-driven convert (PR-6c): same as the User/Shared arms — a
            // member write by a resolved anchor writer re-stamps the target
            // schema version on the monotonic-nonce path, and is likewise
            // suppressed under merge mode (which bypasses the replay-nonce
            // check — see the `skip_nonce` site above), so the convert only
            // lands as a fresh signed delta.
            if !crate::env::in_merge_mode() {
                let target = calimero_sdk::app::schema_version();
                metadata.schema_version = Some(target);
                local_owner_schema_stamp = Some(target);
            }
        }

        let Some((is_new, full_hash)) = Self::save_internal(id, &data, metadata.clone())? else {
            return Ok(None);
        };

        // Owner-driven convert (PR-6c): persist the re-stamped `schema_version`
        // to the stored index entry. `save_internal` → `update_hash_for` only
        // touches the entity hashes + `updated_at` (it deliberately does NOT
        // rewrite stored metadata), so an existing entry's schema tag would
        // otherwise stay frozen at its add-time value. Only fires for a local
        // owner/writer write (one of the stamp branches above), so a non-owner
        // can never drive the convert. Merkle-invisible, so it cannot diverge
        // the root hash.
        if let Some(target) = local_owner_schema_stamp {
            // Read the prior stored stamp before overwriting so the log shows
            // the actual old -> new transition (the convert only "lands" when
            // these differ — a no-op re-write of an already-current entry keeps
            // the same value). NOTE: an owner's own write runs inside the wasm
            // GUEST, where `tracing` does not reach the node log — so this debug
            // is for guest-side diagnosis only. The node-log-observable signal is
            // emitted host-side on the RECEIVER in `apply_action` when it adopts
            // the replicated converted tag ("applied migrated ... schema_version").
            let prior_schema = <Index<S>>::get_metadata(id)?.and_then(|m| m.schema_version);
            <Index<S>>::set_schema_version(id, Some(target))?;
            debug!(
                %id,
                old_schema_version = ?prior_schema,
                new_schema_version = target,
                "owner-driven convert: re-stamped identity-gated entry schema_version"
            );
            // Surface host-side too: this runs inside the wasm GUEST, where the
            // `tracing` debug above has no subscriber and never reaches the node
            // log. `env::log` routes through the guest→host log syscall (the node
            // forwards it as `WASM_LOG`), so the convert is node-observable on the
            // ORIGINATING node — for both organic owner writes and the one-tap
            // `migrate_my_entries`. This is the signal the e2e scenarios assert.
            crate::env::log(&format!(
                "owner-driven convert: re-stamped identity-gated entry schema_version \
                 id={id} old_schema_version={prior_schema:?} new_schema_version={target}"
            ));
        }

        let ancestors = <Index<S>>::get_ancestors_of(id)?;

        let action = if is_new {
            debug!(%id, "save_raw emitting Add action for entity");
            Action::Add {
                id,
                data,
                ancestors,
                metadata,
            }
        } else {
            debug!(%id, "save_raw emitting Update action for entity");
            Action::Update {
                id,
                data,
                ancestors,
                metadata,
            }
        };

        // #2319 root cause: this push is the choke point through which
        // every storage mutation enters the synced delta stream. For
        // `MainStorage` that is correct. For `PrivateStorage` — backing
        // `#[app::private]` tree-collection fields after the macro
        // substitution — it was leaking actions for purely node-local
        // collection bookkeeping (e.g. the `add_child_to(*ROOT_ID, ...)`
        // call inside `Collection::new()` when an UnorderedMap is
        // default-constructed during `PrivateSecrets::default()`). Peers
        // applied those actions to their `MainStorage` and ended up
        // with extra `crdt_type=None, field_name=None` children under
        // context-root that the author didn't have. Gate the push on
        // `S::participates_in_sync()` so private writes stay local.
        if S::participates_in_sync() {
            crate::delta::push_action(action);
        }

        debug!(%id, ?full_hash, is_new, "save_raw completed");

        Ok(Some(full_hash))
    }

    /// Helper to verify an upsert (`Add` or `Update`) action against the
    /// receiver's currently-stored entity.
    ///
    /// Both upsert variants share the same storage-type-match invariant:
    /// once an entity exists locally with a given `StorageType`, no remote
    /// action can change that type. `Update` is the path you'd expect to
    /// see for an existing entity, but `Add` for an entity that already
    /// exists locally must also be gated — otherwise a forged
    /// `Action::Add { storage_type: Public }` for an entity stored as
    /// `Shared`/`User` would land in the `Public` arm of `apply_action`
    /// (which intentionally skips signature verification, see the
    /// `hash_authorization_for_payload` doc), reach `save_internal`, and
    /// silently downgrade the entity to `Public` — the storage-type
    /// downgrade attack the bot review on PR #2386 flagged.
    fn verify_action_update(action: &Action) -> Result<(), StorageError> {
        let (metadata, _data, id) = match action {
            Action::Add {
                metadata, data, id, ..
            }
            | Action::Update {
                metadata, data, id, ..
            } => (metadata, data, *id),
            // DeleteRef has its own type-match check in the main
            // `apply_action`; Compare doesn't mutate.
            _ => return Ok(()),
        };

        // Get existing metadata
        let existing_metadata = <Index<S>>::get_metadata(id)?;

        // Try to get existing metadata to determine if this is an Update or an Add (upsert)
        match existing_metadata {
            // This is indeed an update operation
            Some(existing_metadata) => {
                // Compare storage types and owners
                match (&existing_metadata.storage_type, &metadata.storage_type) {
                    (StorageType::Public, StorageType::Public) => {
                        // no checks needed for Public storage
                        Ok(())
                    }
                    (StorageType::Frozen, StorageType::Frozen) => {
                        // Mutability is verified in the main `apply_action()` function later
                        Ok(())
                    }
                    (
                        StorageType::User {
                            owner: existing_owner,
                            ..
                        },
                        StorageType::User { owner, .. },
                    ) => {
                        // Check owner hasn't changed
                        if *owner != *existing_owner {
                            return Err(StorageError::ActionNotAllowed(
                                "Cannot change owner of User storage".to_owned(),
                            ));
                        }

                        Ok(())
                    }
                    (StorageType::Shared { .. }, StorageType::Shared { .. }) => {
                        // Writer-set changes (rotation) are gated by signature
                        // verification in apply_action against the stored writer set.
                        Ok(())
                    }
                    (
                        StorageType::SharedMember {
                            anchor: existing_anchor,
                            ..
                        },
                        StorageType::SharedMember {
                            anchor: new_anchor, ..
                        },
                    ) => {
                        // A member's anchor is immutable (like User's owner):
                        // re-anchoring would silently move it to a different
                        // writer domain. The write itself is gated by signature
                        // verification against the anchor's writers in
                        // apply_action.
                        if *new_anchor != *existing_anchor {
                            return Err(StorageError::ActionNotAllowed(
                                "Cannot change SharedMember anchor".to_owned(),
                            ));
                        }
                        Ok(())
                    }
                    (existing, new) => {
                        // All other combinations are invalid
                        debug!(?existing, ?new, "Invalid storage type change attempted");
                        Err(StorageError::ActionNotAllowed(
                            "Cannot change StorageType (e.g., User->Public/User->Frozen/etc)"
                                .to_owned(),
                        ))
                    }
                }
            }
            None => {
                // This is an "add" (upsert).
                // TODO: refactor
                // The item doesn't exist. Run the "Add" verification logic (that is currently
                // located in the main `apply_function()`.
                Ok(())
            }
        }
    }
}

/// Verifies an incoming `Frozen` action.
fn verify_frozen_action_upsert(action: &Action, data: &[u8]) -> Result<(), StorageError> {
    // Block all Updates.
    if let Action::Update { .. } = action {
        return Err(StorageError::ActionNotAllowed(
            "Frozen data cannot be updated".to_owned(),
        ));
    }

    // Verify the content-addressing via byte-slicing.
    // The data blob is: [key_hash (32 bytes)] + [value_bytes (N bytes)] + [element_id (32 bytes)]
    const KEY_HASH_SIZE: usize = 32;
    const ELEMENT_ID_SIZE: usize = 32;
    const MIN_LEN: usize = KEY_HASH_SIZE + ELEMENT_ID_SIZE;

    if data.len() < MIN_LEN {
        return Err(StorageError::InvalidData(
            "Frozen data blob is too small.".to_owned(),
        ));
    }

    // Extract the three components
    let key_from_entry = &data[..KEY_HASH_SIZE];
    // We don't need the `Element::Id` from the end, but we know it's there and
    // we need to remove it from the value_bytes.
    let value_bytes = &data[KEY_HASH_SIZE..data.len() - ELEMENT_ID_SIZE];

    // Re-calculate the hash of the `value bytes`
    let calculated_hash: [u8; 32] = Sha256::digest(value_bytes).into();

    // Check: The key inside the `Entry` must match the hash
    // of the value inside the `Entry`.
    if key_from_entry != calculated_hash {
        return Err(StorageError::InvalidData(
            "Frozen data corruption: Entry key does not match hash of Entry value.".to_owned(),
        ));
    }

    // If this check passes, the data is verified.
    Ok(())
}

/// Verifies that the action timestamp is within acceptable bounds of the local clock.
fn verify_action_timestamp(action: &Action) -> Result<(), StorageError> {
    let timestamp = match action {
        Action::Add { metadata, .. } | Action::Update { metadata, .. } => metadata.updated_at(),
        Action::DeleteRef { deleted_at, .. } => *deleted_at,
    };

    let now = time_now();

    // Allow for network latency and small clock skew
    let max_allowed = now.saturating_add(constants::DRIFT_TOLERANCE_NANOS);

    if timestamp > max_allowed {
        debug!(
            %timestamp,
            %now,
            %max_allowed,
            "Interface::verify_action_timestamp action with an invalid timestamp."
        );

        return Err(StorageError::InvalidTimestamp(timestamp, now));
    }

    Ok(())
}
