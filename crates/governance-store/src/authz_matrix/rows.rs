//! Rows and tables for the operations homed in this crate.

use calimero_account::{
    AccountGenesis, AccountMemberEndorsement, AccountProof, DeviceCert, DeviceId, DeviceScope,
    KemPublicKey, Warrant, WarrantTerms,
};
use calimero_context_client::local_governance::GroupOp;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;

use super::{
    assert_covered, assert_matches, observe, Actor, ActorState, GatedOp, Home, OpTable, Outcome,
    Row, World, NAMESPACE_MEMBERS, SUBJECT_MEMBERS,
};
use crate::test_fixtures::{device_kem_secret, device_scope};
use crate::warrant_gate::{check_delegated_delta, WarrantRefusal};
use crate::{
    build_group_key_delivery, sign_apply_local_group_op_borsh, AccountBindingRepository,
    AdmissionCut, GroupKeyring,
};
use ActorState::*;

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
        allow: &[Owner, NamespaceAdmin],
        gap: &[],
    },
    OpTable {
        op: GatedOp::DeviceDescope,
        allow: NAMESPACE_MEMBERS,
        // The signer is judged by its binding alone, which outlives the account's
        // removal from the namespace.
        gap: &[DenyListed],
    },
    OpTable {
        op: GatedOp::ForeignDescope,
        allow: &[],
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
    (GatedOp::ForeignDescope, foreign_descope),
    (GatedOp::RelayAuthor, relay_author),
];

/// Serve the current key of `group` to the actor, as a peer's pull asks for it.
/// Allowed only when the actor can open the reply and it carries that key.
fn key_pull(world: &World, actor: &Actor, group: ContextGroupId) -> Outcome {
    let (envelope, _responder) = build_group_key_delivery(
        &world.store,
        world.namespace.to_bytes().into(),
        group.to_bytes(),
        actor.requester(),
        None,
    )
    .expect("the key responder runs");
    if envelope.is_empty() {
        return Outcome::Refuse;
    }
    let (_id, current) = GroupKeyring::new(&world.store, group)
        .load_current_key()
        .expect("read the keyring")
        .expect("the world keys every group");
    assert_eq!(
        actor.open(&group, &envelope),
        Some(current),
        "the responder served something other than the group's key to its requester"
    );
    Outcome::Allow
}

fn group_key_pull(world: &World, actor: &Actor) -> Outcome {
    key_pull(world, actor, world.subject)
}

fn namespace_key_pull(world: &World, actor: &Actor) -> Outcome {
    key_pull(world, actor, world.namespace)
}

fn open_chain_key_pull(world: &World, actor: &Actor) -> Outcome {
    key_pull(world, actor, world.open_chain)
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

/// `signer` narrows `account`'s `device` out of the world's application, under a
/// scope `root` signed. Allowed when the device's binding is dropped.
fn descope(world: &World, signer: &Actor, account: &Actor, device: DeviceId) -> Outcome {
    let narrowed = DeviceScope::sign(
        &account.root,
        account.account,
        device,
        vec![ApplicationId::from([0xEF; 32])],
        2,
        0,
    )
    .expect("the root narrows its device");
    let op = GroupOp::AccountDeviceDescoped {
        account: account.account,
        device,
        application: Some(world.application),
        scope: Box::new(AccountProof {
            genesis: AccountGenesis::new(account.root.public_key()),
            chain: vec![],
            statement: narrowed,
        }),
    };
    let floor = |store: &Store| {
        AccountBindingRepository::new(store)
            .scope_floor(&world.namespace, account.account, device)
            .expect("read the scope floor")
    };
    let live = |store: &Store| {
        AccountBindingRepository::new(store)
            .live_bindings(&world.namespace)
            .expect("read live bindings")
            .iter()
            .any(|binding| binding.device == device)
    };
    let store = publish(world, &signer.sign_sk, op);
    let narrowed = floor(&store) > floor(&world.store);
    assert!(
        !narrowed || (live(&world.store) && !live(&store)),
        "a recorded narrowing drops the device's live binding"
    );
    narrowed.into()
}

/// The actor narrows the other device of its own account.
fn device_descope(world: &World, actor: &Actor) -> Outcome {
    descope(world, actor, actor, actor.peer)
}

/// The actor presents another account's root-signed narrowing of its device, as
/// anyone could replay one that rode on a link.
fn foreign_descope(world: &World, actor: &Actor) -> Outcome {
    descope(world, actor, &world.victim, world.victim.peer)
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
                matches!(
                    err.downcast_ref::<WarrantRefusal>(),
                    Some(
                        WarrantRefusal::AuthorDeviceRevoked
                            | WarrantRefusal::AuthorNotAMember
                            | WarrantRefusal::AuthorIsReadOnly
                    )
                ),
                "refused for something other than the author's standing: {err}"
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
