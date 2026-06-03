//! State-delta HLC fence (PR-3 / PR-6b O3): decide what to do with a delta
//! produced under a different app schema than the receiver can currently read.
//!
//! PR-3 keyed the fence on the replicated `GroupMeta.app_key` (the *target*
//! schema the group is migrating toward) and silently *dropped* any
//! after-boundary stale-schema delta. PR-6b reworks this on two axes:
//!
//! 1. **O3 — fence on the LOADED reader version, not `GroupMeta.app_key`.**
//!    Under LazyOnAccess the governance `GroupMeta.app_key` advances to the new
//!    schema for *all* members at cascade-apply, but each node's wasm binary
//!    swaps lazily (on next execute). A node still on the v1 binary would see
//!    `target == v2` and mis-fence. The decision must key on the schema the
//!    receiver can *actually read right now* — its loaded `ApplicationMeta`
//!    bytecode blob_id.
//! 2. **Absorb-don't-drop.** A delta the receiver cannot read *yet* (because its
//!    binary hasn't caught up) is `Buffer`ed for later verbatim replay, never
//!    dropped. Dropping is reserved for genuinely unrecoverable cases.
//!
//! This module owns the pure decision rule ([`fence_decision`]) and the
//! store-aware resolver ([`delta_fence_decision`]) that derives the loaded
//! reader key. Persisting / replaying the buffered delta is Task 6b.4 / 6b.5.

use calimero_governance_store::{get_group_for_context, MetaRepository, UpgradesRepository};
use calimero_primitives::context::ContextId;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::{key, Store};

/// What the receiver should do with an incoming state delta after evaluating the
/// schema fence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FenceDecision {
    /// The receiver can read the delta now (matching schema, pre-cascade
    /// history, or no active cascade boundary) — apply it normally.
    Apply,
    /// The receiver cannot read the delta yet — its loaded binary is behind the
    /// incoming schema. Buffer (absorb) it for verbatim replay once the binary
    /// catches up. Never dropped.
    Buffer,
    /// The delta is unrecoverable for this receiver and must be discarded.
    /// Reserved for non-absorbable cases; the migration fence never emits this.
    Drop,
}

/// Pure decision rule. `cascade_hlc == None` ⇒ no active migration ⇒ `Apply`.
///
/// Returns:
/// - [`FenceDecision::Apply`] when the receiver can read the delta now:
///   - there is no cascade boundary (`cascade_hlc == None`), OR
///   - the delta is at-or-before the boundary (`delta_hlc <= boundary`) — it is
///     pre-cascade legitimate history, OR
///   - the delta's schema matches the receiver's loaded reader
///     (`incoming_app_key == loaded_app_key`).
/// - [`FenceDecision::Buffer`] when the delta is *after* the boundary AND its
///   schema differs from the loaded reader — the receiver cannot read it yet,
///   so it is absorbed for later verbatim replay (never dropped).
///
/// `target_app_key` (the replicated `GroupMeta.app_key`) is retained only to
/// describe the migration target; it is *not* used to gate readability — that
/// is what the O3 fix corrects. It is threaded here so the drain can later tell
/// when the binary has caught up to the target.
#[must_use]
pub fn fence_decision(
    incoming_app_key: [u8; 32],
    loaded_app_key: [u8; 32],
    _target_app_key: [u8; 32],
    delta_hlc: HybridTimestamp,
    cascade_hlc: Option<HybridTimestamp>,
) -> FenceDecision {
    let Some(boundary) = cascade_hlc else {
        // No active cascade boundary — never fence.
        return FenceDecision::Apply;
    };

    if delta_hlc <= boundary {
        // Pre-cascade legitimate history — must apply regardless of schema.
        return FenceDecision::Apply;
    }

    if incoming_app_key == loaded_app_key {
        // The receiver's loaded binary can read this schema now.
        return FenceDecision::Apply;
    }

    // After the boundary, schema the loaded reader can't read — absorb it for
    // verbatim replay once the binary advances. Never drop.
    FenceDecision::Buffer
}

/// Pure two-condition rule (PR-3 compatibility shim).
///
/// Retained for callers / tests that only need the boolean "is this delta
/// fenced (i.e. not applied)?" answer with the legacy semantics where the
/// loaded reader equals the context's current schema. Delegates to
/// [`fence_decision`] with `loaded == target == ctx_app_key`.
///
/// Returns `true` iff the delta would NOT be applied (i.e. it is `Buffer` or
/// `Drop`).
#[must_use]
pub fn should_fence(
    delta_app_key: [u8; 32],
    ctx_app_key: [u8; 32],
    delta_hlc: HybridTimestamp,
    cascade_hlc: Option<HybridTimestamp>,
) -> bool {
    !matches!(
        fence_decision(
            delta_app_key,
            ctx_app_key,
            ctx_app_key,
            delta_hlc,
            cascade_hlc,
        ),
        FenceDecision::Apply
    )
}

