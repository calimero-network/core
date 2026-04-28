use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use libp2p::gossipsub::TopicHash;

use super::*;

/// Build an unsigned ack for tests where the signature would be checked
/// downstream (verify_ack tests). Pairs with `signed_ack` below.
fn dummy_ack(op_hash: [u8; 32]) -> SignedAck {
    SignedAck {
        op_hash,
        signer_pubkey: PrivateKey::random(&mut rand::thread_rng()).public_key(),
        signature: [0u8; 64],
    }
}

// ---------------------------------------------------------------------------
// verify_ack
// ---------------------------------------------------------------------------

fn empty_store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

/// Build a properly-signed ack (domain-separated bytes) for a given
/// `op_hash`, signed by `sk`.
fn signed_ack(sk: &PrivateKey, op_hash: [u8; 32]) -> SignedAck {
    let msg = SignedAck::signable_bytes(&op_hash);
    let signature = sk.sign(&msg).expect("sign").to_bytes();
    SignedAck {
        op_hash,
        signer_pubkey: sk.public_key(),
        signature,
    }
}

#[tokio::test]
async fn verify_ack_rejects_wrong_op_hash() {
    let store = empty_store();
    let sk = PrivateKey::random(&mut rand::thread_rng());
    let ack = signed_ack(&sk, [1u8; 32]);
    // Caller is waiting on a different op_hash than the ack carries.
    assert!(!verify_ack(&store, [42u8; 32], [9u8; 32], &ack));
}

#[tokio::test]
async fn verify_ack_rejects_invalid_signature() {
    let store = empty_store();
    // dummy_ack uses [0u8; 64] — Ed25519 verification fails before we
    // ever consult the membership store.
    let ack = dummy_ack([1u8; 32]);
    assert!(!verify_ack(&store, [42u8; 32], [1u8; 32], &ack));
}

#[tokio::test]
async fn verify_ack_rejects_non_member_signer() {
    let store = empty_store();
    // Properly-signed ack, but `store` has no namespace members at all.
    let sk = PrivateKey::random(&mut rand::thread_rng());
    let ack = signed_ack(&sk, [7u8; 32]);
    assert!(!verify_ack(&store, [42u8; 32], [7u8; 32], &ack));
}

// ---------------------------------------------------------------------------
// assert_transport_ready
// ---------------------------------------------------------------------------

#[test]
fn assert_transport_ready_passes_when_solo_namespace() {
    // known_subscribers=0 ⇒ required=0 ⇒ pass regardless of mesh size.
    assert!(assert_transport_ready(0, 0, 4).is_ok());
}

#[test]
fn assert_transport_ready_rejects_when_mesh_below_threshold() {
    let err = assert_transport_ready(1, 4, 4).unwrap_err();
    assert!(matches!(
        err,
        GovernanceBroadcastError::NamespaceNotReady {
            mesh: 1,
            required: 4
        }
    ));
}

#[test]
fn assert_transport_ready_caps_required_by_known_subscribers() {
    // Only 1 subscriber known ⇒ required=1; mesh=1 should pass even
    // though mesh_n_low=4 (a small namespace can never reach the full
    // gossipsub quorum, but is still safe to publish on).
    assert!(assert_transport_ready(1, 1, 4).is_ok());
}

#[test]
fn assert_transport_ready_passes_when_mesh_exceeds_required() {
    // mesh=8 > required=min(4, 6)=4 ⇒ pass.
    assert!(assert_transport_ready(8, 6, 4).is_ok());
}

// ---------------------------------------------------------------------------
// timeout_for_namespace_op
// ---------------------------------------------------------------------------

