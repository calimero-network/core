//! State-delta HLC fence (PR-3): drop a delta produced under a different app
//! schema than the context now targets AND newer than the recorded cascade
//! boundary. A `None` boundary never fences.

use calimero_governance_store::{get_group_for_context, MetaRepository, UpgradesRepository};
use calimero_primitives::context::ContextId;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::Store;

/// Pure two-condition rule. `cascade_hlc == None` ⇒ never fence.
///
/// Returns `true` iff the delta should be dropped:
/// - the delta was produced under a *different* app schema than the context now
///   targets (`delta_app_key != ctx_app_key`), AND
/// - the delta's HLC is *strictly after* the cascade boundary (`delta_hlc > boundary`).
///
/// At-or-before-boundary deltas (`delta_hlc <= boundary`) are pre-cascade
/// legitimate history and must NOT be fenced even when the schema differs.
#[must_use]
pub fn should_fence(
    delta_app_key: [u8; 32],
    ctx_app_key: [u8; 32],
    delta_hlc: HybridTimestamp,
    cascade_hlc: Option<HybridTimestamp>,
) -> bool {
    matches!(cascade_hlc, Some(boundary) if delta_app_key != ctx_app_key && delta_hlc > boundary)
}

/// Store-aware wrapper: resolves the context's current app_key + cascade_hlc and
/// applies `should_fence`. Non-group contexts / missing meta ⇒ never fence.
pub fn delta_is_fenced(
    store: &Store,
    context_id: &ContextId,
    producing_app_key: [u8; 32],
    delta_hlc: HybridTimestamp,
) -> eyre::Result<bool> {
    let Some(gid) = get_group_for_context(store, context_id)? else {
        return Ok(false);
    };
    let Some(meta) = MetaRepository::new(store).load(&gid)? else {
        return Ok(false);
    };
    let cascade_hlc = UpgradesRepository::new(store)
        .load(&gid)?
        .and_then(|v| v.cascade_hlc);
    Ok(should_fence(
        producing_app_key,
        meta.app_key,
        delta_hlc,
        cascade_hlc,
    ))
}

#[cfg(test)]
mod tests {
    use super::should_fence;
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

    /// Sanity-check that our test helper actually produces a value > zero().
    #[test]
    fn hlc_after_zero_is_after_zero() {
        assert!(hlc_after_zero() > HybridTimestamp::zero());
    }

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
