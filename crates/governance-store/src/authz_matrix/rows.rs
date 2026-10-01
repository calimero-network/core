//! Rows and tables for the operations homed in this crate.

use calimero_account::{
    AccountGenesis, AccountMemberEndorsement, AccountProof, DeviceCert, DeviceId, DeviceScope,
    KemPublicKey, Warrant, WarrantTerms,
};
use calimero_context_client::local_governance::GroupOp;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;

use super::{
    assert_covered, assert_matches, observe, Actor, ActorState, GatedOp, Home, OpTable, Outcome,
    Row, World,
};
use crate::test_fixtures::{device_kem_secret, device_scope};
use crate::warrant_gate::{check_delegated_delta, WarrantRefusal};
use crate::{
    build_group_key_delivery, sign_apply_local_group_op_borsh, AccountBindingRepository,
    AdmissionCut,
};
use ActorState::*;

/// Live members of the subject, by any path, on any live device of theirs.
const SUBJECT_MEMBERS: &[ActorState] = &[
    Owner,
    DirectAdmin,
    DirectMember,
    InheritedAdmin,
    InheritedMember,
    ReadmittedAfterKick,
    SecondDevice,
];

/// Accounts the namespace has admitted and not removed, speaking on a live device.
const NAMESPACE_MEMBERS: &[ActorState] = &[
    Owner,
    DirectAdmin,
    DirectMember,
    InheritedAdmin,
    InheritedMember,
    Kicked,
    Left,
    ReadmittedAfterKick,
    SecondDevice,
];

const TABLES: &[OpTable] = &[
    OpTable {
        op: GatedOp::GroupKeyPull,
        allow: SUBJECT_MEMBERS,
        // A removal from an Open subgroup leaves the inherited path standing.
        gap: &[Kicked, Left],
    },
    OpTable {
        op: GatedOp::NamespaceKeyPull,
        allow: NAMESPACE_MEMBERS,
        gap: &[],
    },
    OpTable {
        op: GatedOp::OpenChainKeyPull,
        allow: &[],
        // The group's own key row encrypts nothing while the namespace key covers
        // it, and becomes its key if it turns Restricted.
        gap: NAMESPACE_MEMBERS,
    },
    OpTable {
        op: GatedOp::DeviceLink,
        allow: NAMESPACE_MEMBERS,
        gap: &[],
    },
    OpTable {
        op: GatedOp::DeviceRevoke,
        allow: &[Owner],
        gap: &[],
    },
    OpTable {
        op: GatedOp::DeviceDescope,
        // An account may narrow its own device after its removal: it takes only
        // from itself.
        allow: &[
            Owner,
            DirectAdmin,
            DirectMember,
            InheritedAdmin,
            InheritedMember,
            Kicked,
            Left,
            DenyListed,
            ReadmittedAfterKick,
            SecondDevice,
        ],
        gap: &[],
    },
    OpTable {
        op: GatedOp::RelayAuthor,
        allow: SUBJECT_MEMBERS,
        // Revocation and narrowing are recorded at the namespace and read at the
        // context's group, so a withdrawn device still authors through a relay.
        gap: &[RevokedDevice, DescopedDevice],
    },
];

const ROWS: &[Row] = &[
    (GatedOp::GroupKeyPull, group_key_pull),
    (GatedOp::NamespaceKeyPull, namespace_key_pull),
    (GatedOp::OpenChainKeyPull, open_chain_key_pull),
    (GatedOp::DeviceLink, device_link),
    (GatedOp::DeviceRevoke, device_revoke),
    (GatedOp::DeviceDescope, device_descope),
    (GatedOp::RelayAuthor, relay_author),
];

/// Serve the current key of `group` to the actor, as a peer's pull asks for it.
fn key_pull(world: &World, actor: &Actor, group: [u8; 32]) -> Outcome {
    let namespace = world.home_namespace(actor.state);
    let (envelope, _responder) = build_group_key_delivery(
        &world.store,
        namespace.to_bytes().into(),
        group,
        actor.requester(),
        None,
    )
    .expect("the key responder runs");
    (!envelope.is_empty()).into()
}

fn group_key_pull(world: &World, actor: &Actor) -> Outcome {
    key_pull(world, actor, world.subject.to_bytes())
}

fn namespace_key_pull(world: &World, actor: &Actor) -> Outcome {
    key_pull(world, actor, world.namespace.to_bytes())
}

fn open_chain_key_pull(world: &World, actor: &Actor) -> Outcome {
    key_pull(world, actor, world.open_chain.to_bytes())
}

