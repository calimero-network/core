//! Unit tests for [`super::walk_for_predicate`] depth + cycle safety.
//!
//! The walk maintains its own visited-set and node cap (see the
//! `Cycle and depth safety` doc on `walk_for_predicate`). The production
//! tree-shape invariant is maintained by
//! [`nest_group`][crate::group_store::nest_group]'s pre-nest cycle check,
//! so a real production store never trips either guard. These tests
//! cover two paranoia surfaces:
//!
//! * A **deep but legitimate** chain (depth 10) — the walk must succeed,
//!   not falsely trip its cap. Asserts the cap is generous enough for
//!   the realistic upper bound of namespace nesting.
//! * A **synthesized cycle** at the store level (parent A → child B →
//!   child A, bypassing `nest_group`'s safety check via direct
//!   `GroupChildIndex` writes) — the walk must terminate cleanly rather
//!   than spinning forever, returning either bounded output or an error.
//!   This guards against a future bug that lets a cycle land in the
//!   child-index causing the cascade apply to wedge the namespace
//!   actor.

use std::sync::Arc;

use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::UpgradePolicy;
use calimero_primitives::identity::PublicKey;
use calimero_store::db::InMemoryDB;
use calimero_store::key::{GroupChildIndex, GroupMetaValue};
use calimero_store::Store;

use super::walk_for_predicate;
use crate::group_store::{nest_group, save_group_meta};

const APP_KEY_A: [u8; 32] = [0xA1; 32];

fn test_store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

fn meta_with_app_key(app_key: [u8; 32]) -> GroupMetaValue {
    GroupMetaValue {
        app_key,
        target_application_id: ApplicationId::from([0xCC; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: PublicKey::from([0x01; 32]),
        owner_identity: PublicKey::from([0x01; 32]),
        migration: None,
        auto_join: true,
    }
}

#[test]
fn walk_handles_deep_nesting() {
    // Build a strictly-linear chain root → g1 → g2 → ... → g10, all on
    // app_key A. The walk must emit 11 entries (root + 10 descendants),
    // all matched.
    let store = test_store();

    let mut groups: Vec<ContextGroupId> = Vec::with_capacity(11);
    for i in 0..11u8 {
        // Use the high nibble as a fixture marker, low nibble as the
        // chain index — keeps the bytes visually distinct in any debug
        // output if the test fails.
        let mut bytes = [0xD0u8; 32];
        bytes[0] = 0xD0 | (i & 0x0F);
        bytes[31] = i;
        let gid = ContextGroupId::from(bytes);
        groups.push(gid);
        save_group_meta(&store, &gid, &meta_with_app_key(APP_KEY_A)).unwrap();
    }
    for i in 0..10 {
        nest_group(&store, &groups[i], &groups[i + 1]).unwrap();
    }

    let entries = walk_for_predicate(&store, groups[0], APP_KEY_A).unwrap();

    assert_eq!(
        entries.len(),
        11,
        "walk over depth-10 chain must emit root + 10 descendants, got {} entries",
        entries.len()
    );
    assert!(
        entries.iter().all(|e| e.matched),
        "every entry on a uniform-app_key chain must match"
    );

    // Every fixture group must appear exactly once.
    let emitted: std::collections::HashSet<_> = entries.iter().map(|e| e.group_id).collect();
    assert_eq!(
        emitted.len(),
        11,
        "walk must not emit duplicates over a strict-tree chain"
    );
    for g in &groups {
        assert!(emitted.contains(g), "chain group {g:?} missing from walk");
    }
}

#[test]
fn walk_no_infinite_loop_on_cycle() {
    // Synthesize an A → B → A cycle by writing directly to the
    // `GroupChildIndex` keys (bypassing `nest_group`'s pre-nest cycle
    // check). The walk's own visited-set + node cap must terminate the
    // traversal cleanly rather than spinning the executor.
    let store = test_store();
    let a = ContextGroupId::from([0xAAu8; 32]);
    let b = ContextGroupId::from([0xBBu8; 32]);

    save_group_meta(&store, &a, &meta_with_app_key(APP_KEY_A)).unwrap();
    save_group_meta(&store, &b, &meta_with_app_key(APP_KEY_A)).unwrap();

    // Plant A → B AND B → A directly in the child index. This bypasses
    // `nest_group`'s cycle check — the equivalent of a corrupted store.
    {
        let mut handle = store.handle();
        handle
            .put(&GroupChildIndex::new(a.to_bytes(), b.to_bytes()), &())
            .unwrap();
        handle
            .put(&GroupChildIndex::new(b.to_bytes(), a.to_bytes()), &())
            .unwrap();
    }

    // The walk must terminate. Either:
    //   * Returns Ok with a deduped result (visited-set caught the
    //     cycle and re-pop of A was skipped) — preferred outcome.
    //   * Returns Err because the node cap fired — also acceptable; the
    //     point is the executor isn't blocked.
    let result = walk_for_predicate(&store, a, APP_KEY_A);

    match result {
        Ok(entries) => {
            // Visited-set path: A and B each appear at most once.
            let emitted: std::collections::HashSet<_> =
                entries.iter().map(|e| e.group_id).collect();
            assert_eq!(
                emitted.len(),
                entries.len(),
                "walk on a cycle must not emit duplicates: {entries:?}"
            );
            assert!(emitted.contains(&a), "walk must include the signed group A");
            // B may or may not be visited depending on the order — what
            // matters is no duplicates and bounded output.
            assert!(
                entries.len() <= 2,
                "cycle over {{A, B}} must yield at most 2 entries, got {}",
                entries.len()
            );
        }
        Err(e) => {
            // Node-cap path: at least the executor terminated. Sanity-check
            // the error mentions the cycle / cap so an operator can
            // diagnose store corruption.
            let msg = e.to_string();
            assert!(
                msg.contains("cycle") || msg.contains("cap"),
                "cycle-termination error should mention cycle/cap, got: {msg}"
            );
        }
    }
}
