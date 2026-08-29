//! The structural fingerprint of the unified op envelope.
//!
//! `payload.rs` already pins every [`OpPayload`] tag by *encoding* a sample of
//! each variant. That is exhaustive over variants, but encode-side: it cannot
//! see a field retyped underneath a tag, and it says nothing about [`Op`]
//! itself, whose field ORDER is the whole preimage of `compute_id` and thus of
//! every signature.
//!
//! This module states the layout a second time as a descriptor, pins it to a
//! committed snapshot, and anchors the descriptor to the real types with
//! exhaustive, *typed* destructuring — so adding, removing, reordering or
//! retyping a field fails to compile here before it can reach a peer.
//!
//! See `calimero-wire-descriptor` for why the descriptor is hand-maintained
//! rather than derived.
//!
//! Regenerate after an intended change:
//!
//! ```text
//! UPDATE_WIRE_FINGERPRINT=1 cargo test -p calimero-op wire_fingerprint
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use calimero_account::{AccountGenesis, AccountId, DeviceCert, DeviceId, RootKeyHandoff};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::entities::OpMask;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_wire_descriptor::{assert_snapshot, Field, Leaf, Shape, Surface, TypeDesc, Variant};

use crate::authorship::Authorship;
use crate::payload::OpPayload;
use crate::scope::ScopeId;

const OP_PAYLOAD: TypeDesc = TypeDesc {
    name: "OpPayload",
    shape: Shape::Enum(&[
        Variant {
            ordinal: 0,
            name: "Put",
            fields: &[
                Field {
                    name: "entity",
                    ty: "Id",
                },
                Field {
                    name: "value",
                    ty: "Vec<u8>",
                },
            ],
        },
        Variant {
            ordinal: 1,
            name: "Delete",
            fields: &[Field {
                name: "entity",
                ty: "Id",
            }],
        },
        Variant {
            ordinal: 2,
            name: "SetWriters",
            fields: &[
                Field {
                    name: "object",
                    ty: "Id",
                },
                Field {
                    name: "writers",
                    ty: "BTreeMap<AccountId, OpMask>",
                },
            ],
        },
        Variant {
            ordinal: 3,
            name: "MemberAdded",
            fields: &[
                Field {
                    name: "group",
                    ty: "ContextGroupId",
                },
                Field {
                    name: "member",
                    ty: "AccountId",
                },
                Field {
                    name: "role",
                    ty: "GroupMemberRole",
                },
            ],
        },
        Variant {
            ordinal: 4,
            name: "MemberRemoved",
            fields: &[
                Field {
                    name: "group",
                    ty: "ContextGroupId",
                },
                Field {
                    name: "member",
                    ty: "AccountId",
                },
            ],
        },
        Variant {
            ordinal: 5,
            name: "AdminChanged",
            fields: &[Field {
                name: "new_admin",
                ty: "AccountId",
            }],
        },
        Variant {
            ordinal: 6,
            name: "PolicyUpdated",
            fields: &[Field {
                name: "policy_bytes",
                ty: "Vec<u8>",
            }],
        },
        Variant {
            ordinal: 7,
            name: "SubgroupCreated",
            fields: &[
                Field {
                    name: "child",
                    ty: "ScopeId",
                },
                Field {
                    name: "parent",
                    ty: "ScopeId",
                },
                Field {
                    name: "restricted",
                    ty: "bool",
                },
                Field {
                    name: "admin",
                    ty: "AccountId",
                },
            ],
        },
        Variant {
            ordinal: 8,
            name: "SubgroupReparented",
            fields: &[
                Field {
                    name: "child",
                    ty: "ScopeId",
                },
                Field {
                    name: "new_parent",
                    ty: "ScopeId",
                },
            ],
        },
        Variant {
            ordinal: 9,
            name: "SubgroupDeleted",
            fields: &[Field {
                name: "scope",
                ty: "ScopeId",
            }],
        },
        Variant {
            ordinal: 10,
            name: "SubgroupVisibilitySet",
            fields: &[
                Field {
                    name: "scope",
                    ty: "ScopeId",
                },
                Field {
                    name: "restricted",
                    ty: "bool",
                },
            ],
        },
        Variant {
            ordinal: 11,
            name: "DefaultCapabilitiesSet",
            fields: &[
                Field {
                    name: "group",
                    ty: "ContextGroupId",
                },
                Field {
                    name: "capabilities",
                    ty: "MemberCapabilities",
                },
            ],
        },
        Variant {
            ordinal: 12,
            name: "MemberCapabilitySet",
            fields: &[
                Field {
                    name: "group",
                    ty: "ContextGroupId",
                },
                Field {
                    name: "member",
                    ty: "AccountId",
                },
                Field {
                    name: "capabilities",
                    ty: "MemberCapabilities",
                },
            ],
        },
        Variant {
            ordinal: 13,
            name: "Noop",
            fields: &[],
        },
        Variant {
            ordinal: 14,
            name: "DeviceLinked",
            fields: &[
                Field {
                    name: "genesis",
                    ty: "AccountGenesis",
                },
                Field {
                    name: "chain",
                    ty: "Vec<RootKeyHandoff>",
                },
                Field {
                    name: "cert",
                    ty: "DeviceCert",
                },
            ],
        },
        Variant {
            ordinal: 15,
            name: "DeviceRevoked",
            fields: &[
                Field {
                    name: "account",
                    ty: "AccountId",
                },
                Field {
                    name: "device",
                    ty: "DeviceId",
                },
            ],
        },
        Variant {
            ordinal: 16,
            name: "AccountKeysRotated",
            fields: &[Field {
                name: "handoff",
                ty: "RootKeyHandoff",
            }],
        },
        Variant {
            ordinal: 17,
            name: "MemberJoinedWithDevice",
            fields: &[
                Field {
                    name: "group",
                    ty: "ContextGroupId",
                },
                Field {
                    name: "member",
                    ty: "AccountId",
                },
                Field {
                    name: "role",
                    ty: "GroupMemberRole",
                },
                Field {
                    name: "genesis",
                    ty: "AccountGenesis",
                },
                Field {
                    name: "chain",
                    ty: "Vec<RootKeyHandoff>",
                },
                Field {
                    name: "cert",
                    ty: "DeviceCert",
                },
            ],
        },
        Variant {
            ordinal: 18,
            name: "Opaque",
            fields: &[Field {
                name: "group",
                ty: "ContextGroupId",
            }],
        },
    ]),
};

