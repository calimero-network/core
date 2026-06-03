//! Integration tests for `calimero_context::hlc_fence::delta_is_fenced`.
//!
//! These tests exercise the store-aware wrapper end-to-end: an in-memory store
//! is populated with a group, a context registered to that group, a
//! `GroupMetaValue` pinning the group's current app schema, and a
//! `GroupUpgradeValue` that may (or may not) carry a `cascade_hlc` boundary.
//! Then `delta_is_fenced` is called with various (producing_app_key, delta_hlc)
//! combinations and the result is asserted.

use std::sync::Arc;

use calimero_context::hlc_fence::{
    delta_fence_decision, delta_is_fenced, loaded_reader_app_key, FenceDecision,
};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{register_context_in_group, MetaRepository, UpgradesRepository};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{ContextId, UpgradePolicy};
use calimero_primitives::identity::PublicKey;
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use calimero_store::db::InMemoryDB;
use calimero_store::key::{
    self, ApplicationMeta as ApplicationMetaKey, BlobMeta, ContextMeta as ContextMetaKey,
    GroupMetaValue, GroupUpgradeStatus, GroupUpgradeValue,
};
use calimero_store::types::{ApplicationMeta, ContextMeta};
use calimero_store::Store;
use core::num::NonZeroU128;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The two app keys used across all tests.
const APP_KEY_1: [u8; 32] = [0x11; 32]; // "old" schema — delta was produced under this
const APP_KEY_2: [u8; 32] = [0x22; 32]; // "new" schema — context currently targets this

fn empty_store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

/// An `HybridTimestamp` strictly greater than `HybridTimestamp::zero()`.
/// `zero()` is `Timestamp { time: NTP64(0), id: ID(1) }`.
/// `NTP64(1) > NTP64(0)` ⇒ this value is after zero.
fn hlc_after_zero() -> HybridTimestamp {
    let id = ID::from(NonZeroU128::new(1).expect("1 is non-zero"));
    HybridTimestamp::new(Timestamp::new(NTP64(1), id))
}

