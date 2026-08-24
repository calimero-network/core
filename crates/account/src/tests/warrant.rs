//! Tests for delegated authorship: the warrant, and the bundle that carries it.

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{DeviceId, PrivateKey, PublicKey};

use super::support::{genesis_for, key, sign_cert};
use crate::account::AccountGenesis;
use crate::device::DeviceCert;
use crate::error::AccountError;
use crate::signed::AccountProof;
use crate::warrant::{Delegation, Warrant};

const CTX: [u8; 32] = [0x11; 32];
const OTHER_CTX: [u8; 32] = [0x12; 32];
const INTENT: [u8; 32] = [0xab; 32];

fn ctx(bytes: [u8; 32]) -> ContextId {
    ContextId::from(bytes)
}

/// One party: a root key, the account it addresses, and one device under it.
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

    /// A proof carrying a certificate this party's root genuinely signed.
    fn proof_of(&self, cert: DeviceCert) -> Box<AccountProof<DeviceCert>> {
        Box::new(AccountProof {
            genesis: self.genesis,
            chain: vec![],
            statement: cert,
        })
    }

    /// A proof for this party's own device.
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

/// An author, an executor, and a warrant from the first to the second.
fn fixture() -> (Party, Party, Warrant) {
    let author = party(1, 2, 0x01);
    let executor = party(3, 4, 0x02);
    let warrant = Warrant::sign(
        &author.device_sk,
        ctx(CTX),
        author.account(),
        executor.account(),
        INTENT,
        7,
        1_755_903_600,
    )
    .expect("signing must succeed");
    (author, executor, warrant)
}

fn delegation(author: &Party, executor: &Party, warrant: Warrant) -> Delegation {
    Delegation {
        warrant: Box::new(warrant),
        author_proof: author.own_proof(),
        executor_proof: executor.own_proof(),
        executor_key: executor.device_key(),
    }
}

#[test]
fn a_minted_warrant_verifies_under_the_device_that_signed_it() {
    let (_author, _executor, warrant) = fixture();

    warrant
        .verify_signature()
        .expect("a warrant must verify under the key it names");
}

/// `sign` derives the named key from the secret, so the field cannot claim a key
/// the minter does not hold.
#[test]
fn the_named_device_key_is_the_one_that_signed() {
    let (author, _executor, warrant) = fixture();

    assert_eq!(warrant.author_device_key, author.device_key());
}

/// Every field is in the preimage. Flipping any one of them has to break the
/// signature — otherwise that field is unauthenticated and a relay could rewrite
/// it in flight.
#[test]
fn every_field_is_covered_by_the_signature() {
    let (_author, _executor, warrant) = fixture();
    let other = party(9, 10, 0x09);

    let mutations: Vec<(&str, Warrant)> = vec![
        (
            "context",
            Warrant {
                context: ctx(OTHER_CTX),
                ..warrant
            },
        ),
        (
            "author_account",
            Warrant {
                author_account: other.account(),
                ..warrant
            },
        ),
        (
            "author_device_key",
            Warrant {
                author_device_key: other.device_key(),
                ..warrant
            },
        ),
        (
            "executor",
            Warrant {
                executor: other.account(),
                ..warrant
            },
        ),
        (
            "intent_hash",
            Warrant {
                intent_hash: [0xcd; 32],
                ..warrant
            },
        ),
        (
            "nonce",
            Warrant {
                nonce: warrant.nonce + 1,
                ..warrant
            },
        ),
        (
            "not_after",
            Warrant {
                not_after: warrant.not_after + 1,
                ..warrant
            },
        ),
    ];

    for (field, mutated) in mutations {
        let _refused = mutated.verify_signature().expect_err(&format!(
            "mutating `{field}` must invalidate the signature — it is otherwise \
             unauthenticated and rewritable in flight"
        ));
    }
}

#[test]
fn a_warrant_signed_by_another_key_than_it_names_is_refused() {
    let (_author, _executor, warrant) = fixture();
    let impostor = party(9, 10, 0x09);

    // Keep every signed field, swap only the key the signature is checked under.
    let forged = Warrant {
        author_device_key: impostor.device_key(),
        ..warrant
    };

    assert_eq!(
        forged.verify_signature(),
        Err(AccountError::WarrantSignatureInvalid)
    );
}

#[test]
fn a_warrant_authorises_only_its_own_context_and_executor() {
    let (_author, executor, warrant) = fixture();
    let other = party(9, 10, 0x09);

    warrant
        .authorises(ctx(CTX), executor.account())
        .expect("its own context and executor must be authorised");

    assert_eq!(
        warrant.authorises(ctx(OTHER_CTX), executor.account()),
        Err(AccountError::WarrantContextMismatch),
        "a warrant must not authorise an intent in another context"
    );

    assert_eq!(
        warrant.authorises(ctx(CTX), other.account()),
        Err(AccountError::WarrantExecutorMismatch {
            named: executor.account(),
            expected: other.account(),
        }),
        "a captured warrant must not be spendable by another operator"
    );
}

#[test]
fn a_well_formed_delegation_verifies() {
    let (author, executor, warrant) = fixture();
    let bundle = delegation(&author, &executor, warrant);

    let verified = bundle
        .verify()
        .expect("a well-formed delegation must verify");

    assert_eq!(*verified.get(), warrant);
}

