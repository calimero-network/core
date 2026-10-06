//! Tests for delegated governance: the governance warrant and its bundle.

use calimero_primitives::identity::{DeviceId, PrivateKey, PublicKey};

use super::support::{genesis_for, key, sign_cert};
use crate::account::AccountGenesis;
use crate::creation::{ContextCreationTerms, ContextCreationWarrant};
use crate::device::DeviceCert;
use crate::error::AccountError;
use crate::governance::{
    GovernanceDelegation, GovernanceOpKind, GovernanceTerms, GovernanceWarrant,
};
use crate::signed::AccountProof;
use crate::warrant::MAX_WARRANT_CITED_HEADS;

const SCOPE: [u8; 32] = [0x11; 32];
const OP: &[u8] = b"\x01member-added";

struct Party {
    root: PrivateKey,
    genesis: AccountGenesis,
    device_sk: PrivateKey,
    device: DeviceId,
}

fn party(root_seed: u8, device_seed: u8, nonce: u8) -> Party {
    let root = key(root_seed);
    let genesis = genesis_for(&root);
    let device = DeviceId::mint(genesis.account_id(), [nonce; 16]);
    Party {
        root,
        genesis,
        device_sk: key(device_seed),
        device,
    }
}

impl Party {
    fn account(&self) -> crate::AccountId {
        self.genesis.account_id()
    }
    fn device_key(&self) -> PublicKey {
        self.device_sk.public_key()
    }
    fn proof_of(&self, cert: DeviceCert) -> Box<AccountProof<DeviceCert>> {
        Box::new(AccountProof {
            genesis: self.genesis,
            chain: vec![],
            statement: cert,
        })
    }
    fn own_proof(&self) -> Box<AccountProof<DeviceCert>> {
        let cert = sign_cert(
            &self.root,
            self.account(),
            self.device,
            &self.device_sk,
            0,
            0,
        );
        self.proof_of(cert)
    }
}

fn terms(author: &Party, executor: &Party) -> GovernanceTerms {
    GovernanceTerms {
        scope: SCOPE,
        kind: GovernanceOpKind::Group,
        author_account: author.account(),
        executor: executor.account(),
        op_hash: GovernanceWarrant::op_hash(GovernanceOpKind::Group, OP),
        account_heads: vec![[0x44; 32]],
        governance_floor: vec![[0x55; 32]],
        nonce: 3,
        not_after: 1_755_903_600,
    }
}

fn fixture() -> (Party, Party, GovernanceWarrant) {
    let author = party(1, 2, 0x01);
    let executor = party(3, 4, 0x02);
    let warrant =
        GovernanceWarrant::sign(&author.device_sk, terms(&author, &executor)).expect("sign");
    (author, executor, warrant)
}

fn delegation(
    author: &Party,
    executor: &Party,
    warrant: GovernanceWarrant,
) -> GovernanceDelegation {
    GovernanceDelegation {
        warrant: Box::new(warrant),
        author_proof: author.own_proof(),
        executor_proof: executor.own_proof(),
        executor_key: executor.device_key(),
    }
}

#[test]
fn a_minted_governance_warrant_verifies() {
    let (author, _executor, warrant) = fixture();
    warrant.verify_signature().expect("verifies");
    assert_eq!(warrant.author_device_key, author.device_key());
}

#[test]
fn every_field_is_covered_by_the_signature() {
    let (_author, _executor, w) = fixture();
    let other = party(9, 10, 0x09);
    let mutations: Vec<(&str, GovernanceWarrant)> = vec![
        (
            "scope",
            GovernanceWarrant {
                scope: [0x99; 32],
                ..w.clone()
            },
        ),
        (
            "kind",
            GovernanceWarrant {
                kind: GovernanceOpKind::Root,
                ..w.clone()
            },
        ),
        (
            "author_account",
            GovernanceWarrant {
                author_account: other.account(),
                ..w.clone()
            },
        ),
        (
            "author_device_key",
            GovernanceWarrant {
                author_device_key: other.device_key(),
                ..w.clone()
            },
        ),
        (
            "executor",
            GovernanceWarrant {
                executor: other.account(),
                ..w.clone()
            },
        ),
        (
            "op_hash",
            GovernanceWarrant {
                op_hash: [0xcd; 32],
                ..w.clone()
            },
        ),
        (
            "account_heads",
            GovernanceWarrant {
                account_heads: vec![],
                ..w.clone()
            },
        ),
        (
            "governance_floor",
            GovernanceWarrant {
                governance_floor: vec![],
                ..w.clone()
            },
        ),
        (
            "nonce",
            GovernanceWarrant {
                nonce: w.nonce + 1,
                ..w.clone()
            },
        ),
        (
            "not_after",
            GovernanceWarrant {
                not_after: w.not_after + 1,
                ..w.clone()
            },
        ),
    ];
    for (field, mutated) in mutations {
        assert_eq!(
            mutated.verify_signature(),
            Err(AccountError::GovernanceSignatureInvalid),
            "mutating `{field}` must invalidate the signature"
        );
    }
}

