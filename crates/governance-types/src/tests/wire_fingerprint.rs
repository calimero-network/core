//! The structural fingerprint of this crate's replicated wire surface.
//!
//! # What this adds over the frozen-byte goldens in `super`
//!
//! The goldens in `tests.rs` decode a hand-written byte vector per variant, so
//! they catch a renumbered discriminant. They cannot catch a change that keeps
//! every discriminant and every length: a `u32` retyped to `[u8; 4]`, two
//! same-width fields swapped, a field renamed. They also only cover the
//! variants somebody remembered to write a vector for.
//!
//! This module closes both gaps:
//!
//! 1. A hand-maintained **descriptor** states the layout a second time, in
//!    prose the reviewer reads, and a committed snapshot pins it. The
//!    descriptor is never derived from the type — a derive would move with the
//!    type and be exactly as blind as a same-binary round-trip.
//! 2. The descriptor is **anchored to reality** three ways, so the second
//!    statement cannot quietly drift from the first:
//!    * an exhaustive `match` per enum that destructures every field, so
//!      adding a variant *or a field* does not compile until the descriptor is
//!      updated (`GroupOp` is `#[non_exhaustive]`, which is why this lives
//!      in-crate rather than in a shared gate crate);
//!    * an exhaustive destructuring per struct, same reason;
//!    * `every_described_ordinal_is_the_real_one` decodes the golden corpus and
//!      checks the descriptor's ordinal/name against what borsh actually
//!      produced, and `no_variant_exists_past_the_described_end` proves there
//!      is no undescribed variant hiding after the last one.
//!
//! Regenerate the snapshot after an intended change:
//!
//! ```text
//! UPDATE_WIRE_FINGERPRINT=1 cargo test -p calimero-governance-types wire_fingerprint
//! ```

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::PathBuf;

use calimero_account::{
    AccountGenesis, AccountId, AccountMemberEndorsement, AccountProof, DeviceCert, DeviceId,
    DeviceRevocation, KemPublicKey, RootKeyHandoff, SignedDeviceRevocation,
};
use calimero_context_config::types::{
    BytecodeId, ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation,
};
use calimero_context_config::{MemberCapabilities, VisibilityMode};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PublicKey;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_wire_descriptor::{assert_snapshot, Field, Leaf, Shape, Surface, TypeDesc, Variant};

use crate::{
    ContextCapabilityBits, EncryptedGroupOp, EnvelopeRecipient, GroupOp, JoinAccountCredential,
    KeyEnvelope, KeyId, KeyRotation, NamespaceId, NamespaceOp, OpaqueSkeleton, RootOp,
    SignableGroupOp, SignableNamespaceOp, SignedGroupOp, SignedNamespaceOp, StoredNamespaceEntry,
};

// ---------------------------------------------------------------------------
// The descriptor: a second, independent statement of the wire layout.
// ---------------------------------------------------------------------------

