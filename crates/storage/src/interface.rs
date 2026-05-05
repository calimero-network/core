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
    /// Adds a child entity to a parent's collection.
    ///
    /// Updates Merkle hashes and generates sync actions automatically.
    ///
    /// # Errors
    /// - `SerializationError` if child can't be encoded
    /// - `IndexNotFound` if parent doesn't exist
    ///
    pub fn add_child_to<D: Data>(parent_id: Id, child: &mut D) -> Result<bool, StorageError> {
        if !child.element().is_dirty() {
            return Ok(false);
        }

        let data = to_vec(child).map_err(|e| StorageError::SerializationError(e.into()))?;

        let own_hash = Sha256::digest(&data).into();

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

                        // Replay protection check
                        let new_nonce = sig_data.nonce;
                        let last_nonce = <Index<S>>::get_metadata(*id)?
                            .map(|m| *m.updated_at)
                            .unwrap_or(0);

                        if new_nonce <= last_nonce {
                            return Err(StorageError::NonceReplay(Box::new((*owner, new_nonce))));
                        }

                        let verification_result = crate::env::ed25519_verify(
                            &sig_data.signature,
                            owner.digest(),
                            &payload,
                        );

                        if !verification_result {
                            return Err(StorageError::InvalidSignature);
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
                        // P3 keeps the nonce check unchanged. Per the epic exit
                        // criterion, the nonce check is removed only after weeks
                        // of zero-divergence telemetry between nonce + DAG-causal.
                        // Tests that need to validate the v3 target behavior
                        // (post-nonce-removal) can opt out via the test-only
                        // [`disable_nonce_check_for_testing`] hook.
                        let new_nonce = sig_data.nonce;
                        let last_nonce =
                            stored_metadata.as_ref().map(|m| *m.updated_at).unwrap_or(0);
                        let skip_nonce = nonce_check_disabled_for_testing();
                        if !skip_nonce && new_nonce <= last_nonce {
                            // Use the first authoritative writer as a placeholder identity
                            // for the error since the signer hasn't been identified yet.
                            let placeholder = authoritative_writers
                                .iter()
                                .copied()
                                .next()
                                .unwrap_or_else(|| [0u8; 32].into());
                            return Err(StorageError::NonceReplay(Box::new((
                                placeholder,
                                new_nonce,
                            ))));
                        }

                        // Identify the signer.
                        // Fast path: if the action carries a `signer` hint and that
                        // signer is in the authoritative set, do exactly one verify.
                        // Slow path (no hint): linear scan over authoritative writers.
                        //
                        // Per the #2233 epic compatibility rule, the signer hint is
                        // validated against the *causal* writer set above, not stored —
                        // that's already how it works here since `authoritative_writers`
                        // is now the DAG-causal answer when available.
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

                        // P3 of #2233: rotation-log write hook.
                        //
                        // Fires here — right after signature verification, BEFORE
                        // the apply branch — so the log captures every
                        // signature-verified Shared rotation regardless of
                        // whether `save_internal` later chooses to skip the
                        // write under v2's LWW-by-HLC. Cross-node convergence
                        // (P5) depends on this: the rotation log must reflect
                        // *received causal facts*, not the local node's
                        // storage-merge decisions.
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
                    StorageType::Public => { /* No special checks */ }
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

                                // TODO: refactor to a separate function.
                                // Replay protection check
                                let new_nonce = sig_data.nonce;
                                // The nonce is the `deleted_at` time. We check it against the
                                // last `updated_at` time stored in the index.
                                let last_nonce = *existing_metadata.updated_at;

                                if new_nonce <= last_nonce {
                                    return Err(StorageError::NonceReplay(Box::new((
                                        *owner, new_nonce,
                                    ))));
                                }

                                let payload = action.payload_for_signing();
                                let verification_result = crate::env::ed25519_verify(
                                    &sig_data.signature,
                                    owner.digest(),
                                    &payload,
                                );

                                if !verification_result {
                                    return Err(StorageError::InvalidSignature);
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

                                // Replay protection (per-entity monotonic nonce).
                                // Done BEFORE per-writer Ed25519 scan so replays are
                                // O(1)-rejected (matches User arm + upsert arm).
                                //
                                // Mirrors the upsert arm's [`disable_nonce_check_for_testing`]
                                // bypass so DAG-causal P5 tests that exercise out-of-order
                                // delivery of Shared deletes can opt into the v3 target
                                // behavior. Production codepath unchanged — the const fn
                                // returns false outside cfg(test).
                                let new_nonce = sig_data.nonce;
                                let last_nonce = *existing_metadata.updated_at;
                                let skip_nonce = nonce_check_disabled_for_testing();
                                if !skip_nonce && new_nonce <= last_nonce {
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
                            }
                            _ => {
                                // Action metadata is not Shared, but existing is.
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

                // For new entities, create a minimal index entry first to avoid orphan errors
                if !<Index<S>>::has_index(id) {
                    if id.is_root() {
                        debug!(%id, "Creating root index entry for entity");
                        <Index<S>>::add_root(ChildInfo::new(id, [0; 32], metadata.clone()))?;
                    } else if let Some(parent) = parent {
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
                    }
                }

                // Save data (might merge, producing different hash)
                let Some((_, _full_hash)) = Self::save_internal(id, &data, metadata.clone())?
                else {
                    debug!(
                        %id,
                        "Remote action produced no storage change (save_internal returned None)"
                    );
                    // we didn't save anything, so we skip updating the ancestors
                    return Ok(());
                };

                debug!(
                    %id,
                    ancestor_count = ancestors.len(),
                    "Applied Add/Update action to storage"
                );

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

                crate::delta::push_action(Action::Compare { id });
                debug!(%id, "Queued compare action after apply");
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
                                    ancestors: <Index<S>>::get_ancestors_of(id)?,
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

        for action in remote {
            crate::delta::push_action(action);
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

        <Index<S>>::remove_child_from(parent_id, child_id)?;

        // Use DeleteRef for efficient tombstone-based deletion.
        // More efficient than Delete: only sends ID + timestamp + metadata vs full ancestor tree.
        // The tombstone is created by remove_child_from, we just broadcast the deletion.
        crate::delta::push_action(Action::DeleteRef {
            id: child_id,
            deleted_at,
            // Pass the full metadata
            metadata,
        });

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
        let incoming_updated_at = metadata.updated_at();

        // Compute incoming data hash for tracing
        let incoming_hash: [u8; 32] = Sha256::digest(data).into();

        let last_metadata = <Index<S>>::get_metadata(id)?;
        let final_data = if let Some(last_metadata) = &last_metadata {
            if last_metadata.updated_at > metadata.updated_at {
                return Ok(None);
            } else if id.is_root() {
                // Root entity (app state) - ALWAYS merge using CRDT semantics.
                // The Mergeable impl (auto-generated by #[app::state]) handles:
                // - Counter: G-Counter merge (union of contributions from all nodes)
                // - UnorderedMap: Per-key LWW with proper timestamp handling
                // - Other CRDTs: Their respective merge logic
                //
                // This enables handlers to increment counters independently on each node,
                // with all contributions properly merged during sync.
                if let Some(existing_data) = S::storage_read(Key::Entry(id)) {
                    let existing_hash: [u8; 32] = Sha256::digest(&existing_data).into();
                    info!(
                        target: "storage::root_merge",
                        %id,
                        existing_len = existing_data.len(),
                        existing_hash = %hex::encode(&existing_hash),
                        incoming_len = data.len(),
                        incoming_hash = %hex::encode(&incoming_hash),
                        existing_updated_at = *last_metadata.updated_at,
                        incoming_updated_at,
                        "ROOT MERGE: Starting CRDT merge for root entity"
                    );
                    let merged = Self::try_merge_data(
                        id,
                        &existing_data,
                        data,
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

        _ = S::storage_write(Key::Entry(id), &final_data);

        let is_new = metadata.created_at == *metadata.updated_at;

        Ok(Some((is_new, full_hash)))
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
        existing_timestamp: u64,
        incoming_timestamp: u64,
    ) -> Result<Vec<u8>, StorageError> {
        use crate::merge::merge_root_state;

        // Attempt CRDT merge with merge mode enabled
        // This prevents timestamp generation during merge to ensure deterministic hashes
        let result = crate::env::with_merge_mode(|| {
            merge_root_state(existing, incoming, existing_timestamp, incoming_timestamp)
        });

        // I5 Enforcement: Propagate merge errors instead of falling back to LWW.
        // If no merge function is registered, this prevents silent data loss.
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

        // Check if we have CRDT type metadata
        let Some(crdt_type) = &metadata.crdt_type else {
            // Legacy data - no CRDT type, use LWW
            debug!(
                target: "storage::merge",
                %id,
                "No CRDT type metadata, falling back to LWW"
            );
            return Ok(if incoming_timestamp >= existing_timestamp {
                incoming.to_vec()
            } else {
                existing.to_vec()
            });
        };

        // For built-in types, merge in storage layer
        if is_builtin_crdt(crdt_type) {
            // LwwRegister's merge_by_crdt_type always returns incoming; the
            // actual last-writer-wins comparison must happen here using the
            // HLC timestamps carried in metadata.
            let is_lww = matches!(crdt_type, CrdtType::LwwRegister { .. });
            if is_lww {
                return Ok(if incoming_timestamp >= existing_timestamp {
                    incoming.to_vec()
                } else {
                    existing.to_vec()
                });
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

        // Fall back to LWW
        Ok(if incoming_timestamp >= existing_timestamp {
            incoming.to_vec()
        } else {
            existing.to_vec()
        })
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
        // If this is a local user action, set the nonce
        if let StorageType::User {
            owner,
            signature_data,
        } = metadata.storage_type
        {
            if *owner == crate::env::executor_id() && signature_data.is_none() {
                // This is a new local action. Set the nonce.
                // Use the `updated_at` timestamp as the nonce.
                let nonce = *metadata.updated_at;
                metadata.storage_type = StorageType::User {
                    owner,
                    signature_data: Some(SignatureData {
                        signature: [0; 64], // Placeholder, added by signer
                        nonce,
                        signer: None, // owner is already known for User
                    }),
                };
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
        let shared_to_stamp = if let StorageType::Shared {
            writers: claimed_writers,
            signature_data,
        } = &metadata.storage_type
        {
            if signature_data.is_none() {
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
        }

        let Some((is_new, full_hash)) = Self::save_internal(id, &data, metadata.clone())? else {
            return Ok(None);
        };

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

        crate::delta::push_action(action);

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

    /// Helper to verify a new `Update` action.
    fn verify_action_update(action: &Action) -> Result<(), StorageError> {
        let (metadata, _data, id) = match action {
            Action::Update {
                metadata, data, id, ..
            } => (metadata, data, *id),
            // Should not happen
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
