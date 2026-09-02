//! Cascade-upgrade write-gate decisions for context execution: whether a
//! group-upgrade status blocks writes, whether a committed write should be
//! rejected mid-upgrade, the lazy-on-access migration trigger, and the
//! producing-`bytecode_id` resolver. Extracted from the execute handler.

use calimero_app_downloader::registry::{stored_coords, RegistryCoordsBuf};
use calimero_governance_store::MetaRepository;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_store::Store;
use tracing::{debug, info};

/// `true` when a group-upgrade status blocks ALL writes (user calls and
/// state-ops alike): only `GroupUpgradeStatus::InProgress` blocks. Both
/// the direct-lazy and cascade propagators write via `update_application`,
/// bypassing this gate, so a held `InProgress` record can't self-deadlock.
pub(super) fn upgrade_blocks_write(status: &calimero_store::key::GroupUpgradeStatus) -> bool {
    matches!(
        status,
        calimero_store::key::GroupUpgradeStatus::InProgress { .. }
    )
}

/// Whether the cascade write-gate should fire, given the `migration_v2` flag.
///
/// Equal to `!migration_v2 && upgrade_blocks_write(status)`: with the flag OFF
/// the group-wide `InProgress` freeze applies; with it ON the freeze is lifted
/// (absorb-don't-drop keeps stragglers safe instead).
pub(super) fn should_block(
    migration_v2: bool,
    status: &calimero_store::key::GroupUpgradeStatus,
) -> bool {
    !migration_v2 && upgrade_blocks_write(status)
}

/// Post-execution write-gate decision: during an in-progress upgrade a pure read
/// (`produced_write == false`) is served from the pre-migration root; a
/// side-effecting call is refused. Write-intent is derived post-execution (a
/// committed `root_hash` or queued `xcalls`) because no read-vs-write flag exists
/// upstream (`ExecuteRequest`, RPC, SDK, ABI).
pub(super) fn upgrade_rejects_committed_write(block_writes: bool, produced_write: bool) -> bool {
    block_writes && produced_write
}

/// What the lazy-upgrade path should do for a stale context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LazyUpgradeAction {
    /// Context has no activation marker (never activated anything): a
    /// single jump to the group's current target, method from the
    /// group-level hint. Sound ONLY for marker-less contexts — the hint
    /// describes the group's most recent hop, which may not be a marker-ed
    /// context's next one.
    SingleJump {
        target_application_id: ApplicationId,
        migrate_method: Option<String>,
        target_bytecode_id: [u8; 32],
        coords: Option<RegistryCoordsBuf>,
    },
    /// Context has an activation marker: replay the group's upgrade ladder
    /// from that bound blob, each hop's method resolved from the two
    /// blobs' embedded ABIs. The group-level migration hint is never
    /// executed on this arm.
    Replay { bound: [u8; 32] },
}

/// Whether this context needs an upgrade or migration, and via which mode.
/// Caller loads bytecode by blob key, not app id - the application row may still hold OLD wasm.
pub(super) fn maybe_lazy_upgrade(
    datastore: &Store,
    context_id: &ContextId,
    current_application_id: &ApplicationId,
) -> Option<LazyUpgradeAction> {
    use calimero_governance_store;

    // 1. Check if context belongs to a group
    let group_id = match calimero_governance_store::get_group_for_context(datastore, context_id) {
        Ok(Some(gid)) => gid,
        Ok(None) => return None, // not in a group
        Err(err) => {
            debug!(%err, %context_id, "failed to check group for context during lazy upgrade");
            return None;
        }
    };

    // 2. Load group metadata
    let meta = match MetaRepository::new(datastore).load(&group_id) {
        Ok(Some(m)) => m,
        Ok(None) => return None, // group deleted?
        Err(err) => {
            debug!(%err, ?group_id, "failed to load group meta during lazy upgrade");
            return None;
        }
    };

    // 3. The activation marker decides both staleness and the mode below.
    let activated = crate::activation::activated_bytecode(datastore, context_id);

    // 4. Compare current vs target application
    if *current_application_id == meta.target.application_id {
        // IDs match — bundle ids are version-stable, so this is either a
        // pending migration or a pending code-only bytecode bump. One rule
        // covers both: the context is up to date iff its activation marker
        // equals the group's recorded target blob. A zero bytecode_id carries no
        // bytecode signal to compare against, so nothing can be detected.
        if meta.target.bytecode_id == [0u8; 32] {
            return None;
        }
        if activated == Some(meta.target.bytecode_id) {
            return None; // bytecode + migration current — context is up to date
        }
        // Fall through: activation (migration and/or bytecode swap) pending.
    }

    info!(
        %context_id,
        ?group_id,
        %current_application_id,
        target_app=%meta.target.application_id,
        marker = activated.is_some(),
        "lazy upgrade triggered for context"
    );

    Some(match activated {
        Some(bound) => LazyUpgradeAction::Replay { bound },
        // No activation marker. The context never migrated (a commit would have
        // stamped one), so the bytecode blob its application row points at IS
        // its real current version. Replay the ladder hop-by-hop FROM that
        // version rather than single-jumping the group's latest-hop edge: a
        // context several versions behind must run v1->v2 then v2->v3, never the
        // latest edge (e.g. `migrate_v2_to_v3`) against older state — which
        // mis-decodes and panics. The call site seeds the activation marker to
        // this blob before replaying, which also binds execution to it, so a
        // blocked hop strands the context on its real version instead of running
        // the target's bytecode on un-migrated state.
        None => match crate::hlc_fence::loaded_reader_bytecode_id(datastore, context_id) {
            Ok(Some(current)) if current != meta.target.bytecode_id => {
                LazyUpgradeAction::Replay { bound: current }
            }
            // Current version unresolvable (no row), or it already equals the
            // group target. The latter still needs the single jump: the gate
            // only reaches this arm because activation is pending (no marker at
            // the target), and for a bundle (stable application id) a local
            // install bumps the shared application row to the target blob while
            // the migration is still pending — so `loaded_reader == target`
            // does NOT mean migrated. Returning None here would run the target
            // bytecode against un-migrated state.
            _ => LazyUpgradeAction::SingleJump {
                target_application_id: meta.target.application_id,
                migrate_method: meta
                    .migration
                    .as_ref()
                    .and_then(|bytes| String::from_utf8(bytes.clone()).ok()),
                target_bytecode_id: meta.target.bytecode_id,
                coords: stored_coords(&meta.target.package, &meta.target.version)
                    .map(|c| c.to_buf()),
            },
        },
    })
}