/// Resolve the **loaded reader** app_key for a context: the bytecode blob_id of
/// the `ApplicationMeta` the context currently has installed locally.
///
/// This is the schema-version discriminator the receiver can *actually read
/// right now* — distinct from the replicated `GroupMeta.app_key` (the migration
/// target) under LazyOnAccess, where the governance target advances ahead of the
/// locally-loaded binary (O3).
///
/// Resolution: `ContextMeta.application` (the loaded `ApplicationMeta` key) →
/// load that row → `bytecode.blob_id()`. This is the same blob_id
/// `upgrade_group.rs` writes as `GroupMeta.app_key`, so no extra marker row is
/// needed. Falls back to `GroupMeta.app_key` only when the loaded `ContextMeta`
/// / `ApplicationMeta` row is missing.
///
/// Returns `None` for non-group contexts (no owning group) and when neither the
/// loaded application nor the group meta can supply a key. Store errors are
/// propagated as `Err`.
pub fn loaded_reader_app_key(
    store: &Store,
    context_id: &ContextId,
) -> eyre::Result<Option<[u8; 32]>> {
    // Primary: the locally-loaded application's bytecode blob_id.
    if let Some(ctx_meta) = store.handle().get(&key::ContextMeta::new(*context_id))? {
        if let Some(app_meta) = store.handle().get(&ctx_meta.application)? {
            return Ok(Some(*app_meta.bytecode.blob_id().as_ref()));
        }
    }

    // Fallback: the group's replicated target key (no loaded application row).
    let Some(gid) = get_group_for_context(store, context_id)? else {
        return Ok(None);
    };
    Ok(MetaRepository::new(store)
        .load(&gid)?
        .map(|meta| meta.app_key))
}

/// Resolve the **migration target** app_key for a context: the replicated
/// `GroupMeta.app_key` the owning group is migrating toward.
///
/// This is the schema the node will be able to read *once its binary advances*
/// — the discriminator the absorb drain uses to decide when the binary has
/// caught up to the target (so a buffered straggler delta can be verbatim-
/// replayed). It is distinct from [`loaded_reader_app_key`] (what the node can
/// read *right now*) under LazyOnAccess, where the governance target advances
/// ahead of the locally-loaded binary.
///
/// Returns `None` for non-group contexts (no owning group) and when the group
/// meta row is missing. Store errors are propagated as `Err`.
pub fn target_reader_app_key(
    store: &Store,
    context_id: &ContextId,
) -> eyre::Result<Option<[u8; 32]>> {
    let Some(gid) = get_group_for_context(store, context_id)? else {
        return Ok(None);
    };
    Ok(MetaRepository::new(store)
        .load(&gid)?
        .map(|meta| meta.app_key))
}

/// Store-aware decision: resolves the receiver's loaded reader key + the
/// migration target (`GroupMeta.app_key`) + the cascade boundary, then applies
/// [`fence_decision`]. Non-group contexts / missing meta ⇒ `Apply`.
///
/// This is the O3-corrected replacement for [`delta_is_fenced`]: it keys the
/// readability check on the *loaded* reader, not the replicated target.
pub fn delta_fence_decision(
    store: &Store,
    context_id: &ContextId,
    producing_app_key: [u8; 32],
    delta_hlc: HybridTimestamp,
) -> eyre::Result<FenceDecision> {
    let Some(gid) = get_group_for_context(store, context_id)? else {
        return Ok(FenceDecision::Apply);
    };
    let Some(meta) = MetaRepository::new(store).load(&gid)? else {
        return Ok(FenceDecision::Apply);
    };
    let cascade_hlc = UpgradesRepository::new(store)
        .load(&gid)?
        .and_then(|v| v.cascade_hlc);

    // Loaded reader = schema this node can read now; fall back to the target
    // when the loaded application can't be resolved (parity with PR-3).
    let loaded_app_key = loaded_reader_app_key(store, context_id)?.unwrap_or(meta.app_key);

    Ok(fence_decision(
        producing_app_key,
        loaded_app_key,
        meta.app_key,
        delta_hlc,
        cascade_hlc,
    ))
}

/// Store-aware boolean wrapper (PR-3 compatibility shim): `true` iff the delta
/// would not be applied. Delegates to [`delta_fence_decision`].
///
/// Behavior is identical to PR-3 when the loaded reader equals the context's
/// current schema (no migration in flight / no loaded-reader mismatch). The
/// absorb path (Task 6b.4) switches the gossip-fence call site over to
/// [`delta_fence_decision`] to act on the `Buffer` decision; until then this
/// shim preserves the existing call sites.
pub fn delta_is_fenced(
    store: &Store,
    context_id: &ContextId,
    producing_app_key: [u8; 32],
    delta_hlc: HybridTimestamp,
) -> eyre::Result<bool> {
    Ok(!matches!(
        delta_fence_decision(store, context_id, producing_app_key, delta_hlc)?,
        FenceDecision::Apply
    ))
}

