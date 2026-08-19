//! Enrolling a signing key so tests can name the account it speaks for.
//!
//! Governance rows name accounts, and an account is a one-way hash of a root
//! this crate never sees — so a test cannot simply derive one from a key. It has
//! to enrol the key the way a real join does, and read the account back.
//!
//! Deriving a stand-in instead (`AccountId::from(*pk)`) compiles and is always
//! wrong: both are 32 bytes, so the row lands under a principal that resolves to
//! nobody, and the gate the test meant to exercise refuses for a reason that has
//! nothing to do with what is under test.
//!
//! Available outside `cfg(test)` so the integration suites in `tests/` can use
//! it too; it writes only test rows and is never called from a handler.

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;

/// The credential `sign_pk` would present, derived deterministically from the
/// key so the same key always speaks for the same account.
fn credential_for(
    sign_pk: &PublicKey,
) -> (
    calimero_account::AccountGenesis,
    calimero_account::DeviceCert,
) {
    let root_sk = PrivateKey::from(*(*sign_pk));
    let genesis = calimero_account::AccountGenesis::new(root_sk.public_key());
    let cert = calimero_account::DeviceCert::sign(
        &root_sk,
        genesis.account_id(),
        // The device id is derived from the signing key rather than fixed: a
        // constant would make every credential claim the same device, and the
        // second enrolment in any store would be refused as a reassignment.
        calimero_account::DeviceId::from(*(*sign_pk)),
        sign_pk,
        &calimero_account::KemPublicKey::from([0x2B; 32]),
        0,
        0,
    )
    .expect("the account root signs its own device cert");
    (genesis, cert)
}

/// The credential `sign_pk` presents when it joins.
///
/// Use this wherever an op carries an `account` beside a `member`: the two have
/// to name the same account, and a filler credential is refused before the op
/// reaches whatever the test is aiming at.
#[must_use]
pub fn credential(
    sign_pk: &PublicKey,
) -> Box<calimero_context_client::local_governance::JoinAccountCredential> {
    let (genesis, cert) = credential_for(sign_pk);
    Box::new(
        calimero_context_client::local_governance::JoinAccountCredential {
            genesis,
            chain: vec![],
            statement: cert,
        },
    )
}

/// The account `sign_pk` will speak for once enrolled.
#[must_use]
pub fn account_for(sign_pk: &PublicKey) -> AccountId {
    credential_for(sign_pk).1.account
}

/// Bind `sign_pk` to its account in `namespace`, and return that account.
///
/// Writes both rows a real join writes: the device binding and the endorser
/// entry the member->account direction is read through.
///
/// # Panics
///
/// Panics if the rows cannot be written, which in a test means the fixture is
/// wrong rather than the code under test.
pub fn enrol(store: &Store, namespace: &ContextGroupId, sign_pk: &PublicKey) -> AccountId {
    let (genesis, cert) = credential_for(sign_pk);
    let account = cert.account;
    let bindings = calimero_governance_store::AccountBindingRepository::new(store);
    bindings
        .record_endorser(namespace, account, &account)
        .expect("record the endorser");
    let _ = bindings
        .apply_link(namespace, &genesis, &[], &cert)
        .expect("record the binding");
    account
}
