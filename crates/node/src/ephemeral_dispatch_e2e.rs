//! Routing guard for the ephemeral-presence inbound path.
//!
//! Unlike the helper-only unit tests in `handlers::ephemeral::inbound`
//! (which call `resolve_and_decrypt` / `emit_ephemeral_diff` directly),
//! this module drives a real `BroadcastMessage::Ephemeral` through the
//! **production `Handler<NetworkEvent>` match** in `handlers::network_event`
//! — the exact entrypoint a gossipsub message takes.
//!
//! The critical property: if the explicit `BroadcastMessage::Ephemeral =>
//! handle_ephemeral_broadcast(...)` arm is deleted or moved *below* the
//! `_ =>` wildcard, an `Ephemeral` message falls through to the
//! "unknown broadcast" debug arm, no decrypt / store-apply / emit runs, and
//! the awaited `ContextEventPayload::Ephemeral` never arrives — so this
//! test times out and FAILS. That is the guard the brief required and the
//! helper-only tests could not provide.

use std::time::Duration;

use calimero_context_client::group::RemoveGroupMembersRequest;
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::VisibilityMode;
use calimero_crypto::SharedKey;
use calimero_governance_store::{
    register_context_in_group, CapabilitiesRepository, GroupKeyring, MembershipRepository,
    MetaRepository, NamespaceRepository,
};
use calimero_network_primitives::messages::{IdentTopic, Message, MessageId, NetworkEvent};
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::events::{ContextEventPayload, EphemeralPayload, NodeEvent};
use calimero_primitives::identity::PrivateKey;
use calimero_store::key::{GroupMetaValue, GroupTarget};
use calimero_store::Store;
use futures_util::StreamExt;
use serial_test::serial;

use crate::handlers::ephemeral::inbound::EphemeralEnvelope;
use crate::test_node_harness::{boot_test_node, TestNode};

/// Milliseconds since the UNIX epoch — the same reading the outbound path
/// stamps and the inbound freshness gate compares against.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Borsh-encode a `BroadcastMessage::Ephemeral` and wrap it in a
/// `NetworkEvent::Message` on `topic`, exactly as the gossipsub layer would
/// hand it to the node actor.
fn ephemeral_network_event(
    source: libp2p::PeerId,
    topic: &str,
    envelope: EphemeralEnvelope,
) -> NetworkEvent {
    let EphemeralEnvelope {
        context_id,
        author,
        seq,
        key_id,
        sent_at_ms,
        nonce,
        ciphertext,
        signature,
    } = envelope;
    let payload = BroadcastMessage::Ephemeral {
        context_id,
        author,
        seq,
        key_id,
        sent_at_ms,
        nonce,
        ciphertext: ciphertext.into(),
        signature,
    };
    let data = borsh::to_vec(&payload).expect("borsh encode Ephemeral");

    NetworkEvent::Message {
        id: MessageId(b"test-ephemeral".to_vec()),
        message: Message {
            source: Some(source),
            data,
            sequence_number: Some(1),
            topic: IdentTopic::new(topic.to_owned()).hash(),
        },
    }
}

/// A decryptable `BroadcastMessage::Ephemeral`, routed through the real
/// `Handler<NetworkEvent>` dispatch, must surface a decrypted
/// `ContextEventPayload::Ephemeral` on the node's event sink.
///
/// This exercises the explicit match arm — the routing guard. Deleting the
/// arm (so `Ephemeral` hits the `_ =>` wildcard) makes the awaited event
/// never arrive and this test times out.
#[tokio::test]
#[serial(boot_test_node)]
async fn ephemeral_broadcast_routes_to_awareness_store_and_emits_event() {
    let node = boot_test_node().await;

    let context_id = ContextId::from([0xE1u8; 32]);
    let group_id = ContextGroupId::from([0xE3u8; 32]);
    register_context_in_group(&node.store, &group_id, &context_id)
        .expect("register_context_in_group");
    let group_key = [0x42u8; 32];
    let key_id = GroupKeyring::new(&node.store, group_id)
        .store_key(&group_key)
        .expect("store_key");

    let author_sk = PrivateKey::from([0xE2u8; 32]);
    let slice = b"cursor={x:7,y:3}";
    let received = dispatch_presence(
        &node,
        signed_envelope(context_id, &author_sk, group_key, key_id, slice),
        Duration::from_secs(5),
    )
    .await
    .expect(
        "expected a ContextEventPayload::Ephemeral on the sink within 5s — \
         the BroadcastMessage::Ephemeral match arm routed to handle_ephemeral_broadcast",
    );

    assert_eq!(
        received.author,
        author_sk.public_key(),
        "author must match the sender"
    );
    assert_eq!(
        received.state.as_deref(),
        Some(slice.as_ref()),
        "decrypted slice must reach the client event"
    );
    assert!(!received.removed, "an upsert must not be marked removed");
}