#[cfg(test)]
mod tests {
    use super::{fence_decision, should_fence, FenceDecision};
    use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
    use core::num::NonZeroU128;

    /// Returns an `HybridTimestamp` strictly greater than `HybridTimestamp::zero()`.
    ///
    /// `zero()` is `Timestamp { time: NTP64(0), id: ID(1) }`.
    /// Ordering on `HybridTimestamp` delegates lexicographically to `(time, id)`.
    /// `NTP64(1) > NTP64(0)` (same id), so `Timestamp { NTP64(1), ID(1) }` is
    /// strictly greater than `zero()`.
    fn hlc_after_zero() -> HybridTimestamp {
        // SAFETY: 1 ≠ 0.
        let id = ID::from(NonZeroU128::new(1).expect("1 is non-zero"));
        HybridTimestamp::new(Timestamp::new(NTP64(1), id))
    }

    fn zero() -> HybridTimestamp {
        HybridTimestamp::zero()
    }

    /// Sanity-check that our test helper actually produces a value > zero().
    #[test]
    fn hlc_after_zero_is_after_zero() {
        assert!(hlc_after_zero() > HybridTimestamp::zero());
    }

    // -- O3: decision keys on the LOADED reader, not the target -------------

    /// O3 core: the decision must key on the loaded reader, independent of the
    /// migration target. A v1 delta to a v1-reader node (target advanced to v2)
    /// is readable now → `Apply`. A v2 delta to a v1-reader node is not
    /// readable yet → `Buffer` (absorb, never drop).
    #[test]
    fn fences_when_incoming_differs_from_loaded_reader() {
        // delta produced under v1; node still on v1 binary, target advanced to v2.
        assert_eq!(
            fence_decision([1; 32], [1; 32], [2; 32], hlc_after_zero(), Some(zero())),
            FenceDecision::Apply
        );
        // delta produced under v2; node still on v1 reader → cannot read → BUFFER.
        assert_eq!(
            fence_decision([2; 32], [1; 32], [2; 32], hlc_after_zero(), Some(zero())),
            FenceDecision::Buffer
        );
    }

    /// A delta matching the loaded reader is always applied, even after the
    /// boundary, regardless of the target.
    #[test]
    fn applies_when_incoming_matches_loaded_reader() {
        assert_eq!(
            fence_decision([2; 32], [2; 32], [2; 32], hlc_after_zero(), Some(zero())),
            FenceDecision::Apply
        );
    }

    /// At-or-before the boundary is pre-cascade history → always `Apply` even
    /// when the schema differs from the loaded reader (strict `>` required).
    #[test]
    fn applies_at_or_before_boundary_via_decision() {
        assert_eq!(
            fence_decision([2; 32], [1; 32], [2; 32], zero(), Some(zero())),
            FenceDecision::Apply
        );
    }

    /// No cascade boundary → never fence → always `Apply`.
    #[test]
    fn applies_without_boundary_via_decision() {
        assert_eq!(
            fence_decision([2; 32], [1; 32], [2; 32], hlc_after_zero(), None),
            FenceDecision::Apply
        );
    }

    // -- PR-3 compatibility shim (`should_fence`) ---------------------------
    // These mirror the original PR-3 unit tests; with `loaded == ctx == target`
    // the boolean semantics are unchanged.

    /// Different schema, delta is after the boundary → MUST be fenced.
    #[test]
    fn fences_stale_schema_delta_after_boundary() {
        assert!(should_fence(
            [1; 32],
            [2; 32],
            hlc_after_zero(),
            Some(HybridTimestamp::zero())
        ));
    }

    /// Same schema as the context → MUST NOT be fenced regardless of HLC.
    #[test]
    fn does_not_fence_matching_app_key() {
        assert!(!should_fence(
            [2; 32],
            [2; 32],
            hlc_after_zero(),
            Some(HybridTimestamp::zero())
        ));
    }

    /// Different schema but `delta_hlc == boundary` → strict `>` required, so
    /// an at-boundary delta is pre-cascade history and MUST NOT be fenced.
    #[test]
    fn does_not_fence_at_or_before_boundary() {
        // delta_hlc == boundary  => false (strict >)
        assert!(!should_fence(
            [1; 32],
            [2; 32],
            HybridTimestamp::zero(),
            Some(HybridTimestamp::zero())
        ));
    }

    /// No boundary (`None`) → never fence, no matter what.
    #[test]
    fn does_not_fence_without_boundary() {
        assert!(!should_fence([1; 32], [2; 32], hlc_after_zero(), None));
    }
}
