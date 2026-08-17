//! The stand-in seam: what [`writer_account`] resolves a signing key to, before
//! and after its device binding is withdrawn.

use calimero_account::{
    sign_account_endorsement, sign_device_cert, AccountGenesis, AccountId, DeviceId, KemPublicKey,
};
use calimero_authz::AclView;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::GroupOp;
use calimero_op::{Op, OpPayload, ScopeId};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_projection::ScopeState;

use crate::tests::support::{authorship_of, hlc};
use crate::{legacy_account_id, payload_from_group_op, writer_account};

/// **Revoking a device withdraws the authority its key had to write as its
/// account** — on the writer plane, not just the membership plane.
///
/// The writer plane resolves a signature to a principal through
/// [`writer_account`], which is what `ScopeProjections::device_account_at_cut`
/// arms the receiver with. Revocation removes the binding, so the key stops
/// resolving to the account and a writer set naming that account no longer
/// matches it.
///
/// The refusal is checked to follow from the revocation and not from some
/// incidental gap: the tombstone is asserted present while the binding is
/// asserted gone, which is what distinguishes a revoked device from one whose
/// link this node simply has not folded yet. Those two must not be confused —
/// the second is a timing gap that has to defer, the first is terminal.
#[test]
fn revoking_a_device_withdraws_its_authority_on_the_writer_plane() {
    let root = PrivateKey::from([1u8; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [5u8; 16]);
    let device_sk = PrivateKey::from([5u8; 32]);
    let device_key = device_sk.public_key();
    let cert = sign_device_cert(
        &root,
        account,
        device,
        &device_key,
        &KemPublicKey::from([5u8; 32]),
        0,
        0,
    )
    .expect("sign cert");
    let group = ContextGroupId::from([9u8; 32]);

    // The receiver's rule, verbatim: find a live binding for the signing key,
    // else fall back to the key's stand-in account.
    let resolve = |view: &AclView, key: &PublicKey| {
        let binding = view
            .devices
            .values()
            .find(|b| b.sign_pk == *key)
            .map(|b| b.account);
        writer_account(binding, key)
    };

    let link = Op::from_parts(
        [7u8; 32],
        ScopeId::from([9u8; 32]),
        vec![],
        authorship_of(AccountId::from([0xA0; 32]), root.public_key()),
        hlc(1),
        payload_from_group_op(
            group,
            &GroupOp::AccountDeviceLinked {
                genesis,
                chain: vec![],
                cert,
                endorsement: sign_account_endorsement(&root, account).expect("sign endorsement"),
            },
        )
        .expect("device link maps to a payload"),
        [0u8; 32],
        [0u8; 64],
    );
    let revoke = Op::from_parts(
        [8u8; 32],
        ScopeId::from([9u8; 32]),
        vec![[7u8; 32]],
        authorship_of(AccountId::from([0xA0; 32]), root.public_key()),
        hlc(2),
        OpPayload::DeviceRevoked { account, device },
        [0u8; 32],
        [0u8; 64],
    );

    // At the cut before the revocation the device writes as its account, so a
    // writer set naming the account admits it.
    let mut linked = ScopeState::default();
    linked.apply(&link);
    let before = linked.acl_view();
    assert_eq!(
        resolve(&before, &device_key),
        account,
        "precondition: while bound, the device's key must resolve to its              account, or the assertion below would hold for a device that never              had authority in the first place"
    );

    // Fold the revocation. The binding is gone, the tombstone is there.
    let mut revoked_state = linked;
    revoked_state.apply(&revoke);
    let after = revoked_state.acl_view();
    assert!(
        after.revoked_devices.contains(&device),
        "the revocation must be recorded, or the refusal below proves nothing              about revocation"
    );
    assert!(
        !after.devices.contains_key(&device),
        "and the binding must be withdrawn — a revocation that left the              binding in force would resolve the thief's key to the account"
    );

    let resolved = resolve(&after, &device_key);
    assert_ne!(
        resolved, account,
        "a revoked device must no longer write as the account it was              withdrawn from"
    );
    assert_eq!(
        resolved,
        legacy_account_id(&device_key),
        "it falls back to speaking only for itself"
    );

    // **The caveat, asserted rather than assumed.** The fallback is a stable
    // account, so a writer set that names a device's STAND-IN — as happens
    // when a set is seeded for a key before its account exists — keeps
    // admitting that key after revocation. That is consistent rather than a
    // hole: revoking a device withdraws its authority to speak for an
    // ACCOUNT, and says nothing about a grant made to the key itself, which
    // is undone by rotating the writer set. Worth pinning, because "I revoked
    // the device and it can still write" is a surprising way to learn it.
    assert_eq!(
        resolved,
        resolve(&after, &device_key),
        "the stand-in is stable, so this refusal is permanent rather than a              retryable one — the caller must not treat it as a timing gap"
    );

    // Causal honour on the writer plane: the pre-revocation cut still resolves
    // to the account, so a write authored before the revocation stays
    // authorized when re-judged at its own cut. `device_account_at_cut` takes
    // the write's heads for exactly this reason — resolving at the receiver's
    // latest cut would retroactively invalidate history the sender's root hash
    // already includes, leaving the two unable to agree on a root.
    assert_eq!(
        resolve(&before, &device_key),
        account,
        "an earlier cut must keep its answer after a later revocation folds"
    );
}