const GROUP_OP: TypeDesc = TypeDesc {
    name: "GroupOp",
    shape: Shape::Enum(&[
        Variant {
            ordinal: 0,
            name: "Noop",
            fields: &[],
        },
        Variant {
            ordinal: 1,
            name: "MemberAdded",
            fields: &[
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
            ordinal: 2,
            name: "MemberRemoved",
            fields: &[
                Field {
                    name: "member",
                    ty: "AccountId",
                },
                Field {
                    name: "expected_group_state_hash",
                    ty: "[u8; 32]",
                },
                Field {
                    name: "expected_context_state_hashes",
                    ty: "Vec<(ContextId, [u8; 32])>",
                },
            ],
        },
        Variant {
            ordinal: 3,
            name: "MemberLeft",
            fields: &[
                Field {
                    name: "member",
                    ty: "AccountId",
                },
                Field {
                    name: "expected_group_state_hash",
                    ty: "[u8; 32]",
                },
                Field {
                    name: "expected_context_state_hashes",
                    ty: "Vec<(ContextId, [u8; 32])>",
                },
            ],
        },
        Variant {
            ordinal: 4,
            name: "MemberRoleSet",
            fields: &[
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
            ordinal: 5,
            name: "MemberCapabilitySet",
            fields: &[
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
            ordinal: 6,
            name: "DefaultCapabilitiesSet",
            fields: &[Field {
                name: "capabilities",
                ty: "MemberCapabilities",
            }],
        },
        Variant {
            ordinal: 7,
            name: "TargetApplicationSet",
            fields: &[
                Field {
                    name: "bytecode_id",
                    ty: "BytecodeId",
                },
                Field {
                    name: "target_application_id",
                    ty: "ApplicationId",
                },
            ],
        },
        Variant {
            ordinal: 8,
            name: "ContextRegistered",
            fields: &[
                Field {
                    name: "context_id",
                    ty: "ContextId",
                },
                Field {
                    name: "application_id",
                    ty: "ApplicationId",
                },
                Field {
                    name: "blob_id",
                    ty: "BlobId",
                },
                Field {
                    name: "source",
                    ty: "String",
                },
                Field {
                    name: "service_name",
                    ty: "Option<String>",
                },
            ],
        },
        Variant {
            ordinal: 9,
            name: "ContextDetached",
            fields: &[Field {
                name: "context_id",
                ty: "ContextId",
            }],
        },
        Variant {
            ordinal: 10,
            name: "SubgroupVisibilitySet",
            fields: &[Field {
                name: "mode",
                ty: "VisibilityMode",
            }],
        },
        Variant {
            ordinal: 11,
            name: "GroupMetadataSet",
            fields: &[
                Field {
                    name: "name",
                    ty: "Option<String>",
                },
                Field {
                    name: "data",
                    ty: "BTreeMap<String, String>",
                },
            ],
        },
        Variant {
            ordinal: 12,
            name: "MemberMetadataSet",
            fields: &[
                Field {
                    name: "member",
                    ty: "AccountId",
                },
                Field {
                    name: "name",
                    ty: "Option<String>",
                },
                Field {
                    name: "data",
                    ty: "BTreeMap<String, String>",
                },
            ],
        },
        Variant {
            ordinal: 13,
            name: "ContextMetadataSet",
            fields: &[
                Field {
                    name: "context_id",
                    ty: "ContextId",
                },
                Field {
                    name: "name",
                    ty: "Option<String>",
                },
                Field {
                    name: "data",
                    ty: "BTreeMap<String, String>",
                },
            ],
        },
        Variant {
            ordinal: 14,
            name: "GroupDelete",
            fields: &[],
        },
        Variant {
            ordinal: 15,
            name: "GroupMigrationSet",
            fields: &[Field {
                name: "migration",
                ty: "Option<Vec<u8>>",
            }],
        },
        Variant {
            ordinal: 16,
            name: "ContextCapabilityGranted",
            fields: &[
                Field {
                    name: "context_id",
                    ty: "ContextId",
                },
                Field {
                    name: "member",
                    ty: "AccountId",
                },
                Field {
                    name: "capability",
                    ty: "ContextCapabilityBits",
                },
            ],
        },
        Variant {
            ordinal: 17,
            name: "ContextCapabilityRevoked",
            fields: &[
                Field {
                    name: "context_id",
                    ty: "ContextId",
                },
                Field {
                    name: "member",
                    ty: "AccountId",
                },
                Field {
                    name: "capability",
                    ty: "ContextCapabilityBits",
                },
            ],
        },
        Variant {
            ordinal: 18,
            name: "TeeAdmissionPolicySet",
            fields: &[
                Field {
                    name: "allowed_mrtd",
                    ty: "Vec<String>",
                },
                Field {
                    name: "allowed_rtmr0",
                    ty: "Vec<String>",
                },
                Field {
                    name: "allowed_rtmr1",
                    ty: "Vec<String>",
                },
                Field {
                    name: "allowed_rtmr2",
                    ty: "Vec<String>",
                },
                Field {
                    name: "allowed_rtmr3",
                    ty: "Vec<String>",
                },
                Field {
                    name: "allowed_tcb_statuses",
                    ty: "Vec<String>",
                },
                Field {
                    name: "accept_mock",
                    ty: "bool",
                },
            ],
        },
        Variant {
            ordinal: 19,
            name: "MemberJoinedViaTeeAttestation",
            fields: &[
                Field {
                    name: "member",
                    ty: "AccountId",
                },
                Field {
                    name: "quote_hash",
                    ty: "[u8; 32]",
                },
                Field {
                    name: "mrtd",
                    ty: "String",
                },
                Field {
                    name: "rtmr0",
                    ty: "String",
                },
                Field {
                    name: "rtmr1",
                    ty: "String",
                },
                Field {
                    name: "rtmr2",
                    ty: "String",
                },
                Field {
                    name: "rtmr3",
                    ty: "String",
                },
                Field {
                    name: "tcb_status",
                    ty: "String",
                },
                Field {
                    name: "role",
                    ty: "GroupMemberRole",
                },
            ],
        },
        Variant {
            ordinal: 20,
            name: "MemberSetAutoFollow",
            fields: &[
                Field {
                    name: "target",
                    ty: "AccountId",
                },
                Field {
                    name: "auto_follow_contexts",
                    ty: "bool",
                },
                Field {
                    name: "auto_follow_subgroups",
                    ty: "bool",
                },
            ],
        },
        Variant {
            ordinal: 21,
            name: "TransferOwnership",
            fields: &[Field {
                name: "new_owner",
                ty: "AccountId",
            }],
        },
        Variant {
            ordinal: 22,
            name: "CascadeUpgrade",
            fields: &[
                Field {
                    name: "from_bytecode_id",
                    ty: "BytecodeId",
                },
                Field {
                    name: "bytecode_id",
                    ty: "BytecodeId",
                },
                Field {
                    name: "target_application_id",
                    ty: "ApplicationId",
                },
                Field {
                    name: "to_state_version",
                    ty: "u32",
                },
                Field {
                    name: "migration",
                    ty: "Option<Vec<u8>>",
                },
                Field {
                    name: "cascade_hlc",
                    ty: "HybridTimestamp",
                },
            ],
        },
        Variant {
            ordinal: 23,
            name: "GroupKeyRotated",
            fields: &[Field {
                name: "departed",
                ty: "AccountId",
            }],
        },
        Variant {
            ordinal: 24,
            name: "AccountDeviceLinked",
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
                Field {
                    name: "endorsement",
                    ty: "AccountMemberEndorsement",
                },
            ],
        },
        Variant {
            ordinal: 25,
            name: "AccountDeviceUnlinked",
            fields: &[
                Field {
                    name: "account",
                    ty: "AccountId",
                },
                Field {
                    name: "device",
                    ty: "DeviceId",
                },
                Field {
                    name: "proof",
                    ty: "Option<SignedDeviceRevocation>",
                },
            ],
        },
        Variant {
            ordinal: 26,
            name: "AccountKeysRotated",
            fields: &[Field {
                name: "handoff",
                ty: "RootKeyHandoff",
            }],
        },
        Variant {
            ordinal: 27,
            name: "GroupKeyRotatedForDevice",
            fields: &[Field {
                name: "device",
                ty: "DeviceId",
            }],
        },
    ]),
};

const NAMESPACE_OP: TypeDesc = TypeDesc {
    name: "NamespaceOp",
    shape: Shape::Enum(&[
        Variant {
            ordinal: 0,
            name: "Root",
            fields: &[Field {
                name: "0",
                ty: "RootOp",
            }],
        },
        Variant {
            ordinal: 1,
            name: "Group",
            fields: &[
                Field {
                    name: "group_id",
                    ty: "ContextGroupId",
                },
                Field {
                    name: "key_id",
                    ty: "KeyId",
                },
                Field {
                    name: "encrypted",
                    ty: "EncryptedGroupOp",
                },
                Field {
                    name: "key_rotation",
                    ty: "Option<KeyRotation>",
                },
            ],
        },
    ]),
};

const ROOT_OP: TypeDesc = TypeDesc {
    name: "RootOp",
    shape: Shape::Enum(&[
        Variant {
            ordinal: 0,
            name: "GroupCreated",
            fields: &[
                Field {
                    name: "group_id",
                    ty: "ContextGroupId",
                },
                Field {
                    name: "parent_id",
                    ty: "ContextGroupId",
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
            ordinal: 1,
            name: "GroupReparented",
            fields: &[
                Field {
                    name: "child_group_id",
                    ty: "ContextGroupId",
                },
                Field {
                    name: "new_parent_id",
                    ty: "ContextGroupId",
                },
            ],
        },
        Variant {
            ordinal: 2,
            name: "GroupDeleted",
            fields: &[
                Field {
                    name: "root_group_id",
                    ty: "ContextGroupId",
                },
                Field {
                    name: "cascade_group_ids",
                    ty: "Vec<ContextGroupId>",
                },
                Field {
                    name: "cascade_context_ids",
                    ty: "Vec<ContextId>",
                },
            ],
        },
        Variant {
            ordinal: 3,
            name: "AdminChanged",
            fields: &[Field {
                name: "new_admin",
                ty: "AccountId",
            }],
        },
        Variant {
            ordinal: 4,
            name: "PolicyUpdated",
            fields: &[Field {
                name: "policy_bytes",
                ty: "Vec<u8>",
            }],
        },
        Variant {
            ordinal: 5,
            name: "MemberJoined",
            fields: &[
                Field {
                    name: "member",
                    ty: "AccountId",
                },
                Field {
                    name: "signed_invitation",
                    ty: "SignedGroupOpenInvitation",
                },
                Field {
                    name: "account",
                    ty: "Box<JoinAccountCredential>",
                },
            ],
        },
        Variant {
            ordinal: 6,
            name: "KeyDelivery",
            fields: &[
                Field {
                    name: "group_id",
                    ty: "ContextGroupId",
                },
                Field {
                    name: "envelope",
                    ty: "KeyEnvelope",
                },
            ],
        },
        Variant {
            ordinal: 7,
            name: "MemberJoinedOpen",
            fields: &[
                Field {
                    name: "member",
                    ty: "AccountId",
                },
                Field {
                    name: "group_id",
                    ty: "ContextGroupId",
                },
                Field {
                    name: "account",
                    ty: "Box<JoinAccountCredential>",
                },
            ],
        },
        Variant {
            ordinal: 8,
            name: "MemberJoinedAt",
            fields: &[
                Field {
                    name: "member",
                    ty: "AccountId",
                },
                Field {
                    name: "signed_invitation",
                    ty: "SignedGroupOpenInvitation",
                },
                Field {
                    name: "joined_at",
                    ty: "u64",
                },
                Field {
                    name: "account",
                    ty: "Box<JoinAccountCredential>",
                },
            ],
        },
        Variant {
            ordinal: 9,
            name: "NamespaceCreated",
            fields: &[
                Field {
                    name: "founder",
                    ty: "AccountId",
                },
                Field {
                    name: "account",
                    ty: "Box<JoinAccountCredential>",
                },
            ],
        },
        Variant {
            ordinal: 10,
            name: "MemberJoinedViaTeeAttestation",
            fields: &[
                Field {
                    name: "group_id",
                    ty: "ContextGroupId",
                },
                Field {
                    name: "member",
                    ty: "PublicKey",
                },
                Field {
                    name: "quote_hash",
                    ty: "[u8; 32]",
                },
                Field {
                    name: "mrtd",
                    ty: "String",
                },
                Field {
                    name: "rtmr0",
                    ty: "String",
                },
                Field {
                    name: "rtmr1",
                    ty: "String",
                },
                Field {
                    name: "rtmr2",
                    ty: "String",
                },
                Field {
                    name: "rtmr3",
                    ty: "String",
                },
                Field {
                    name: "tcb_status",
                    ty: "String",
                },
                Field {
                    name: "role",
                    ty: "GroupMemberRole",
                },
                Field {
                    name: "account",
                    ty: "Box<JoinAccountCredential>",
                },
            ],
        },
    ]),
};

const ENVELOPE_RECIPIENT: TypeDesc = TypeDesc {
    name: "EnvelopeRecipient",
    shape: Shape::Enum(&[
        Variant {
            ordinal: 0,
            name: "Member",
            fields: &[
                Field {
                    name: "identity",
                    ty: "PublicKey",
                },
                Field {
                    name: "ephemeral_pk",
                    ty: "PublicKey",
                },
            ],
        },
        Variant {
            ordinal: 1,
            name: "Device",
            fields: &[
                Field {
                    name: "device",
                    ty: "DeviceId",
                },
                Field {
                    name: "ephemeral_pk",
                    ty: "KemPublicKey",
                },
            ],
        },
    ]),
};

const STORED_NAMESPACE_ENTRY: TypeDesc = TypeDesc {
    name: "StoredNamespaceEntry",
    shape: Shape::Enum(&[
        Variant {
            ordinal: 0,
            name: "Signed",
            fields: &[Field {
                name: "0",
                ty: "SignedNamespaceOp",
            }],
        },
        Variant {
            ordinal: 1,
            name: "Opaque",
            fields: &[Field {
                name: "0",
                ty: "OpaqueSkeleton",
            }],
        },
    ]),
};

// ---- structs -------------------------------------------------------------
//
// Field ORDER is the encoding. A struct has no tag, so inserting a field in
// the middle silently reinterprets every byte after it — strictly worse than
// an enum renumber, because there is no discriminant to fail on.

const SIGNABLE_GROUP_OP: TypeDesc = TypeDesc {
    name: "SignableGroupOp",
    shape: Shape::Struct(&[
        Field {
            name: "version",
            ty: "u8",
        },
        Field {
            name: "group_id",
            ty: "ContextGroupId",
        },
        Field {
            name: "parent_op_hashes",
            ty: "Vec<[u8; 32]>",
        },
        Field {
            name: "signer",
            ty: "PublicKey",
        },
        Field {
            name: "nonce",
            ty: "u64",
        },
        Field {
            name: "op",
            ty: "GroupOp",
        },
    ]),
};

const SIGNED_GROUP_OP: TypeDesc = TypeDesc {
    name: "SignedGroupOp",
    shape: Shape::Struct(&[
        Field {
            name: "version",
            ty: "u8",
        },
        Field {
            name: "group_id",
            ty: "ContextGroupId",
        },
        Field {
            name: "parent_op_hashes",
            ty: "Vec<[u8; 32]>",
        },
        Field {
            name: "signer",
            ty: "PublicKey",
        },
        Field {
            name: "nonce",
            ty: "u64",
        },
        Field {
            name: "op",
            ty: "GroupOp",
        },
        Field {
            name: "signature",
            ty: "[u8; 64]",
        },
    ]),
};

const SIGNABLE_NAMESPACE_OP: TypeDesc = TypeDesc {
    name: "SignableNamespaceOp",
    shape: Shape::Struct(&[
        Field {
            name: "version",
            ty: "u8",
        },
        Field {
            name: "namespace_id",
            ty: "NamespaceId",
        },
        Field {
            name: "parent_op_hashes",
            ty: "Vec<[u8; 32]>",
        },
        Field {
            name: "signer",
            ty: "PublicKey",
        },
        Field {
            name: "nonce",
            ty: "u64",
        },
        Field {
            name: "op",
            ty: "NamespaceOp",
        },
    ]),
};

const SIGNED_NAMESPACE_OP: TypeDesc = TypeDesc {
    name: "SignedNamespaceOp",
    shape: Shape::Struct(&[
        Field {
            name: "version",
            ty: "u8",
        },
        Field {
            name: "namespace_id",
            ty: "NamespaceId",
        },
        Field {
            name: "parent_op_hashes",
            ty: "Vec<[u8; 32]>",
        },
        Field {
            name: "signer",
            ty: "PublicKey",
        },
        Field {
            name: "nonce",
            ty: "u64",
        },
        Field {
            name: "op",
            ty: "NamespaceOp",
        },
        Field {
            name: "signature",
            ty: "[u8; 64]",
        },
    ]),
};

const ENCRYPTED_GROUP_OP: TypeDesc = TypeDesc {
    name: "EncryptedGroupOp",
    shape: Shape::Struct(&[
        Field {
            name: "nonce",
            ty: "[u8; 12]",
        },
        Field {
            name: "ciphertext",
            ty: "Vec<u8>",
        },
    ]),
};

const KEY_ENVELOPE: TypeDesc = TypeDesc {
    name: "KeyEnvelope",
    shape: Shape::Struct(&[
        Field {
            name: "recipient",
            ty: "EnvelopeRecipient",
        },
        Field {
            name: "sender",
            ty: "PublicKey",
        },
        Field {
            name: "nonce",
            ty: "[u8; 12]",
        },
        Field {
            name: "ciphertext",
            ty: "Vec<u8>",
        },
        Field {
            name: "signature",
            ty: "[u8; 64]",
        },
    ]),
};

const KEY_ROTATION: TypeDesc = TypeDesc {
    name: "KeyRotation",
    shape: Shape::Struct(&[
        Field {
            name: "new_key_id",
            ty: "KeyId",
        },
        Field {
            name: "envelopes",
            ty: "Vec<KeyEnvelope>",
        },
    ]),
};

const OPAQUE_SKELETON: TypeDesc = TypeDesc {
    name: "OpaqueSkeleton",
    shape: Shape::Struct(&[
        Field {
            name: "delta_id",
            ty: "[u8; 32]",
        },
        Field {
            name: "parent_op_hashes",
            ty: "Vec<[u8; 32]>",
        },
        Field {
            name: "group_id",
            ty: "ContextGroupId",
        },
        Field {
            name: "signer",
            ty: "PublicKey",
        },
    ]),
};

/// Every described type, in the order they appear in the snapshot.
fn described_types() -> Vec<TypeDesc> {
    vec![
        NAMESPACE_OP,
        ROOT_OP,
        GROUP_OP,
        ENVELOPE_RECIPIENT,
        STORED_NAMESPACE_ENTRY,
        SIGNABLE_GROUP_OP,
        SIGNED_GROUP_OP,
        SIGNABLE_NAMESPACE_OP,
        SIGNED_NAMESPACE_OP,
        ENCRYPTED_GROUP_OP,
        KEY_ENVELOPE,
        KEY_ROTATION,
        OPAQUE_SKELETON,
    ]
}

// ---------------------------------------------------------------------------
// Compile-time anchors.
//
// None of these functions is ever called. They exist so that `cargo test`
// FAILS TO BUILD when a variant or a field is added, which is the moment the
// author has to decide whether their change is an append (compatible) or a
// reshuffle (a break). A test that merely asserts at runtime would be silent
// until someone thought to run it against an old peer.
// ---------------------------------------------------------------------------

fn group_op_variant_name(op: &GroupOp) -> &'static str {
    match op {
        GroupOp::Noop => "Noop",
        GroupOp::MemberAdded { member, role } => {
            let _: &AccountId = member;
            let _: &GroupMemberRole = role;
            "MemberAdded"
        }
        GroupOp::MemberRemoved {
            member,
            expected_group_state_hash,
            expected_context_state_hashes,
        } => {
            let _: &AccountId = member;
            let _: &[u8; 32] = expected_group_state_hash;
            let _: &Vec<(ContextId, [u8; 32])> = expected_context_state_hashes;
            "MemberRemoved"
        }
        GroupOp::MemberLeft {
            member,
            expected_group_state_hash,
            expected_context_state_hashes,
        } => {
            let _: &AccountId = member;
            let _: &[u8; 32] = expected_group_state_hash;
            let _: &Vec<(ContextId, [u8; 32])> = expected_context_state_hashes;
            "MemberLeft"
        }
        GroupOp::MemberRoleSet { member, role } => {
            let _: &AccountId = member;
            let _: &GroupMemberRole = role;
            "MemberRoleSet"
        }
        GroupOp::MemberCapabilitySet {
            member,
            capabilities,
        } => {
            let _: &AccountId = member;
            let _: &MemberCapabilities = capabilities;
            "MemberCapabilitySet"
        }
        GroupOp::DefaultCapabilitiesSet { capabilities } => {
            let _: &MemberCapabilities = capabilities;
            "DefaultCapabilitiesSet"
        }
        GroupOp::TargetApplicationSet {
            bytecode_id,
            target_application_id,
        } => {
            let _: &BytecodeId = bytecode_id;
            let _: &ApplicationId = target_application_id;
            "TargetApplicationSet"
        }
        GroupOp::ContextRegistered {
            context_id,
            application_id,
            blob_id,
            source,
            service_name,
        } => {
            let _: &ContextId = context_id;
            let _: &ApplicationId = application_id;
            let _: &BlobId = blob_id;
            let _: &String = source;
            let _: &Option<String> = service_name;
            "ContextRegistered"
        }
        GroupOp::ContextDetached { context_id } => {
            let _: &ContextId = context_id;
            "ContextDetached"
        }
        GroupOp::SubgroupVisibilitySet { mode } => {
            let _: &VisibilityMode = mode;
            "SubgroupVisibilitySet"
        }
        GroupOp::GroupMetadataSet { name, data } => {
            let _: &Option<String> = name;
            let _: &BTreeMap<String, String> = data;
            "GroupMetadataSet"
        }
        GroupOp::MemberMetadataSet { member, name, data } => {
            let _: &AccountId = member;
            let _: &Option<String> = name;
            let _: &BTreeMap<String, String> = data;
            "MemberMetadataSet"
        }
        GroupOp::ContextMetadataSet {
            context_id,
            name,
            data,
        } => {
            let _: &ContextId = context_id;
            let _: &Option<String> = name;
            let _: &BTreeMap<String, String> = data;
            "ContextMetadataSet"
        }
        GroupOp::GroupDelete => "GroupDelete",
        GroupOp::GroupMigrationSet { migration } => {
            let _: &Option<Vec<u8>> = migration;
            "GroupMigrationSet"
        }
        GroupOp::ContextCapabilityGranted {
            context_id,
            member,
            capability,
        } => {
            let _: &ContextId = context_id;
            let _: &AccountId = member;
            let _: &ContextCapabilityBits = capability;
            "ContextCapabilityGranted"
        }
        GroupOp::ContextCapabilityRevoked {
            context_id,
            member,
            capability,
        } => {
            let _: &ContextId = context_id;
            let _: &AccountId = member;
            let _: &ContextCapabilityBits = capability;
            "ContextCapabilityRevoked"
        }
        GroupOp::TeeAdmissionPolicySet {
            allowed_mrtd,
            allowed_rtmr0,
            allowed_rtmr1,
            allowed_rtmr2,
            allowed_rtmr3,
            allowed_tcb_statuses,
            accept_mock,
        } => {
            let _: &Vec<String> = allowed_mrtd;
            let _: &Vec<String> = allowed_rtmr0;
            let _: &Vec<String> = allowed_rtmr1;
            let _: &Vec<String> = allowed_rtmr2;
            let _: &Vec<String> = allowed_rtmr3;
            let _: &Vec<String> = allowed_tcb_statuses;
            let _: &bool = accept_mock;
            "TeeAdmissionPolicySet"
        }
        GroupOp::MemberJoinedViaTeeAttestation {
            member,
            quote_hash,
            mrtd,
            rtmr0,
            rtmr1,
            rtmr2,
            rtmr3,
            tcb_status,
            role,
        } => {
            let _: &AccountId = member;
            let _: &[u8; 32] = quote_hash;
            let _: &String = mrtd;
            let _: &String = rtmr0;
            let _: &String = rtmr1;
            let _: &String = rtmr2;
            let _: &String = rtmr3;
            let _: &String = tcb_status;
            let _: &GroupMemberRole = role;
            "MemberJoinedViaTeeAttestation"
        }
        GroupOp::MemberSetAutoFollow {
            target,
            auto_follow_contexts,
            auto_follow_subgroups,
        } => {
            let _: &AccountId = target;
            let _: &bool = auto_follow_contexts;
            let _: &bool = auto_follow_subgroups;
            "MemberSetAutoFollow"
        }
        GroupOp::TransferOwnership { new_owner } => {
            let _: &AccountId = new_owner;
            "TransferOwnership"
        }
        GroupOp::CascadeUpgrade {
            from_bytecode_id,
            bytecode_id,
            target_application_id,
            to_state_version,
            migration,
            cascade_hlc,
        } => {
            let _: &BytecodeId = from_bytecode_id;
            let _: &BytecodeId = bytecode_id;
            let _: &ApplicationId = target_application_id;
            let _: &u32 = to_state_version;
            let _: &Option<Vec<u8>> = migration;
            let _: &HybridTimestamp = cascade_hlc;
            "CascadeUpgrade"
        }
        GroupOp::GroupKeyRotated { departed } => {
            let _: &AccountId = departed;
            "GroupKeyRotated"
        }
        GroupOp::AccountDeviceLinked {
            genesis,
            chain,
            cert,
            endorsement,
        } => {
            let _: &AccountGenesis = genesis;
            let _: &Vec<RootKeyHandoff> = chain;
            let _: &DeviceCert = cert;
            let _: &AccountMemberEndorsement = endorsement;
            "AccountDeviceLinked"
        }
        GroupOp::AccountDeviceUnlinked {
            account,
            device,
            proof,
        } => {
            let _: &AccountId = account;
            let _: &DeviceId = device;
            let _: &Option<SignedDeviceRevocation> = proof;
            "AccountDeviceUnlinked"
        }
        GroupOp::AccountKeysRotated { handoff } => {
            let _: &RootKeyHandoff = handoff;
            "AccountKeysRotated"
        }
        GroupOp::GroupKeyRotatedForDevice { device } => {
            let _: &DeviceId = device;
            "GroupKeyRotatedForDevice"
        }
    }
}

fn root_op_variant_name(op: &RootOp) -> &'static str {
    match op {
        RootOp::GroupCreated {
            group_id,
            parent_id,
            restricted,
            admin,
        } => {
            let _: &ContextGroupId = group_id;
            let _: &ContextGroupId = parent_id;
            let _: &bool = restricted;
            let _: &AccountId = admin;
            "GroupCreated"
        }
        RootOp::GroupReparented {
            child_group_id,
            new_parent_id,
        } => {
            let _: &ContextGroupId = child_group_id;
            let _: &ContextGroupId = new_parent_id;
            "GroupReparented"
        }
        RootOp::GroupDeleted {
            root_group_id,
            cascade_group_ids,
            cascade_context_ids,
        } => {
            let _: &ContextGroupId = root_group_id;
            let _: &Vec<ContextGroupId> = cascade_group_ids;
            let _: &Vec<ContextId> = cascade_context_ids;
            "GroupDeleted"
        }
        RootOp::AdminChanged { new_admin } => {
            let _: &AccountId = new_admin;
            "AdminChanged"
        }
        RootOp::PolicyUpdated { policy_bytes } => {
            let _: &Vec<u8> = policy_bytes;
            "PolicyUpdated"
        }
        RootOp::MemberJoined {
            member,
            signed_invitation,
            account,
        } => {
            let _: &AccountId = member;
            let _: &SignedGroupOpenInvitation = signed_invitation;
            let _: &Box<JoinAccountCredential> = account;
            "MemberJoined"
        }
        RootOp::KeyDelivery { group_id, envelope } => {
            let _: &ContextGroupId = group_id;
            let _: &KeyEnvelope = envelope;
            "KeyDelivery"
        }
        RootOp::MemberJoinedOpen {
            member,
            group_id,
            account,
        } => {
            let _: &AccountId = member;
            let _: &ContextGroupId = group_id;
            let _: &Box<JoinAccountCredential> = account;
            "MemberJoinedOpen"
        }
        RootOp::MemberJoinedAt {
            member,
            signed_invitation,
            joined_at,
            account,
        } => {
            let _: &AccountId = member;
            let _: &SignedGroupOpenInvitation = signed_invitation;
            let _: &u64 = joined_at;
            let _: &Box<JoinAccountCredential> = account;
            "MemberJoinedAt"
        }
        RootOp::NamespaceCreated { founder, account } => {
            let _: &AccountId = founder;
            let _: &Box<JoinAccountCredential> = account;
            "NamespaceCreated"
        }
        RootOp::MemberJoinedViaTeeAttestation {
            group_id,
            member,
            quote_hash,
            mrtd,
            rtmr0,
            rtmr1,
            rtmr2,
            rtmr3,
            tcb_status,
            role,
            account,
        } => {
            let _: &ContextGroupId = group_id;
            let _: &PublicKey = member;
            let _: &[u8; 32] = quote_hash;
            let _: &String = mrtd;
            let _: &String = rtmr0;
            let _: &String = rtmr1;
            let _: &String = rtmr2;
            let _: &String = rtmr3;
            let _: &String = tcb_status;
            let _: &GroupMemberRole = role;
            let _: &Box<JoinAccountCredential> = account;
            "MemberJoinedViaTeeAttestation"
        }
    }
}

fn namespace_op_variant_name(op: &NamespaceOp) -> &'static str {
    match op {
        NamespaceOp::Root(f0) => {
            let _: &RootOp = f0;
            "Root"
        }
        NamespaceOp::Group {
            group_id,
            key_id,
            encrypted,
            key_rotation,
        } => {
            let _: &ContextGroupId = group_id;
            let _: &KeyId = key_id;
            let _: &EncryptedGroupOp = encrypted;
            let _: &Option<KeyRotation> = key_rotation;
            "Group"
        }
    }
}

fn envelope_recipient_variant_name(op: &EnvelopeRecipient) -> &'static str {
    match op {
        EnvelopeRecipient::Member {
            identity,
            ephemeral_pk,
        } => {
            let _: &PublicKey = identity;
            let _: &PublicKey = ephemeral_pk;
            "Member"
        }
        EnvelopeRecipient::Device {
            device,
            ephemeral_pk,
        } => {
            let _: &DeviceId = device;
            let _: &KemPublicKey = ephemeral_pk;
            "Device"
        }
    }
}