fn op_payload_variant_name(p: &OpPayload) -> &'static str {
    match p {
        OpPayload::Put { entity, value } => {
            let _: &Id = entity;
            let _: &Vec<u8> = value;
            "Put"
        }
        OpPayload::Delete { entity } => {
            let _: &Id = entity;
            "Delete"
        }
        OpPayload::SetWriters { object, writers } => {
            let _: &Id = object;
            let _: &BTreeMap<AccountId, OpMask> = writers;
            "SetWriters"
        }
        OpPayload::MemberAdded {
            group,
            member,
            role,
        } => {
            let _: &ContextGroupId = group;
            let _: &AccountId = member;
            let _: &GroupMemberRole = role;
            "MemberAdded"
        }
        OpPayload::MemberRemoved { group, member } => {
            let _: &ContextGroupId = group;
            let _: &AccountId = member;
            "MemberRemoved"
        }
        OpPayload::AdminChanged { new_admin } => {
            let _: &AccountId = new_admin;
            "AdminChanged"
        }
        OpPayload::PolicyUpdated { policy_bytes } => {
            let _: &Vec<u8> = policy_bytes;
            "PolicyUpdated"
        }
        OpPayload::SubgroupCreated {
            child,
            parent,
            restricted,
            admin,
        } => {
            let _: &ScopeId = child;
            let _: &ScopeId = parent;
            let _: &bool = restricted;
            let _: &AccountId = admin;
            "SubgroupCreated"
        }
        OpPayload::SubgroupReparented { child, new_parent } => {
            let _: &ScopeId = child;
            let _: &ScopeId = new_parent;
            "SubgroupReparented"
        }
        OpPayload::SubgroupDeleted { scope } => {
            let _: &ScopeId = scope;
            "SubgroupDeleted"
        }
        OpPayload::SubgroupVisibilitySet { scope, restricted } => {
            let _: &ScopeId = scope;
            let _: &bool = restricted;
            "SubgroupVisibilitySet"
        }
        OpPayload::DefaultCapabilitiesSet {
            group,
            capabilities,
        } => {
            let _: &ContextGroupId = group;
            let _: &MemberCapabilities = capabilities;
            "DefaultCapabilitiesSet"
        }
        OpPayload::MemberCapabilitySet {
            group,
            member,
            capabilities,
        } => {
            let _: &ContextGroupId = group;
            let _: &AccountId = member;
            let _: &MemberCapabilities = capabilities;
            "MemberCapabilitySet"
        }
        OpPayload::Noop => "Noop",
        OpPayload::DeviceLinked {
            genesis,
            chain,
            cert,
        } => {
            let _: &AccountGenesis = genesis;
            let _: &Vec<RootKeyHandoff> = chain;
            let _: &DeviceCert = cert;
            "DeviceLinked"
        }
        OpPayload::DeviceRevoked { account, device } => {
            let _: &AccountId = account;
            let _: &DeviceId = device;
            "DeviceRevoked"
        }
        OpPayload::AccountKeysRotated { handoff } => {
            let _: &RootKeyHandoff = handoff;
            "AccountKeysRotated"
        }
        OpPayload::MemberJoinedWithDevice {
            group,
            member,
            role,
            genesis,
            chain,
            cert,
        } => {
            let _: &ContextGroupId = group;
            let _: &AccountId = member;
            let _: &GroupMemberRole = role;
            let _: &AccountGenesis = genesis;
            let _: &Vec<RootKeyHandoff> = chain;
            let _: &DeviceCert = cert;
            "MemberJoinedWithDevice"
        }
        OpPayload::Opaque { group } => {
            let _: &ContextGroupId = group;
            "Opaque"
        }
    }
}

