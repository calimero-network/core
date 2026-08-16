//! Node-level tests for the provable non-member beacon, both directions.
//!
//! These boot a real `NodeManager` over the shared in-process harness. The
//! receive-path cases hand it a synthesized gossipsub `NetworkEvent`, covering
//! the wiring the pure unit tests in `handlers::network_event::readiness` cannot
//! reach: which peer the unlocked pull targets, that a stale beacon unlocks
//! nothing, and that a beacon accepted on this path never enters the readiness
//! cache. The emit-path case runs a real `ReadinessManager` over the same
//! harness and decodes what it actually published.
//!
//! Every case is `#[serial(boot_test_node)]` for the reason the sibling
//! `boot_test_node` modules are: booting a node rebinds process-global
//! singletons (the `op_events` bridges, the TEE-admit subscriber), so a
//! concurrent boot steals another module's event stream mid-assertion.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use actix::Actor;
use calimero_context_client::local_governance::{
    NamespaceOp, NamespaceTopicMsg, RootOp, SignedNamespaceOp, SignedReadinessBeacon,
};
use calimero_context_config::types::{
    ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
};
use calimero_governance_store::{now_millis, MembershipRepository, NamespaceRepository};
use calimero_network_primitives::messages::{IdentTopic, Message, MessageId, NetworkEvent};
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use libp2p::PeerId;
use serial_test::serial;
use sha2::{Digest, Sha256};
use tokio::time::sleep;

use crate::readiness::{
    EmitOutOfCycleBeacon, PendingRepublish, ReadinessCache, ReadinessConfig, ReadinessManager,
    ReadinessState, ReadinessTier,
};
use crate::test_node_harness::{boot_test_node, TestNode};
use serial_test::serial;

const NS: [u8; 32] = [42u8; 32];

/// Long enough for the actor to run a handler and for the future it spawns to
/// complete - two store reads plus the stubbed stream open on the receive path,
/// the stubbed publish on the emit path.
const SETTLE: Duration = Duration::from_millis(300);

fn signed_invitation(
    inviter_sk: &PrivateKey,
    group_id: ContextGroupId,
) -> SignedGroupOpenInvitation {
    let invitation = GroupInvitationFromAdmin {
        inviter_identity: SignerId::from(*inviter_sk.public_key().digest()),
        group_id,
        expiration_timestamp: 0,
        invitation_nonce: [0x42; 32],
        invited_role: 1,
    };
    let bytes = borsh::to_vec(&invitation).expect("borsh");
    let signature = inviter_sk
        .sign(&Sha256::digest(&bytes))
        .expect("sign invitation");
    SignedGroupOpenInvitation {
        inviter_account: None,
        invitation,
        inviter_signature: hex::encode(signature.to_bytes()),
        application_id: None,
        app_key: None,
    }
}

fn signed_beacon(
    sk: &PrivateKey,
    ts_millis: u64,
    admission_proof: Option<SignedGroupOpenInvitation>,
) -> SignedReadinessBeacon {
    let mut beacon = SignedReadinessBeacon {
        namespace_id: NS.into(),
        peer_pubkey: sk.public_key(),
        dag_head: [9u8; 32],
        applied_through: 17,
        ts_millis,
        strong: true,
        admission_proof,
        signature: [0u8; 64],
    };
    beacon.signature = sk
        .sign(&beacon.signable_bytes().expect("signable_bytes"))
        .expect("sign beacon")
        .to_bytes();
    beacon
}

/// Make this node an established member of `NS` with an inviting admin: an
/// Admin member row (so the invitation's inviter passes the permission gate)
/// plus a non-zero governance head (so the divergence check sees local state).
fn seed_established_namespace(store: &Store, admin_sk: &PrivateKey) {
    // Enrolled: the beacon's admission check resolves the inviter's KEY to the
    // account the member row is keyed by.
    let admin_account = calimero_context::test_support::enrol(
        store,
        &ContextGroupId::from(NS),
        &admin_sk.public_key(),
    );
    MembershipRepository::new(store)
        .add_member(
            &ContextGroupId::from(NS),
            &admin_account,
            GroupMemberRole::Admin,
        )
        .expect("seed admin member row");

    let mut handle = store.handle();
    handle
        .put(
            &calimero_store::key::NamespaceGovHead::new(NS),
            &calimero_store::key::NamespaceGovHeadValue {
                sequence: 1,
                dag_heads: vec![[1u8; 32]],
            },
        )
        .expect("seed namespace governance head");
}