#[expect(
    dead_code,
    reason = "compile-time anchor: StoredNamespaceEntry is storage-side and has \
              no golden corpus to call this from, but the exhaustive match must \
              still fail to compile when a variant is added"
)]
fn stored_entry_variant_name(op: &StoredNamespaceEntry) -> &'static str {
    match op {
        StoredNamespaceEntry::Signed(f0) => {
            let _: &SignedNamespaceOp = f0;
            "Signed"
        }
        StoredNamespaceEntry::Opaque(f0) => {
            let _: &OpaqueSkeleton = f0;
            "Opaque"
        }
    }
}

/// Struct field lists, destructured WITHOUT `..` so a new field is a compile
/// error rather than a silently undescribed trailing field.
#[expect(
    dead_code,
    reason = "compile-time anchor: exists to be type-checked, never called"
)]
fn struct_fields_are_described(
    signable_group: &SignableGroupOp,
    signed_group: &SignedGroupOp,
    signable_ns: &SignableNamespaceOp,
    signed_ns: &SignedNamespaceOp,
    encrypted: &EncryptedGroupOp,
    envelope: &KeyEnvelope,
    rotation: &KeyRotation,
    skeleton: &OpaqueSkeleton,
) {
    let SignableGroupOp {
        version,
        group_id,
        parent_op_hashes,
        signer,
        nonce,
        op,
    } = signable_group;
    let _: &u8 = version;
    let _: &ContextGroupId = group_id;
    let _: &Vec<[u8; 32]> = parent_op_hashes;
    let _: &PublicKey = signer;
    let _: &u64 = nonce;
    let _: &GroupOp = op;
    let SignedGroupOp {
        version,
        group_id,
        parent_op_hashes,
        signer,
        nonce,
        op,
        signature,
    } = signed_group;
    let _: &u8 = version;
    let _: &ContextGroupId = group_id;
    let _: &Vec<[u8; 32]> = parent_op_hashes;
    let _: &PublicKey = signer;
    let _: &u64 = nonce;
    let _: &GroupOp = op;
    let _: &[u8; 64] = signature;
    let SignableNamespaceOp {
        version,
        namespace_id,
        parent_op_hashes,
        signer,
        nonce,
        op,
    } = signable_ns;
    let _: &u8 = version;
    let _: &NamespaceId = namespace_id;
    let _: &Vec<[u8; 32]> = parent_op_hashes;
    let _: &PublicKey = signer;
    let _: &u64 = nonce;
    let _: &NamespaceOp = op;
    let SignedNamespaceOp {
        version,
        namespace_id,
        parent_op_hashes,
        signer,
        nonce,
        op,
        signature,
    } = signed_ns;
    let _: &u8 = version;
    let _: &NamespaceId = namespace_id;
    let _: &Vec<[u8; 32]> = parent_op_hashes;
    let _: &PublicKey = signer;
    let _: &u64 = nonce;
    let _: &NamespaceOp = op;
    let _: &[u8; 64] = signature;
    let EncryptedGroupOp { nonce, ciphertext } = encrypted;
    let _: &[u8; 12] = nonce;
    let _: &Vec<u8> = ciphertext;
    let KeyEnvelope {
        recipient,
        sender,
        nonce,
        ciphertext,
        signature,
    } = envelope;
    let _: &EnvelopeRecipient = recipient;
    let _: &PublicKey = sender;
    let _: &[u8; 12] = nonce;
    let _: &Vec<u8> = ciphertext;
    let _: &[u8; 64] = signature;
    let KeyRotation {
        new_key_id,
        envelopes,
    } = rotation;
    let _: &KeyId = new_key_id;
    let _: &Vec<KeyEnvelope> = envelopes;
    let OpaqueSkeleton {
        delta_id,
        parent_op_hashes,
        group_id,
        signer,
    } = skeleton;
    let _: &[u8; 32] = delta_id;
    let _: &Vec<[u8; 32]> = parent_op_hashes;
    let _: &ContextGroupId = group_id;
    let _: &PublicKey = signer;
}