/// The kind is inside the commitment: the same bytes as a group op and as a
/// root op are different consents.
#[test]
fn the_op_commitment_separates_the_two_planes() {
    let (_author, _executor, warrant) = fixture();
    assert!(warrant.covers_op(GovernanceOpKind::Group, OP));
    assert!(!warrant.covers_op(GovernanceOpKind::Root, OP));
    assert!(!warrant.covers_op(GovernanceOpKind::Group, b"\x01member-removed"));
    assert_ne!(
        GovernanceWarrant::op_hash(GovernanceOpKind::Group, OP),
        GovernanceWarrant::op_hash(GovernanceOpKind::Root, OP)
    );
}

/// A creation warrant's signature does not verify as a governance warrant,
/// nor the reverse: the two consents are different acts.
#[test]
fn consent_to_create_is_not_consent_to_govern() {
    let (author, executor, governance) = fixture();
    let creation = ContextCreationWarrant::sign(
        &author.device_sk,
        ContextCreationTerms {
            group: SCOPE,
            seed: [0; 32],
            author_account: author.account(),
            executor: executor.account(),
            application_id: [0; 32].into(),
            service_name: None,
            name: None,
            init_hash: governance.op_hash,
            account_heads: governance.account_heads.clone(),
            governance_floor: governance.governance_floor.clone(),
            nonce: governance.nonce,
            not_after: governance.not_after,
        },
    )
    .expect("sign");
    let lifted = GovernanceWarrant {
        signature: creation.signature,
        ..governance
    };
    assert_eq!(
        lifted.verify_signature(),
        Err(AccountError::GovernanceSignatureInvalid)
    );
}

#[test]
fn too_many_cited_heads_are_refused() {
    let author = party(1, 2, 0x01);
    let executor = party(3, 4, 0x02);
    let result = GovernanceWarrant::sign(
        &author.device_sk,
        GovernanceTerms {
            account_heads: vec![[0; 32]; MAX_WARRANT_CITED_HEADS + 1],
            ..terms(&author, &executor)
        },
    );
    assert!(matches!(
        result,
        Err(AccountError::WarrantTooManyCitedHeads { .. })
    ));
}

#[test]
fn a_well_formed_bundle_verifies_and_round_trips() {
    let (author, executor, warrant) = fixture();
    let bundle = delegation(&author, &executor, warrant.clone());
    assert_eq!(*bundle.verify().expect("verify").get(), warrant);

    let bytes = borsh::to_vec(&bundle).expect("encode");
    let decoded: GovernanceDelegation = borsh::from_slice(&bytes).expect("decode");
    assert_eq!(decoded, bundle);
}

#[test]
fn a_proof_for_another_device_or_key_is_refused() {
    let (author, executor, warrant) = fixture();
    let other_sk = key(11);
    let cert = sign_cert(
        &author.root,
        author.account(),
        DeviceId::mint(author.account(), [0x77; 16]),
        &other_sk,
        0,
        0,
    );
    let bundle = GovernanceDelegation {
        warrant: Box::new(warrant.clone()),
        author_proof: author.proof_of(cert),
        executor_proof: executor.own_proof(),
        executor_key: executor.device_key(),
    };
    assert_eq!(bundle.verify(), Err(AccountError::WarrantProofKeyMismatch));

    let mut bundle = delegation(&author, &executor, warrant);
    bundle.executor_key = key(12).public_key();
    assert_eq!(bundle.verify(), Err(AccountError::WarrantProofKeyMismatch));
}

/// A governance warrant is spendable only by the executor device it names.
#[test]
fn a_bundle_from_a_device_the_warrant_does_not_name_is_refused() {
    let (author, executor, warrant) = fixture();
    let sibling_sk = key(13);
    let sibling = sign_cert(
        &executor.root,
        executor.account(),
        DeviceId::mint(executor.account(), [0x78; 16]),
        &sibling_sk,
        0,
        0,
    );
    let bundle = GovernanceDelegation {
        warrant: Box::new(warrant),
        author_proof: author.own_proof(),
        executor_proof: executor.proof_of(sibling),
        executor_key: sibling_sk.public_key(),
    };

    let err = bundle
        .verify()
        .expect_err("a sibling device of the executor must not spend this warrant");
    assert!(err.to_string().contains("executor key"), "{err}");
}

#[test]
fn a_proof_for_the_wrong_account_is_refused() {
    let (_author, executor, warrant) = fixture();
    let stranger = party(9, 10, 0x09);
    let bundle = GovernanceDelegation {
        warrant: Box::new(warrant),
        author_proof: stranger.own_proof(),
        executor_proof: executor.own_proof(),
        executor_key: executor.device_key(),
    };
    assert!(matches!(
        bundle.verify(),
        Err(AccountError::GenesisMismatch { .. })
    ));
}