/// The blob-derived bytecode id the sender executes under (`GroupMeta.bytecode_id`
/// of the owning group) — the schema discriminator stamped onto state-delta
/// broadcasts so receivers can fence stale-schema deltas. `None` for
/// non-group contexts.
pub(super) fn resolve_producing_bytecode_id(
    datastore: &Store,
    context_id: &ContextId,
) -> eyre::Result<Option<[u8; 32]>> {
    let Some(gid) = calimero_governance_store::get_group_for_context(datastore, context_id)? else {
        return Ok(None);
    };
    Ok(MetaRepository::new(datastore)
        .load(&gid)?
        .map(|m| m.target.bytecode_id))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_app_downloader::registry::stored_coords;
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{
        apply_local_signed_group_op, MembershipRepository, UpgradeLadderRepository,
    };
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;
    use calimero_store::key::GroupTarget;
    use calimero_store::types::{ApplicationMeta as ApplicationMetaValue, ContextMeta};

    use super::*;

    const BYTECODE_ID_OLD: [u8; 32] = [0x01; 32];
    const BYTECODE_ID_NEW: [u8; 32] = [0x02; 32];

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn target_app() -> ApplicationId {
        ApplicationId::from([0xAA; 32])
    }

    fn seed_group(store: &Store, ctx: &ContextId) -> ContextGroupId {
        let gid = ContextGroupId::from([0x60; 32]);
        let mut handle = store.handle();
        handle
            .put(
                &calimero_store::key::ContextGroupRef::new((**ctx).into()),
                &gid.to_bytes(),
            )
            .unwrap();
        let admin = calimero_primitives::identity::PublicKey::from([0x07; 32]);
        calimero_governance_store::MetaRepository::new(store)
            .save(
                &gid,
                &GroupMetaValue {
                    target: GroupTarget {
                        application_id: target_app(),
                        bytecode_id: BYTECODE_ID_NEW,
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 0,
                    admin_identity: crate::test_support::account_for(&admin),
                    owner_identity: crate::test_support::account_for(&admin),
                    migration: Some(b"migrate_v2_to_v3".to_vec()),
                    auto_join: false,
                },
            )
            .unwrap();
        gid
    }

    // The wrong-hop hole (PR-4 regression guard): a marker-ed context must
    // NEVER receive the group-level migration method — the hint describes
    // the group's most recent hop, while this context may be several rungs
    // below it. Running that method against older state mis-decodes or
    // corrupts. Marker-ed contexts replay the ladder instead.
    #[test]
    fn marker_ed_context_replays_and_never_carries_the_group_method() {
        let store = store();
        let ctx = ContextId::from([0x50; 32]);
        let _gid = seed_group(&store, &ctx);
        crate::activation::record_activation(&store, &ctx, BYTECODE_ID_OLD);

        let action = maybe_lazy_upgrade(&store, &ctx, &target_app()).expect("stale -> fires");
        assert_eq!(
            action,
            LazyUpgradeAction::Replay {
                bound: BYTECODE_ID_OLD
            }
        );
    }

    /// Seed a context's application row so `loaded_reader_bytecode_id` resolves the
    /// context's current bytecode blob to `blob` (an installed-but-never-migrated
    /// version). `app_id` keys the row; for a bundle it equals the group target.
    fn seed_app_row(
        store: &Store,
        ctx: &ContextId,
        app_id: ApplicationId,
        blob: [u8; 32],
        coords: (&str, &str),
    ) {
        let mut handle = store.handle();
        handle
            .put(
                &calimero_store::key::ApplicationMeta::new(app_id),
                &ApplicationMetaValue::new(
                    calimero_store::key::BlobMeta::new(blob.into()),
                    0,
                    String::new().into_boxed_str(),
                    Box::new([]),
                    calimero_store::key::BlobMeta::new([0u8; 32].into()),
                    calimero_store::types::PackageInfo {
                        package: coords.0.into(),
                        version: coords.1.into(),
                        signer_id: String::new().into_boxed_str(),
                        state_version: 0,
                    },
                ),
            )
            .unwrap();
        handle
            .put(
                &calimero_store::key::ContextMeta::new(*ctx),
                &ContextMeta::new(
                    calimero_store::key::ApplicationMeta::new(app_id),
                    [0u8; 32],
                    vec![],
                    None,
                ),
            )
            .unwrap();
    }

    // Regression guard for the marker-less multi-version-behind hole: a fresh
    // joiner (no activation marker) whose group has advanced several versions
    // must REPLAY the ladder from its current row version, NOT single-jump the
    // group's latest-hop edge against older state (which mis-decodes + panics).
    #[test]
    fn marker_less_context_with_current_row_replays_from_its_version() {
        let store = store();
        let ctx = ContextId::from([0x51; 32]);
        let _gid = seed_group(&store, &ctx);
        // Context installed (never migrated) at BYTECODE_ID_OLD; group target is
        // BYTECODE_ID_NEW (bundle: same application id, different blob).
        seed_app_row(&store, &ctx, target_app(), BYTECODE_ID_OLD, ("", ""));

        let action = maybe_lazy_upgrade(&store, &ctx, &target_app()).expect("stale -> fires");
        assert_eq!(
            action,
            LazyUpgradeAction::Replay {
                bound: BYTECODE_ID_OLD
            }
        );
    }

    // A marker-less context whose current version is unresolvable (no row, so
    // `loaded_reader_bytecode_id` falls back to the group target) keeps the single
    // jump: the gate only reaches this arm because activation is pending, and
    // `loaded_reader == target` does NOT prove migration ran (a bundle install
    // bumps the shared row ahead of the marker). Returning None here would run
    // target bytecode on un-migrated state.
    #[test]
    fn marker_less_context_without_resolvable_row_keeps_the_single_jump() {
        let store = store();
        let ctx = ContextId::from([0x51; 32]);
        let _gid = seed_group(&store, &ctx);

        let action = maybe_lazy_upgrade(&store, &ctx, &target_app()).expect("stale -> fires");
        assert_eq!(
            action,
            LazyUpgradeAction::SingleJump {
                target_application_id: target_app(),
                migrate_method: Some("migrate_v2_to_v3".to_owned()),
                target_bytecode_id: BYTECODE_ID_NEW,
                // Nothing appended a rung here, so no rung names the blob.
                coords: None,
            }
        );
    }

    #[test]
    fn up_to_date_marker_returns_none() {
        let store = store();
        let ctx = ContextId::from([0x52; 32]);
        let _gid = seed_group(&store, &ctx);
        crate::activation::record_activation(&store, &ctx, BYTECODE_ID_NEW);

        assert_eq!(maybe_lazy_upgrade(&store, &ctx, &target_app()), None);
    }

    #[test]
    fn non_group_context_returns_none() {
        let store = store();
        let ctx = ContextId::from([0x54; 32]);
        assert_eq!(maybe_lazy_upgrade(&store, &ctx, &target_app()), None);
    }

    /// Apply a real `TargetApplicationSet` naming `coords`, signed by an admin
    /// this test enrols, so the ladder is written by the production choke point.
    fn upgrade_group_to(
        store: &Store,
        gid: ContextGroupId,
        bytecode_id: [u8; 32],
        coords: (&str, &str),
    ) {
        let admin_sk = PrivateKey::random(&mut rand::rngs::OsRng);
        let admin = crate::test_support::enrol(store, &gid, &admin_sk.public_key());
        MembershipRepository::new(store)
            .add_member(&gid, &admin, GroupMemberRole::Admin)
            .unwrap();

        let op = SignedGroupOp::sign(
            &admin_sk,
            gid.to_bytes().into(),
            vec![],
            1,
            GroupOp::TargetApplicationSet {
                bytecode_id: bytecode_id.into(),
                target_application_id: target_app(),
                package: coords.0.to_owned(),
                version: coords.1.to_owned(),
            },
        )
        .unwrap();
        apply_local_signed_group_op(store, &op).expect("apply TargetApplicationSet");
    }

    /// A bundle's id is version-stable, so a member on the previous version keeps
    /// its row - which is why the row cannot carry the target's coordinates.
    #[test]
    fn an_installed_member_still_reads_the_new_targets_coordinates() {
        let store = store();
        let ctx = ContextId::from([0x56; 32]);
        let gid = ContextGroupId::from([0x64; 32]);
        store
            .handle()
            .put(
                &calimero_store::key::ContextGroupRef::new((*ctx).into()),
                &gid.to_bytes(),
            )
            .unwrap();
        calimero_governance_store::MetaRepository::new(&store)
            .save(
                &gid,
                &GroupMetaValue {
                    target: GroupTarget {
                        application_id: target_app(),
                        bytecode_id: BYTECODE_ID_OLD,
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 0,
                    admin_identity: crate::test_support::account_for(
                        &calimero_primitives::identity::PublicKey::from([0x07; 32]),
                    ),
                    owner_identity: crate::test_support::account_for(
                        &calimero_primitives::identity::PublicKey::from([0x07; 32]),
                    ),
                    migration: None,
                    auto_join: false,
                },
            )
            .unwrap();
        // This member ALREADY holds the row for the version-stable bundle id,
        // pinned to v1 - the case the row-derived coordinates silently lost.
        seed_app_row(
            &store,
            &ctx,
            target_app(),
            BYTECODE_ID_OLD,
            ("com.acme.app", "1.0.0"),
        );

        upgrade_group_to(&store, gid, BYTECODE_ID_NEW, ("com.acme.app", "2.0.0"));

        // The local install always wins, so the row still names v1's bytes.
        let row = store
            .handle()
            .get(&calimero_store::key::ApplicationMeta::new(target_app()))
            .unwrap()
            .expect("the pre-existing row survives the upgrade op");
        assert_eq!(
            *row.bytecode.blob_id().as_ref(),
            BYTECODE_ID_OLD,
            "seed_target_application_row must never overwrite a local install"
        );
        assert_eq!(
            stored_coords(&row.package, &row.version).map(|c| c.version),
            Some("1.0.0"),
            "the row's coordinates are the INSTALLED version, not the target's"
        );

        // An installed-but-unmigrated member replays from its own version, and
        // the rung it replays must carry the target's own coordinates.
        assert_eq!(
            maybe_lazy_upgrade(&store, &ctx, &target_app()),
            Some(LazyUpgradeAction::Replay {
                bound: BYTECODE_ID_OLD
            })
        );
        let ladder = UpgradeLadderRepository::new(&store).load(&gid).unwrap();
        let rung = crate::activation::next_rung(&ladder, BYTECODE_ID_OLD, BYTECODE_ID_NEW)
            .expect("a stale member has a next rung");
        assert_eq!(rung.bytecode_id, BYTECODE_ID_NEW);
        assert_eq!(
            (rung.package.as_str(), rung.version.as_str()),
            ("com.acme.app", "2.0.0"),
            "the rung, not the row, is what the fetch path resolves the target from"
        );
    }

    /// The marker-less single jump reads the same ladder record, so a fresh
    /// context resolves the target's own coordinates too.
    #[test]
    fn the_single_jump_carries_the_ladder_rungs_coordinates() {
        let store = store();
        let ctx = ContextId::from([0x57; 32]);
        let gid = ContextGroupId::from([0x65; 32]);
        store
            .handle()
            .put(
                &calimero_store::key::ContextGroupRef::new((*ctx).into()),
                &gid.to_bytes(),
            )
            .unwrap();
        let account = crate::test_support::account_for(
            &calimero_primitives::identity::PublicKey::from([0x07; 32]),
        );
        calimero_governance_store::MetaRepository::new(&store)
            .save(
                &gid,
                &GroupMetaValue {
                    target: GroupTarget {
                        application_id: target_app(),
                        bytecode_id: BYTECODE_ID_OLD,
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 0,
                    admin_identity: account,
                    owner_identity: account,
                    migration: None,
                    auto_join: false,
                },
            )
            .unwrap();

        upgrade_group_to(&store, gid, BYTECODE_ID_NEW, ("com.acme.app", "2.0.0"));

        // No context row, so the current version is unresolvable: single jump.
        let Some(LazyUpgradeAction::SingleJump { coords, .. }) =
            maybe_lazy_upgrade(&store, &ctx, &target_app())
        else {
            panic!("a context with no row must single-jump");
        };
        assert_eq!(
            coords.as_ref().map(|c| (&*c.package, &*c.version)),
            Some(("com.acme.app", "2.0.0"))
        );
    }
}