// ---------------------------------------------------------------------------
// Leaves: types this crate embeds but does not own.
//
// The descriptor stops at this crate's boundary on purpose (covering the whole
// crate graph would make every unrelated refactor a wire review). Instead each
// foreign type is held still by the bytes a canonical all-zero instance
// encodes to — a field added to `DeviceCert` moves its leaf line without this
// file knowing anything about `DeviceCert`.
// ---------------------------------------------------------------------------

fn zero_pk() -> PublicKey {
    PublicKey::from([0u8; 32])
}
const ZERO_ACCOUNT: AccountId = AccountId::from_raw([0u8; 32]);
const ZERO_DEVICE: DeviceId = DeviceId::from_raw([0u8; 32]);

fn zero_genesis() -> AccountGenesis {
    AccountGenesis::new(zero_pk())
}

fn zero_cert() -> DeviceCert {
    DeviceCert {
        account: ZERO_ACCOUNT,
        device: ZERO_DEVICE,
        sign_pk: zero_pk(),
        kem_pk: KemPublicKey::from([0u8; 32]),
        key_epoch: 0,
        device_epoch: 0,
        signature: [0u8; 64],
    }
}

fn zero_handoff() -> RootKeyHandoff {
    RootKeyHandoff {
        account: ZERO_ACCOUNT,
        from_epoch: 0,
        new_root_sign_pk: zero_pk(),
        signature: [0u8; 64],
    }
}