/// The envelope.
///
/// `id` is first and it is PRIVATE — it is still on the wire, and a struct has
/// no tag to fail on, so a field inserted before it would be read as the id by
/// an old peer. Described here precisely because no `pub` API reveals it.
const OP: TypeDesc = TypeDesc {
    name: "Op",
    shape: Shape::Struct(&[
        Field {
            name: "id",
            ty: "[u8; 32]",
        },
        Field {
            name: "scope",
            ty: "ScopeId",
        },
        Field {
            name: "parents",
            ty: "Vec<[u8; 32]>",
        },
        Field {
            name: "authorship",
            ty: "Authorship",
        },
        Field {
            name: "hlc",
            ty: "HybridTimestamp",
        },
        Field {
            name: "payload",
            ty: "OpPayload",
        },
        Field {
            name: "expected_scope_root",
            ty: "[u8; 32]",
        },
        Field {
            name: "signature",
            ty: "[u8; 64]",
        },
    ]),
};

const AUTHORSHIP: TypeDesc = TypeDesc {
    name: "Authorship",
    shape: Shape::Struct(&[
        Field {
            name: "account",
            ty: "AccountId",
        },
        Field {
            name: "device",
            ty: "DeviceId",
        },
        Field {
            name: "device_key",
            ty: "PublicKey",
        },
    ]),
};

const SCOPE_ID: TypeDesc = TypeDesc {
    name: "ScopeId",
    shape: Shape::Struct(&[Field {
        name: "0",
        ty: "[u8; 32]",
    }]),
};

/// Compile-time anchor: no `..`, and every binding carries its declared type,
/// so a field added, removed, reordered or retyped stops the build here.
///
/// [`Op`] itself is anchored by `op::op_fields_are_described` instead: its
/// leading `id` field is private to the `op` module, so not even a sibling
/// test module can destructure it. That privacy is the reason this gate cannot
/// live in a central crate.
#[expect(
    dead_code,
    reason = "compile-time anchor: exists to be type-checked, never called"
)]
fn struct_fields_are_described(authorship: &Authorship, scope: &ScopeId) {
    let Authorship {
        account,
        device,
        device_key,
    } = authorship;
    let _: &AccountId = account;
    let _: &DeviceId = device;
    let _: &PublicKey = device_key;

    let _: &[u8; 32] = scope.as_bytes();
}

fn leaf<T: borsh::BorshSerialize>(name: &'static str, value: &T) -> Leaf {
    Leaf {
        name,
        bytes: borsh::to_vec(value).expect("borsh-encode a canonical leaf instance"),
    }
}

const ZERO_ACCOUNT: AccountId = AccountId::from_raw([0u8; 32]);
const ZERO_DEVICE: DeviceId = DeviceId::from_raw([0u8; 32]);

fn zero_genesis() -> AccountGenesis {
    AccountGenesis::new(PublicKey::from([0u8; 32]))
}

