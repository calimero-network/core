//! The append-only wire format.
//!
//! One test, and it is the crate's most consequential one: it pins the borsh
//! tag of every variant, and its exhaustive `match` means a new variant does
//! not compile until someone appends it here with a tag of its own — which is
//! the point at which they find out that inserting it in the middle would have
//! invalidated the signature of every stored op after it.

use std::collections::BTreeMap;

use calimero_account::{AccountGenesis, AccountId, DeviceCert, DeviceId, RootKeyHandoff};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::GroupMemberRole;
use calimero_storage::address::Id;

use crate::tests::support::key;
use crate::{OpPayload, ScopeId};

#[test]
fn op_payload_discriminants_are_pinned() {
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

    // Every variant, paired with the borsh discriminant it MUST keep forever
    // (see the append-only note on `OpPayload`). The exhaustive `match` below
    // means adding a variant fails to compile until it is appended here with
    // its own pinned tag — never inserted in the middle.
    let all = [
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
    ];

    // Exhaustive: a new variant forces a new arm here.
    fn pinned_tag(p: &OpPayload) -> u8 {
        match p {
            OpPayload::Put { .. } => 0,
            OpPayload::Delete { .. } => 1,
            OpPayload::SetWriters { .. } => 2,
            OpPayload::MemberAdded { .. } => 3,
            OpPayload::MemberRemoved { .. } => 4,
            OpPayload::AdminChanged { .. } => 5,
            OpPayload::PolicyUpdated { .. } => 6,
            OpPayload::SubgroupCreated { .. } => 7,
            OpPayload::SubgroupReparented { .. } => 8,
            OpPayload::SubgroupDeleted { .. } => 9,
            OpPayload::SubgroupVisibilitySet { .. } => 10,
            OpPayload::DefaultCapabilitiesSet { .. } => 11,
            OpPayload::MemberCapabilitySet { .. } => 12,
            OpPayload::Noop => 13,
            OpPayload::DeviceLinked { .. } => 14,
            OpPayload::DeviceRevoked { .. } => 15,
            OpPayload::AccountKeysRotated { .. } => 16,
            OpPayload::MemberJoinedWithDevice { .. } => 17,
        }
    }

    assert_eq!(all.len(), 18, "every OpPayload variant must be listed");
    for payload in &all {
        let bytes = borsh::to_vec(payload).expect("serialize");
        assert_eq!(
            bytes[0],
            pinned_tag(payload),
            "borsh discriminant drifted for {payload:?} — variants must be append-only"
        );
    }
}