fn zero_revocation() -> DeviceRevocation {
    DeviceRevocation {
        account: ZERO_ACCOUNT,
        device: ZERO_DEVICE,
        key_epoch: 0,
        signature: [0u8; 64],
    }
}

fn zero_invitation() -> SignedGroupOpenInvitation {
    SignedGroupOpenInvitation {
        invitation: GroupInvitationFromAdmin {
            inviter_identity: (*zero_pk()).into(),
            group_id: ContextGroupId::from([0u8; 32]),
            expiration_timestamp: 0_u64.into(),
            invitation_nonce: [0u8; 32],
            invited_role: 1,
        },
        inviter_signature: String::new(),
        inviter_account: None,
        application_id: None,
        bytecode_id: None,
    }
}

fn leaf<T: borsh::BorshSerialize>(name: &'static str, value: &T) -> Leaf {
    Leaf {
        name,
        bytes: borsh::to_vec(value).expect("borsh-encode a canonical leaf instance"),
    }
}

fn described_leaves() -> Vec<Leaf> {
    vec![
        leaf("AccountGenesis", &zero_genesis()),
        leaf(
            "AccountMemberEndorsement",
            &AccountMemberEndorsement {
                account: ZERO_ACCOUNT,
                member: zero_pk(),
                signature: [0u8; 64],
            },
        ),
        leaf("DeviceCert", &zero_cert()),
        leaf("RootKeyHandoff", &zero_handoff()),
        leaf("DeviceRevocation", &zero_revocation()),
        leaf(
            "JoinAccountCredential",
            &AccountProof {
                genesis: zero_genesis(),
                chain: vec![],
                statement: zero_cert(),
            },
        ),
        leaf(
            "SignedDeviceRevocation",
            &AccountProof {
                genesis: zero_genesis(),
                chain: vec![],
                statement: zero_revocation(),
            },
        ),
        leaf("SignedGroupOpenInvitation", &zero_invitation()),
        leaf("HybridTimestamp::zero", &HybridTimestamp::zero()),
        leaf("MemberCapabilities::empty", &MemberCapabilities::empty()),
        // Role and visibility are enums whose tags decide AUTHORITY, so every
        // variant is pinned individually rather than by one representative.
        leaf("GroupMemberRole::Admin", &GroupMemberRole::Admin),
        leaf("GroupMemberRole::Member", &GroupMemberRole::Member),
        leaf("GroupMemberRole::ReadOnly", &GroupMemberRole::ReadOnly),
        leaf(
            "GroupMemberRole::ReadOnlyTee",
            &GroupMemberRole::ReadOnlyTee,
        ),
        leaf("VisibilityMode::Open", &VisibilityMode::Open),
        leaf("VisibilityMode::Restricted", &VisibilityMode::Restricted),
        // The 32-byte id newtypes: transparent today, and this is what says so.
        leaf("AccountId", &ZERO_ACCOUNT),
        leaf("DeviceId", &ZERO_DEVICE),
        leaf("PublicKey", &zero_pk()),
        leaf("KemPublicKey", &KemPublicKey::from([0u8; 32])),
        leaf("ContextId", &ContextId::from([0u8; 32])),
        leaf("ContextGroupId", &ContextGroupId::from([0u8; 32])),
        leaf("BytecodeId", &BytecodeId::from([0u8; 32])),
        leaf("ApplicationId", &ApplicationId::from([0u8; 32])),
        leaf("BlobId", &BlobId::from([0u8; 32])),
        leaf("NamespaceId", &NamespaceId::new([0u8; 32])),
        leaf("KeyId", &KeyId::new([0u8; 32])),
    ]
}

