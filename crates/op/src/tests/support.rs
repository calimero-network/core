//! Fixtures shared by the test files beside this one.
//!
//! Everything is derived from a `u8` seed so a failure reproduces exactly: the
//! same seed always yields the same keypair, hence the same account, device and
//! op id.

use std::collections::BTreeMap;

use calimero_account::{AccountGenesis, AccountId, DeviceCert, DeviceId, RootKeyHandoff};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_storage::address::Id;
use calimero_storage::logical_clock::HybridTimestamp;

use crate::{Authorship, OpPayload, ScopeId};

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

/// Every `OpPayload` variant, in declaration order, with placeholder contents.
///
/// Shared by `payload::op_payload_discriminants_are_pinned` (which checks the
/// tag each one encodes to) and `wire_fingerprint` (which checks the same
/// samples against the committed structural descriptor). One list, so the two
/// gates can never disagree about which variants exist.
pub(crate) fn every_op_payload() -> Vec<OpPayload> {
    let id = Id::new([1u8; 32]);
    let pk = AccountId::from([2u8; 32]);
    let scope = ScopeId::from([3u8; 32]);
    let group = ContextGroupId::from([4u8; 32]);
    let caps = MemberCapabilities::empty();
    let genesis = AccountGenesis::new(key(1).public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [1u8; 16]);
    let handoff = RootKeyHandoff {
        account,
        from_epoch: 0,
        new_root_sign_pk: key(2).public_key(),
        signature: [0u8; 64],
    };
    let cert = DeviceCert {
        account,
        device,
        sign_pk: key(3).public_key(),
        kem_pk: calimero_account::KemPublicKey::from([4u8; 32]),
        key_epoch: 0,
        device_epoch: 0,
        signature: [0u8; 64],
    };

    vec![
        OpPayload::Put {
            entity: id,
            value: vec![1],
        },
        OpPayload::Delete { entity: id },
        OpPayload::SetWriters {
            object: id,
            writers: BTreeMap::new(),
        },
        OpPayload::MemberAdded {
            group,
            member: pk,
            role: GroupMemberRole::Member,
        },
        OpPayload::MemberRemoved { group, member: pk },
        OpPayload::AdminChanged { new_admin: pk },
        OpPayload::PolicyUpdated {
            policy_bytes: vec![],
        },
        OpPayload::SubgroupCreated {
            child: scope,
            parent: scope,
            restricted: false,
            admin: pk,
        },
        OpPayload::SubgroupReparented {
            child: scope,
            new_parent: scope,
        },
        OpPayload::SubgroupDeleted { scope },
        OpPayload::SubgroupVisibilitySet {
            scope,
            restricted: true,
        },
        OpPayload::DefaultCapabilitiesSet {
            group,
            capabilities: caps,
        },
        OpPayload::MemberCapabilitySet {
            group,
            member: pk,
            capabilities: caps,
        },
        OpPayload::Noop,
        OpPayload::DeviceLinked {
            genesis,
            chain: vec![],
            cert,
        },
        OpPayload::DeviceRevoked { account, device },
        OpPayload::AccountKeysRotated { handoff },
        OpPayload::MemberJoinedWithDevice {
            group,
            member: account,
            role: GroupMemberRole::Member,
            genesis,
            chain: vec![],
            cert,
        },
        OpPayload::Opaque { group },
    ]
}
