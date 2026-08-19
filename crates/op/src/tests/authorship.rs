//! The authorship triple, exercised through the only thing that protects it:
//! the id preimage. Each of the three ids is separately forgeable if it is not
//! hashed, so the test that matters is that changing any one of them changes
//! the id.

use calimero_account::{AccountId, DeviceId};
use calimero_storage::address::Id;

use crate::tests::support::{hlc0, key, real_authorship};
use crate::{Authorship, Op, OpPayload, ScopeId};

#[test]
fn compute_id_covers_every_part_of_authorship() {
    // Each field is separately exploitable if unsigned: swapping the
    // account replays a device's op under someone else, swapping the
    // device forges a replica id, swapping the key substitutes the signer.
    let scope = ScopeId::from([7u8; 32]);
    let hlc = hlc0();
    let payload = OpPayload::Delete {
        entity: Id::new([2u8; 32]),
    };
    let base = real_authorship(1, 2);
    let id = |a: &Authorship| Op::compute_id(scope, &[], a, &hlc, &payload);

    let mut other_account = base;
    other_account.account = AccountId::from([42u8; 32]);
    assert_ne!(id(&base), id(&other_account), "account must be signed");

    let mut other_device = base;
    other_device.device = DeviceId::from([43u8; 32]);
    assert_ne!(id(&base), id(&other_device), "device must be signed");

    let mut other_key = base;
    other_key.device_key = key(9).public_key();
    assert_ne!(id(&base), id(&other_key), "device_key must be signed");
}