fn surface() -> Surface {
    Surface {
        label: "calimero-governance-types",
        types: described_types(),
        leaves: described_leaves(),
    }
}

fn snapshot_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wire-fingerprint.txt")
}

#[test]
fn wire_fingerprint_matches_snapshot() {
    assert_snapshot(&surface(), &snapshot_path());
}

// ---------------------------------------------------------------------------
// Descriptor <-> reality.
// ---------------------------------------------------------------------------

/// The golden corpus, keyed by the ordinal each vector claims.
///
/// This is the registry the exhaustiveness test iterates: a variant with no
/// entry here has no frozen bytes, and the test says so by name.
const GROUP_OP_GOLDENS: &[(u8, &[u8])] = &[
    (0, super::GOLDEN_GROUP_OP_NOOP),
    (1, super::GOLDEN_GROUP_OP_MEMBER_ADDED),
    (2, super::GOLDEN_GROUP_OP_MEMBER_REMOVED),
    (3, super::GOLDEN_GROUP_OP_MEMBER_LEFT),
    (4, super::GOLDEN_GROUP_OP_MEMBER_ROLE_SET),
    (5, super::GOLDEN_GROUP_OP_MEMBER_CAPABILITY_SET),
    (6, super::GOLDEN_GROUP_OP_DEFAULT_CAPABILITIES_SET),
    (7, super::GOLDEN_GROUP_OP_TARGET_APPLICATION_SET),
    (8, super::GOLDEN_GROUP_OP_CONTEXT_REGISTERED),
    (9, super::GOLDEN_GROUP_OP_CONTEXT_DETACHED),
    (10, super::GOLDEN_GROUP_OP_SUBGROUP_VISIBILITY_SET),
    (11, super::GOLDEN_GROUP_OP_GROUP_METADATA_SET),
    (12, super::GOLDEN_GROUP_OP_MEMBER_METADATA_SET),
    (13, super::GOLDEN_GROUP_OP_CONTEXT_METADATA_SET),
    (14, super::GOLDEN_GROUP_OP_GROUP_DELETE),
    (15, super::GOLDEN_GROUP_OP_GROUP_MIGRATION_SET),
    (16, super::GOLDEN_GROUP_OP_CONTEXT_CAPABILITY_GRANTED),
    (17, super::GOLDEN_GROUP_OP_CONTEXT_CAPABILITY_REVOKED),
    (18, super::GOLDEN_GROUP_OP_TEE_ADMISSION_POLICY_SET),
    (19, super::GOLDEN_GROUP_OP_MEMBER_JOINED_VIA_TEE),
    (20, super::GOLDEN_GROUP_OP_MEMBER_SET_AUTO_FOLLOW),
    (21, super::GOLDEN_GROUP_OP_TRANSFER_OWNERSHIP),
    (22, super::GOLDEN_GROUP_OP_CASCADE_UPGRADE),
    (23, super::GOLDEN_GROUP_OP_GROUP_KEY_ROTATED),
    (24, super::GOLDEN_GROUP_OP_ACCOUNT_DEVICE_LINKED),
    (25, super::GOLDEN_GROUP_OP_ACCOUNT_DEVICE_UNLINKED),
    (26, super::GOLDEN_GROUP_OP_ACCOUNT_KEYS_ROTATED),
    (27, super::GOLDEN_GROUP_OP_GROUP_KEY_ROTATED_FOR_DEVICE),
];