/// A forged `author` driven through the production dispatch must produce no
/// presence event. Companion to the positive routing test above: that one
/// fails if the match arm is deleted, this one fails if the signature check
/// is — it is the negative twin, proving the rejection survives the real
/// `Handler<NetworkEvent>` dispatch, not just the unit-level
/// `resolve_and_decrypt` call in `handlers::ephemeral::inbound`.
#[tokio::test]
#[serial(boot_test_node)]
async fn forged_author_produces_no_presence_event() {
    let node = boot_test_node().await;

    let context_id = ContextId::from([0xF1u8; 32]);
    let group_id = ContextGroupId::from([0xF3u8; 32]);
    register_context_in_group(&node.store, &group_id, &context_id)
        .expect("register_context_in_group");
    let group_key = [0x42u8; 32];
    let key_id = GroupKeyring::new(&node.store, group_id)
        .store_key(&group_key)
        .expect("store_key");

    // Decrypts cleanly under the current group key; the only thing wrong is
    // the authorship claim, which the attacker's signature does not back.
    let attacker = PrivateKey::from([0xF4u8; 32]);
    let mut envelope = signed_envelope(context_id, &attacker, group_key, key_id, b"cursor");
    envelope.author = PrivateKey::from([0xF5u8; 32]).public_key();

    let got = dispatch_presence(&node, envelope, Duration::from_secs(2)).await;
    assert!(
        got.is_none(),
        "a forged author must not produce a presence event, got {got:?}"
    );
}

/// Seal `slice` under `group_key` and sign it as `author_sk`, as the outbound
/// path does.
fn signed_envelope(
    context_id: ContextId,
    author_sk: &PrivateKey,
    group_key: [u8; 32],
    key_id: [u8; 32],
    slice: &[u8],
) -> EphemeralEnvelope {
    let author = author_sk.public_key();
    let seq = 1u64;
    let sent_at_ms = now_ms();
    let (nonce, ciphertext) = SharedKey::from_sk(&PrivateKey::from(group_key))
        .encrypt(slice.to_vec())
        .expect("encrypt");
    let payload = crate::handlers::ephemeral::auth::ephemeral_signature_payload(
        crate::handlers::ephemeral::auth::SignedEnvelope {
            context_id,
            author,
            seq,
            key_id,
            sent_at_ms,
            nonce,
            ciphertext: &ciphertext,
        },
    )
    .expect("signature payload");
    EphemeralEnvelope {
        context_id,
        author,
        seq,
        key_id,
        sent_at_ms,
        nonce,
        ciphertext,
        signature: author_sk.sign(&payload).expect("sign").to_bytes(),
    }
}

