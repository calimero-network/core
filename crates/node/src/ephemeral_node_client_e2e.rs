//! End-to-end tests for the `NodeClient::set_local_ephemeral` /
//! `ephemeral_snapshot` API (Task 8 carry-forward of the Task 7 coverage gap).
//!
//! These tests exercise the behaviors that Task 7 specified for
//! `set_local_ephemeral` but could not test at the entry point because the
//! JSON-RPC `set_ephemeral` endpoint did not yet exist. Now that it does,
//! the same behaviors are verified here through the real
//! `NodeClient → NodeMessage → NodeManager` path — no hardcoded seq numbers,
//! no hand-crafted awareness-store calls.
//!
//! **Behaviors under test:**
//!
//! 1. First `set_local_ephemeral` call seeds the awareness store with the
//!    slice; `ephemeral_snapshot` returns exactly that entry.
//! 2. A second call with a different slice bumps the seq counter (LWW wins)
//!    and the snapshot reflects the updated slice.
//! 3. An oversized slice (> `EPHEMERAL_MAX_BYTES`) returns
//!    `Err(SliceTooLarge)` and the awareness store is NOT modified — a
//!    subsequent snapshot returns the previous (non-oversized) slice.
//!
//! Every test seeds a `ContextIdentity` row for its author via
//! `store_local_identity` — `set_local_ephemeral` now resolves a local
//! signing key synchronously before anything else, so a call for an author
//! this node holds no key for returns `Err(NoLocalSigningKey)` before ever
//! touching the awareness store.
//!
//! Run with: `cargo test -p calimero-node ephemeral_node_client_e2e`

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PrivateKey;
use serial_test::serial;

use crate::handlers::ephemeral::EPHEMERAL_MAX_BYTES;
use crate::test_node_harness::{boot_test_node, TestNode};

/// Seed the `ContextIdentity` row that marks `sk` as a local signing identity
/// for `context_id`, so `resolve_local_signing_key` (and therefore
/// `set_local_ephemeral`'s synchronous signing-key guard) finds it. Mirrors
/// `handlers::ephemeral::outbound::tests::store_local_identity`.
fn store_local_identity(node: &TestNode, context_id: &ContextId, sk: &PrivateKey) {
    let key = calimero_store::key::ContextIdentity::new(*context_id, sk.public_key());
    let value = calimero_store::types::ContextIdentity {
        private_key: Some(*sk.as_bytes()),
    };
    node.store.handle().put(&key, &value).expect("put identity");
}

// -------------------------------------------------------------------------
// Test 1 + 2: first call seeds; second call increments seq and updates slice
// -------------------------------------------------------------------------

#[tokio::test]
#[serial(boot_test_node)]
async fn set_ephemeral_seeds_then_updates_snapshot() {
    let node = boot_test_node().await;

    let context_id = ContextId::from([0xF1u8; 32]);
    let author_sk = PrivateKey::from([0xF2u8; 32]);
    let author = author_sk.public_key();
    store_local_identity(&node, &context_id, &author_sk);
    let slice1 = b"cursor={x:1,y:2}".to_vec();
    let slice2 = b"cursor={x:9,y:8}".to_vec();

    // --- First call (seq will become 1 inside the actor) ---
    node.node_client
        .set_local_ephemeral(context_id, author, slice1.clone())
        .await
        .expect("first set_local_ephemeral must succeed");

    let snap1 = node
        .node_client
        .ephemeral_snapshot(context_id)
        .await
        .expect("ephemeral_snapshot must succeed");

    assert_eq!(snap1.len(), 1, "exactly one entry after first call");
    assert_eq!(snap1[0].0, author, "author must match");
    assert_eq!(snap1[0].1, slice1, "slice must match the first set");

    // --- Second call (seq will become 2) ---
    node.node_client
        .set_local_ephemeral(context_id, author, slice2.clone())
        .await
        .expect("second set_local_ephemeral must succeed");

    let snap2 = node
        .node_client
        .ephemeral_snapshot(context_id)
        .await
        .expect("ephemeral_snapshot must succeed after second call");

    assert_eq!(snap2.len(), 1, "still exactly one entry (same author)");
    assert_eq!(
        snap2[0].1, slice2,
        "slice must reflect the second (newer) call"
    );
}

// -------------------------------------------------------------------------
// Test 3: oversized slice is rejected; awareness store is unchanged
// -------------------------------------------------------------------------

#[tokio::test]
#[serial(boot_test_node)]
async fn oversized_slice_returns_error_and_store_unchanged() {
    let node = boot_test_node().await;

    let context_id = ContextId::from([0xF3u8; 32]);
    let author_sk = PrivateKey::from([0xF4u8; 32]);
    let author = author_sk.public_key();
    store_local_identity(&node, &context_id, &author_sk);
    let good_slice = b"typing=true".to_vec();

    // Seed a valid slice first.
    node.node_client
        .set_local_ephemeral(context_id, author, good_slice.clone())
        .await
        .expect("initial set_local_ephemeral must succeed");

    // Now attempt an oversized slice — MUST fail.
    let oversized = vec![0xFFu8; EPHEMERAL_MAX_BYTES + 1];
    let err = node
        .node_client
        .set_local_ephemeral(context_id, author, oversized)
        .await
        .expect_err("oversized slice must return Err");

    let err_msg = err.to_string();
    assert!(
        err_msg.contains("too large") || err_msg.contains("SliceTooLarge"),
        "error must mention 'too large', got: {err_msg}"
    );

    // The awareness store must still hold the previous good slice.
    let snap = node
        .node_client
        .ephemeral_snapshot(context_id)
        .await
        .expect("ephemeral_snapshot must succeed");

    assert_eq!(
        snap.len(),
        1,
        "exactly one entry — oversize did not corrupt"
    );
    assert_eq!(
        snap[0].1, good_slice,
        "previous good slice must still be in the store"
    );
}