/// `RootOp` goldens are `NamespaceOp`-wrapped (byte 0 is the `NamespaceOp`
/// tag, byte 1 the `RootOp` tag) — see the header of `tests.rs`.
const ROOT_OP_GOLDENS: &[(u8, &[u8])] = &[
    (0, super::GOLDEN_ROOT_OP_GROUP_CREATED),
    (1, super::GOLDEN_ROOT_OP_GROUP_REPARENTED),
    (2, super::GOLDEN_ROOT_OP_GROUP_DELETED),
    (3, super::GOLDEN_ROOT_OP_ADMIN_CHANGED),
    (4, super::GOLDEN_ROOT_OP_POLICY_UPDATED),
    (5, super::GOLDEN_ROOT_OP_MEMBER_JOINED),
    (6, super::GOLDEN_ROOT_OP_KEY_DELIVERY),
    (7, super::GOLDEN_ROOT_OP_MEMBER_JOINED_OPEN),
    (8, super::GOLDEN_ROOT_OP_MEMBER_JOINED_AT),
    (9, super::GOLDEN_ROOT_OP_NAMESPACE_CREATED),
    (10, super::GOLDEN_ROOT_OP_MEMBER_JOINED_VIA_TEE),
];

const NAMESPACE_OP_GOLDENS: &[(u8, &[u8])] = &[
    (0, super::GOLDEN_ROOT_OP_GROUP_CREATED),
    (1, super::GOLDEN_NAMESPACE_OP_GROUP),
];

fn variants_of(ty: &TypeDesc) -> &'static [Variant] {
    match ty.shape {
        Shape::Enum(v) => v,
        Shape::Struct(_) => panic!("{} is not an enum", ty.name),
    }
}

