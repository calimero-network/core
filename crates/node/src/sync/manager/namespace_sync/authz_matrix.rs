//! Authorization matrix rows for the namespace sync responders and for the key
//! a joiner or a recovering node accepts.

use std::sync::Arc;
use std::time::Duration;

use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::authz_matrix::{
    assert_covered, assert_matches, Actor, ActorState, GatedOp, Home, Observed, OpTable, Outcome,
    World, SUBJECT_MEMBERS,
};
use calimero_governance_store::test_fixtures::signed_invitation_for;
use calimero_governance_store::{member_account_in_namespace, GroupKeyring};
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::client::OpenSubgroupJoinParams;
use calimero_node_primitives::sync::{InitPayload, InitProof, MessagePayload, StreamMessage};
use calimero_primitives::context::ContextId;
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;

use super::group_key_recovery_anchor_tests::{manager_over, respond};
use super::open_subgroup_key_tests::spawn_responder;
use super::{fetch_open_subgroup_key, open_subgroup_envelope_acceptable};
use crate::sync::network::mock::MockSyncNetwork;
use crate::sync::SyncManager;
use ActorState::*;

const TABLES: &[OpTable] = &[
    OpTable {
        op: GatedOp::NamespaceJoinKey,
        // A valid invitation is the authority to join, for a newcomer too.
        allow: &[
            Owner,
            NamespaceAdmin,
            DirectAdmin,
            DirectMember,
            InheritedAdmin,
            InheritedMember,
            Kicked,
            Left,
            ReadmittedAfterKick,
            SecondDevice,
            OtherNamespaceMember,
            NonMember,
        ],
        gap: &[],
    },
    OpTable {
        op: GatedOp::OpenSubgroupJoinKey,
        allow: SUBJECT_MEMBERS,
        gap: &[],
    },
    OpTable {
        op: GatedOp::AcceptOpenSubgroupKey,
        // The subject's anchors and those of the parent it inherits from.
        allow: &[Owner, DirectAdmin, InheritedAdmin, SecondDevice],
        gap: &[],
    },
    OpTable {
        op: GatedOp::AcceptRecoveredKey,
        // The subject's own anchors only: the parent's admin is refused, though
        // it may rotate the subject's key, as key deliveries are judged.
        allow: &[Owner, DirectAdmin, SecondDevice],
        gap: &[],
    },
];

const ROWS: &[GatedOp] = &[
    GatedOp::NamespaceJoinKey,
    GatedOp::OpenSubgroupJoinKey,
    GatedOp::AcceptOpenSubgroupKey,
    GatedOp::AcceptRecoveredKey,
];

async fn row(op: GatedOp, world: &World, actor: &Actor) -> Outcome {
    match op {
        GatedOp::NamespaceJoinKey => namespace_join_key(world, actor).await,
        GatedOp::OpenSubgroupJoinKey => open_subgroup_join_key(world, actor).await,
        GatedOp::AcceptOpenSubgroupKey => accept_open_subgroup_key(world, actor).await,
        GatedOp::AcceptRecoveredKey => accept_recovered_key(world, actor).await,
        other => unreachable!("{other:?} has no row in this crate"),
    }
}

/// Dial `manager` as the actor, proving its key for the world's namespace, and
/// return the first reply to `payload`.
async fn exchange(
    manager: &SyncManager,
    world: &World,
    actor: &Actor,
    payload: InitPayload,
) -> Option<StreamMessage<'static>> {
    let peer = PeerId::random();
    let party_id = actor.sign_pk();
    let proof = InitProof {
        signature: actor
            .sign_sk
            .sign(&InitProof::message(
                &ContextId::from(world.namespace.to_bytes()),
                &party_id,
                &peer.to_bytes(),
            ))
            .expect("sign the proof")
            .to_bytes(),
    };
    let init = StreamMessage::Init {
        context_id: ContextId::from([0u8; 32]),
        party_id,
        payload,
        next_nonce: crate::sync::helpers::generate_nonce(),
        pop: Some(proof),
    };
    let (responder, mut dialer) = Stream::test_pair();
    let dial = async move {
        crate::sync::stream::send(&mut dialer, &init, None)
            .await
            .expect("send the request");
        crate::sync::stream::recv(&mut dialer, None, Duration::from_secs(5))
            .await
            .ok()
            .flatten()
    };
    let ((), reply) = tokio::join!(
        manager.handle_opened_stream(peer, Box::new(responder)),
        dial
    );
    reply
}