/// Build a minimal `GroupMetaValue` targeting `app_key`.
fn meta_for(app_key: [u8; 32]) -> GroupMetaValue {
    let admin = PublicKey::from([0x01; 32]);
    GroupMetaValue {
        app_key,
        target_application_id: ApplicationId::from([0xAA; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: admin,
        owner_identity: admin,
        migration: None,
        auto_join: true,
    }
}

/// Build a `GroupUpgradeValue` with the given `cascade_hlc`.
fn upgrade_with_hlc(cascade_hlc: Option<HybridTimestamp>) -> GroupUpgradeValue {
    GroupUpgradeValue {
        from_version: "1.0.0".to_owned(),
        to_version: "2.0.0".to_owned(),
        migration: None,
        initiated_at: 1_700_000_000,
        initiated_by: PublicKey::from([0x01; 32]),
        status: GroupUpgradeStatus::Completed { completed_at: None },
        cascade_hlc,
    }
}

/// Set up a store with:
/// - a group `gid` whose meta targets `app_key`
/// - optionally a `GroupUpgradeValue` with `cascade_hlc`
/// - a context `ctx_id` registered to `gid`
///
/// Returns the store ready for `delta_is_fenced` calls.
fn setup(
    app_key: [u8; 32],
    cascade_hlc: Option<HybridTimestamp>,
    with_upgrade_record: bool,
) -> (Store, ContextGroupId, ContextId) {
    let store = empty_store();
    let gid = ContextGroupId::from([0x10; 32]);
    let ctx_id = ContextId::from([0x20; 32]);

    MetaRepository::new(&store)
        .save(&gid, &meta_for(app_key))
        .unwrap();

    if with_upgrade_record {
        UpgradesRepository::new(&store)
            .save(&gid, &upgrade_with_hlc(cascade_hlc))
            .unwrap();
    }

    register_context_in_group(&store, &gid, &ctx_id).unwrap();

    (store, gid, ctx_id)
}

/// The application id of the locally-loaded application (the one the
/// `ContextMeta.application` key points at).
const LOADED_APP_ID: [u8; 32] = [0xAB; 32];

/// Install an `ApplicationMeta` value keyed by `app_id` whose `bytecode`
/// blob-id is `app_key`. `loaded_reader_app_key` resolves
/// `ApplicationMeta(application).bytecode.blob_id()`, so driving the test's
/// loaded reader key through this field is what exercises the resolver's
/// primary (store-level) branch — mirrors
/// `crates/node/src/cascade_dispatch_e2e.rs::install_application`.
fn install_application(store: &Store, app_id: ApplicationId, app_key: [u8; 32]) {
    let bytecode_blob = BlobMeta::new(BlobId::from(app_key));
    // `compiled` is unused by the fence resolver; reuse `bytecode_blob` to
    // keep the fixture minimal.
    let meta = ApplicationMeta::new(
        bytecode_blob,
        /* size = */ 1,
        "test://hlc-fence".to_owned().into_boxed_str(),
        Box::new([]),
        bytecode_blob,
        "hlc-fence-test-pkg".to_owned().into_boxed_str(),
        "1.0.0".to_owned().into_boxed_str(),
        "hlc-fence-test-signer".to_owned().into_boxed_str(),
    );
    store
        .handle()
        .put(&ApplicationMetaKey::new(app_id), &meta)
        .expect("put ApplicationMeta");
}

/// Write a `ContextMeta` row for `ctx_id` whose `application` key points at
/// `app_id`. This is the locally-loaded application — the resolver reads it
/// to derive the loaded reader key.
fn install_context_meta(store: &Store, ctx_id: ContextId, app_id: ApplicationId) {
    let meta = ContextMeta::new(
        ApplicationMetaKey::new(app_id),
        /* root_hash = */ [0x01; 32],
        /* dag_heads = */ Vec::new(),
        /* service_name = */ None,
    );
    store
        .handle()
        .put(&ContextMetaKey::new(ctx_id), &meta)
        .expect("put ContextMeta");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A delta produced under the OLD schema (APP_KEY_1) while the context now
/// targets APP_KEY_2, with an HLC strictly after the cascade boundary, MUST
/// be fenced.
#[test]
fn fences_stale_schema_delta_after_boundary() {
    let boundary = HybridTimestamp::zero();
    let (store, _, ctx_id) = setup(APP_KEY_2, Some(boundary), true);

    // producing_app_key = APP_KEY_1 (old), delta_hlc > boundary
    let result = delta_is_fenced(&store, &ctx_id, APP_KEY_1, hlc_after_zero())
        .expect("delta_is_fenced must not error");
    assert!(result, "old-schema delta after boundary must be fenced");
}

/// A delta produced under the CURRENT schema (APP_KEY_2), even when after the
/// boundary, must NOT be fenced — it was produced under the same schema the
/// context now targets.
#[test]
fn does_not_fence_matching_app_key() {
    let boundary = HybridTimestamp::zero();
    let (store, _, ctx_id) = setup(APP_KEY_2, Some(boundary), true);

    // producing_app_key == ctx_app_key => no fence
    let result = delta_is_fenced(&store, &ctx_id, APP_KEY_2, hlc_after_zero())
        .expect("delta_is_fenced must not error");
    assert!(!result, "current-schema delta must never be fenced");
}

/// A delta produced under the OLD schema but with an HLC at (not after) the
/// boundary is pre-cascade legitimate history and MUST NOT be fenced.
/// The comparison is strict `>`.
#[test]
fn does_not_fence_at_or_before_boundary() {
    let boundary = HybridTimestamp::zero();
    let (store, _, ctx_id) = setup(APP_KEY_2, Some(boundary), true);

    // delta_hlc == boundary (zero == zero) => not fenced (strict >)
    let result = delta_is_fenced(&store, &ctx_id, APP_KEY_1, HybridTimestamp::zero())
        .expect("delta_is_fenced must not error");
    assert!(
        !result,
        "at-boundary delta must not be fenced (strict > required)"
    );
}

/// When the group has NO `GroupUpgradeValue` record at all (i.e., the group
/// was never upgraded via cascade), `cascade_hlc` resolves to `None` and no
/// delta should be fenced.
#[test]
fn does_not_fence_without_upgrade_record() {
    // with_upgrade_record = false => no GroupUpgradeValue in store
    let (store, _, ctx_id) = setup(APP_KEY_2, None, false);

    let result = delta_is_fenced(&store, &ctx_id, APP_KEY_1, hlc_after_zero())
        .expect("delta_is_fenced must not error");
    assert!(
        !result,
        "no upgrade record => cascade_hlc is None => never fence"
    );
}

/// When the group has a `GroupUpgradeValue` but `cascade_hlc` is `None` (e.g.,
/// a non-cascade upgrade), no delta should be fenced.
#[test]
fn does_not_fence_when_cascade_hlc_is_none_in_upgrade_record() {
    // with_upgrade_record = true, cascade_hlc = None
    let (store, _, ctx_id) = setup(APP_KEY_2, None, true);

    let result = delta_is_fenced(&store, &ctx_id, APP_KEY_1, hlc_after_zero())
        .expect("delta_is_fenced must not error");
    assert!(
        !result,
        "cascade_hlc: None in upgrade record => never fence"
    );
}

/// A context that is NOT registered to any group returns `false` — no group
/// membership means no cascade boundary can be resolved.
#[test]
fn does_not_fence_for_ungrouped_context() {
    let store = empty_store();
    // This context_id is never registered to any group.
    let ctx_id = ContextId::from([0xFF; 32]);

    let result = delta_is_fenced(&store, &ctx_id, APP_KEY_1, hlc_after_zero())
        .expect("delta_is_fenced must not error");
    assert!(!result, "ungrouped context must never be fenced");
}

// ---------------------------------------------------------------------------
// O3 loaded-reader divergence — the PRIMARY resolver branch.
//
// The tests above never write a `ContextMeta` row, so `loaded_reader_app_key`
// always lands on the `unwrap_or(meta.app_key)` fallback — i.e. loaded == target.
// The fence the O3 fix exists for only bites when the locally-loaded reader is
// BEHIND the replicated `GroupMeta.app_key`: a node still on the v1 binary while
// the governance target has advanced to v2. These tests install a `ContextMeta`
// → `ApplicationMeta` (v1 bytecode blob) divergence against a v2 `GroupMeta`,
// exercising the resolver's primary store-level branch directly.
// ---------------------------------------------------------------------------

/// The resolver must return the LOADED application's bytecode blob_id
/// (APP_KEY_1), NOT the replicated `GroupMeta.app_key` (APP_KEY_2). This is the
/// core of O3: under LazyOnAccess the governance target advances ahead of the
/// locally-loaded binary.
#[test]
fn loaded_reader_resolves_loaded_application_not_group_target() {
    // GroupMeta.app_key = v2 (target advanced), loaded application = v1.
    let (store, _, ctx_id) = setup(APP_KEY_2, Some(HybridTimestamp::zero()), true);
    let app_id = ApplicationId::from(LOADED_APP_ID);
    install_application(&store, app_id, APP_KEY_1);
    install_context_meta(&store, ctx_id, app_id);

    let loaded = loaded_reader_app_key(&store, &ctx_id)
        .expect("loaded_reader_app_key must not error");
    assert_eq!(
        loaded,
        Some(APP_KEY_1),
        "must resolve the loaded application's bytecode blob_id (v1), not GroupMeta.app_key (v2)"
    );
}

/// With the loaded reader on v1 and the target advanced to v2, an after-boundary
/// delta produced under v2 cannot be read yet → `Buffer` (absorb, never drop),
/// while a v1 delta is readable now → `Apply`. This is the divergence the fence
/// exists for, and it is invisible to the `unwrap_or(meta.app_key)` fallback.
#[test]
fn fence_decision_buffers_v2_delta_for_v1_loaded_reader() {
    let (store, _, ctx_id) = setup(APP_KEY_2, Some(HybridTimestamp::zero()), true);
    let app_id = ApplicationId::from(LOADED_APP_ID);
    install_application(&store, app_id, APP_KEY_1);
    install_context_meta(&store, ctx_id, app_id);

    // v2 delta after the boundary: the v1 reader cannot read it → Buffer.
    let buffer = delta_fence_decision(&store, &ctx_id, APP_KEY_2, hlc_after_zero())
        .expect("delta_fence_decision must not error");
    assert_eq!(
        buffer,
        FenceDecision::Buffer,
        "v2 delta to a v1-loaded reader must be absorbed (Buffer), not applied or dropped"
    );

    // v1 delta after the boundary: matches the loaded reader → Apply.
    let apply = delta_fence_decision(&store, &ctx_id, APP_KEY_1, hlc_after_zero())
        .expect("delta_fence_decision must not error");
    assert_eq!(
        apply,
        FenceDecision::Apply,
        "v1 delta matching the v1-loaded reader must Apply"
    );
}

/// Fallthrough: a `ContextMeta` row exists but its `ApplicationMeta` value is
/// missing (no `install_application`). The resolver must fall back to
/// `GroupMeta.app_key` (the v2 target), proving the fallback branch is reached
/// only when the loaded application row genuinely cannot be loaded.
#[test]
fn loaded_reader_falls_back_to_group_target_when_application_meta_missing() {
    let (store, _, ctx_id) = setup(APP_KEY_2, Some(HybridTimestamp::zero()), true);
    let app_id = ApplicationId::from(LOADED_APP_ID);
    // ContextMeta written, but the ApplicationMeta value is intentionally absent.
    install_context_meta(&store, ctx_id, app_id);
    assert!(
        store
            .handle()
            .get(&key::ApplicationMeta::new(app_id))
            .expect("get ApplicationMeta")
            .is_none(),
        "fixture precondition: ApplicationMeta value must be missing",
    );

    let loaded = loaded_reader_app_key(&store, &ctx_id)
        .expect("loaded_reader_app_key must not error");
    assert_eq!(
        loaded,
        Some(APP_KEY_2),
        "missing ApplicationMeta value must fall back to GroupMeta.app_key (v2)"
    );

    // And the decision degrades to the PR-3 semantics keyed on the target: a v1
    // delta after the boundary fences (Buffer), a v2 delta applies.
    let buffer = delta_fence_decision(&store, &ctx_id, APP_KEY_1, hlc_after_zero())
        .expect("delta_fence_decision must not error");
    assert_eq!(buffer, FenceDecision::Buffer);
    let apply = delta_fence_decision(&store, &ctx_id, APP_KEY_2, hlc_after_zero())
        .expect("delta_fence_decision must not error");
    assert_eq!(apply, FenceDecision::Apply);
}
