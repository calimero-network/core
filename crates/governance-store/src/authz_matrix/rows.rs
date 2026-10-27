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
    Row, World, NAMESPACE_MEMBERS, SPACE_MEMBERS, SUBJECT_MEMBERS,
};
use crate::test_fixtures::{binding_rejection, device_kem_secret, device_scope};
use crate::warrant_gate::{check_delegated_delta, WarrantRefusal};
use crate::{
    build_group_key_delivery, sign_apply_local_group_op_borsh, AccountBindingRepository,
    AdmissionCut, BindingRejected, GroupKeyring,
};
use ActorState::*;

const TABLES: &[OpTable] = &[
    OpTable {
        op: GatedOp::GroupKeyPull,
        allow: SPACE_MEMBERS,
        gap: &[],
    },
    OpTable {
        op: GatedOp::NamespaceKeyPull,
        allow: NAMESPACE_MEMBERS,
        gap: &[],
    },
    OpTable {
        op: GatedOp::OpenChainKeyPull,
        // The namespace key covers an Open chain, so the group's own key row is never served.
        allow: &[],
        gap: &[],
    },
    OpTable {
        op: GatedOp::DeviceLink,
        allow: NAMESPACE_MEMBERS,
        gap: &[],
    },
    OpTable {
        op: GatedOp::DeviceRevoke,
        allow: &[Owner, NamespaceAdmin, SecondDevice],
        gap: &[],
    },
    OpTable {
        op: GatedOp::DeviceDescope,
        allow: NAMESPACE_MEMBERS,
        // A removed account still writes namespace governance: the signer is
        // judged by its binding, which outlives the removal.
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
        gap: &[],
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
    // The subject is a folder in a space: its key is the space's.
    key_pull(world, actor, world.restricted)
}

fn namespace_key_pull(world: &World, actor: &Actor) -> Outcome {
    key_pull(world, actor, world.namespace)
}

fn open_chain_key_pull(world: &World, actor: &Actor) -> Outcome {
    key_pull(world, actor, world.open_chain)
}

/// Sign and apply `op` in the world's namespace on a private copy, and the apply's
/// result, so each row reads the effect it wanted and checks how it was refused.
fn publish(world: &World, signer: &PrivateKey, op: GroupOp) -> (Store, eyre::Result<()>) {
    let store = world.fork();
    let applied = sign_apply_local_group_op_borsh(&store, &world.namespace, signer, op);
    (store, applied.map(|_| ()))
}

/// A row's verdict: the effect of an op that applied, or a refusal `refused` names.
/// Any other failure is the harness's, never a refusal.
fn verdict(
    applied: eyre::Result<()>,
    effect: bool,
    refused: fn(&BindingRejected) -> bool,
) -> Outcome {
    let Err(err) = applied else {
        return effect.into();
    };
    assert!(
        binding_rejection(&err).as_ref().is_some_and(refused),
        "not the refusal this row expects: {err:#}"
    );
    assert!(!effect, "a refused op wrote its effect");
    Outcome::Refuse
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
    let (store, applied) = publish(world, &actor.sign_sk, op);
    let linked = AccountBindingRepository::new(&store)
        .live_bindings(&world.namespace)
        .expect("read live bindings")
        .iter()
        .any(|binding| binding.device == device);
    verdict(applied, linked, |reason| {
        matches!(reason, BindingRejected::AccountNotMember)
    })
}

/// The actor withdraws another member's device, with no proof from its account.
fn device_revoke(world: &World, actor: &Actor) -> Outcome {
    let op = GroupOp::AccountDeviceUnlinked {
        account: world.victim.account,
        device: world.victim.peer,
        proof: None,
    };
    let (store, applied) = publish(world, &actor.sign_sk, op);
    let revoked = AccountBindingRepository::new(&store)
        .is_revoked(&world.namespace, world.victim.peer)
        .expect("read a tombstone");
    verdict(applied, revoked, |reason| {
        matches!(reason, BindingRejected::WithdrawalUnauthorised)
    })
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
    let (store, applied) = publish(world, &signer.sign_sk, op);
    let narrowed = floor(&store) > floor(&world.store);
    assert!(
        !narrowed || (live(&world.store) && !live(&store)),
        "a recorded narrowing drops the device's live binding"
    );
    verdict(applied, narrowed, |reason| {
        matches!(reason, BindingRejected::SignerNotTheAccount)
    })
}

/// The actor narrows the other device of its own account.
fn device_descope(world: &World, actor: &Actor) -> Outcome {
    descope(world, actor, actor, actor.peer)
}

/// The actor presents another account's root-signed narrowing of its device, as
/// anyone could replay one that rode on a link.
fn foreign_descope(world: &World, actor: &Actor) -> Outcome {
    assert_eq!(
        descope(world, &world.victim, &world.victim, world.victim.peer),
        Outcome::Allow,
        "control: the account itself may present it"
    );
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
            executor_key: relay.sign_pk(),
            release_bytecode_id: [0; 32],
            release_version: String::new(),
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
