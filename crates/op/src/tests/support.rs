//! Fixtures shared by the test files beside this one.
//!
//! Everything is derived from a `u8` seed so a failure reproduces exactly: the
//! same seed always yields the same keypair, hence the same account, device and
//! op id.

use calimero_account::{AccountGenesis, DeviceId};
use calimero_primitives::identity::PrivateKey;
use calimero_storage::logical_clock::HybridTimestamp;

use crate::Authorship;

/// Deterministic keypair, so failures reproduce exactly.
pub(crate) fn key(seed: u8) -> PrivateKey {
    PrivateKey::from([seed; 32])
}

/// A real (non-self) account with one device, for authorship tests.
pub(crate) fn real_authorship(root_seed: u8, dev_seed: u8) -> Authorship {
    let account = AccountGenesis::new(key(root_seed).public_key()).account_id();
    Authorship {
        account,
        device: DeviceId::mint(account, [dev_seed; 16]),
        device_key: key(dev_seed).public_key(),
    }
}

pub(crate) fn hlc0() -> HybridTimestamp {
    use core::num::NonZeroU128;

    use calimero_storage::logical_clock::{Timestamp, ID, NTP64};
    HybridTimestamp::new(Timestamp::new(
        NTP64(0),
        ID::from(NonZeroU128::new(1).unwrap()),
    ))
}