/// An empty envelope is a refusal; any other must carry `group`'s key to the actor.
fn served(world: &World, actor: &Actor, group: ContextGroupId, envelope: &[u8]) -> Outcome {
    if envelope.is_empty() {
        return Outcome::Refuse;
    }
    let (_id, current) = GroupKeyring::new(&world.store, group)
        .load_current_key()
        .expect("read the keyring")
        .expect("the world keys every group");
    assert_eq!(
        actor.open(&group, envelope),
        Some(current),
        "the responder served something other than the group's key"
    );
    Outcome::Allow
}

/// The actor joins the namespace on a fresh invitation from the owner.
async fn namespace_join_key(world: &World, actor: &Actor) -> Outcome {
    match join_namespace(world, actor).await {
        Some(envelope) => served(world, actor, world.namespace, &envelope),
        None => {
            // The same invitation admits a newcomer, so the refusal is about the actor.
            let control = join_namespace(world, world.actor(NonMember)).await;
            assert!(control.is_some_and(|envelope| !envelope.is_empty()));
            Outcome::Refuse
        }
    }
}

/// The key envelope a namespace join answers with, or `None` when it is rejected.
async fn join_namespace(world: &World, actor: &Actor) -> Option<Vec<u8>> {
    let (manager, _tmp) = manager_over(world.fork(), Arc::new(MockSyncNetwork::default())).await;
    let invitation = signed_invitation_for(&world.owner_sk, world.namespace, [0xE3; 32]);
    let payload = InitPayload::NamespaceJoinRequest {
        namespace_id: world.namespace.to_bytes(),
        invitation_bytes: borsh::to_vec(&invitation).expect("borsh the invitation"),
        joiner_public_key: actor.sign_pk(),
        joiner_credential_bytes: borsh::to_vec(&actor.proof()).expect("borsh the credential"),
    };
    match exchange(&manager, world, actor, payload).await {
        Some(StreamMessage::Message {
            payload:
                MessagePayload::NamespaceJoinResponse {
                    key_envelope_bytes, ..
                },
            ..
        }) => Some(key_envelope_bytes),
        Some(StreamMessage::Message {
            payload: MessagePayload::NamespaceJoinRejected { .. },
            ..
        }) => None,
        other => panic!("unexpected reply to a namespace join: {other:?}"),
    }
}

/// The actor asks for the subject's key as an Open-subgroup joiner.
async fn open_subgroup_join_key(world: &World, actor: &Actor) -> Outcome {
    let (manager, _tmp) = manager_over(world.fork(), Arc::new(MockSyncNetwork::default())).await;
    let payload = InitPayload::OpenSubgroupJoinRequest {
        namespace_id: world.namespace.to_bytes(),
        subgroup_id: world.subject.to_bytes(),
        joiner_public_key: actor.sign_pk(),
    };
    match exchange(&manager, world, actor, payload).await {
        Some(StreamMessage::Message {
            payload: MessagePayload::OpenSubgroupJoinResponse { key_envelope_bytes },
            ..
        }) => served(world, actor, world.subject, &key_envelope_bytes),
        Some(StreamMessage::Message {
            payload: MessagePayload::OpenSubgroupJoinRejected { .. },
            ..
        }) => Outcome::Refuse,
        // A key bound to no account here ends the handler before it answers.
        Some(StreamMessage::OpaqueError) => {
            assert!(
                member_account_in_namespace(&world.store, &world.subject, &actor.sign_pk())
                    .expect("resolve the joiner")
                    .is_none(),
                "the responder failed for a joiner it could resolve"
            );
            Outcome::Refuse
        }
        other => panic!("unexpected reply to an open-subgroup join: {other:?}"),
    }
}

