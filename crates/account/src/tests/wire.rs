//! Wire format — the one property that spans every type in the crate: what a
//! credential encodes to must decode back to the same value, or a peer reads a
//! different credential than the one that was signed.

use calimero_primitives::identity::{AccountId, DeviceId};

use super::support::{genesis_for, key, sign_cert, sign_handoff};
use crate::account::AccountGenesis;
use crate::device::DeviceCert;
use crate::domain::borsh_bytes;
use crate::root_key::RootKeyHandoff;

#[test]
fn credentials_round_trip_through_borsh() {
    let (root, r1, dev) = (key(1), key(2), key(5));
    let g = genesis_for(&root);
    let account = g.account_id();
    let device = DeviceId::mint(account, [3u8; 16]);
    let handoff = sign_handoff(&root, account, 0, &r1);
    let cert = sign_cert(&root, account, device, &dev, 0, 0);

    for (label, bytes, ok) in [
        (
            "genesis",
            borsh_bytes(&g),
            borsh::from_slice::<AccountGenesis>(&borsh_bytes(&g)).map(|v| v == g),
        ),
        (
            "handoff",
            borsh_bytes(&handoff),
            borsh::from_slice::<RootKeyHandoff>(&borsh_bytes(&handoff)).map(|v| v == handoff),
        ),
        (
            "cert",
            borsh_bytes(&cert),
            borsh::from_slice::<DeviceCert>(&borsh_bytes(&cert)).map(|v| v == cert),
        ),
    ] {
        assert!(!bytes.is_empty(), "{label} encodes to nothing");
        assert_eq!(ok.ok(), Some(true), "{label} did not round-trip");
    }
}

#[test]
fn ids_round_trip_through_borsh() {
    let account = genesis_for(&key(1)).account_id();
    let device = DeviceId::mint(account, [3u8; 16]);
    assert_eq!(
        borsh::from_slice::<AccountId>(&borsh_bytes(&account)).expect("decode"),
        account
    );
    assert_eq!(
        borsh::from_slice::<DeviceId>(&borsh_bytes(&device)).expect("decode"),
        device
    );
}
