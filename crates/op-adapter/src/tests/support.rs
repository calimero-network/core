//! Fixtures shared by the test files beside this one.

use core::num::NonZeroU64;

use calimero_account::{
    AccountGenesis, AccountId, DeviceCert, DeviceId, GovernanceDelegation, GovernanceOpKind,
    GovernanceTerms, GovernanceWarrant, KemPublicKey,
};
use calimero_governance_types::JoinAccountCredential;
use calimero_op::Authorship;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};

/// The [`Authorship`] for an op whose author is named directly.
///
/// An unattributed authorship names no account at all,
/// which couples a test to the stand-in bridge even when all it wanted was "some
/// author". This states the principal and the key separately, which is what the
/// two fields actually mean.
pub(crate) fn authorship_of(account: AccountId, device_key: PublicKey) -> Authorship {
    Authorship {
        account,
        device: DeviceId::from(*account.as_bytes()),
        device_key,
    }
}

pub(crate) fn hlc(ns: u64) -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(ns),
        ID::from(NonZeroU64::new(1).unwrap()),
    ))
}

/// A credential that actually VERIFIES for `sign_pk`. The filler fixture below
/// cannot fold a device now that the shared predicate checks the certificate, so
/// any test asserting the device half needs this one.
pub(crate) fn real_join_account_for(sign_pk: PublicKey, seed: u8) -> Box<JoinAccountCredential> {
    let root_sk = PrivateKey::from([seed; 32]);
    let genesis = AccountGenesis::new(root_sk.public_key());
    let cert = DeviceCert::sign(
        &root_sk,
        genesis.account_id(),
        DeviceId::mint(genesis.account_id(), [0x3E; 16]),
        &sign_pk,
        &KemPublicKey::from([0x2B; 32]),
        0,
        0,
    )
    .expect("sign the device cert");
    Box::new(JoinAccountCredential {
        genesis,
        chain: vec![],
        statement: cert,
    })
}

/// A joiner credential whose signature is filler, so `verify_device_cert`
/// refuses it. Tests that assert a credential folds NO device use this: it is
/// present and well-shaped, and still not admissible.
pub(crate) fn test_join_account_for(sign_pk: PublicKey) -> Box<JoinAccountCredential> {
    let root = PublicKey::from([0x7A; 32]);
    let genesis = AccountGenesis::new(root);
    Box::new(JoinAccountCredential {
        statement: DeviceCert {
            account: genesis.account_id(),
            device: DeviceId::mint(genesis.account_id(), [0x3E; 16]),
            sign_pk,
            kem_pk: KemPublicKey::from([0x2B; 32]),
            key_epoch: 0,
            device_epoch: 0,
            signature: [0x11; 64],
        },
        genesis,
        chain: vec![],
    })
}

/// A relay's bundle for one `kind` op, in force through `not_after`. The fold
/// never verifies a bundle, so its proofs only need to be well-shaped.
pub(crate) fn governance_delegation(
    kind: GovernanceOpKind,
    not_after: u64,
) -> GovernanceDelegation {
    let author_sk = PrivateKey::from([0x6A; 32]);
    let relay_pk = PrivateKey::from([0x6B; 32]).public_key();
    let terms = GovernanceTerms {
        scope: [0x6C; 32],
        kind,
        author_account: AccountId::from([0x6D; 32]),
        executor: AccountId::from([0x6E; 32]),
        executor_key: relay_pk,
        op_hash: [0; 32],
        account_heads: vec![],
        governance_floor: vec![],
        nonce: 1,
        not_after,
    };
    GovernanceDelegation {
        warrant: Box::new(GovernanceWarrant::sign(&author_sk, terms).expect("sign the warrant")),
        author_proof: test_join_account_for(author_sk.public_key()),
        executor_proof: test_join_account_for(relay_pk),
        executor_key: relay_pk,
    }
}