#[test]
fn timeout_classifier_assigns_per_op_kind() {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp};

    let pk = PrivateKey::random(&mut rand::thread_rng()).public_key();

    // Cheap class: single-row writes, no inheritance walk.
    assert_eq!(
        timeout_for_namespace_op(&NamespaceOp::Root(RootOp::AdminChanged { new_admin: pk })),
        OP_ACK_CHEAP_TIMEOUT
    );

    // Member-change class: membership-table mutations, possible inheritance walks.
    assert_eq!(
        timeout_for_namespace_op(&NamespaceOp::Root(RootOp::GroupCreated {
            group_id: [0u8; 32],
            parent_id: [0u8; 32],
        })),
        OP_ACK_MEMBER_CHANGE_TIMEOUT
    );

    // Heavy class: cascade deletes / KeyDelivery side-effects.
    assert_eq!(
        timeout_for_namespace_op(&NamespaceOp::Root(RootOp::GroupDeleted {
            root_group_id: [0u8; 32],
            cascade_group_ids: Vec::new(),
            cascade_context_ids: Vec::new(),
        })),
        OP_ACK_HEAVY_TIMEOUT
    );

    // Encrypted Group ops: classified as member-change baseline because
    // the inner GroupOp variant isn't visible without decrypting.
    assert_eq!(
        timeout_for_namespace_op(&NamespaceOp::Group {
            group_id: [0u8; 32],
            key_id: [0u8; 32],
            encrypted: calimero_context_client::local_governance::EncryptedGroupOp {
                ciphertext: Vec::new(),
                nonce: [0u8; 12],
            },
            key_rotation: None,
        }),
        OP_ACK_MEMBER_CHANGE_TIMEOUT
    );
}

// ---------------------------------------------------------------------------
// publish_and_await_ack_namespace
// ---------------------------------------------------------------------------

struct StubTransport;

#[async_trait]
impl BroadcastTransport for StubTransport {
    async fn mesh_peer_count(&self, _: TopicHash) -> usize {
        0
    }
    async fn publish(&self, _: TopicHash, _: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
}

fn mk_signed_op(sk: &PrivateKey, namespace_id: [u8; 32]) -> SignedNamespaceOp {
    SignedNamespaceOp::sign(
        sk,
        namespace_id,
        Vec::new(),
        [0u8; 32],
        0,
        NamespaceOp::Root(RootOp::AdminChanged {
            new_admin: sk.public_key(),
        }),
    )
    .expect("sign")
}

fn plant_namespace_member(
    store: &Store,
    namespace_id: [u8; 32],
    pk: &calimero_primitives::identity::PublicKey,
) {
    let gid = ContextGroupId::from(namespace_id);
    crate::group_store::add_group_member(store, &gid, pk, GroupMemberRole::Member).expect("plant");
}

#[tokio::test]
async fn publish_and_await_ack_times_out_when_no_ack_arrives() {
    let store = empty_store();
    let router = AckRouter::default();
    let transport = StubTransport;
    let topic = TopicHash::from_raw("ns/test");
    let signer = PrivateKey::random(&mut rand::thread_rng());
    let signed_op = mk_signed_op(&signer, [42u8; 32]);

    let res = publish_and_await_ack_namespace(
        &store,
        &transport,
        &router,
        [42u8; 32],
        topic,
        signed_op,
        Duration::from_millis(50),
        1,
        None,
    )
    .await;

    assert!(matches!(
        res,
        Err(GovernanceBroadcastError::NoAckReceived { .. })
    ));
}

#[tokio::test]
async fn publish_and_await_ack_dedups_acks_from_same_signer() {
    // min_acks=2 must NOT be satisfied by 3 acks from a single signer —
    // each `signer_pubkey` counts once toward the threshold.
    let store = empty_store();
    let router = Arc::new(AckRouter::default());
    let transport = StubTransport;
    let topic = TopicHash::from_raw("ns/test-dedup");
    let namespace_id = [42u8; 32];

    // The op signer publishes; alice (a different identity) is the
    // namespace member that acks.
    let publisher = PrivateKey::random(&mut rand::thread_rng());
    let alice = PrivateKey::random(&mut rand::thread_rng());
    plant_namespace_member(&store, namespace_id, &publisher.public_key());
    plant_namespace_member(&store, namespace_id, &alice.public_key());

    let signed_op = mk_signed_op(&publisher, namespace_id);
    let op_hash = calimero_context_client::local_governance::hash_scoped_namespace(
        topic.as_str().as_bytes(),
        &signed_op,
    )
    .expect("hash");

    let alice_pk = alice.public_key();
    let alice_signature = alice
        .sign(&SignedAck::signable_bytes(&op_hash))
        .expect("sign")
        .to_bytes();
    let router_clone = Arc::clone(&router);
    tokio::spawn(async move {
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            router_clone.route(SignedAck {
                op_hash,
                signer_pubkey: alice_pk,
                signature: alice_signature,
            });
        }
    });