/// Every described variant must have frozen bytes that decode to exactly that
/// variant, at exactly that ordinal.
///
/// This is the join between the two halves of the gate: the descriptor says
/// "ordinal 7 is TargetApplicationSet", the golden says "these bytes start
/// with 7", and the enum says "these bytes are a TargetApplicationSet". Break
/// any one of the three and this test names it.
#[test]
fn every_described_variant_has_a_golden_that_decodes_to_it() {
    let mut failures: Vec<String> = Vec::new();

    for v in variants_of(&GROUP_OP) {
        let Some((_, bytes)) = GROUP_OP_GOLDENS.iter().find(|(o, _)| *o == v.ordinal) else {
            failures.push(format!(
                "GroupOp::{} (ordinal {}) has no frozen bytes — add a \
                 GOLDEN_GROUP_OP_* vector and register it in GROUP_OP_GOLDENS",
                v.name, v.ordinal
            ));
            continue;
        };
        match borsh::from_slice::<GroupOp>(bytes) {
            Err(e) => failures.push(format!("GroupOp ordinal {}: decode failed: {e}", v.ordinal)),
            Ok(op) => {
                let got = group_op_variant_name(&op);
                if got != v.name {
                    failures.push(format!(
                        "GroupOp ordinal {}: descriptor says {}, bytes decoded as {got}",
                        v.ordinal, v.name
                    ));
                }
                if bytes[0] != v.ordinal {
                    failures.push(format!(
                        "GroupOp::{}: descriptor ordinal {} but golden's tag byte is {}",
                        v.name, v.ordinal, bytes[0]
                    ));
                }
            }
        }
    }

    for v in variants_of(&ROOT_OP) {
        let Some((_, bytes)) = ROOT_OP_GOLDENS.iter().find(|(o, _)| *o == v.ordinal) else {
            failures.push(format!(
                "RootOp::{} (ordinal {}) has no frozen bytes — add a \
                 GOLDEN_ROOT_OP_* vector and register it in ROOT_OP_GOLDENS",
                v.name, v.ordinal
            ));
            continue;
        };
        match borsh::from_slice::<NamespaceOp>(bytes) {
            Err(e) => failures.push(format!("RootOp ordinal {}: decode failed: {e}", v.ordinal)),
            Ok(NamespaceOp::Group { .. }) => failures.push(format!(
                "RootOp ordinal {}: golden is not NamespaceOp::Root-wrapped",
                v.ordinal
            )),
            Ok(NamespaceOp::Root(root)) => {
                let got = root_op_variant_name(&root);
                if got != v.name {
                    failures.push(format!(
                        "RootOp ordinal {}: descriptor says {}, bytes decoded as {got}",
                        v.ordinal, v.name
                    ));
                }
                if bytes[1] != v.ordinal {
                    failures.push(format!(
                        "RootOp::{}: descriptor ordinal {} but golden's tag byte is {}",
                        v.name, v.ordinal, bytes[1]
                    ));
                }
            }
        }
    }

    for v in variants_of(&NAMESPACE_OP) {
        let Some((_, bytes)) = NAMESPACE_OP_GOLDENS.iter().find(|(o, _)| *o == v.ordinal) else {
            failures.push(format!(
                "NamespaceOp::{} (ordinal {}) has no frozen bytes",
                v.name, v.ordinal
            ));
            continue;
        };
        match borsh::from_slice::<NamespaceOp>(bytes) {
            Err(e) => failures.push(format!(
                "NamespaceOp ordinal {}: decode failed: {e}",
                v.ordinal
            )),
            Ok(op) => {
                let got = namespace_op_variant_name(&op);
                if got != v.name {
                    failures.push(format!(
                        "NamespaceOp ordinal {}: descriptor says {}, bytes decoded as {got}",
                        v.ordinal, v.name
                    ));
                }
            }
        }
    }

    // `EnvelopeRecipient` rides inside a `KeyDelivery`, so its two goldens are
    // the two KeyDelivery vectors that differ only in the addressing tag.
    for (bytes, expected) in [
        (super::GOLDEN_ROOT_OP_KEY_DELIVERY, "Member"),
        (super::GOLDEN_ROOT_OP_KEY_DELIVERY_TO_DEVICE, "Device"),
    ] {
        match borsh::from_slice::<NamespaceOp>(bytes) {
            Ok(NamespaceOp::Root(RootOp::KeyDelivery { envelope, .. })) => {
                let got = envelope_recipient_variant_name(&envelope.recipient);
                if got != expected {
                    failures.push(format!(
                        "EnvelopeRecipient: expected {expected}, decoded {got}"
                    ));
                }
            }
            other => failures.push(format!(
                "EnvelopeRecipient golden is not a KeyDelivery: {other:?}"
            )),
        }
    }

    assert!(
        failures.is_empty(),
        "descriptor/golden mismatches ({}):\n{}\n\n\
         Every variant on the replicated wire needs frozen bytes: a same-binary \
         round-trip cannot catch a renumbered discriminant, because encoder and \
         decoder shift together.",
        failures.len(),
        failures.join("\n")
    );
}

/// There must be no variant AFTER the last described one.
///
/// Without this, appending a variant and forgetting the descriptor would be
/// invisible: every described ordinal still checks out, and the new one is
/// simply never looked at. Decoding a one-byte buffer whose only content is
/// the tag distinguishes the three cases cleanly:
///
/// * unknown tag        -> `InvalidData` ("Unexpected variant tag") — correct;
/// * known tag, fields  -> `UnexpectedEof` (it wanted more bytes) — undescribed;
/// * known tag, unit    -> `Ok` — undescribed.
#[test]
fn no_variant_exists_past_the_described_end() {
    fn assert_absent(what: &str, prefix: &[u8], next_ordinal: u8, decode: fn(&[u8]) -> ErrorKind) {
        let mut buf = prefix.to_vec();
        buf.push(next_ordinal);
        assert_eq!(
            decode(&buf),
            ErrorKind::InvalidData,
            "{what} ordinal {next_ordinal} decodes to something — a variant was \
             appended without adding it to the descriptor in this file (and \
             without frozen bytes). Append it to the descriptor, add a golden, \
             then regenerate the snapshot."
        );
    }

    fn group_kind(b: &[u8]) -> ErrorKind {
        borsh::from_slice::<GroupOp>(b).map_or_else(|e| e.kind(), |_| ErrorKind::Other)
    }
    fn ns_kind(b: &[u8]) -> ErrorKind {
        borsh::from_slice::<NamespaceOp>(b).map_or_else(|e| e.kind(), |_| ErrorKind::Other)
    }

    let group_variants = variants_of(&GROUP_OP);
    let next = u8::try_from(group_variants.len()).expect("fewer than 256 variants");
    assert_absent("GroupOp", &[], next, group_kind);

    let root_variants = variants_of(&ROOT_OP);
    let next = u8::try_from(root_variants.len()).expect("fewer than 256 variants");
    assert_absent("RootOp", &[0], next, ns_kind);

    let ns_variants = variants_of(&NAMESPACE_OP);
    let next = u8::try_from(ns_variants.len()).expect("fewer than 256 variants");
    assert_absent("NamespaceOp", &[], next, ns_kind);
}

/// `StoredNamespaceEntry` is storage-side rather than gossip-side, so it has
/// no golden corpus; the descriptor is still anchored by its exhaustive match
/// above and by this ordinal-boundary check.
#[test]
fn stored_namespace_entry_has_no_variant_past_the_described_end() {
    let variants = variants_of(&STORED_NAMESPACE_ENTRY);
    let next = u8::try_from(variants.len()).expect("fewer than 256 variants");
    let err = borsh::from_slice::<StoredNamespaceEntry>(&[next]).expect_err("no such variant");
    assert_eq!(err.kind(), ErrorKind::InvalidData);
}