/// Wrap `beacon` exactly as the publisher does and deliver it to the running
/// `Handler<NetworkEvent>` from `source`.
async fn deliver(node: &TestNode, source: PeerId, beacon: SignedReadinessBeacon) {
    let payload =
        borsh::to_vec(&NamespaceTopicMsg::ReadinessBeacon(beacon)).expect("borsh beacon msg");
    let envelope = BroadcastMessage::NamespaceGovernanceDelta {
        namespace_id: NS,
        delta_id: [0u8; 32],
        parent_ids: Vec::new(),
        payload,
    };
    let event = NetworkEvent::Message {
        id: MessageId(b"test-beacon".to_vec()),
        message: Message {
            source: Some(source),
            data: borsh::to_vec(&envelope).expect("borsh envelope"),
            sequence_number: Some(1),
            topic: IdentTopic::new(format!("ns/{}", hex::encode(NS))).hash(),
        },
    };
    node.node_addr
        .send(event)
        .await
        .expect("deliver NetworkEvent to node actor");
    sleep(SETTLE).await;
}

fn stream_opens(node: &TestNode) -> Vec<PeerId> {
    node.stream_opens
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Whether the established-member arm currently holds `NS`'s debounce slot.
fn member_slot_claimed(node: &TestNode) -> bool {
    node.ns_beacon_sync_debounce
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains_key(&NS)
}

#[actix::test]
#[serial(boot_test_node)]
async fn provable_beacon_pulls_from_its_signer_and_never_caches() {
    let node = boot_test_node().await;
    let admin_sk = PrivateKey::random(&mut rand::thread_rng());
    let joiner_sk = PrivateKey::random(&mut rand::thread_rng());
    seed_established_namespace(&node.store, &admin_sk);

    let inv = signed_invitation(&admin_sk, ContextGroupId::from(NS));
    let joiner_peer = PeerId::random();

    // A beacon whose wall-clock is a minute-plus behind ours is a replay
    // candidate, so it must unlock nothing - even carrying the same valid
    // invitation the fresh beacon below is accepted with.
    let stale = signed_beacon(
        &joiner_sk,
        now_millis().saturating_sub(61_000),
        Some(inv.clone()),
    );
    deliver(&node, joiner_peer, stale).await;
    assert!(
        stream_opens(&node).is_empty(),
        "a stale beacon must not trigger a pull"
    );

    // Same beacon, current clock: the pull fires, and it goes to the beacon's
    // own publisher - the only peer holding its not-yet-gossiped join op.
    let fresh = signed_beacon(&joiner_sk, now_millis(), Some(inv));
    deliver(&node, joiner_peer, fresh).await;
    assert_eq!(
        stream_opens(&node),
        vec![joiner_peer],
        "the pull must target the beacon's signer, not an arbitrary mesh peer"
    );

    // The whole point of the path: it unlocks a pull and writes nothing. A
    // non-member's advertised state must never become peer readiness state.
    assert!(
        node.readiness_cache
            .fresh_peers(NS, Duration::from_secs(60))
            .is_empty(),
        "a provable non-member beacon must never enter the readiness cache"
    );
}

#[actix::test]
#[serial(boot_test_node)]
async fn provable_pull_does_not_consume_the_member_debounce_slot() {
    let node = boot_test_node().await;
    let admin_sk = PrivateKey::random(&mut rand::thread_rng());
    let joiner_sk = PrivateKey::random(&mut rand::thread_rng());
    seed_established_namespace(&node.store, &admin_sk);

    // A seeded member's beacon verifies, so it takes the established-member arm
    // and claims that arm's slot. Nothing releases it: the member pull is not
    // targeted, so a zero-op result says nothing about the peer that beaconed.
    deliver(
        &node,
        PeerId::random(),
        signed_beacon(&admin_sk, now_millis(), None),
    )
    .await;
    assert!(
        member_slot_claimed(&node),
        "the member arm must have claimed a slot for this to be a real test"
    );

    // Same namespace, same debounce window: the provable arm has its own slot,
    // so a joiner still gets its pull. Sharing one slot would mean these two
    // arms could silence each other, whichever beaconed first.
    let inv = signed_invitation(&admin_sk, ContextGroupId::from(NS));
    let joiner_peer = PeerId::random();
    deliver(
        &node,
        joiner_peer,
        signed_beacon(&joiner_sk, now_millis(), Some(inv)),
    )
    .await;
    assert_eq!(
        stream_opens(&node),
        vec![joiner_peer],
        "an in-window member sync must not block the provable pull"
    );
}

#[actix::test]
#[serial(boot_test_node)]
async fn a_provable_pull_that_returns_nothing_gives_its_slot_back() {
    let node = boot_test_node().await;
    let admin_sk = PrivateKey::random(&mut rand::thread_rng());
    let joiner_sk = PrivateKey::random(&mut rand::thread_rng());
    seed_established_namespace(&node.store, &admin_sk);

    let inv = signed_invitation(&admin_sk, ContextGroupId::from(NS));
    let joiner_peer = PeerId::random();

    // The stub has no transport, so every pull comes back with zero ops. Both
    // beacons land far inside the debounce window, and both must still pull:
    // a pull that fetched nothing has corrected no divergence.
    for _ in 0..2 {
        deliver(
            &node,
            joiner_peer,
            signed_beacon(&joiner_sk, now_millis(), Some(inv.clone())),
        )
        .await;
    }
    assert_eq!(
        stream_opens(&node),
        vec![joiner_peer, joiner_peer],
        "a zero-op pull must not spend the window it claimed"
    );
}

#[actix::test]
#[serial(boot_test_node)]
async fn unprovable_beacon_pulls_nothing() {
    let node = boot_test_node().await;
    let admin_sk = PrivateKey::random(&mut rand::thread_rng());
    let stranger_sk = PrivateKey::random(&mut rand::thread_rng());
    seed_established_namespace(&node.store, &admin_sk);

    // Correctly signed, current clock, but no admission proof: indistinguishable
    // from today's silent drop.
    let beacon = signed_beacon(&stranger_sk, now_millis(), None);
    deliver(&node, PeerId::random(), beacon).await;

    assert!(
        stream_opens(&node).is_empty(),
        "a beacon without an admission proof must not trigger a pull"
    );
    assert!(
        node.readiness_cache
            .fresh_peers(NS, Duration::from_secs(60))
            .is_empty(),
        "an unverifiable beacon must not enter the readiness cache"
    );
}

/// A real `ReadinessManager` over the harness's store and network stub, pinned
/// to a ready tier so a probe response emits without waiting out `boot_grace`.
fn start_readiness_manager(
    node: &TestNode,
    joiner_sk: &PrivateKey,
) -> actix::Addr<ReadinessManager> {
    NamespaceRepository::new(&node.store)
        .store_identity(
            &ContextGroupId::from(NS),
            &joiner_sk.public_key(),
            joiner_sk.as_bytes(),
        )
        .expect("store namespace identity");

    let mut state_per_namespace = HashMap::new();
    let _ = state_per_namespace.insert(
        NS,
        ReadinessState {
            tier: ReadinessTier::LocallyReady,
            local_applied_through: 1,
            local_pending_ops: 0,
            subscribed_at: Instant::now(),
        },
    );

    ReadinessManager {
        cache: Arc::new(ReadinessCache::default()),
        config: ReadinessConfig::default(),
        state_per_namespace,
        node_client: node.node_client.clone(),
        datastore: node.store.clone(),
        last_probe_response_at: HashMap::new(),
        pending_republish: HashMap::new(),
    }
    .start()
}

/// The join op a joiner queues for republish when its broadcast reached nobody.
fn queued_join_op(
    joiner_sk: &PrivateKey,
    invitation: SignedGroupOpenInvitation,
) -> SignedNamespaceOp {
    SignedNamespaceOp::sign(
        joiner_sk,
        NS.into(),
        Vec::new(),
        1,
        NamespaceOp::Root(RootOp::MemberJoinedAt {
            // The account the credential beside it certifies — a pair that
            // disagrees is refused before it reaches the beacon proof.
            member: test_join_account().cert.account,
            signed_invitation: invitation,
            joined_at: 0,
            account: test_join_account(),
        }),
    )
    .expect("sign join op")
}

/// Every readiness beacon this node published, decoded through the real
/// namespace-topic envelope. Filters out the republished op the probe handler
/// also emits.
fn emitted_beacons(node: &TestNode) -> Vec<SignedReadinessBeacon> {
    node.publishes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter_map(|bytes| {
            let BroadcastMessage::NamespaceGovernanceDelta { payload, .. } =
                borsh::from_slice(bytes).ok()?
            else {
                return None;
            };
            match borsh::from_slice(&payload).ok()? {
                NamespaceTopicMsg::ReadinessBeacon(beacon) => Some(beacon),
                _ => None,
            }
        })
        .collect()
}

/// The single gossipsub payload this node put on the wire, decoded through the
/// real namespace-topic envelope.
fn only_published_msg(node: &TestNode) -> NamespaceTopicMsg {
    let published = node
        .publishes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let [bytes] = published.as_slice() else {
        panic!("expected exactly one publish, got {}", published.len());
    };
    let BroadcastMessage::NamespaceGovernanceDelta { payload, .. } =
        borsh::from_slice(bytes).expect("decode published envelope")
    else {
        panic!("a republish must use the namespace governance envelope");
    };
    borsh::from_slice(&payload).expect("decode namespace topic msg")
}

async fn emit_beacon(addr: &actix::Addr<ReadinessManager>) {
    addr.send(EmitOutOfCycleBeacon {
        namespace_id: NS,
        requesting_peer: PeerId::random(),
    })
    .await
    .expect("probe reaches the readiness actor");
    sleep(SETTLE).await;
}

/// The join path's own entry point, over the `ReadinessManager` the node mounts
/// itself: `NodeClient` -> `NodeManager` -> retry registry -> the wire. The
/// tests above start their own actor and post `PendingRepublish` directly, so a
/// break anywhere in that routing would leave every one of them green.
#[actix::test]
#[serial(boot_test_node)]
async fn a_queued_join_reaches_the_wire_through_the_node_client() {
    let node = boot_test_node().await;
    let joiner_sk = PrivateKey::random(&mut rand::thread_rng());
    let inviter_sk = PrivateKey::random(&mut rand::thread_rng());
    let op = queued_join_op(
        &joiner_sk,
        signed_invitation(&inviter_sk, ContextGroupId::from(NS)),
    );

    node.node_client.queue_membership_republish(NS, op.clone());

    // A namespace peer subscribing is what drains the registry in production.
    node.node_addr
        .send(NetworkEvent::Subscribed {
            peer_id: PeerId::random(),
            topic: IdentTopic::new(format!("ns/{}", hex::encode(NS))).hash(),
        })
        .await
        .expect("deliver Subscribed to node actor");
    sleep(SETTLE).await;

    // This node holds no readiness state for NS, so no beacon is due: the one
    // publish can only be the drain.
    let NamespaceTopicMsg::Op(republished) = only_published_msg(&node) else {
        panic!("the drained publish must carry the queued namespace op");
    };
    assert_eq!(
        borsh::to_vec(&republished).expect("encode republished op"),
        borsh::to_vec(&op).expect("encode queued op"),
        "the queued op must go back out verbatim, never re-signed"
    );
}

#[actix::test]
#[serial(boot_test_node)]
async fn queued_join_rides_the_next_beacon_as_its_admission_proof() {
    let node = boot_test_node().await;
    let joiner_sk = PrivateKey::random(&mut rand::thread_rng());
    let addr = start_readiness_manager(&node, &joiner_sk);

    let inviter_sk = PrivateKey::random(&mut rand::thread_rng());
    let invitation = signed_invitation(&inviter_sk, ContextGroupId::from(NS));
    addr.send(PendingRepublish {
        namespace_id: NS,
        op: queued_join_op(&joiner_sk, invitation.clone()),
    })
    .await
    .expect("queue the unacked join op");

    emit_beacon(&addr).await;

    let beacons = emitted_beacons(&node);
    let [beacon] = beacons.as_slice() else {
        panic!("expected exactly one emitted beacon, got {}", beacons.len());
    };
    let proof = beacon
        .admission_proof
        .as_ref()
        .expect("an unconfirmed joiner's beacon must carry its admission proof");
    assert_eq!(
        borsh::to_vec(proof).expect("encode emitted proof"),
        borsh::to_vec(&invitation).expect("encode invitation"),
        "the proof must be the invitation embedded in the queued join op"
    );
    // The proof is inside the signed body, so a receiver only trusts it if the
    // beacon still verifies with it attached.
    assert!(beacon.verify_signature().is_ok());
}

#[actix::test]
#[serial(boot_test_node)]
async fn beacon_carries_no_proof_without_a_queued_join() {
    let node = boot_test_node().await;
    let joiner_sk = PrivateKey::random(&mut rand::thread_rng());
    let addr = start_readiness_manager(&node, &joiner_sk);

    emit_beacon(&addr).await;

    let beacons = emitted_beacons(&node);
    let [beacon] = beacons.as_slice() else {
        panic!("expected exactly one emitted beacon, got {}", beacons.len());
    };
    assert!(
        beacon.admission_proof.is_none(),
        "steady state must stay one byte: no queued join means no proof"
    );
}

/// A joiner credential for tests that only need a `MemberJoinedAt` to be
/// well-formed — readiness and beacon tests assert on op flow, not on accounts.
fn test_join_account() -> Box<calimero_context_client::local_governance::JoinAccountCredential> {
    let root = calimero_primitives::identity::PublicKey::from([0x7A; 32]);
    let genesis = calimero_account::AccountGenesis::new(root);
    Box::new(
        calimero_context_client::local_governance::JoinAccountCredential {
            cert: calimero_account::DeviceCert {
                account: genesis.account_id(),
                device: calimero_account::DeviceId::from([0x3E; 32]),
                sign_pk: calimero_primitives::identity::PublicKey::from([0x7B; 32]),
                kem_pk: calimero_account::KemPublicKey::from([0x2B; 32]),
                key_epoch: 0,
                device_epoch: 0,
                signature: [0x11; 64],
            },
            genesis,
            chain: vec![],
        },
    )
}
