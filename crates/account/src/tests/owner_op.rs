//! An owner-op authorisation must verify from the account id alone, and bind
//! every term it names so that none of them can be swapped after signing.

use calimero_primitives::identity::PrivateKey;

use super::support::rotated;
use crate::account::AccountGenesis;
use crate::error::AccountError;
use crate::owner_op::{OwnerOpAuthorization, OwnerOpKind, OwnerOpTerms, SignedOwnerOp};

fn terms(root: &PrivateKey, key_epoch: u32) -> OwnerOpTerms {
    let account = AccountGenesis::new(root.public_key()).account_id();
    OwnerOpTerms {
        account,
        namespace_id: [0x11; 32],
        group_id: [0x22; 32],
        kind: OwnerOpKind::TransferOwnership,
        op_digest: OwnerOpAuthorization::op_digest(b"transfer to bob"),
        counter: 3,
        key_epoch,
    }
}

fn proof(root: &PrivateKey, terms: OwnerOpTerms) -> SignedOwnerOp {
    SignedOwnerOp {
        genesis: AccountGenesis::new(root.public_key()),
        chain: vec![],
        statement: OwnerOpAuthorization::sign(root, terms).expect("sign"),
    }
}

#[test]
fn a_root_signed_authorisation_verifies_from_the_account_id_alone() {
    let root = PrivateKey::from([7u8; 32]);
    let terms = terms(&root, 0);
    assert!(proof(&root, terms).verify(terms.account).is_ok());
}

#[test]
fn an_authorisation_signed_by_a_device_key_is_refused() {
    // The whole point: a device key speaks for the account everywhere else, and
    // must not here.
    let root = PrivateKey::from([7u8; 32]);
    let device = PrivateKey::from([9u8; 32]);
    let terms = terms(&root, 0);
    let mut forged = proof(&root, terms);
    forged.statement = OwnerOpAuthorization::sign(&device, terms).expect("sign");
    assert_eq!(
        forged.verify(terms.account),
        Err(AccountError::OwnerOpSignatureInvalid)
    );
}

#[test]
fn every_bound_term_is_covered_by_the_signature() {
    let root = PrivateKey::from([7u8; 32]);
    let terms = terms(&root, 0);
    let honest = proof(&root, terms);

    let tamper: [fn(&mut OwnerOpAuthorization); 6] = [
        |s| s.namespace_id = [0x99; 32],
        |s| s.group_id = [0x99; 32],
        |s| s.kind = OwnerOpKind::GroupDelete,
        |s| s.op_digest = OwnerOpAuthorization::op_digest(b"transfer to mallory"),
        |s| s.counter += 1,
        |s| s.key_epoch = 1,
    ];
    for change in tamper {
        let mut edited = honest.clone();
        change(&mut edited.statement);
        assert!(
            edited.verify(terms.account).is_err(),
            "a term was changed after signing and the proof still verified: {edited:?}"
        );
    }
}

#[test]
fn an_authorisation_for_another_account_is_refused() {
    let root = PrivateKey::from([7u8; 32]);
    let other = AccountGenesis::new(PrivateKey::from([8u8; 32]).public_key()).account_id();
    let terms = terms(&root, 0);
    assert!(matches!(
        proof(&root, terms).verify(other),
        Err(AccountError::GenesisMismatch { .. })
    ));
}

#[test]
fn any_epoch_the_chain_reaches_may_sign() {
    // Same rule as a revocation: an old root may still sign, and so may the new
    // one. The apply path adds the recorded-epoch floor on top.
    let root = PrivateKey::from([7u8; 32]);
    let next = PrivateKey::from([8u8; 32]);
    let (genesis, handoff) = rotated(&root, &next);

    for (signer, epoch) in [(&root, 0), (&next, 1)] {
        let terms = terms(&root, epoch);
        let proof = SignedOwnerOp {
            genesis,
            chain: vec![handoff],
            statement: OwnerOpAuthorization::sign(signer, terms).expect("sign"),
        };
        assert!(proof.verify(terms.account).is_ok(), "epoch {epoch}");
    }

    // A chain that stops short of the claimed epoch proves nothing about it.
    let terms = terms(&root, 1);
    let short = SignedOwnerOp {
        genesis,
        chain: vec![],
        statement: OwnerOpAuthorization::sign(&next, terms).expect("sign"),
    };
    assert!(matches!(
        short.verify(terms.account),
        Err(AccountError::EpochOutOfRange { .. })
    ));
}

#[test]
fn the_kind_tag_is_its_borsh_byte() {
    // `tag` is what is signed and borsh is what travels; they must not disagree.
    for kind in [
        OwnerOpKind::TransferOwnership,
        OwnerOpKind::AdminChanged,
        OwnerOpKind::GroupDelete,
        OwnerOpKind::TeeAdmissionPolicy,
        OwnerOpKind::TeeAuthoringPolicy,
        OwnerOpKind::TeeReleaseAdmissionPolicy,
    ] {
        assert_eq!(borsh::to_vec(&kind).expect("encode"), vec![kind.tag()]);
    }
}

/// Pinned bytes for the SDKs, which mint these proofs without this crate.
///
/// A change here means every client's signer is now producing signatures this
/// build refuses, so it needs a paired client release.
#[test]
fn signing_payload_and_encoding_are_pinned() {
    let root = PrivateKey::from([7u8; 32]);
    let terms = terms(&root, 0);
    let proof = proof(&root, terms);

    assert_eq!(
        hex::encode(OwnerOpAuthorization::op_digest(b"transfer to bob")),
        PINNED_OP_DIGEST
    );
    assert_eq!(hex::encode(terms.signing_payload()), PINNED_PAYLOAD);
    assert_eq!(
        hex::encode(borsh::to_vec(&proof).expect("encode")),
        PINNED_PROOF
    );
}

const PINNED_OP_DIGEST: &str = "350519af50455451fe2a1f181ec2f4c5363104f168055c6e9088c5a5b65384df";
const PINNED_PAYLOAD: &str = "a0855fcab12a8f8000345a0f25658f87d7650da170c50924be55fa780ca345a8";
const PINNED_PROOF: &str =
    "02ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c000000009f9d3474c108b0bc7809ba35b5434980dd5b34f4a5042bdbb4de5519102324fb1111111111111111111111111111111111111111111111111111111111111111222222222222222222222222222222222222222222222222222222222222222200350519af50455451fe2a1f181ec2f4c5363104f168055c6e9088c5a5b65384df030000000000000000000000c62ebe7140bdffe975203603a4dc27d4ced893a0e337698479885822deccf5ea97f1d4efdf071656a5ea50cc4446abbb1a2f2a2e6720b8a1f2ac416bbf32020f";