    let res = publish_and_await_ack_namespace(
        &store,
        &transport,
        &router,
        namespace_id,
        topic,
        signed_op,
        Duration::from_millis(200),
        2,
        None,
    )
    .await;
    assert!(
        matches!(res, Err(GovernanceBroadcastError::NoAckReceived { .. })),
        "min_acks=2 must not be satisfied by 3 acks from one signer; got {:?}",
        res
    );
}

#[tokio::test]
async fn publish_and_await_ack_returns_ok_on_min_acks_satisfied() {
    // Happy path: two distinct member signers each ack once;
    // min_acks=2 is satisfied and DeliveryReport is returned.
    let store = empty_store();
    let router = Arc::new(AckRouter::default());
    let transport = StubTransport;
    let topic = TopicHash::from_raw("ns/test-happy");
    let namespace_id = [42u8; 32];

    let publisher = PrivateKey::random(&mut rand::thread_rng());
    let alice = PrivateKey::random(&mut rand::thread_rng());
    let bob = PrivateKey::random(&mut rand::thread_rng());
    plant_namespace_member(&store, namespace_id, &publisher.public_key());
    plant_namespace_member(&store, namespace_id, &alice.public_key());
    plant_namespace_member(&store, namespace_id, &bob.public_key());

    let signed_op = mk_signed_op(&publisher, namespace_id);
    let op_hash = calimero_context_client::local_governance::hash_scoped_namespace(
        topic.as_str().as_bytes(),
        &signed_op,
    )
    .expect("hash");

    let signable = SignedAck::signable_bytes(&op_hash);
    let alice_ack = SignedAck {
        op_hash,
        signer_pubkey: alice.public_key(),
        signature: alice.sign(&signable).expect("sign").to_bytes(),
    };
    let bob_ack = SignedAck {
        op_hash,
        signer_pubkey: bob.public_key(),
        signature: bob.sign(&signable).expect("sign").to_bytes(),
    };
    let router_clone = Arc::clone(&router);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        router_clone.route(alice_ack);
        tokio::time::sleep(Duration::from_millis(10)).await;
        router_clone.route(bob_ack);
    });

    let report = publish_and_await_ack_namespace(
        &store,
        &transport,
        &router,
        namespace_id,
        topic,
        signed_op,
        Duration::from_millis(500),
        2,
        None,
    )
    .await
    .expect("happy path must return Ok(DeliveryReport)");

    assert_eq!(report.op_hash, op_hash);
    assert_eq!(report.acked_by.len(), 2);
    assert!(report.acked_by.contains(&alice.public_key()));
    assert!(report.acked_by.contains(&bob.public_key()));
}

