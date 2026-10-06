//! Tests for delegated context creation: the creation warrant, and the bundle
//! that carries it.

use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::{DeviceId, PrivateKey, PublicKey};

use super::support::{genesis_for, key, sign_cert};
use crate::account::AccountGenesis;
use crate::creation::{
    ContextCreationDelegation, ContextCreationTerms, ContextCreationWarrant, MAX_CREATION_LABEL_LEN,
};
use crate::device::DeviceCert;
use crate::error::AccountError;
use crate::signed::AccountProof;
use crate::warrant::{Warrant, WarrantTerms, MAX_WARRANT_CITED_HEADS};

const GROUP: [u8; 32] = [0x11; 32];
const OTHER_GROUP: [u8; 32] = [0x12; 32];
const SEED: [u8; 32] = [0x13; 32];
const APP: [u8; 32] = [0x33; 32];
const INIT_ARGS: &[u8] = br#"{"name":"general"}"#;
const ACCOUNT_HEAD: [u8; 32] = [0x44; 32];
const GOVERNANCE_HEAD: [u8; 32] = [0x55; 32];

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

fn terms(author: &Party, executor: &Party) -> ContextCreationTerms {
    ContextCreationTerms {
        group: GROUP,
        seed: SEED,
        author_account: author.account(),
        executor: executor.account(),
        executor_key: executor.device_key(),
        application_id: ApplicationId::from(APP),
        service_name: Some("chat".to_owned()),
        name: Some("general".to_owned()),
        init_hash: ContextCreationWarrant::init_hash(INIT_ARGS),
        account_heads: vec![ACCOUNT_HEAD],
        governance_floor: vec![GOVERNANCE_HEAD],
        nonce: 7,
        not_after: 1_755_903_600,
    }
}

fn fixture() -> (Party, Party, ContextCreationWarrant) {
    let author = party(1, 2, 0x01);
    let executor = party(3, 4, 0x02);
    let warrant = ContextCreationWarrant::sign(&author.device_sk, terms(&author, &executor))
        .expect("signing must succeed");
    (author, executor, warrant)
}

fn delegation(
    author: &Party,
    executor: &Party,
    warrant: ContextCreationWarrant,
) -> ContextCreationDelegation {
    ContextCreationDelegation {
        warrant: Box::new(warrant),
        author_proof: author.own_proof(),
        executor_proof: executor.own_proof(),
        executor_key: executor.device_key(),
    }
}

#[test]
fn a_minted_creation_warrant_verifies_under_the_device_that_signed_it() {
    let (author, _executor, warrant) = fixture();

    warrant
        .verify_signature()
        .expect("a creation warrant must verify under the key it names");
    assert_eq!(warrant.author_device_key, author.device_key());
}

