//! Node-level test for membership-republish routing.
//!
//! This boots a real `NodeManager` over the shared in-process harness and
//! asserts on what the node actually published, decoded off the wire.
//!
//! The module used to cover beacons carrying an admission proof, and was named
//! for it. The receive-path cases went when a valid invitation stopped
//! unlocking a governance pull, and the emit-path ones went with the field
//! itself: a beacon has no optional payload left to assert on. What survives
//! is the routing case - a queued join reaching the wire through the node
//! client - which never depended on the proof.
//!
//! Every case is `#[serial(boot_test_node)]` for the reason the sibling
//! `boot_test_node` modules are: booting a node rebinds process-global
//! singletons (the `op_events` bridges, the TEE-admit subscriber), so a
//! concurrent boot steals another module's event stream mid-assertion.
use std::time::Duration;

use calimero_context_client::local_governance::{
    NamespaceOp, NamespaceTopicMsg, RootOp, SignedNamespaceOp,
};
use calimero_context_config::types::{
    ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
};
use calimero_network_primitives::messages::{IdentTopic, NetworkEvent};
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_primitives::identity::PrivateKey;
use libp2p::PeerId;
use serial_test::serial;
use sha2::{Digest, Sha256};
use tokio::time::sleep;

use crate::test_node_harness::{boot_test_node, TestNode};

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
        admitters: Vec::new(),
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
        bytecode_id: None,
        admitter_addrs: Vec::new(),
    }
}

/// The join op a joiner queues for republish when its broadcast reached nobody.
fn queued_join_op(
    joiner_sk: &PrivateKey,
    invitation: SignedGroupOpenInvitation,
) -> SignedNamespaceOp {
    let op = SignedNamespaceOp::sign(
        joiner_sk,
        NS.into(),
        Vec::new(),
        1,
        NamespaceOp::Root(RootOp::MemberJoinedAt {
            // The account the credential beside it certifies — a pair that
            // disagrees is refused before it reaches the beacon proof.
            member: test_join_account().statement.account,
            signed_invitation: invitation,
            joined_at: 0,
            account: test_join_account(),
        }),
    )
    .expect("sign join op");

    // A joiner only ever queues an endorsed join — without one it fails rather
    // than republishing, so the republish fixture carries one. On the envelope,
    // after signing: the endorsement is outside the joiner's signature.
    let mut op = op;
    op.admitter_endorsement = Some(Box::new(
        calimero_governance_types::AdmitterEndorsement::sign(
            &calimero_primitives::identity::PrivateKey::from([5u8; 32]),
            &[7u8; 32],
            &test_join_account().statement.account,
            &[0u8; 32],
        )
        .expect("sign endorsement"),
    ));
    op
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

/// A joiner credential for tests that only need a `MemberJoinedAt` to be
/// well-formed — readiness and beacon tests assert on op flow, not on accounts.
fn test_join_account() -> Box<calimero_context_client::local_governance::JoinAccountCredential> {
    let root = calimero_primitives::identity::PublicKey::from([0x7A; 32]);
    let genesis = calimero_account::AccountGenesis::new(root);
    Box::new(
        calimero_context_client::local_governance::JoinAccountCredential {
            statement: calimero_account::DeviceCert {
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