#[tokio::test]
async fn publish_and_await_ack_returns_ok_immediately_when_min_acks_is_zero() {
    // `min_acks == 0` means "publish-only, no wait". Without the early
    // short-circuit the collect loop sits at `tokio::time::timeout` for
    // the full `op_timeout` (no ack ever arrives) and then returns
    // `NoAckReceived` instead of `Ok` — the wrong outcome for the
    // contract. Test deadline is intentionally large so a regressed
    // implementation would visibly time out rather than appear to pass.
    let store = empty_store();
    let router = AckRouter::default();
    let transport = StubTransport;
    let topic = TopicHash::from_raw("ns/min-acks-zero");
    let signer = PrivateKey::random(&mut rand::thread_rng());
    let signed_op = mk_signed_op(&signer, [42u8; 32]);

    let started = std::time::Instant::now();
    let res = publish_and_await_ack_namespace(
        &store,
        &transport,
        &router,
        [42u8; 32],
        topic,
        signed_op,
        Duration::from_secs(5),
        0,
        None,
    )
    .await;

    let report = res.expect("min_acks=0 must return Ok immediately");
    assert!(report.acked_by.is_empty());
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "min_acks=0 must not wait for op_timeout; elapsed = {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn verify_ack_rejects_signature_without_domain_prefix() {
    // Defense in depth: a signer that signed `op_hash` directly (without
    // the ACK_SIGN_DOMAIN prefix) must not have their ack accepted, even
    // if the signer is otherwise legitimate. This prevents lifting a
    // signature from another protocol surface that happens to sign a
    // 32-byte hash.
    let store = empty_store();
    let sk = PrivateKey::random(&mut rand::thread_rng());
    let op_hash = [11u8; 32];
    let signature = sk.sign(&op_hash).expect("sign").to_bytes(); // no domain prefix
    let ack = SignedAck {
        op_hash,
        signer_pubkey: sk.public_key(),
        signature,
    };
    assert!(!verify_ack(&store, [42u8; 32], op_hash, &ack));
}

/// Transport stub whose publish always returns the libp2p
/// `NoPeersSubscribedToTopic` error — used to exercise the
/// solo-namespace / mesh-not-yet-grafted code path in
/// `publish_and_await_ack_namespace` without standing up a real swarm.
struct NoPeersTransport;

#[async_trait]
impl BroadcastTransport for NoPeersTransport {
    async fn mesh_peer_count(&self, _: TopicHash) -> usize {
        0
    }
    async fn publish(&self, _: TopicHash, _: Vec<u8>) -> Result<(), String> {
        Err("InsufficientPeers(NoPeersSubscribedToTopic)".to_owned())
    }
}

#[tokio::test]
async fn no_peers_with_min_acks_zero_returns_ok_empty() {
    // Solo-namespace fast-path: caller passed `min_acks = 0` to opt out
    // of confirmation, so the no-peers publish error is a legitimate
    // Ok-with-empty outcome (matches legacy `publish_signed_namespace_op`
    // best-effort semantics).
    let store = empty_store();
    let router = AckRouter::default();
    let transport = NoPeersTransport;
    let topic = TopicHash::from_raw("ns/test-solo");
    let signer = PrivateKey::random(&mut rand::thread_rng());
    let signed_op = mk_signed_op(&signer, [42u8; 32]);

    let report = publish_and_await_ack_namespace(
        &store,
        &transport,
        &router,
        [42u8; 32],
        topic,
        signed_op,
        Duration::from_millis(50),
        0,
        None,
    )
    .await
    .expect("solo namespace must return Ok with empty acks");

    assert!(report.acked_by.is_empty());
}

#[tokio::test]
async fn no_peers_with_min_acks_positive_returns_no_ack_received() {
    // When the caller asked for confirmation (`min_acks > 0`), receiving
    // `NoPeersSubscribedToTopic` from the transport must not be silently
    // promoted to `Ok(DeliveryReport { acked_by: [] })` — that would lie
    // about the contract and let workflow steps that claim "returns only
    // after node-2 acks" pass without actually delivering.
    let store = empty_store();
    let router = AckRouter::default();
    let transport = NoPeersTransport;
    let topic = TopicHash::from_raw("ns/test-multi");
    let signer = PrivateKey::random(&mut rand::thread_rng());
    let signed_op = mk_signed_op(&signer, [42u8; 32]);

    let res = publish_and_await_ack_namespace(
        &store,
        &transport,
        &router,
        [42u8; 32],
        topic,
        signed_op,
        Duration::from_millis(50),
        1,
        None,
    )
    .await;

    assert!(
        matches!(res, Err(GovernanceBroadcastError::NoAckReceived { .. })),
        "min_acks=1 + NoPeersSubscribedToTopic must surface as NoAckReceived; got {:?}",
        res
    );
}
