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
//! 4. A context with no group — or a group with no key — returns the typed
//!    error to the CALLER rather than echoing presence that can never be
//!    published.
//!
//! Every test seeds a `ContextIdentity` row for its author via
//! `store_local_identity` AND registers the context into a keyed group via
//! `seed_group_and_key`: `set_local_ephemeral` resolves the group, the current
//! group key, and the local signing key synchronously before anything else, so
//! a call missing any of the three returns `Err` before ever touching the
//! awareness store.
//!
//! Run with: `cargo test -p calimero-node ephemeral_node_client_e2e`

use calimero_context_config::types::ContextGroupId;
use calimero_context_config::VisibilityMode;
use calimero_crypto::{Nonce, SharedKey};
use calimero_governance_store::{register_context_in_group, GroupKeyring};
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PrivateKey;
use serial_test::serial;

use crate::ephemeral_dispatch_e2e::nest;
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

/// Register `context_id` into a group and seed that group's encryption key, so
/// `set_local_ephemeral`'s synchronous group / group-key resolution succeeds.
/// Without this the call now fails fast with `NoGroup` — see
/// `no_group_error_reaches_the_caller` below.
fn seed_group_and_key(node: &TestNode, context_id: &ContextId, group_id: [u8; 32]) {
    let group_id = ContextGroupId::from(group_id);
    register_context_in_group(&node.store, &group_id, context_id).expect("register context");
    let _key_id = GroupKeyring::new(&node.store, group_id)
        .store_key(&[0x42u8; 32])
        .expect("store group key");
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
    seed_group_and_key(&node, &context_id, [0xA1u8; 32]);
    let slice1 = b"cursor={x:1,y:2}".to_vec();
    let slice2 = b"cursor={x:9,y:8}".to_vec();

    // --- First call (the actor seeds the seq from the wall clock, then bumps) ---
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

    // --- Second call (seq bumps by one from the seeded value) ---
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
    seed_group_and_key(&node, &context_id, [0xA3u8; 32]);
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

// -------------------------------------------------------------------------
// Test 4: a context that can never publish must say so to the caller
//
// These are the failures that used to be resolved inside the spawned publish
// and swallowed at `debug!`: the RPC returned Ok(()), the local awareness
// store was updated, and the setting client watched its own presence echo back
// while no peer ever saw a byte of it. They are now resolved synchronously,
// before the local echo, so they reach the caller exactly as SliceTooLarge and
// NoLocalSigningKey already did.
// -------------------------------------------------------------------------

#[tokio::test]
#[serial(boot_test_node)]
async fn no_group_error_reaches_the_caller_and_nothing_is_echoed() {
    let node = boot_test_node().await;

    let context_id = ContextId::from([0xF5u8; 32]);
    let author_sk = PrivateKey::from([0xF6u8; 32]);
    let author = author_sk.public_key();
    // Signing identity present, but the context belongs to no group.
    store_local_identity(&node, &context_id, &author_sk);

    let err = node
        .node_client
        .set_local_ephemeral(context_id, author, b"cursor={x:1}".to_vec())
        .await
        .expect_err("a context with no group must return Err, not Ok(())");
    assert!(
        err.to_string().contains("no group"),
        "error must name the missing group, got: {err}"
    );

    let snap = node
        .node_client
        .ephemeral_snapshot(context_id)
        .await
        .expect("ephemeral_snapshot must succeed");
    assert!(
        snap.is_empty(),
        "nothing may be echoed locally when the publish is impossible, got {snap:?}"
    );
}

#[tokio::test]
#[serial(boot_test_node)]
async fn no_group_key_error_reaches_the_caller_and_nothing_is_echoed() {
    let node = boot_test_node().await;

    let context_id = ContextId::from([0xF7u8; 32]);
    let author_sk = PrivateKey::from([0xF8u8; 32]);
    let author = author_sk.public_key();
    store_local_identity(&node, &context_id, &author_sk);
    // Registered in a group, but that group's keyring is empty — nothing could
    // seal the slice.
    register_context_in_group(
        &node.store,
        &ContextGroupId::from([0xA7u8; 32]),
        &context_id,
    )
    .expect("register context");

    let err = node
        .node_client
        .set_local_ephemeral(context_id, author, b"cursor={x:1}".to_vec())
        .await
        .expect_err("a group with no current key must return Err, not Ok(())");
    assert!(
        err.to_string().contains("no current group key"),
        "error must name the missing group key, got: {err}"
    );

    let snap = node
        .node_client
        .ephemeral_snapshot(context_id)
        .await
        .expect("ephemeral_snapshot must succeed");
    assert!(
        snap.is_empty(),
        "nothing may be echoed locally when the publish is impossible, got {snap:?}"
    );
}

// -------------------------------------------------------------------------
// Test 5: presence is sealed under the keyring that covers the context
// -------------------------------------------------------------------------

/// Publish `slice` as `author_sk` on `context_id` and return the `key_id`,
/// nonce and ciphertext of the envelope that went out on the wire.
async fn publish_and_capture(
    node: &TestNode,
    context_id: ContextId,
    author_sk: &PrivateKey,
    slice: &[u8],
) -> ([u8; 32], Nonce, Vec<u8>) {
    store_local_identity(node, &context_id, author_sk);
    node.node_client
        .set_local_ephemeral(context_id, author_sk.public_key(), slice.to_vec())
        .await
        .expect("set_local_ephemeral must succeed");

    for _ in 0..100 {
        let captured = node
            .publishes
            .lock()
            .expect("publishes lock")
            .iter()
            .find_map(
                |bytes| match borsh::from_slice::<BroadcastMessage<'_>>(bytes) {
                    Ok(BroadcastMessage::Ephemeral {
                        context_id: sent_to,
                        key_id,
                        nonce,
                        ciphertext,
                        ..
                    }) if sent_to == context_id => Some((key_id, nonce, ciphertext.into_owned())),
                    _ => None,
                },
            );
        if let Some(captured) = captured {
            return captured;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("no Ephemeral publish for {context_id:?} reached the network");
}

/// A member who inherits an Open subgroup holds only the namespace key. Their
/// presence must publish, sealed under that key.
#[tokio::test]
#[serial(boot_test_node)]
async fn inherited_member_publishes_presence_on_an_open_subgroup() {
    let node = boot_test_node().await;

    let ns = ContextGroupId::from([0x51u8; 32]);
    let sub = ContextGroupId::from([0x52u8; 32]);
    let context_id = ContextId::from([0x53u8; 32]);
    nest(&node.store, &ns, &sub, VisibilityMode::Open);
    register_context_in_group(&node.store, &sub, &context_id).expect("register context");
    let ns_key = [0x54u8; 32];
    let ns_key_id = GroupKeyring::new(&node.store, ns)
        .store_key(&ns_key)
        .expect("store namespace key");

    let slice = b"cursor={x:5,y:1}";
    let (key_id, nonce, ciphertext) =
        publish_and_capture(&node, context_id, &PrivateKey::from([0x55u8; 32]), slice).await;

    assert_eq!(key_id, ns_key_id, "must be sealed under the namespace key");
    let opened = SharedKey::from_sk(&PrivateKey::from(ns_key))
        .decrypt(ciphertext, nonce)
        .expect("the namespace key must open the published slice");
    assert_eq!(opened, slice);
}

/// Behind a Restricted wall presence must be sealed under the subgroup's own
/// key, never the namespace key, even on a node holding both.
#[tokio::test]
#[serial(boot_test_node)]
async fn presence_behind_a_restricted_wall_is_not_sealed_for_the_namespace() {
    let node = boot_test_node().await;

    let ns = ContextGroupId::from([0x56u8; 32]);
    let ns_key_id = GroupKeyring::new(&node.store, ns)
        .store_key(&[0x57u8; 32])
        .expect("store namespace key");
    let restricted = ContextGroupId::from([0x58u8; 32]);
    nest(&node.store, &ns, &restricted, VisibilityMode::Restricted);
    let walled_open = ContextGroupId::from([0x59u8; 32]);
    nest(&node.store, &restricted, &walled_open, VisibilityMode::Open);

    for (seed, group) in [(0x5Au8, restricted), (0x5Bu8, walled_open)] {
        let context_id = ContextId::from([seed; 32]);
        register_context_in_group(&node.store, &group, &context_id).expect("register context");
        let own_key_id = GroupKeyring::new(&node.store, group)
            .store_key(&[seed.wrapping_add(0x10); 32])
            .expect("store subgroup key");

        let (key_id, ..) =
            publish_and_capture(&node, context_id, &PrivateKey::from([seed; 32]), b"x").await;
        assert_ne!(
            key_id, ns_key_id,
            "group {group:?} must not seal for the namespace"
        );
        assert_eq!(
            key_id, own_key_id,
            "group {group:?} must seal under its own key"
        );
    }
}