/// Every field is in the preimage: flipping any one must break the signature,
/// or that field is something the relay could rewrite in flight.
#[test]
fn every_field_is_covered_by_the_signature() {
    let (_author, _executor, warrant) = fixture();
    let other = party(9, 10, 0x09);

    let mutations: Vec<(&str, ContextCreationWarrant)> = vec![
        (
            "group",
            ContextCreationWarrant {
                group: OTHER_GROUP,
                ..warrant.clone()
            },
        ),
        (
            "seed",
            ContextCreationWarrant {
                seed: [0x99; 32],
                ..warrant.clone()
            },
        ),
        (
            "author_account",
            ContextCreationWarrant {
                author_account: other.account(),
                ..warrant.clone()
            },
        ),
        (
            "author_device_key",
            ContextCreationWarrant {
                author_device_key: other.device_key(),
                ..warrant.clone()
            },
        ),
        (
            "executor",
            ContextCreationWarrant {
                executor: other.account(),
                ..warrant.clone()
            },
        ),
        (
            "executor_key",
            ContextCreationWarrant {
                executor_key: other.device_key(),
                ..warrant.clone()
            },
        ),
        (
            "application_id",
            ContextCreationWarrant {
                application_id: ApplicationId::from([0x99; 32]),
                ..warrant.clone()
            },
        ),
        (
            "service_name",
            ContextCreationWarrant {
                service_name: Some("other".to_owned()),
                ..warrant.clone()
            },
        ),
        (
            "service_name (removed)",
            ContextCreationWarrant {
                service_name: None,
                ..warrant.clone()
            },
        ),
        (
            "name",
            ContextCreationWarrant {
                name: Some("random".to_owned()),
                ..warrant.clone()
            },
        ),
        (
            "name (removed)",
            ContextCreationWarrant {
                name: None,
                ..warrant.clone()
            },
        ),
        (
            "init_hash",
            ContextCreationWarrant {
                init_hash: [0xcd; 32],
                ..warrant.clone()
            },
        ),
        (
            "account_heads",
            ContextCreationWarrant {
                account_heads: vec![[0x99; 32]],
                ..warrant.clone()
            },
        ),
        (
            "account_heads (emptied)",
            ContextCreationWarrant {
                account_heads: vec![],
                ..warrant.clone()
            },
        ),
        (
            "governance_floor",
            ContextCreationWarrant {
                governance_floor: vec![[0x99; 32]],
                ..warrant.clone()
            },
        ),
        (
            "governance_floor (emptied)",
            ContextCreationWarrant {
                governance_floor: vec![],
                ..warrant.clone()
            },
        ),
        (
            "nonce",
            ContextCreationWarrant {
                nonce: warrant.nonce + 1,
                ..warrant.clone()
            },
        ),
        (
            "not_after",
            ContextCreationWarrant {
                not_after: warrant.not_after + 1,
                ..warrant.clone()
            },
        ),
    ];

    for (field, mutated) in mutations {
        assert_eq!(
            mutated.verify_signature(),
            Err(AccountError::CreationSignatureInvalid),
            "mutating `{field}` must invalidate the signature"
        );
    }
}

/// `None` and `Some("")` must sign differently, or a relay could turn an
/// unnamed context into one named the empty string without the author noticing.
#[test]
fn an_absent_label_and_an_empty_label_sign_differently() {
    let (_author, _executor, warrant) = fixture();

    let unnamed = ContextCreationWarrant {
        name: None,
        ..warrant.clone()
    };
    let empty = ContextCreationWarrant {
        name: Some(String::new()),
        ..warrant.clone()
    };
    assert_ne!(unnamed.signing_payload(), empty.signing_payload());

    let no_service = ContextCreationWarrant {
        service_name: None,
        ..warrant.clone()
    };
    let empty_service = ContextCreationWarrant {
        service_name: Some(String::new()),
        ..warrant
    };
    assert_ne!(
        no_service.signing_payload(),
        empty_service.signing_payload()
    );
}

/// The two labels are adjacent in the preimage; moving bytes from one to the
/// other must not keep the signature valid.
#[test]
fn bytes_cannot_shift_between_the_two_labels() {
    let (_author, _executor, warrant) = fixture();

    let a = ContextCreationWarrant {
        service_name: Some("ab".to_owned()),
        name: Some("c".to_owned()),
        ..warrant.clone()
    };
    let b = ContextCreationWarrant {
        service_name: Some("a".to_owned()),
        name: Some("bc".to_owned()),
        ..warrant
    };
    assert_ne!(a.signing_payload(), b.signing_payload());
}

#[test]
fn a_creation_warrant_signed_by_another_key_than_it_names_is_refused() {
    let (_author, _executor, warrant) = fixture();
    let impostor = party(9, 10, 0x09);

    let forged = ContextCreationWarrant {
        author_device_key: impostor.device_key(),
        ..warrant
    };
    assert_eq!(
        forged.verify_signature(),
        Err(AccountError::CreationSignatureInvalid)
    );
}

#[test]
fn a_creation_warrant_authorises_only_its_own_group_and_executor() {
    let (_author, executor, warrant) = fixture();
    let other = party(9, 10, 0x09);

    warrant
        .authorises(GROUP, executor.account())
        .expect("its own group and executor must be authorised");
    assert_eq!(
        warrant.authorises(OTHER_GROUP, executor.account()),
        Err(AccountError::CreationGroupMismatch),
        "a creation warrant must not create a context in another group"
    );
    assert_eq!(
        warrant.authorises(GROUP, other.account()),
        Err(AccountError::WarrantExecutorMismatch {
            named: executor.account(),
            expected: other.account(),
        }),
        "a captured creation warrant must not be spendable by another operator"
    );
}