/// Types embedded in the payload but owned elsewhere: pinned by the bytes a
/// canonical all-zero instance encodes to rather than described field by
/// field. See `calimero-wire-descriptor::Leaf`.
fn described_leaves() -> Vec<Leaf> {
    vec![
        leaf("AccountGenesis", &zero_genesis()),
        leaf(
            "DeviceCert",
            &DeviceCert {
                account: ZERO_ACCOUNT,
                device: ZERO_DEVICE,
                sign_pk: PublicKey::from([0u8; 32]),
                kem_pk: calimero_account::KemPublicKey::from([0u8; 32]),
                key_epoch: 0,
                device_epoch: 0,
                signature: [0u8; 64],
            },
        ),
        leaf(
            "RootKeyHandoff",
            &RootKeyHandoff {
                account: ZERO_ACCOUNT,
                from_epoch: 0,
                new_root_sign_pk: PublicKey::from([0u8; 32]),
                signature: [0u8; 64],
            },
        ),
        leaf("HybridTimestamp::zero", &HybridTimestamp::zero()),
        leaf("MemberCapabilities::empty", &MemberCapabilities::empty()),
        leaf("OpMask::default", &OpMask::default()),
        leaf("Id", &Id::new([0u8; 32])),
        leaf("GroupMemberRole::Admin", &GroupMemberRole::Admin),
        leaf("GroupMemberRole::Member", &GroupMemberRole::Member),
        leaf("GroupMemberRole::ReadOnly", &GroupMemberRole::ReadOnly),
        leaf(
            "GroupMemberRole::ReadOnlyTee",
            &GroupMemberRole::ReadOnlyTee,
        ),
        leaf("AccountId", &ZERO_ACCOUNT),
        leaf("DeviceId", &ZERO_DEVICE),
        leaf("PublicKey", &PublicKey::from([0u8; 32])),
        leaf("ContextGroupId", &ContextGroupId::from([0u8; 32])),
        leaf("ScopeId", &ScopeId::from([0u8; 32])),
    ]
}

fn surface() -> Surface {
    Surface {
        label: "calimero-op",
        types: vec![OP, AUTHORSHIP, SCOPE_ID, OP_PAYLOAD],
        leaves: described_leaves(),
    }
}

#[test]
fn wire_fingerprint_matches_snapshot() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wire-fingerprint.txt");
    assert_snapshot(&surface(), &path);
}

/// The descriptor's ordinals are what borsh actually emits.
///
/// `op_payload_discriminants_are_pinned` in `payload.rs` checks the same tags
/// against a hand-written table; this checks them against the DESCRIPTOR, so
/// the snapshot can never claim an ordinal the encoder disagrees with.
#[test]
fn described_ordinals_are_the_real_borsh_tags() {
    let Shape::Enum(variants) = OP_PAYLOAD.shape else {
        panic!("OpPayload is an enum");
    };

    let mut failures: Vec<String> = Vec::new();
    for (payload, v) in super::support::every_op_payload().iter().zip(variants) {
        let name = op_payload_variant_name(payload);
        if name != v.name {
            failures.push(format!(
                "ordinal {}: descriptor says {}, sample is {name}",
                v.ordinal, v.name
            ));
            continue;
        }
        let bytes = borsh::to_vec(payload).expect("serialize");
        if bytes[0] != v.ordinal {
            failures.push(format!(
                "OpPayload::{}: descriptor ordinal {} but borsh emitted {}",
                v.name, v.ordinal, bytes[0]
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "descriptor/encoder mismatches ({}):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// No variant may exist past the last described one.
///
/// A one-byte buffer separates the cases: an unknown tag is `InvalidData`
/// ("Unexpected variant tag"), whereas a *known* tag either wants more bytes
/// (`UnexpectedEof`) or decodes outright. Either of the latter two means a
/// variant was appended without being described.
#[test]
fn no_variant_exists_past_the_described_end() {
    let Shape::Enum(variants) = OP_PAYLOAD.shape else {
        panic!("OpPayload is an enum");
    };
    let next = u8::try_from(variants.len()).expect("fewer than 256 variants");
    let kind = borsh::from_slice::<OpPayload>(&[next])
        .map_or_else(|e| e.kind(), |_| std::io::ErrorKind::Other);
    assert_eq!(
        kind,
        std::io::ErrorKind::InvalidData,
        "OpPayload ordinal {next} decodes to something — a variant was appended \
         without adding it to the descriptor in this file. Append it, then \
         regenerate the snapshot."
    );
}