/// Sign and apply `op` in the world's namespace on a private copy. A refused op
/// is logged and records nothing, so each row reads the effect it wanted.
fn publish(world: &World, signer: &PrivateKey, op: GroupOp) -> Store {
    let store = world.fork();
    let _signed = sign_apply_local_group_op_borsh(&store, &world.namespace, signer, op)
        .expect("the apply records or declines the op; it does not fail");
    store
}

/// The actor pairs a new device of its own account, vouching for it itself.
fn device_link(world: &World, actor: &Actor) -> Outcome {
    let device_sk = PrivateKey::from([0xA7; 32]);
    let device = DeviceId::mint(actor.account, [0xA7; 16]);
    let cert = DeviceCert::sign(
        &actor.root,
        actor.account,
        device,
        &device_sk.public_key(),
        &KemPublicKey::from(
            *device_kem_secret(*device.as_bytes())
                .public_key()
                .as_bytes(),
        ),
        0,
        0,
    )
    .expect("the account root certifies its device");
    let op = GroupOp::AccountDeviceLinked {
        genesis: AccountGenesis::new(actor.root.public_key()),
        chain: vec![],
        cert,
        endorsement: AccountMemberEndorsement::sign(&actor.sign_sk, actor.account)
            .expect("endorse the account"),
        scope: Box::new(device_scope(&actor.root, &cert, vec![], 0)),
    };
    let store = publish(world, &actor.sign_sk, op);
    AccountBindingRepository::new(&store)
        .live_bindings(&world.namespace)
        .expect("read live bindings")
        .iter()
        .any(|binding| binding.device == device)
        .into()
}

/// The actor withdraws another member's device, with no proof from its account.
fn device_revoke(world: &World, actor: &Actor) -> Outcome {
    let op = GroupOp::AccountDeviceUnlinked {
        account: world.victim.account,
        device: world.victim.peer,
        proof: None,
    };
    let store = publish(world, &actor.sign_sk, op);
    AccountBindingRepository::new(&store)
        .is_revoked(&world.namespace, world.victim.peer)
        .expect("read a tombstone")
        .into()
}

/// The actor narrows the other device of its own account out of the world's
/// application, under a scope its account root signed.
fn device_descope(world: &World, actor: &Actor) -> Outcome {
    let narrowed = DeviceScope::sign(
        &actor.root,
        actor.account,
        actor.peer,
        vec![ApplicationId::from([0xEF; 32])],
        2,
        0,
    )
    .expect("the root narrows its device");
    let op = GroupOp::AccountDeviceDescoped {
        account: actor.account,
        device: actor.peer,
        application: Some(world.application),
        scope: Box::new(AccountProof {
            genesis: AccountGenesis::new(actor.root.public_key()),
            chain: vec![],
            statement: narrowed,
        }),
    };
    let floor = |store: &Store| {
        AccountBindingRepository::new(store)
            .scope_floor(&world.namespace, actor.account, actor.peer)
            .expect("read the scope floor")
    };
    let before = floor(&world.store);
    let store = publish(world, &actor.sign_sk, op);
    (floor(&store) > before).into()
}

/// The actor's device authors a write to the subject's context through the
/// world's relay.
fn relay_author(world: &World, actor: &Actor) -> Outcome {
    let relay = &world.relay;
    let warrant = Warrant::sign(
        &actor.sign_sk,
        WarrantTerms {
            context: world.context,
            author_account: actor.account,
            executor: relay.account,
            app_version: world.application,
            method: "set".to_owned(),
            intent_hash: Warrant::intent_hash("set", b"{}"),
            account_heads: vec![],
            governance_floor: vec![],
            nonce: 1,
            not_after: u64::MAX,
        },
    )
    .expect("the author signs its warrant");
    let delegation = calimero_account::Delegation {
        warrant: Box::new(warrant),
        author_proof: Box::new(actor.proof()),
        executor_proof: Box::new(relay.proof()),
        executor_key: relay.sign_pk(),
    };
    match check_delegated_delta(
        &world.store,
        &world.context,
        &delegation,
        AdmissionCut::live(),
    ) {
        Ok(()) => Outcome::Allow,
        Err(err) => {
            assert!(
                err.downcast_ref::<WarrantRefusal>().is_some(),
                "the gate failed rather than refused: {err}"
            );
            Outcome::Refuse
        }
    }
}

#[test]
fn every_operation_homed_here_has_a_row_and_a_table() {
    let ops: Vec<GatedOp> = ROWS.iter().map(|(op, _)| *op).collect();
    assert_covered(Home::GovernanceStore, &ops, TABLES);
}

#[test]
fn authorization_matrix() {
    assert_matches("governance-store", TABLES, &observe(ROWS));
}
