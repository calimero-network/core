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
//! - [`compare_trees()`](Interface::compare_trees()) - Generate sync actions
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
use std::collections::{BTreeMap, BTreeSet};

use borsh::{from_slice, to_vec};
use calimero_primitives::identity::PublicKey;
use indexmap::IndexMap;
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use crate::address::Id;
use crate::constants;
use crate::entities::{ChildInfo, Data, Metadata, SignatureData, StorageType};
use crate::env::time_now;
use crate::index::Index;
use crate::store::{Key, MainStorage, StorageAdaptor};

// Re-export types for convenience
pub use crate::action::{Action, ComparisonData};
pub use crate::error::StorageError;

/// Convenient type alias for the main storage system.
pub type MainInterface = Interface<MainStorage>;

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
    /// skips the v2 stored-writers fallback. Resolved by the node sync
    /// layer per #2266.
    pub effective_writers: Option<BTreeSet<PublicKey>>,

    /// Hash of the `CausalDelta` containing the action being applied. Used
    /// by the rotation-log write hook to record the originating delta on
    /// detected rotations. `None` for local apply / snapshot leaf push.
    pub delta_id: Option<[u8; 32]>,

    /// Hybrid timestamp of the containing `CausalDelta`. Used by the
    /// rotation-log write hook (sibling tiebreak per ADR 0001). `None` for
    /// callers without a `CausalDelta` in scope.
    pub delta_hlc: Option<crate::logical_clock::HybridTimestamp>,
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
    fn resolve_anchor_writers(anchor: Id) -> BTreeSet<PublicKey> {
        if let Ok(Some(log)) = crate::rotation_log::load::<S>(anchor) {
            if let Some(writers) = crate::rotation_log::resolve_local(&log) {
                return writers;
            }
        }
        if let Ok(Some(metadata)) = <Index<S>>::get_metadata(anchor) {
            if let StorageType::Shared { writers, .. } = metadata.storage_type {
                return writers;
            }
        }
        BTreeSet::new()
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
    /// * `User` / `Shared` with `signature_data: Some(_)` — compute
    ///   `payload_for_signing` from a synthetic `Action::Add { id,
    ///   data, ancestors: vec![], metadata }` and `ed25519_verify`
    ///   against the writer (owner for User, signer-hint or
    ///   writer-set scan for Shared).
    /// * `User` / `Shared` with `signature_data: None` — rejected as
    ///   `InvalidSignature`. After the bootstrap-signing fix
    ///   (`persist_signed_signatures` in
    ///   `crates/context/src/handlers/execute/mod.rs`), no locally
    ///   stored entity should carry `None` past `sign_authorized_actions`,
    ///   so a snapshot record with `None` is from a buggy or hostile
    ///   peer.
    ///
    /// Returns `Ok(())` if the entity is verified or doesn't require
    /// verification; `Err(StorageError::InvalidSignature)` otherwise.
    /// Does not write to storage.
    ///
    /// # Errors
    /// - `InvalidSignature` if the `signature_data` is `None`,
    ///   carries the `[0; 64]` placeholder, or fails ed25519
    ///   verification against the access-control rules in
    ///   `metadata.storage_type`.
    pub fn verify_snapshot_entity_signature(
        id: crate::address::Id,
        data: &[u8],
        metadata: &crate::entities::Metadata,
    ) -> Result<(), StorageError> {
        use crate::action::Action;
        use crate::entities::StorageType;

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
                if crate::env::ed25519_verify(&sig_data.signature, owner.digest(), &payload) {
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
                // above — particularly important for `Shared` since
                // the writer-set scan would do up to N ed25519
                // verifies (one per writer) before the crypto
                // library rejects the placeholder, which is a free
                // CPU-burn vector for a misbehaving peer.
                if sig_data.signature == [0u8; 64] {
                    return Err(StorageError::InvalidSignature);
                }
                // Fast path: signer hint + verify once. Slow path:
                // linear scan over writers. Mirrors `apply_action`.
                let verified = match sig_data.signer {
                    Some(hint) if writers.contains(&hint) => {
                        crate::env::ed25519_verify(&sig_data.signature, hint.digest(), &payload)
                    }
                    _ => writers.iter().any(|w| {
                        crate::env::ed25519_verify(&sig_data.signature, w.digest(), &payload)
                    }),
                };
                if verified {
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
                // A member carries no writer set: resolve it from the anchor's
                // locally-verified state. An unsynced anchor yields the empty
                // set, which fails verification (the scan finds no writer) —
                // fail closed rather than accept an unverifiable member.
                let writers = Self::resolve_anchor_writers(*anchor);
                let verified = match sig_data.signer {
                    Some(hint) if writers.contains(&hint) => {
                        crate::env::ed25519_verify(&sig_data.signature, hint.digest(), &payload)
                    }
                    _ => writers.iter().any(|w| {
                        crate::env::ed25519_verify(&sig_data.signature, w.digest(), &payload)
                    }),
                };
                if verified {
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
    /// placeholder reject, signer-hint fast path, else a scan over `writers`.
    ///
    /// `metadata.storage_type` must be `SharedMember`; any other variant is a
    /// caller error and is rejected as `InvalidData`.
    ///
    /// # Errors
    /// `InvalidSignature` if `signature_data` is `None`, carries the `[0; 64]`
    /// placeholder, or fails ed25519 verification against `writers`;
    /// `InvalidData` if `metadata.storage_type` is not `SharedMember`.
    pub fn verify_snapshot_member_signature(
        id: crate::address::Id,
        data: &[u8],
        metadata: &crate::entities::Metadata,
        writers: &BTreeSet<PublicKey>,
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
        let verified = match sig_data.signer {
            Some(hint) if writers.contains(&hint) => {
                crate::env::ed25519_verify(&sig_data.signature, hint.digest(), &payload)
            }
            _ => writers
                .iter()
                .any(|w| crate::env::ed25519_verify(&sig_data.signature, w.digest(), &payload)),
        };
        if verified {
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

        let data = to_vec(child).map_err(|e| StorageError::SerializationError(e.into()))?;

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
    /// Handles Add/Update/Delete actions, creating missing ancestors if needed.
    /// Generates Compare action for hash verification after applying changes.
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
    /// - `ActionNotAllowed` if Compare action is passed directly
    ///
    pub fn apply_action(action: Action, ctx: &ApplyContext) -> Result<(), StorageError> {
        // Verify that the action timestamp is not too far in the future
        // to prevent LWW Time Drift attacks.
        verify_action_timestamp(&action)?;

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
                        let verification_result = crate::env::ed25519_verify(
                            &sig_data.signature,
                            owner.digest(),
                            &payload,
                        );

                        if !verification_result {
                            return Err(StorageError::InvalidSignature);
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
                        // Identify the signer. Fast path: if the
                        // action carries a `signer` hint and that
                        // signer is in the authoritative set, do
                        // exactly one verify. Slow path (no hint):
                        // linear scan over authoritative writers.
                        //
                        // Per the #2233 epic compatibility rule, the
                        // signer hint is validated against the
                        // *causal* writer set above, not stored —
                        // that's already how it works here since
                        // `authoritative_writers` is now the
                        // DAG-causal answer when available.
                        let payload = action.payload_for_signing();
                        let verified = match sig_data.signer {
                            Some(hint) if authoritative_writers.contains(&hint) => {
                                crate::env::ed25519_verify(
                                    &sig_data.signature,
                                    hint.digest(),
                                    &payload,
                                )
                            }
                            _ => authoritative_writers.iter().any(|w| {
                                crate::env::ed25519_verify(
                                    &sig_data.signature,
                                    w.digest(),
                                    &payload,
                                )
                            }),
                        };
                        if !verified {
                            return Err(StorageError::InvalidSignature);
                        }

                        if !skip_nonce && new_nonce < last_nonce {
                            // Strictly stale: signature verified, but our
                            // local state is already AHEAD of this nonce.
                            // Drop silently — an authentic but older write
                            // whose newer twin already landed (HashComparison
                            // re-delivery / DAG-catchup out-of-order). A hard
                            // NonceReplay here would propagate through
                            // `Root::sync().expect()` and abort the sync
                            // batch, blocking convergence.
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
                            return Ok(());
                        }

                        // P3 of #2233: rotation-log write hook.
                        //
                        // Fires here — right after signature verification AND
                        // after the stale-nonce silent-skip guard above — so
                        // the log captures every fresh signature-verified
                        // Shared rotation that will reach the apply branch.
                        // Stale-but-authentic actions short-circuit before
                        // this point (silent-skip Ok), so they do NOT append
                        // a rotation entry — that's the right call: the log
                        // tracks rotations that influence storage state, and
                        // a stale rotation is a no-op (its newer counterpart
                        // already landed and was logged on its own apply).
                        //
                        // Cross-node convergence (P5) still works because
                        // peers see the same set of *causally-newer*
                        // rotations, just possibly in different orders.
                        //
                        // Idempotent: `rotation_log::append` dedups on
                        // `delta_id`, so a replayed delta produces no extra
                        // entry.
                        Self::maybe_append_rotation_log(
                            *id,
                            metadata,
                            ctx,
                            stored_writers.clone(),
                        )?;
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
                            None => Self::resolve_anchor_writers(*anchor),
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
                        let verified = match sig_data.signer {
                            Some(hint) if authoritative_writers.contains(&hint) => {
                                crate::env::ed25519_verify(
                                    &sig_data.signature,
                                    hint.digest(),
                                    &payload,
                                )
                            }
                            _ => authoritative_writers.iter().any(|w| {
                                crate::env::ed25519_verify(
                                    &sig_data.signature,
                                    w.digest(),
                                    &payload,
                                )
                            }),
                        };
                        if !verified {
                            return Err(StorageError::InvalidSignature);
                        }

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
                                    return Err(StorageError::InvalidSignature);
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
                                // DeleteRef keeps the strict `Err` on
                                // `<=` (unlike upsert's silent skip)
                                // because a stale delete being
                                // silently accepted vs dropped
                                // carries different semantics than
                                // a stale upsert, and rare-by-design
                                // deletes don't drive the
                                // post-divergence convergence problem
                                // upsert silent-skip fixes.
                                let payload = action.payload_for_signing();
                                let verification_result = crate::env::ed25519_verify(
                                    &sig_data.signature,
                                    owner.digest(),
                                    &payload,
                                );
                                if !verification_result {
                                    return Err(StorageError::InvalidSignature);
                                }

                                // Replay protection: nonce is the
                                // `deleted_at` time, checked against
                                // the last `updated_at` stored in
                                // the index.
                                let new_nonce = sig_data.nonce;
                                let last_nonce = *existing_metadata.updated_at;
                                if new_nonce <= last_nonce {
                                    return Err(StorageError::NonceReplay(Box::new((
                                        *owner, new_nonce,
                                    ))));
                                }
                            }
                            _ => {
                                // Action metadata is not User, but existing is.
                                return Err(StorageError::InvalidSignature);
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
                                    return Err(StorageError::InvalidSignature);
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
                                // DeleteRef keeps the strict `Err` on
                                // `<=` (unlike upsert's silent skip)
                                // — see the User DeleteRef arm for
                                // rationale.
                                //
                                // Identify the signer.
                                // Fast path: if the action carries a `signer` hint and that
                                // signer is in the authoritative set, do exactly one verify.
                                // Slow path (no hint): linear scan (matches Add/Update arm).
                                let payload = action.payload_for_signing();
                                let signer = match sig_data.signer {
                                    Some(hint) if existing_writers.contains(&hint) => {
                                        if crate::env::ed25519_verify(
                                            &sig_data.signature,
                                            hint.digest(),
                                            &payload,
                                        ) {
                                            Some(hint)
                                        } else {
                                            None
                                        }
                                    }
                                    _ => existing_writers.iter().copied().find(|w| {
                                        crate::env::ed25519_verify(
                                            &sig_data.signature,
                                            w.digest(),
                                            &payload,
                                        )
                                    }),
                                };
                                if signer.is_none() {
                                    return Err(StorageError::InvalidSignature);
                                }

                                // Replay protection (per-entity monotonic nonce).
                                //
                                // Strict `<=` Err, symmetric with the
                                // User DeleteRef arm above and matching
                                // the rationale documented there: stale
                                // delete semantics differ from upsert
                                // silent-skip, and DeleteRef tests do
                                // not opt into the test-only bypass.
                                // Removing the previously-speculative
                                // `nonce_check_disabled_for_testing`
                                // guard here so the two delete arms
                                // behave identically.
                                let new_nonce = sig_data.nonce;
                                let last_nonce = *existing_metadata.updated_at;
                                if new_nonce <= last_nonce {
                                    let placeholder = existing_writers
                                        .iter()
                                        .copied()
                                        .next()
                                        .unwrap_or_else(|| [0u8; 32].into());
                                    return Err(StorageError::NonceReplay(Box::new((
                                        placeholder,
                                        new_nonce,
                                    ))));
                                }
                            }
                            _ => {
                                // Action metadata is not Shared, but existing is.
                                return Err(StorageError::InvalidSignature);
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
                                    return Err(StorageError::InvalidSignature);
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
                                        Self::resolve_anchor_writers(existing_anchor)
                                    });

                                let payload = action.payload_for_signing();
                                let signer = match sig_data.signer {
                                    Some(hint) if existing_writers.contains(&hint) => {
                                        if crate::env::ed25519_verify(
                                            &sig_data.signature,
                                            hint.digest(),
                                            &payload,
                                        ) {
                                            Some(hint)
                                        } else {
                                            None
                                        }
                                    }
                                    _ => existing_writers.iter().copied().find(|w| {
                                        crate::env::ed25519_verify(
                                            &sig_data.signature,
                                            w.digest(),
                                            &payload,
                                        )
                                    }),
                                };
                                if signer.is_none() {
                                    return Err(StorageError::InvalidSignature);
                                }

                                // Replay protection (strict `<=` Err, as Shared).
                                let new_nonce = sig_data.nonce;
                                let last_nonce = *existing_metadata.updated_at;
                                if new_nonce <= last_nonce {
                                    let placeholder = existing_writers
                                        .iter()
                                        .copied()
                                        .next()
                                        .unwrap_or_else(|| [0u8; 32].into());
                                    return Err(StorageError::NonceReplay(Box::new((
                                        placeholder,
                                        new_nonce,
                                    ))));
                                }
                            }
                            _ => {
                                // Action metadata is not SharedMember, but existing is.
                                return Err(StorageError::InvalidSignature);
                            }
                        }
                    }
                    StorageType::Public => { /* No special checks */ }
                }
            }
            Action::Compare { .. } => { /* No checks needed */ }
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
                debug!(
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

                // Save data (might merge, producing different hash)
                let Some((_, _full_hash)) = Self::save_internal(id, &data, metadata.clone())?
                else {
                    debug!(
                        %id,
                        "Remote action produced no storage change (save_internal returned None)"
                    );
                    // save_internal short-circuited because stored.updated_at >
                    // incoming.updated_at: nothing changed locally, but the
                    // apply still "happened" from the network's perspective —
                    // we received and acknowledged this delta. Push
                    // Action::Compare so this node's current Merkle state for
                    // `id` ships in the next outbound delta and peers can
                    // reconcile via state-based sync. Without this, two nodes
                    // that concurrently merge the same entity (each holding
                    // the locally-newer side) keep emitting deltas the other
                    // drops, and the Merkle root divergence stalls until an
                    // unrelated trigger forces a hash-comparison sweep.
                    if S::participates_in_sync() {
                        crate::delta::push_action(Action::Compare { id });
                    }
                    return Ok(());
                };

                debug!(
                    %id,
                    ancestor_count = ancestors.len(),
                    "Applied Add/Update action to storage"
                );

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

                if S::participates_in_sync() {
                    crate::delta::push_action(Action::Compare { id });
                    debug!(%id, "Queued compare action after apply");
                }
            }
            Action::Compare { .. } => {
                return Err(StorageError::ActionNotAllowed("Compare".to_owned()))
            }
            Action::DeleteRef { id, deleted_at, .. } => {
                Self::apply_delete_ref_action(id, deleted_at)?;
            }
        };

        debug!("Interface::apply_action completed");

        Ok(())
    }

    /// Applies DeleteRef action with CRDT conflict resolution.
    ///
    /// Uses guard clauses for clarity (KISS principle).
    /// Handles three cases:
    /// 1. Already deleted - update tombstone if newer
    /// Append to the rotation log when applying a Shared rotation.
    ///
    /// Rotation-log write hook (#2233 phase 3). Called from
    /// [`apply_action`] after `save_internal` succeeds. No-op for
    /// non-Shared actions, for value-writes (writers unchanged), or
    /// when ctx lacks the delta id/hlc the entry needs.
    ///
    /// `pre_apply_writers` is the writer set in the index *before* this
    /// action mutated it — `Some` for an existing Shared entity, `None`
    /// for bootstrap (first time we see this entity). Bootstrap counts as
    /// the first rotation.
    ///
    /// Skips silently rather than erroring on missing context.
    /// Empty-ctx callers (snapshot leaf push, local apply, the
    /// `StorageDelta::Actions` artifact path) are not network-received
    /// causal deltas and don't have an originating `CausalDelta` to
    /// record. Network-sync deltas arrive via
    /// [`StorageDelta::CausalActions`](crate::delta::StorageDelta::CausalActions)
    /// (per #2266) which populates `delta_id`/`delta_hlc`, lighting up
    /// the hook in production.
    ///
    /// # Caller invariant
    ///
    /// Must not be called twice for the same entity within one delta —
    /// see [`rotation_log::append`](crate::rotation_log::append) for why
    /// (delta_id-only dedup). Multi-action deltas with two rotations on
    /// the same entity are not supported: a second call with differing
    /// entry contents returns
    /// [`StorageError::DuplicateRotationInDelta`](crate::error::StorageError::DuplicateRotationInDelta);
    /// a replay with identical contents is idempotent.
    ///
    /// # Log may diverge from stored state
    ///
    /// This hook fires right after signature verification but BEFORE the
    /// `save_internal` apply branch, so it records every signature-verified
    /// rotation regardless of whether `save_internal` later drops the data
    /// write under v2's LWW-by-HLC. This is intentional — cross-node
    /// convergence (P5) requires the rotation log to reflect *received
    /// causal facts*, not the local node's storage-merge decisions. The
    /// consequence is that `RotationLog::entries` may contain rotations
    /// whose data write was dropped; downstream readers (`writers_at`,
    /// future P6 compaction, audit tools) must treat the log as the
    /// authoritative writer-set history independent of stored data.
    fn maybe_append_rotation_log(
        id: Id,
        metadata: &Metadata,
        ctx: &ApplyContext,
        pre_apply_writers: Option<BTreeSet<PublicKey>>,
    ) -> Result<(), StorageError> {
        // Only Shared entities have a rotation log.
        let StorageType::Shared {
            writers,
            signature_data,
        } = &metadata.storage_type
        else {
            return Ok(());
        };

        // Only append on rotation: bootstrap (no prior entry) OR writers changed.
        // Value-writes that don't touch the writer set don't need to log.
        let is_rotation = match &pre_apply_writers {
            Some(stored) => stored != writers,
            None => true,
        };
        if !is_rotation {
            return Ok(());
        }

        // Need the originating delta's identity to record an entry the
        // node-side reader can later look up. Empty-ctx callers (snapshot
        // leaf push, local apply, `StorageDelta::Actions`) pass None here
        // and the hook silently no-ops; only `StorageDelta::CausalActions`
        // (#2266) populates these and lights up the hook.
        let (Some(delta_id), Some(delta_hlc)) = (ctx.delta_id, ctx.delta_hlc) else {
            return Ok(());
        };

        let signer = signature_data.as_ref().and_then(|s| s.signer);
        let nonce = signature_data.as_ref().map(|s| s.nonce).unwrap_or(0);

        crate::rotation_log::append::<S>(
            id,
            crate::rotation_log::RotationLogEntry {
                delta_id,
                delta_hlc,
                signer,
                new_writers: writers.clone(),
                writers_nonce: nonce,
            },
        )?;
        debug!(
            target: "storage::p3_write_hook",
            %id,
            writer_count = writers.len(),
            "Rotation log entry appended"
        );
        Ok(())
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

        // Guard: Local update is newer, deletion loses
        if deleted_at < *metadata.updated_at {
            // Local update wins, ignore older deletion
            return Ok(());
        }

        // Get parent ID BEFORE deleting - we need it to update the Merkle tree
        let parent_id = <Index<S>>::get_parent_id(id)?;

        // Deletion wins - apply it
        let _ignored = S::storage_remove(Key::Entry(id));
        let _ignored = <Index<S>>::mark_deleted(id, deleted_at);

        // CRITICAL: Update parent's children list and recalculate hashes
        // Without this, the receiving node would have a different root hash than
        // the node that performed the deletion locally.
        if let Some(parent_id) = parent_id {
            // Remove child from parent's children list and recalculate hashes
            <Index<S>>::update_parent_after_child_removal(parent_id, id)?;
            <Index<S>>::recalculate_ancestor_hashes_for(parent_id)?;
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

    /// Compares local and remote entity trees, generating sync actions.
    ///
    /// Compares Merkle hashes recursively, producing action lists for both sides.
    /// Returns `(local_actions, remote_actions)` to bring trees into sync.
    ///
    /// # Errors
    /// Returns error if index lookup or hash comparison fails.
    ///
    pub fn compare_trees(
        foreign_entity_data: Option<Vec<u8>>,
        foreign_index_data: ComparisonData,
    ) -> Result<(Vec<Action>, Vec<Action>), StorageError> {
        let mut actions = (vec![], vec![]);

        let id = foreign_index_data.id;

        let local_metadata = <Index<S>>::get_metadata(id)?;

        let Some(local_entity) = Self::find_by_id_raw(id) else {
            if let Some(foreign_entity) = foreign_entity_data {
                // Local entity doesn't exist, so we need to add it
                actions.0.push(Action::Add {
                    id,
                    data: foreign_entity,
                    ancestors: foreign_index_data.ancestors,
                    metadata: foreign_index_data.metadata,
                });
            }

            return Ok(actions);
        };

        let local_metadata = local_metadata.ok_or(StorageError::IndexNotFound(id))?;

        let (local_full_hash, local_own_hash) =
            <Index<S>>::get_hashes_for(id)?.ok_or(StorageError::IndexNotFound(id))?;

        // Compare full Merkle hashes
        if local_full_hash == foreign_index_data.full_hash {
            return Ok(actions);
        }

        // Compare own hashes and timestamps
        if local_own_hash != foreign_index_data.own_hash {
            match foreign_entity_data {
                Some(foreign_entity_data)
                    if local_metadata.updated_at <= foreign_index_data.metadata.updated_at =>
                {
                    actions.0.push(Action::Update {
                        id,
                        data: foreign_entity_data,
                        ancestors: foreign_index_data.ancestors,
                        metadata: foreign_index_data.metadata,
                    });
                }
                _ => {
                    actions.1.push(Action::Update {
                        id,
                        data: local_entity,
                        ancestors: <Index<S>>::get_ancestors_of(id)?,
                        metadata: local_metadata,
                    });
                }
            }
        }

        // The list of collections from the type will be the same on both sides, as
        // the type is the same.

        let local_collection_names = <Index<S>>::get_collection_names_for(id)?;

        let local_collections = local_collection_names
            .into_iter()
            .map(|name| {
                let children = <Index<S>>::get_children_of(id)?;
                Ok((name, children))
            })
            .collect::<Result<BTreeMap<_, _>, StorageError>>()?;

        // Compare children
        for (local_coll_name, local_children) in &local_collections {
            if let Some(foreign_children) = foreign_index_data.children.get(local_coll_name) {
                let local_child_map: IndexMap<_, _> = local_children
                    .iter()
                    .map(|child| (child.id(), child.merkle_hash()))
                    .collect();
                let foreign_child_map: IndexMap<_, _> = foreign_children
                    .iter()
                    .map(|child| (child.id(), child.merkle_hash()))
                    .collect();

                for (child_id, local_hash) in &local_child_map {
                    match foreign_child_map.get(child_id) {
                        Some(foreign_hash) if local_hash != foreign_hash => {
                            actions.0.push(Action::Compare { id: *child_id });
                            actions.1.push(Action::Compare { id: *child_id });
                        }
                        None => {
                            if let Some(local_child) = Self::find_by_id_raw(*child_id) {
                                let metadata = <Index<S>>::get_metadata(*child_id)?
                                    .ok_or(StorageError::IndexNotFound(*child_id))?;

                                actions.1.push(Action::Add {
                                    id: *child_id,
                                    data: local_child,
                                    // Ancestors of the entity being added (the
                                    // child), not its parent `id` — apply
                                    // rebuilds the path down to `*child_id` and
                                    // links it under `ancestors[0]` (its
                                    // immediate parent). Using `id` here dropped
                                    // the immediate parent from the chain,
                                    // orphaning the child on the receiver (the
                                    // "collection entirely missing" arm below
                                    // already does this correctly).
                                    ancestors: <Index<S>>::get_ancestors_of(*child_id)?,
                                    metadata,
                                });
                            }
                        }
                        // Hashes match, no action needed
                        _ => {}
                    }
                }

                for id in foreign_child_map.keys() {
                    if !local_child_map.contains_key(id) {
                        // Child exists in foreign but not locally, compare.
                        // We can't get the full data for the foreign child, so we flag it for
                        // comparison.
                        actions.1.push(Action::Compare { id: *id });
                    }
                }
            } else {
                // The entire collection is missing from the foreign entity
                for child in local_children {
                    if let Some(local_child) = Self::find_by_id_raw(child.id()) {
                        let metadata = <Index<S>>::get_metadata(child.id())?
                            .ok_or(StorageError::IndexNotFound(child.id()))?;

                        actions.1.push(Action::Add {
                            id: child.id(),
                            data: local_child,
                            ancestors: <Index<S>>::get_ancestors_of(child.id())?,
                            metadata,
                        });
                    }
                }
            }
        }

        // Check for collections in the foreign entity that don't exist locally
        for (foreign_coll_name, foreign_children) in &foreign_index_data.children {
            if !local_collections.contains_key(foreign_coll_name) {
                for child in foreign_children {
                    // We can't get the full data for the foreign child, so we flag it for comparison
                    actions.1.push(Action::Compare { id: child.id() });
                }
            }
        }

        Ok(actions)
    }

    /// Compares entities and automatically applies sync actions locally.
    ///
    /// Convenience wrapper around [`compare_trees()`](Self::compare_trees()) that applies
    /// local actions immediately and pushes remote actions to sync queue.
    ///
    /// # Errors
    /// Returns error if comparison or action application fails.
    ///
    pub fn compare_affective(
        data: Option<Vec<u8>>,
        comparison_data: ComparisonData,
        ctx: &ApplyContext,
    ) -> Result<(), StorageError> {
        let (local, remote) = <Interface<S>>::compare_trees(data, comparison_data)?;

        for action in local {
            if let Action::Compare { .. } = &action {
                continue;
            }

            <Interface<S>>::apply_action(action, ctx)?;
        }

        if S::participates_in_sync() {
            for action in remote {
                crate::delta::push_action(action);
            }
        }

        Ok(())
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
        // Check if entity is deleted (tombstone)
        if <Index<S>>::is_deleted(id)? {
            return Ok(None); // Entity is deleted
        }

        let value = S::storage_read(Key::Entry(id));

        let Some(slice) = value else {
            return Ok(None);
        };

        let mut item = from_slice::<D>(&slice).map_err(StorageError::DeserializationError)?;

        // Single `EntityIndex` read for both merkle_hash and metadata.
        let index = <Index<S>>::get_index(id)?.ok_or(StorageError::IndexNotFound(id))?;
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

    /// Generates comparison metadata for tree synchronization.
    ///
    /// Includes hashes, ancestors, children info. Used by [`compare_trees()`](Self::compare_trees()).
    ///
    /// # Errors
    /// Returns error if index lookup fails.
    ///
    pub fn generate_comparison_data(id: Option<Id>) -> Result<ComparisonData, StorageError> {
        let id = id.unwrap_or_else(Id::root);

        let (full_hash, own_hash) = <Index<S>>::get_hashes_for(id)?.unwrap_or_default();

        let metadata = <Index<S>>::get_metadata(id)?.unwrap_or_default();

        let ancestors = <Index<S>>::get_ancestors_of(id)?;

        let collection_names = <Index<S>>::get_collection_names_for(id)?;

        let children = collection_names
            .into_iter()
            .map(|collection_name| {
                <Index<S>>::get_children_of(id).map(|children| (collection_name.clone(), children))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;

        Ok(ComparisonData {
            id,
            own_hash,
            full_hash,
            ancestors,
            children,
            metadata,
        })
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
    /// # Errors
    /// Returns error if parent or child doesn't exist.
    ///
    pub fn remove_child_from(parent_id: Id, child_id: Id) -> Result<bool, StorageError> {
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

        // If this is a local user action, set the nonce
        if let StorageType::User { owner, .. } = metadata.storage_type {
            if *owner == crate::env::executor_id() {
                // Use the deletion timestamp as the nonce
                metadata.storage_type = StorageType::User {
                    owner,
                    signature_data: Some(SignatureData {
                        signature: [0; 64], // Placeholder, added by signer
                        nonce: deleted_at,
                        signer: None, // owner is already known for User
                    }),
                };
            }
        }

        // If this is a local shared action by a writer, set the nonce.
        // Note: unlike save_raw, here `metadata` was just loaded from the index
        // a few lines above and represents the current stored state. There's
        // no separate "claimed" set to union against — the executor must be
        // in the stored writer set to authorize the delete.
        let shared_to_stamp = if let StorageType::Shared {
            writers: stored, ..
        } = &metadata.storage_type
        {
            let executor: calimero_primitives::identity::PublicKey =
                crate::env::executor_id().into();
            if stored.contains(&executor) {
                Some((stored.clone(), executor))
            } else {
                None
            }
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
                let executor: calimero_primitives::identity::PublicKey =
                    crate::env::executor_id().into();
                let writers = Self::resolve_anchor_writers(*anchor);
                if writers.contains(&executor) {
                    Some((*anchor, executor))
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

        <Index<S>>::remove_child_from(parent_id, child_id)?;

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

            let data = to_vec(&root).map_err(|e| StorageError::SerializationError(e.into()))?;

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

        let data = to_vec(entity).map_err(|e| StorageError::SerializationError(e.into()))?;

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

        // Compute incoming data hash for tracing
        let incoming_hash: [u8; 32] = Sha256::digest(data).into();

        let last_metadata = <Index<S>>::get_metadata(id)?;
        let final_data = if let Some(last_metadata) = &last_metadata {
            if last_metadata.updated_at > metadata.updated_at {
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
                if let Some(existing_data) = S::storage_read(Key::Entry(id)) {
                    let existing_hash: [u8; 32] = Sha256::digest(&existing_data).into();
                    info!(
                        target: "storage::root_merge",
                        %id,
                        existing_len = existing_data.len(),
                        existing_hash = %hex::encode(&existing_hash),
                        incoming_len = data.len(),
                        incoming_hash = %hex::encode(&incoming_hash),
                        existing_created_at = last_metadata.created_at,
                        existing_updated_at = *last_metadata.updated_at,
                        incoming_updated_at,
                        "ROOT MERGE: Starting CRDT merge for root entity"
                    );
                    let merged = Self::try_merge_data(
                        id,
                        &existing_data,
                        data,
                        last_metadata.created_at,
                        *last_metadata.updated_at,
                        *metadata.updated_at,
                    )?;
                    let merged_hash: [u8; 32] = Sha256::digest(&merged).into();
                    info!(
                        target: "storage::root_merge",
                        %id,
                        merged_len = merged.len(),
                        merged_hash = %hex::encode(&merged_hash),
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
                        incoming_hash = %hex::encode(&incoming_hash),
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
                info!(
                    target: "storage::root_merge",
                    %id,
                    incoming_len = data.len(),
                    incoming_hash = %hex::encode(&incoming_hash),
                    "ROOT MERGE: First time creating root entity"
                );
                <Index<S>>::add_root(ChildInfo::new(id, [0_u8; 32], metadata.clone()))?;
            }
            data.to_vec()
        };

        let own_hash: [u8; 32] = Sha256::digest(&final_data).into();

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
        //     caller (`compare_trees` and its sync-layer cousins in
        //     `hash_comparison{,_protocol}.rs`, `level_sync.rs`)
        //     reaches `find_by_id_raw` only after iterating a parent's
        //     index-derived child list — and the orphan's id is, by
        //     definition, not in any parent's index. `compare_trees`
        //     called directly with the orphan's id also bails: line
        //     1525 returns `IndexNotFound` from the `local_metadata`
        //     check before the orphan bytes can become an `Action`.
        //   * The next successful `apply_action` for the same id
        //     overwrites the orphan bytes, so the storage cost is
        //     transient.
        // The pre-fix ordering (index-then-entry) had the symmetric
        // problem with much worse user-visible behavior — the rga
        // "Hello Wor" flake described above — because the read path
        // *does* propagate index-advertised entries through every
        // production caller, so a "hash exists, bytes don't"
        // inconsistency surfaces immediately as a wrong-content read.
        let full_hash = <Index<S>>::update_hash_for(id, own_hash, Some(metadata.updated_at))?;

        if id.is_root() {
            info!(
                target: "storage::root_merge",
                %id,
                own_hash = %hex::encode(&own_hash),
                full_hash = %hex::encode(&full_hash),
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
        let full_hash = <Index<S>>::update_hash_for(id, own_hash, Some(metadata.updated_at))?;
        Ok(full_hash)
    }

    /// Attempt to merge two versions of data using CRDT semantics.
    ///
    /// Returns the merged data, or an error if merge fails.
    /// Merge mode is enabled to prevent timestamp generation during merge operations.
    ///
    /// # Errors
    ///
    /// Returns `StorageError::MergeFailure` if no merge function is registered
    /// for the root entity type. This enforces I5 (No Silent Data Loss) by failing
    /// loudly rather than silently falling back to LWW.
    fn try_merge_data(
        _id: Id,
        existing: &[u8],
        incoming: &[u8],
        existing_created_at: u64,
        existing_timestamp: u64,
        incoming_timestamp: u64,
    ) -> Result<Vec<u8>, StorageError> {
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

        // I5 Enforcement: Propagate merge errors instead of falling back to LWW.
        // If no merge function is registered and the entity isn't in the
        // bootstrap-default state, this prevents silent data loss.
        // The MergeError is preserved for programmatic error handling.
        result.map_err(StorageError::from)
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
            let is_lww = matches!(crdt_type, CrdtType::LwwRegister { .. });
            if is_lww {
                return Ok(lww_pick(existing, incoming));
            }

            let result =
                crate::env::with_merge_mode(|| merge_by_crdt_type(crdt_type, existing, incoming));

            match result {
                Ok(merged) => {
                    debug!(
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
            if *owner == crate::env::executor_id() {
                let nonce = *metadata.updated_at;
                metadata.storage_type = StorageType::User {
                    owner,
                    signature_data: Some(SignatureData {
                        signature: [0; 64], // Placeholder, added by signer
                        nonce,
                        signer: None, // owner is already known for User
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
        //
        // Stamping authority is the union of (stored writers) and (action's claimed writers):
        //   - Stored: the writer set as currently persisted in the index.
        //   - Claimed: the writer set in the action's own metadata.
        // Stamp if the executor is in EITHER. This is what enables rotate-self-out:
        // a writer rotating themselves out has executor ∈ stored but ∉ claimed; the
        // verifier on remote also uses stored, so the signature still verifies there.
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
            let executor: calimero_primitives::identity::PublicKey =
                crate::env::executor_id().into();
            let stored_has_executor = <Index<S>>::get_metadata(id)?
                .as_ref()
                .map(|m| match &m.storage_type {
                    StorageType::Shared { writers, .. } => writers.contains(&executor),
                    _ => false,
                })
                .unwrap_or(false);
            let claimed_has_executor = claimed_writers.contains(&executor);
            if stored_has_executor || claimed_has_executor {
                Some((claimed_writers.clone(), executor))
            } else {
                None
            }
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
                let executor: calimero_primitives::identity::PublicKey =
                    crate::env::executor_id().into();
                if Self::resolve_anchor_writers(*anchor).contains(&executor) {
                    Some((*anchor, executor))
                } else {
                    None
                }
            } else {
                None
            };
        if let Some((anchor, signer)) = member_to_stamp {
            let nonce = *metadata.updated_at;
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

    /// Validates Merkle tree integrity.
    ///
    /// **Note**: Not yet implemented.
    ///
    /// # Errors
    /// Currently panics (unimplemented).
    ///
    pub fn validate() -> Result<(), StorageError> {
        unimplemented!()
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
        Action::Compare { .. } => return Ok(()),
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