/// Deliver `envelope` through the production dispatch and return the presence
/// event it produced, or `None` if nothing surfaced within `window`.
async fn dispatch_presence(
    node: &TestNode,
    envelope: EphemeralEnvelope,
    window: Duration,
) -> Option<EphemeralPayload> {
    let context_id = envelope.context_id;
    let mut events = Box::pin(node.node_client.receive_events());
    let topic = format!("context/{}", hex::encode(context_id.as_ref()));
    node.node_addr
        .send(ephemeral_network_event(
            libp2p::PeerId::random(),
            &topic,
            envelope,
        ))
        .await
        .expect("deliver Ephemeral NetworkEvent to node actor");

    tokio::time::timeout(window, async {
        loop {
            match events.next().await {
                Some(NodeEvent::Context(ctx_event)) if ctx_event.context_id == context_id => {
                    if let ContextEventPayload::Ephemeral(payload) = ctx_event.payload {
                        break payload;
                    }
                }
                Some(_) => {}
                None => panic!("event stream closed"),
            }
        }
    })
    .await
    .ok()
}

/// Nest `child` under `parent` with the given visibility.
pub(super) fn nest(
    store: &Store,
    parent: &ContextGroupId,
    child: &ContextGroupId,
    mode: VisibilityMode,
) {
    NamespaceRepository::new(store)
        .nest(parent, child)
        .expect("nest");
    CapabilitiesRepository::new(store)
        .set_subgroup_visibility(child, mode)
        .expect("set visibility");
}

/// A member who inherits an Open subgroup holds only the namespace key, which
/// is what that subgroup's traffic is sealed under. Their presence must land.
#[tokio::test]
#[serial(boot_test_node)]
async fn inherited_member_receives_presence_on_an_open_subgroup() {
    let node = boot_test_node().await;

    let ns = ContextGroupId::from([0x61u8; 32]);
    let sub = ContextGroupId::from([0x62u8; 32]);
    let context_id = ContextId::from([0x63u8; 32]);
    nest(&node.store, &ns, &sub, VisibilityMode::Open);
    register_context_in_group(&node.store, &sub, &context_id).expect("register context");
    let ns_key = [0x64u8; 32];
    let ns_key_id = GroupKeyring::new(&node.store, ns)
        .store_key(&ns_key)
        .expect("store namespace key");

    let author_sk = PrivateKey::from([0x65u8; 32]);
    let slice = b"cursor={x:4,y:2}";
    let got = dispatch_presence(
        &node,
        signed_envelope(context_id, &author_sk, ns_key, ns_key_id, slice),
        Duration::from_secs(5),
    )
    .await
    .expect("presence sealed under the namespace key must reach an inherited member");

    assert_eq!(got.author, author_sk.public_key());
    assert_eq!(got.state.as_deref(), Some(slice.as_ref()));
    assert!(!got.removed);
}

/// Behind a Restricted wall the namespace key must open nothing, even for a
/// receiver who holds it alongside the subgroup's own key.
#[tokio::test]
#[serial(boot_test_node)]
async fn namespace_key_cannot_open_presence_behind_a_restricted_wall() {
    let node = boot_test_node().await;

    let ns = ContextGroupId::from([0x81u8; 32]);
    let ns_key = [0x82u8; 32];
    let ns_key_id = GroupKeyring::new(&node.store, ns)
        .store_key(&ns_key)
        .expect("store namespace key");

    // A Restricted subgroup, and an Open one whose parent is Restricted.
    let restricted = ContextGroupId::from([0x83u8; 32]);
    nest(&node.store, &ns, &restricted, VisibilityMode::Restricted);
    let walled_open = ContextGroupId::from([0x84u8; 32]);
    nest(&node.store, &restricted, &walled_open, VisibilityMode::Open);

    let author_sk = PrivateKey::from([0x85u8; 32]);
    for (seed, group) in [(0x86u8, restricted), (0x87u8, walled_open)] {
        let context_id = ContextId::from([seed; 32]);
        register_context_in_group(&node.store, &group, &context_id).expect("register context");
        let own_key = [seed.wrapping_add(0x10); 32];
        let own_key_id = GroupKeyring::new(&node.store, group)
            .store_key(&own_key)
            .expect("store subgroup key");

        let sealed_for_namespace = dispatch_presence(
            &node,
            signed_envelope(context_id, &author_sk, ns_key, ns_key_id, b"leak"),
            Duration::from_secs(2),
        )
        .await;
        assert!(
            sealed_for_namespace.is_none(),
            "presence sealed under the namespace key must be dropped behind a Restricted \
             wall (group {group:?}), got {sealed_for_namespace:?}"
        );

        let sealed_for_members = dispatch_presence(
            &node,
            signed_envelope(context_id, &author_sk, own_key, own_key_id, b"member"),
            Duration::from_secs(5),
        )
        .await
        .expect("presence under the subgroup's own key must still land");
        assert_eq!(
            sealed_for_members.state.as_deref(),
            Some(b"member".as_ref())
        );
    }
}