/// The world's own node walks the mesh for the subject's key with the judge
/// `initiate_open_subgroup_join` passes, and the actor answers.
async fn accept_open_subgroup_key(world: &World, actor: &Actor) -> Outcome {
    let node = world.owner_sk.public_key();
    let mock = MockSyncNetwork::default();
    let peer = PeerId::random();
    let envelope = GroupKeyring::wrap_for_member(
        &actor.sign_sk,
        &node,
        &world.subject.to_bytes(),
        &[0x9B; 32],
    )
    .expect("wrap the key");
    let envelope = borsh::to_vec(&envelope).expect("borsh the envelope");
    let responder = spawn_responder(mock.push_open_stream_ok_with_peer(), envelope.clone());

    let joined = fetch_open_subgroup_key(
        &mock,
        &namespace_topic(world),
        &OpenSubgroupJoinParams {
            namespace_id: world.namespace.to_bytes(),
            subgroup_id: world.subject.to_bytes(),
            joiner_public_key: node,
        },
        None,
        &|envelope: &[u8]| {
            open_subgroup_envelope_acceptable(
                &world.store,
                &world.namespace,
                &world.subject,
                envelope,
            )
        },
        vec![peer],
        Duration::from_secs(5),
        crate::sync::config::OPEN_SUBGROUP_JOIN_KEY_ROUNDS,
        Duration::from_millis(crate::sync::config::OPEN_SUBGROUP_JOIN_KEY_RETRY_DELAY_MS),
    )
    .await;
    // The actor answered, so a failed join is the answer refused.
    responder.await.expect("the responder answers");
    mock.assert_all_consumed();
    match joined {
        Ok(taken) => {
            assert_eq!(taken, envelope, "the join took the actor's envelope");
            Outcome::Allow
        }
        Err(_) => Outcome::Refuse,
    }
}

fn namespace_topic(world: &World) -> TopicHash {
    TopicHash::from_raw(format!("ns/{}", hex::encode(world.namespace.to_bytes())))
}

/// The world's own node, which created the subject, lost its key and recovers
/// it; the actor answers.
async fn accept_recovered_key(world: &World, actor: &Actor) -> Outcome {
    let store = world.fork();
    let node = world.owner_sk.public_key();
    let keyring = GroupKeyring::new(&store, world.subject);
    while let Some((key_id, _key)) = keyring.load_current_key().expect("read the keyring") {
        keyring.delete_key_by_id(&key_id).expect("drop a key");
    }

    let mock = Arc::new(MockSyncNetwork::default());
    let (manager, _tmp) = manager_over(store.clone(), Arc::clone(&mock)).await;
    let peer = PeerId::random();
    let _mock = mock.push_subscribed_peers_for(namespace_topic(world), vec![peer]);
    // The answering peer relayed the owner's gossip, as any peer can; that alone
    // must not make it an anchor.
    let _previous = manager
        .node_state
        .peer_identities
        .insert(peer, [node].into_iter().collect());
    let offered = [0x9A; 32];
    let envelope =
        GroupKeyring::wrap_for_member(&actor.sign_sk, &node, &world.subject.to_bytes(), &offered)
            .expect("wrap the key");
    let responder = respond(
        mock.push_open_stream_ok_with_peer(),
        borsh::to_vec(&envelope).expect("borsh the envelope"),
        actor.sign_pk(),
    );

    manager
        .recover_missing_group_keys(world.namespace.to_bytes(), None)
        .await;
    responder.await.expect("the responder answers");
    mock.assert_all_consumed();
    let held = keyring
        .load_current_key()
        .expect("read the keyring")
        .map(|(_id, key)| key);
    (held == Some(offered)).into()
}

#[test]
fn every_operation_homed_here_has_a_row_and_a_table() {
    assert_covered(Home::Node, ROWS, TABLES);
}

#[tokio::test]
async fn authorization_matrix() {
    let world = World::shared();
    let mut observed = Observed::default();
    for op in ROWS {
        for state in ActorState::ALL {
            observed.record(*op, *state, row(*op, world, world.actor(*state)).await);
        }
    }
    assert_matches("node", TABLES, &observed);
}