/// The failure that looks like success.
///
/// The certificate is genuinely signed by the author's root and genuinely names
/// the author's account — it is simply about a *different* device. A check that
/// stopped at `AccountProof::verify` would accept it, and the warrant would then
/// be vouched for by a key its account never certified for this purpose.
#[test]
fn an_author_proof_for_a_different_device_of_the_same_account_is_refused() {
    let (author, executor, warrant) = fixture();

    let other_device_sk = key(11);
    let other_device = DeviceId::mint(author.account(), [0x77; 16]);
    let cert_for_other = sign_cert(
        &author.root,
        author.account(),
        other_device,
        &other_device_sk,
        0,
        0,
    );

    // Sanity: the proof really is valid for this account, so the rejection below
    // is the key-equality check and not a broken fixture.
    let proof = author.proof_of(cert_for_other);
    let _valid = proof
        .verify(author.account())
        .expect("precondition: the certificate must be genuinely root-signed");

    let bundle = Delegation {
        warrant: Box::new(warrant),
        author_proof: proof,
        executor_proof: executor.own_proof(),
        executor_key: executor.device_key(),
    };

    assert_eq!(bundle.verify(), Err(AccountError::WarrantProofKeyMismatch));
}

/// The same trap on the executor side: the bundle must not be able to present a
/// certificate for one of the operator's other devices to vouch for the key that
/// actually signed.
#[test]
fn an_executor_proof_for_a_different_key_than_signed_is_refused() {
    let (author, executor, warrant) = fixture();
    let mut bundle = delegation(&author, &executor, warrant);

    // Claim a key the executor's root never certified. The proof still verifies
    // for the account, so only the key-equality check can refuse this.
    bundle.executor_key = key(12).public_key();

    assert_eq!(bundle.verify(), Err(AccountError::WarrantProofKeyMismatch));
}

/// A proof whose genesis belongs to somebody else is caught by
/// `AccountProof::verify` before the key comparison is reached.
#[test]
fn an_author_proof_for_the_wrong_account_is_refused() {
    let (author, executor, warrant) = fixture();
    let stranger = party(9, 10, 0x09);

    let bundle = Delegation {
        warrant: Box::new(warrant),
        // A perfectly good proof — for the wrong account.
        author_proof: stranger.own_proof(),
        executor_proof: executor.own_proof(),
        executor_key: executor.device_key(),
    };

    let err = bundle
        .verify()
        .expect_err("a proof anchored at another account must not vouch for this author");
    assert!(
        matches!(err, AccountError::GenesisMismatch { .. }),
        "expected the genesis to be rejected, got {err:?}"
    );
    let _unused = author;
}

/// The bundle is what travels, so it has to survive the encoding it travels in.
#[test]
fn a_delegation_round_trips_through_borsh_and_still_verifies() {
    let (author, executor, warrant) = fixture();
    let bundle = delegation(&author, &executor, warrant);

    let bytes = borsh::to_vec(&bundle).expect("borsh must encode");
    let decoded: Delegation = borsh::from_slice(&bytes).expect("borsh must decode");

    assert_eq!(decoded, bundle);
    let verified = decoded
        .verify()
        .expect("a decoded delegation must still verify");
    assert_eq!(*verified.get(), warrant);
}

/// Boxing the proofs must not change the wire form — `Box<T>` encodes as `T`, and
/// this pins that so a later unboxing (or a third boxed field) cannot silently
/// alter the bytes.
#[test]
fn boxing_the_proofs_is_invisible_on_the_wire() {
    let (author, executor, warrant) = fixture();
    let bundle = delegation(&author, &executor, warrant);

    let boxed = borsh::to_vec(&bundle).expect("borsh must encode");
    // Field order, and it is load-bearing: this is the pin that catches a
    // reorder as the wire break it is.
    let unboxed = borsh::to_vec(&(
        *bundle.warrant,
        &*bundle.author_proof,
        &*bundle.executor_proof,
        bundle.executor_key,
    ))
    .expect("borsh must encode");

    assert_eq!(boxed, unboxed);
}

/// A warrant authorises one intent, not any intent. Without this the executor
/// could run whatever it liked under a genuinely signed warrant.
#[test]
fn a_warrant_covers_only_the_intent_it_was_minted_for() {
    let author = party(1, 2, 0x01);
    let executor = party(3, 4, 0x02);
    let args = br#"{"channel":"general","text":"on my way"}"#;

    let warrant = Warrant::sign(
        &author.device_sk,
        ctx(CTX),
        author.account(),
        executor.account(),
        Warrant::intent_hash("send_message", args),
        7,
        1_755_903_600,
    )
    .expect("signing must succeed");

    assert!(
        warrant.covers_intent("send_message", args),
        "the intent it was minted for must be covered"
    );
    assert!(
        !warrant.covers_intent("delete_channel", args),
        "a different method must not be covered"
    );
    assert!(
        !warrant.covers_intent("send_message", br#"{"channel":"general","text":"pwned"}"#),
        "different arguments must not be covered"
    );
}

/// The method is length-prefixed, so the split between method and arguments
/// cannot be shifted across the boundary to forge a match.
#[test]
fn the_method_args_boundary_cannot_be_shifted() {
    assert_ne!(
        Warrant::intent_hash("ab", b"x"),
        Warrant::intent_hash("a", b"bx"),
        "moving a byte across the method/args boundary must change the commitment"
    );
}