/// Removing a member from the namespace rotates the namespace key, and their
/// presence under the key they still hold is dropped from then on.
#[tokio::test]
#[serial(boot_test_node)]
async fn presence_from_a_member_removed_from_the_namespace_is_dropped() {
    let node = boot_test_node().await;

    let ns = ContextGroupId::from([0x91u8; 32]);
    let admin_sk = PrivateKey::from([0x92u8; 32]);
    let admin_pk = admin_sk.public_key();
    let admin = calimero_context::test_support::enrol(&node.store, &ns, &admin_pk);
    MetaRepository::new(&node.store)
        .save(
            &ns,
            &GroupMetaValue {
                target: GroupTarget {
                    application_id: ApplicationId::from([0xCC; 32]),
                    bytecode_id: [0xBB; 32],
                    package: Box::default(),
                    version: Box::default(),
                },
                created_at: 1_700_000_000,
                admin_identity: admin,
                owner_identity: admin,
                migration: None,
                auto_join: true,
            },
        )
        .expect("save namespace meta");
    MembershipRepository::new(&node.store)
        .add_member(&ns, &admin, GroupMemberRole::Admin)
        .expect("add admin");
    let _ = GroupKeyring::new(&node.store, ns)
        .store_key(&[0x93u8; 32])
        .expect("store namespace key");
    NamespaceRepository::new(&node.store)
        .replace_identity(&ns, &admin_pk, admin_sk.as_bytes())
        .expect("store namespace identity");

    let evicted_sk = PrivateKey::from([0x94u8; 32]);
    let evicted = calimero_context::test_support::enrol(&node.store, &ns, &evicted_sk.public_key());
    MembershipRepository::new(&node.store)
        .add_member(&ns, &evicted, GroupMemberRole::Member)
        .expect("add member");

    let sub = ContextGroupId::from([0x95u8; 32]);
    let context_id = ContextId::from([0x96u8; 32]);
    nest(&node.store, &ns, &sub, VisibilityMode::Open);
    register_context_in_group(&node.store, &sub, &context_id).expect("register context");

    let before = GroupKeyring::new(&node.store, ns)
        .load_current_key_record()
        .expect("read namespace key")
        .expect("namespace key");

    node.context_client
        .remove_group_members(RemoveGroupMembersRequest {
            group_id: ns,
            members: vec![evicted],
        })
        .await
        .expect("remove the member from the namespace");

    let after = GroupKeyring::new(&node.store, ns)
        .load_current_key_record()
        .expect("read namespace key")
        .expect("namespace key");
    assert_ne!(
        after.key_id, before.key_id,
        "removing a member from the namespace must rotate the namespace key"
    );

    let from_evicted = dispatch_presence(
        &node,
        signed_envelope(
            context_id,
            &evicted_sk,
            before.group_key,
            before.key_id,
            b"ghost",
        ),
        Duration::from_secs(2),
    )
    .await;
    assert!(
        from_evicted.is_none(),
        "presence under the pre-removal namespace key must be dropped, got {from_evicted:?}"
    );

    let from_admin = dispatch_presence(
        &node,
        signed_envelope(
            context_id,
            &admin_sk,
            after.group_key,
            after.key_id,
            b"here",
        ),
        Duration::from_secs(5),
    )
    .await
    .expect("presence under the rotated namespace key must land");
    assert_eq!(from_admin.author, admin_pk);
    assert_eq!(from_admin.state.as_deref(), Some(b"here".as_ref()));
}