#[test]
fn the_init_commitment_covers_exactly_its_arguments() {
    let (_author, _executor, warrant) = fixture();

    assert!(warrant.covers_init(INIT_ARGS));
    assert!(!warrant.covers_init(br#"{"name":"random"}"#));
    assert!(!warrant.covers_init(b""));
}

/// The init commitment is under its own domain, so it can never double as the
/// commitment a method warrant carries — a relay must not be able to lift one
/// author consent onto the other kind of act.
#[test]
fn the_init_commitment_is_not_a_method_intent_hash() {
    assert_ne!(
        ContextCreationWarrant::init_hash(INIT_ARGS),
        Warrant::intent_hash("init", INIT_ARGS)
    );
    assert_ne!(
        ContextCreationWarrant::init_hash(INIT_ARGS),
        Warrant::intent_hash("", INIT_ARGS)
    );
}

/// A creation warrant and a method warrant with the same keys and overlapping
/// values sign different preimages, so neither signature verifies as the other.
#[test]
fn a_method_warrant_signature_does_not_verify_as_a_creation_warrant() {
    let (author, executor, creation) = fixture();

    let method = Warrant::sign(
        &author.device_sk,
        WarrantTerms {
            context: calimero_primitives::context::ContextId::from(SEED),
            author_account: author.account(),
            executor: executor.account(),
            executor_key: creation.executor_key,
            app_version: ApplicationId::from(APP),
            method: "init".to_owned(),
            intent_hash: creation.init_hash,
            account_heads: vec![ACCOUNT_HEAD],
            governance_floor: vec![GOVERNANCE_HEAD],
            nonce: creation.nonce,
            not_after: creation.not_after,
        },
    )
    .expect("sign");

    let lifted = ContextCreationWarrant {
        signature: method.signature,
        ..creation
    };
    assert_eq!(
        lifted.verify_signature(),
        Err(AccountError::CreationSignatureInvalid)
    );
}

#[test]
fn over_long_labels_are_refused_at_mint_and_at_verify() {
    let author = party(1, 2, 0x01);
    let executor = party(3, 4, 0x02);
    let too_long = "x".repeat(MAX_CREATION_LABEL_LEN + 1);

    for (field, t) in [
        (
            "name",
            ContextCreationTerms {
                name: Some(too_long.clone()),
                ..terms(&author, &executor)
            },
        ),
        (
            "service_name",
            ContextCreationTerms {
                service_name: Some(too_long.clone()),
                ..terms(&author, &executor)
            },
        ),
    ] {
        assert_eq!(
            ContextCreationWarrant::sign(&author.device_sk, t),
            Err(AccountError::CreationLabelTooLong {
                len: MAX_CREATION_LABEL_LEN + 1,
                max: MAX_CREATION_LABEL_LEN,
            }),
            "an over-long `{field}` must be refused at mint"
        );
    }

    // At the bound is fine.
    let at_bound = ContextCreationTerms {
        name: Some("x".repeat(MAX_CREATION_LABEL_LEN)),
        ..terms(&author, &executor)
    };
    let warrant = ContextCreationWarrant::sign(&author.device_sk, at_bound).expect("at the bound");
    warrant.verify_signature().expect("at the bound verifies");

    // A statement that arrives over-long from the wire is refused before the
    // signature is even looked at.
    let smuggled = ContextCreationWarrant {
        name: Some(too_long),
        ..warrant
    };
    assert!(matches!(
        smuggled.verify_signature(),
        Err(AccountError::CreationLabelTooLong { .. })
    ));
}

#[test]
fn too_many_cited_heads_are_refused() {
    let author = party(1, 2, 0x01);
    let executor = party(3, 4, 0x02);
    let heads = vec![[0x01; 32]; MAX_WARRANT_CITED_HEADS + 1];

    for t in [
        ContextCreationTerms {
            account_heads: heads.clone(),
            ..terms(&author, &executor)
        },
        ContextCreationTerms {
            governance_floor: heads.clone(),
            ..terms(&author, &executor)
        },
    ] {
        assert_eq!(
            ContextCreationWarrant::sign(&author.device_sk, t),
            Err(AccountError::WarrantTooManyCitedHeads {
                len: MAX_WARRANT_CITED_HEADS + 1,
                max: MAX_WARRANT_CITED_HEADS,
            })
        );
    }
}

#[test]
fn a_well_formed_creation_delegation_verifies() {
    let (author, executor, warrant) = fixture();
    let bundle = delegation(&author, &executor, warrant.clone());

    let verified = bundle.verify().expect("must verify");
    assert_eq!(*verified.get(), warrant);
}

/// A genuine certificate for one of the author's OTHER devices must not vouch
/// for the key that signed the warrant.
#[test]
fn an_author_proof_for_a_different_device_of_the_same_account_is_refused() {
    let (author, executor, warrant) = fixture();
    let other_device_sk = key(11);
    let other_device = DeviceId::mint(author.account(), [0x77; 16]);
    let cert = sign_cert(
        &author.root,
        author.account(),
        other_device,
        &other_device_sk,
        0,
        0,
    );
    let proof = author.proof_of(cert);
    let _valid = proof
        .verify(author.account())
        .expect("precondition: genuinely root-signed");

    let bundle = ContextCreationDelegation {
        warrant: Box::new(warrant),
        author_proof: proof,
        executor_proof: executor.own_proof(),
        executor_key: executor.device_key(),
    };
    assert_eq!(bundle.verify(), Err(AccountError::WarrantProofKeyMismatch));
}

#[test]
fn an_executor_key_the_executor_never_certified_is_refused() {
    let (author, executor, warrant) = fixture();
    let mut bundle = delegation(&author, &executor, warrant);
    bundle.executor_key = key(12).public_key();

    assert_eq!(bundle.verify(), Err(AccountError::WarrantProofKeyMismatch));
}

/// A creation warrant is spendable only by the executor device it names.
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
    let bundle = ContextCreationDelegation {
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
fn an_author_proof_for_the_wrong_account_is_refused() {
    let (_author, executor, warrant) = fixture();
    let stranger = party(9, 10, 0x09);

    let bundle = ContextCreationDelegation {
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

/// Swapping the executor's proof for the author's (both genuine) must fail: the
/// executor proof has to be for the account the warrant names as executor.
#[test]
fn an_executor_proof_for_the_author_is_refused() {
    let (author, _executor, warrant) = fixture();

    let bundle = ContextCreationDelegation {
        warrant: Box::new(warrant),
        author_proof: author.own_proof(),
        executor_proof: author.own_proof(),
        executor_key: author.device_key(),
    };
    assert!(matches!(
        bundle.verify(),
        Err(AccountError::GenesisMismatch { .. })
    ));
}

#[test]
fn a_creation_delegation_round_trips_through_borsh_and_still_verifies() {
    let (author, executor, warrant) = fixture();
    let bundle = delegation(&author, &executor, warrant.clone());

    let bytes = borsh::to_vec(&bundle).expect("encode");
    let decoded: ContextCreationDelegation = borsh::from_slice(&bytes).expect("decode");

    assert_eq!(decoded, bundle);
    assert_eq!(*decoded.verify().expect("verify").get(), warrant);
}

#[test]
fn boxing_is_invisible_on_the_wire() {
    let (author, executor, warrant) = fixture();
    let bundle = delegation(&author, &executor, warrant);

    let boxed = borsh::to_vec(&bundle).expect("encode");
    let unboxed = borsh::to_vec(&(
        &*bundle.warrant,
        &*bundle.author_proof,
        &*bundle.executor_proof,
        bundle.executor_key,
    ))
    .expect("encode");
    assert_eq!(boxed, unboxed);
}
