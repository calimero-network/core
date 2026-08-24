//! The per-context activation marker: which bytecode blob this context last
//! ACTIVATED (a migration commit, or a code-only swap). One fact shared by
//! the sync gate, the lazy trigger, and the migration rollup, with a single
//! up-to-date rule: `marker == group.bytecode_id`.

use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_store::key::LadderRung;
use calimero_store::Store;
use tracing::debug;

/// The blob this context last activated, if the marker is set.
pub fn activated_blob(store: &Store, context_id: &ContextId) -> Option<[u8; 32]> {
    store
        .handle()
        .get(&calimero_store::key::ContextActivatedBlob::new(*context_id))
        .ok()
        .flatten()
        .map(|v| v.blob)
}

/// Record that `context_id` now executes `blob` (migration committed or
/// code-only activation applied). Best-effort: a failed write means the
/// context re-runs its (idempotent) activation on next access.
pub fn record_activation(store: &Store, context_id: &ContextId, blob: [u8; 32]) {
    let mut handle = store.handle();
    if let Err(err) = handle.put(
        &calimero_store::key::ContextActivatedBlob::new(*context_id),
        &calimero_store::types::ContextActivatedBlob { blob },
    ) {
        debug!(%context_id, %err, "failed to record activation marker");
    }
}

/// The ABI state version the context's activated bytecode declares, if it was
/// resolvable when the activation was recorded.
pub fn activated_state_version(store: &Store, context_id: &ContextId) -> Option<u32> {
    store
        .handle()
        .get(&calimero_store::key::ContextActivatedStateVersion::new(
            *context_id,
        ))
        .ok()
        .flatten()
        .map(|v| v.state_version)
}

/// Record the state version `context_id`'s newly-activated bytecode declares.
/// Best-effort, like [`record_activation`]: without it the rollup falls back to
/// the install row, which under-reports rather than over-reports.
pub fn record_activated_state_version(store: &Store, context_id: &ContextId, state_version: u32) {
    let mut handle = store.handle();
    if let Err(err) = handle.put(
        &calimero_store::key::ContextActivatedStateVersion::new(*context_id),
        &calimero_store::types::ContextActivatedStateVersion { state_version },
    ) {
        debug!(%context_id, %err, "failed to record activated state version");
    }
}

/// Whether this context's STATE has reached the bytecode its group records as
/// current (`marker == group.bytecode_id`), or `None` when there is no group
/// bytecode signal to compare against - no group, no meta, or a zero `bytecode_id`.
///
/// An install never moves the marker, which is what makes this a migration
/// signal where `ApplicationMeta.state_version` is only an install one.
pub fn activated_at_group_target(store: &Store, context_id: &ContextId) -> Option<bool> {
    let group_id = calimero_governance_store::get_group_for_context(store, context_id)
        .ok()
        .flatten()?;
    let meta = calimero_governance_store::MetaRepository::new(store)
        .load(&group_id)
        .ok()
        .flatten()?;
    (meta.bytecode_id != [0u8; 32])
        .then(|| activated_blob(store, context_id) == Some(meta.bytecode_id))
}

/// The next upgrade rung a context bound to `bound` must replay from the
/// group's ladder, or `None` when it is already at the group's current
/// bytecode. The LAST occurrence of `bound` positions the context (an
/// A→B→A re-pin means the group currently sits at the later A). A bound
/// blob the ladder never recorded (creation version, or a pre-ladder
/// upgrade target) starts from the first rung; an empty or stale ladder
/// degrades to a single synthesized jump to the group's current target —
/// exactly the pre-ladder behavior.
pub fn next_rung(
    ladder: &[LadderRung],
    bound: [u8; 32],
    group_bytecode_id: [u8; 32],
    group_target: ApplicationId,
) -> Option<LadderRung> {
    if bound == group_bytecode_id {
        return None;
    }
    let single_jump = LadderRung {
        bytecode_id: group_bytecode_id,
        application_id: group_target,
    };
    match ladder.iter().rposition(|r| r.bytecode_id == bound) {
        Some(i) => Some(ladder.get(i + 1).cloned().unwrap_or(single_jump)),
        None => Some(ladder.first().cloned().unwrap_or(single_jump)),
    }
}

/// The `ApplicationId` the upgrade ladder pairs with bytecode blob `schema`,
/// or the group target when `schema` is the group's current `bytecode_id`. `None`
/// when the ladder never recorded `schema`, so a caller leaves the existing
/// binding untouched. Used by the resync settle to reconcile a context's bound
/// id to the schema its synced state actually carries (single-wasm apps key a
/// distinct id per version; bundle apps share one, so this is a no-op there).
pub fn application_for_schema(
    ladder: &[LadderRung],
    schema: [u8; 32],
    group_bytecode_id: [u8; 32],
    group_target: ApplicationId,
) -> Option<ApplicationId> {
    if schema == group_bytecode_id {
        return Some(group_target);
    }
    ladder
        .iter()
        .rev()
        .find(|r| r.bytecode_id == schema)
        .map(|r| r.application_id)
}

/// Reconcile a context's bound `ApplicationId` (`ContextMeta.application`) after
/// a resync adopts a peer's state. Without this a single-wasm context whose
/// per-version id differs from its stale binding would keep tripping the
/// pending-upgrade gate even though its state now matches the target. Best
/// effort: a read miss or write error leaves the binding as-is.
pub fn reconcile_context_application(
    store: &Store,
    context_id: &ContextId,
    application_id: ApplicationId,
) {
    let key = calimero_store::key::ContextMeta::new(*context_id);
    let mut handle = store.handle();
    let Ok(Some(mut meta)) = handle.get(&key) else {
        return;
    };
    if meta.application.application_id() == application_id {
        return;
    }
    meta.application = calimero_store::key::ApplicationMeta::new(application_id);
    if let Err(err) = handle.put(&key, &meta) {
        debug!(%context_id, %err, "failed to reconcile context application after resync");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_store::db::InMemoryDB;

    use super::*;

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn rung(byte: u8) -> LadderRung {
        LadderRung {
            bytecode_id: [byte; 32],
            application_id: ApplicationId::from([byte; 32]),
        }
    }

    const TARGET: [u8; 32] = [0x09; 32];

    fn target_id() -> ApplicationId {
        ApplicationId::from(TARGET)
    }

    #[test]
    fn up_to_date_returns_none() {
        // Even with a populated ladder, bound == group bytecode_id is terminal.
        let ladder = vec![rung(0x01), rung(0x09)];
        assert_eq!(next_rung(&ladder, TARGET, TARGET, target_id()), None);
    }

    #[test]
    fn mid_ladder_returns_next() {
        let ladder = vec![rung(0x01), rung(0x02), rung(0x09)];
        assert_eq!(
            next_rung(&ladder, [0x01; 32], TARGET, target_id()),
            Some(rung(0x02))
        );
        assert_eq!(
            next_rung(&ladder, [0x02; 32], TARGET, target_id()),
            Some(rung(0x09))
        );
    }

    #[test]
    fn bound_not_in_ladder_starts_at_first_rung() {
        // A context still at its creation version: the ladder only records
        // upgrade targets, so the creation blob is never in it.
        let ladder = vec![rung(0x02), rung(0x09)];
        assert_eq!(
            next_rung(&ladder, [0x77; 32], TARGET, target_id()),
            Some(rung(0x02))
        );
    }

    #[test]
    fn empty_ladder_synthesizes_single_jump() {
        // Pre-ladder group: degrade to today's one-jump-to-target behavior.
        assert_eq!(
            next_rung(&[], [0x77; 32], TARGET, target_id()),
            Some(LadderRung {
                bytecode_id: TARGET,
                application_id: target_id(),
            })
        );
    }

    #[test]
    fn last_occurrence_positions_the_context() {
        // A→B→A→C: a context bound at A sits at the LATER A (the group
        // re-pinned A as its third upgrade), so its next hop is C.
        let ladder = vec![rung(0x01), rung(0x02), rung(0x01), rung(0x09)];
        assert_eq!(
            next_rung(&ladder, [0x01; 32], TARGET, target_id()),
            Some(rung(0x09))
        );
    }

    #[test]
    fn stale_ladder_top_synthesizes_single_jump() {
        // Bound is the ladder's last rung but the group meta already points
        // past it (fold raced ahead of the ladder, or pre-ladder upgrade):
        // degrade to the single jump rather than walking nowhere.
        let ladder = vec![rung(0x01), rung(0x02)];
        assert_eq!(
            next_rung(&ladder, [0x02; 32], TARGET, target_id()),
            Some(LadderRung {
                bytecode_id: TARGET,
                application_id: target_id(),
            })
        );
    }

    #[test]
    fn application_for_schema_resolves_target_ladder_and_unknown() {
        let ladder = vec![rung(0x01), rung(0x02), rung(0x09)];
        // The group target bytecode_id maps to the group target id.
        assert_eq!(
            application_for_schema(&ladder, TARGET, TARGET, target_id()),
            Some(target_id())
        );
        // An intermediate schema maps to that rung's id.
        assert_eq!(
            application_for_schema(&ladder, [0x02; 32], TARGET, target_id()),
            Some(ApplicationId::from([0x02; 32]))
        );
        // A schema the ladder never recorded leaves the binding untouched.
        assert_eq!(
            application_for_schema(&ladder, [0x77; 32], TARGET, target_id()),
            None
        );
    }

    #[test]
    fn reconcile_context_application_updates_binding() {
        use calimero_store::key::{ApplicationMeta, ContextMeta as ContextMetaKey};
        use calimero_store::types::ContextMeta;

        let store = store();
        let ctx = ContextId::from([0x33; 32]);
        let old = ApplicationId::from([0x01; 32]);
        let new = ApplicationId::from([0x09; 32]);
        store
            .handle()
            .put(
                &ContextMetaKey::new(ctx),
                &ContextMeta::new(ApplicationMeta::new(old), [0u8; 32], vec![], None),
            )
            .unwrap();

        reconcile_context_application(&store, &ctx, new);

        let meta = store
            .handle()
            .get(&ContextMetaKey::new(ctx))
            .unwrap()
            .unwrap();
        assert_eq!(meta.application.application_id(), new);
    }

    /// The rollup reads this to decide residue, so every arm matters: a `None`
    /// leaves it on the application row, a `Some(false)` counts the context as
    /// outstanding. An absent bytecode signal must never manufacture either.
    #[test]
    fn activated_at_group_target_reads_the_marker_against_the_group_blob() {
        use calimero_context_config::types::ContextGroupId;
        use calimero_store::key::GroupMetaValue;

        let store = store();
        let ctx = ContextId::from([0x61; 32]);
        let gid = ContextGroupId::from([0x62; 32]);

        // No group at all: nothing to compare against.
        assert_eq!(activated_at_group_target(&store, &ctx), None);

        store
            .handle()
            .put(
                &calimero_store::key::ContextGroupRef::new((*ctx).into()),
                &gid.to_bytes(),
            )
            .unwrap();
        // Group, but no meta yet.
        assert_eq!(activated_at_group_target(&store, &ctx), None);

        let account = crate::test_support::account_for(
            &calimero_primitives::identity::PublicKey::from([0x07; 32]),
        );
        let mut meta = GroupMetaValue {
            bytecode_id: [0u8; 32],
            target_application_id: ApplicationId::from([0xAA; 32]),
            created_at: 0,
            admin_identity: account,
            owner_identity: account,
            migration: None,
            auto_join: false,
        };
        let save = |meta: &GroupMetaValue| {
            calimero_governance_store::MetaRepository::new(&store)
                .save(&gid, meta)
                .unwrap();
        };
        // A zero bytecode_id carries no bytecode signal to compare against.
        save(&meta);
        assert_eq!(activated_at_group_target(&store, &ctx), None);

        meta.bytecode_id = [0x02; 32];
        save(&meta);
        // Group has a target blob, context has no marker: not there yet.
        assert_eq!(activated_at_group_target(&store, &ctx), Some(false));

        record_activation(&store, &ctx, [0x01; 32]);
        assert_eq!(activated_at_group_target(&store, &ctx), Some(false));

        record_activation(&store, &ctx, [0x02; 32]);
        assert_eq!(activated_at_group_target(&store, &ctx), Some(true));
    }

    #[test]
    fn marker_roundtrip() {
        let store = store();
        let ctx = ContextId::from([1u8; 32]);
        assert_eq!(activated_blob(&store, &ctx), None);
        record_activation(&store, &ctx, [7u8; 32]);
        assert_eq!(activated_blob(&store, &ctx), Some([7u8; 32]));
        // Moves forward on re-activation.
        record_activation(&store, &ctx, [8u8; 32]);
        assert_eq!(activated_blob(&store, &ctx), Some([8u8; 32]));
    }
}
