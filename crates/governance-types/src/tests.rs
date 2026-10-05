use super::*;

use calimero_primitives::identity::{PrivateKey, PublicKey};
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;

// ---------------------------------------------------------------------------
// Borsh discriminant golden tests
//
// Each test below embeds a fully frozen byte vector that encodes one
// variant of `GroupOp` or `RootOp` (wrapped in `NamespaceOp::Root`).
// The bytes are decoded with the CURRENT enum — never re-encoded here.
//
// A same-binary encode → decode round-trip CANNOT catch a mid-enum
// insertion: both the encoder and decoder use the shifted ordinal, so
// they silently agree on the wrong variant. Decoding FROZEN bytes is the
// only test that catches a renumber: the discriminant byte stays fixed in
// the source, but the enum shifts under it, so the decoder sees an
// unexpected variant or fails.
//
// Construction: all-zero fixed data was used for every field
// (PublicKey = [0u8;32], IDs = [0u8;32], integers = 0, Options = None,
// collections = empty). Borsh reads these without ed25519 or range
// validation — the frozen bytes are stable across builds. Registry
// coordinates are the one exception: they are mandatory and rejected when
// empty, so the application ops carry a real `"app"@"1.0.0"` pair.
// ---------------------------------------------------------------------------

/// Borsh bytes the mandatory `"app"` / `"1.0.0"` coordinate pair adds to an op.
const GOLDEN_COORD_TAIL_BYTES: usize = 16;

// ---- GroupOp golden bytes ----
//
// Byte layout for GroupOp: bytes[0] = variant discriminant (u8 ordinal),
// remainder = field payload, all fields zeroed / empty.

/// GroupOp ordinal 0 — Noop (no fields; full encoding = discriminant only)
const GOLDEN_GROUP_OP_NOOP: &[u8] = &[0];

/// GroupOp ordinal 1 — MemberAdded { member: [0;32], role: Member(1) }
const GOLDEN_GROUP_OP_MEMBER_ADDED: &[u8] = &[
    1, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // member PublicKey [0u8;32]
    1, // role = Member (ordinal 1)
];

/// GroupOp ordinal 2 — MemberRemoved { member: [0;32], hash: [0;32], hashes: [] }
const GOLDEN_GROUP_OP_MEMBER_REMOVED: &[u8] = &[
    2, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // member
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // expected_group_state_hash
    0, 0, 0, 0, // expected_context_state_hashes (vec len = 0)
];

/// GroupOp ordinal 3 — MemberLeft (same shape as MemberRemoved)
const GOLDEN_GROUP_OP_MEMBER_LEFT: &[u8] = &[
    3, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // member
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // expected_group_state_hash
    0, 0, 0, 0, // expected_context_state_hashes (vec len = 0)
];

/// GroupOp ordinal 4 — MemberRoleSet { member: [0;32], role: Admin(0) }
const GOLDEN_GROUP_OP_MEMBER_ROLE_SET: &[u8] = &[
    4, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // member
    0, // role = Admin (ordinal 0)
];

/// GroupOp ordinal 5 — MemberCapabilitySet { member: [0;32], capabilities: 0 }
const GOLDEN_GROUP_OP_MEMBER_CAPABILITY_SET: &[u8] = &[
    5, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // member
    0, 0, 0, 0, // capabilities u32 = 0
];

/// GroupOp ordinal 6 — DefaultCapabilitiesSet { capabilities: 0 }
const GOLDEN_GROUP_OP_DEFAULT_CAPABILITIES_SET: &[u8] = &[
    6, // discriminant
    0, 0, 0, 0, // capabilities u32 = 0
];

/// GroupOp ordinal 7 - TargetApplicationSet { bytecode_id: [0;32].into(), target: [0;32], "app"@"1.0.0" }
const GOLDEN_GROUP_OP_TARGET_APPLICATION_SET: &[u8] = &[
    7, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // bytecode_id [0u8;32]
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // target_application_id [0u8;32]
    3, 0, 0, 0, b'a', b'p', b'p', // package = "app"
    5, 0, 0, 0, b'1', b'.', b'0', b'.', b'0', // version = "1.0.0"
];

/// GroupOp ordinal 8 - ContextRegistered (empty/zero fields; coordinates "app"@"1.0.0")
const GOLDEN_GROUP_OP_CONTEXT_REGISTERED: &[u8] = &[
    8, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // context_id
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // application_id
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // blob_id
    0, 0, 0, 0, // source String (len = 0)
    0, // service_name = None
    3, 0, 0, 0, b'a', b'p', b'p', // package = "app"
    5, 0, 0, 0, b'1', b'.', b'0', b'.', b'0', // version = "1.0.0"
];

/// GroupOp ordinal 9 - ContextDetached { context_id: [0;32] }
const GOLDEN_GROUP_OP_CONTEXT_DETACHED: &[u8] = &[
    9, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // context_id
];

/// GroupOp ordinal 10 - SubgroupVisibilitySet { mode: 0 }
const GOLDEN_GROUP_OP_SUBGROUP_VISIBILITY_SET: &[u8] = &[
    10, // discriminant
    0,  // mode = 0
];

/// GroupOp ordinal 11 - GroupMetadataSet { name: None, data: {} }
const GOLDEN_GROUP_OP_GROUP_METADATA_SET: &[u8] = &[
    11, // discriminant
    0,  // name = None
    0, 0, 0, 0, // data BTreeMap len = 0
];

/// GroupOp ordinal 12 - MemberMetadataSet { member: [0;32], name: None, data: {} }
const GOLDEN_GROUP_OP_MEMBER_METADATA_SET: &[u8] = &[
    12, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // member
    0, // name = None
    0, 0, 0, 0, // data len = 0
];

/// GroupOp ordinal 13 - ContextMetadataSet { context_id: [0;32], name: None, data: {} }
const GOLDEN_GROUP_OP_CONTEXT_METADATA_SET: &[u8] = &[
    13, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // context_id
    0, // name = None
    0, 0, 0, 0, // data len = 0
];

/// GroupOp ordinal 14 - GroupDelete (no fields; full encoding = discriminant only)
const GOLDEN_GROUP_OP_GROUP_DELETE: &[u8] = &[14];

/// GroupOp ordinal 15 - GroupMigrationSet { migration: None }
const GOLDEN_GROUP_OP_GROUP_MIGRATION_SET: &[u8] = &[
    15, // discriminant
    0,  // migration = None
];

/// GroupOp ordinal 16 - ContextCapabilityGranted { context_id: [0;32], member: [0;32], capability: 1 }
const GOLDEN_GROUP_OP_CONTEXT_CAPABILITY_GRANTED: &[u8] = &[
    16, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // context_id
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // member
    1, // capability (must be non-zero: ContextCapabilityBits rejects 0 on the wire)
];

/// GroupOp ordinal 17 - ContextCapabilityRevoked (same shape as Granted)
const GOLDEN_GROUP_OP_CONTEXT_CAPABILITY_REVOKED: &[u8] = &[
    17, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // context_id
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // member
    1, // capability (must be non-zero: ContextCapabilityBits rejects 0 on the wire)
];

/// GroupOp ordinal 18 - TeeAdmissionPolicySet (6 empty Vec<String> + accept_mock=false)
const GOLDEN_GROUP_OP_TEE_ADMISSION_POLICY_SET: &[u8] = &[
    18, // discriminant
    0, 0, 0, 0, // allowed_mrtd vec len = 0
    0, 0, 0, 0, // allowed_rtmr0 vec len = 0
    0, 0, 0, 0, // allowed_rtmr1 vec len = 0
    0, 0, 0, 0, // allowed_rtmr2 vec len = 0
    0, 0, 0, 0, // allowed_rtmr3 vec len = 0
    0, 0, 0, 0, // allowed_tcb_statuses vec len = 0
    0, // accept_mock = false
];

/// GroupOp ordinal 19 - MemberJoinedViaTeeAttestation (all empty/zero)
const GOLDEN_GROUP_OP_MEMBER_JOINED_VIA_TEE: &[u8] = &[
    19, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // member
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // quote_hash
    0, 0, 0, 0, // mrtd String len = 0
    0, 0, 0, 0, // rtmr0 String len = 0
    0, 0, 0, 0, // rtmr1 String len = 0
    0, 0, 0, 0, // rtmr2 String len = 0
    0, 0, 0, 0, // rtmr3 String len = 0
    0, 0, 0, 0, // tcb_status String len = 0
    1, // role = Member (ordinal 1)
];

/// GroupOp ordinal 20 - MemberSetAutoFollow { target: [0;32], auto_follow_contexts: false, auto_follow_subgroups: false }
const GOLDEN_GROUP_OP_MEMBER_SET_AUTO_FOLLOW: &[u8] = &[
    20, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // target
    0, // auto_follow_contexts = false
    0, // auto_follow_subgroups = false
];

/// GroupOp ordinal 21 - TransferOwnership { new_owner: [0;32] }
const GOLDEN_GROUP_OP_TRANSFER_OWNERSHIP: &[u8] = &[
    21, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // new_owner
];

/// Borsh encoding of `HybridTimestamp::zero()` — 16 bytes.
///
/// Verified by `hlc_zero_golden_bytes_are_self_consistent` below; kept as a
/// named constant so both the CascadeUpgrade golden vector and the verifier
/// test reference the same source of truth.
const GOLDEN_HLC_ZERO: &[u8] = &[0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0];

#[test]
fn hlc_zero_golden_bytes_are_self_consistent() {
    // Pins the Borsh encoding of HybridTimestamp::zero() so that if the
    // HybridTimestamp layout changes the constant above is updated together
    // with the CascadeUpgrade golden vector that embeds it.
    let actual = borsh::to_vec(&HybridTimestamp::zero()).expect("serialize HLC zero");
    assert_eq!(
        actual.as_slice(),
        GOLDEN_HLC_ZERO,
        "HybridTimestamp::zero() Borsh encoding changed — update GOLDEN_HLC_ZERO \
         and GOLDEN_GROUP_OP_CASCADE_UPGRADE to match the new layout"
    );
    // Verify that the HLC bytes embedded inline in GOLDEN_GROUP_OP_CASCADE_UPGRADE
    // match GOLDEN_HLC_ZERO.  The two must stay in sync: if HybridTimestamp gains a
    // field, updating GOLDEN_HLC_ZERO alone would leave CASCADE_UPGRADE stale.
    // The two trailing coordinate strings sit after the HLC, so the window
    // stops short of the end.
    let hlc_end = GOLDEN_GROUP_OP_CASCADE_UPGRADE.len() - GOLDEN_COORD_TAIL_BYTES;
    assert_eq!(
        &GOLDEN_GROUP_OP_CASCADE_UPGRADE[hlc_end - GOLDEN_HLC_ZERO.len()..hlc_end],
        GOLDEN_HLC_ZERO,
        "HLC bytes embedded in GOLDEN_GROUP_OP_CASCADE_UPGRADE diverged from \
         GOLDEN_HLC_ZERO — update both constants together"
    );
}

/// GroupOp ordinal 24 - AccountDeviceLinked (genesis + empty chain + cert +
/// endorsement + the root-signed scope the link was made under)
const GOLDEN_GROUP_OP_ACCOUNT_DEVICE_LINKED: &[u8] = &[
    0x18, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// GroupOp ordinal 25 - AccountDeviceUnlinked { account: [0;32], device: [0;32], proof: None }
const GOLDEN_GROUP_OP_ACCOUNT_DEVICE_UNLINKED: &[u8] = &[
    25, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // account
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // device
    // `proof: None` — one tag byte. Pinned so the admin path stays the cheap
    // encoding it was before self-service revocation existed, and so a proof can
    // never be silently dropped by a peer that decodes the shorter form.
    0, // proof: None
];

/// GroupOp ordinal 26 - AccountKeysRotated { handoff }
const GOLDEN_GROUP_OP_ACCOUNT_KEYS_ROTATED: &[u8] = &[
    26, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // handoff.account
    0, 0, 0, 0, // handoff.from_epoch
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // handoff.new_root_sign_pk
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // handoff.signature
];

/// GroupOp ordinal 23 - GroupKeyRotated { departed: [0;32] }
const GOLDEN_GROUP_OP_GROUP_KEY_ROTATED: &[u8] = &[
    23, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // departed
];

/// GroupOp ordinal 22 - CascadeUpgrade (zero fields; HybridTimestamp::zero() via GOLDEN_HLC_ZERO; "app"@"1.0.0")
const GOLDEN_GROUP_OP_CASCADE_UPGRADE: &[u8] = &[
    22, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // from_bytecode_id
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // bytecode_id
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // target_application_id
    0, 0, 0, 0, // to_state_version u32 = 0
    0, // migration = None
    // HybridTimestamp::zero() — same bytes as GOLDEN_HLC_ZERO (verified by hlc_zero_golden_bytes_are_self_consistent)
    0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, b'a', b'p',
    b'p', // package = "app"
    5, 0, 0, 0, b'1', b'.', b'0', b'.', b'0', // version = "1.0.0"
];

/// GroupOp ordinal 43 - SharedWritersRotated (zero ids, empty sets, nonce 0)
const GOLDEN_GROUP_OP_SHARED_WRITERS_ROTATED: &[u8] = &[
    43, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // context_id
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // cell
    0, 0, 0, 0, // prior: empty
    0, 0, 0, 0, 0, 0, 0, 0, // nonce
    0, 0, 0, 0, // new: empty
];

#[test]
fn group_op_discriminants_are_golden() {
    // Ties the ordinals frozen below to the schema version they describe, so a
    // rebase that drops the version bump while keeping the enum deletions fails
    // here instead of shipping a silent variant confusion on the wire.
    assert_eq!(
        SIGNED_GROUP_OP_SCHEMA_VERSION, 15,
        "the ordinals frozen below are the v15 layout; bump them together"
    );

    // Decode each frozen byte vector and verify the correct variant is returned.
    // A mid-enum insertion shifts ordinals so the wrong variant is decoded (or
    // decoding fails). Failures are accumulated so ALL mismatches are reported
    // in one run rather than stopping at the first panic.
    let mut failures: Vec<String> = Vec::new();
    macro_rules! check_group_op {
        ($golden:expr, $pat:pat $(if $guard:expr)?, $discriminant:expr) => {{
            match borsh::from_slice::<GroupOp>($golden) {
                Err(e) => failures.push(format!(
                    "GroupOp ordinal {}: decode failed: {e}",
                    $discriminant
                )),
                Ok(decoded) if !matches!(decoded, $pat $(if $guard)?) => failures.push(format!(
                    "GroupOp ordinal {}: decoded as {:?} — a variant was inserted \
                     before this one, or a frozen field byte was edited",
                    $discriminant, decoded
                )),
                Ok(decoded) => {
                    let reencoded = borsh::to_vec(&decoded).expect("re-encode");
                    if reencoded != $golden {
                        failures.push(format!(
                            "GroupOp ordinal {}: golden is {:?} but re-encoding the \
                             decoded value produced {reencoded:?}",
                            $discriminant, $golden
                        ));
                    }
                }
            }
        }};
    }

    // Every vector is built with all fields zero/empty (the coordinate pair and
    // the non-zero capability bits excepted), so each guard pins those values:
    // without one, decode-then-re-encode reproduces an edited byte unnoticed.
    let zero_account = AccountId::from([0u8; 32]);
    let zero_context = ContextId::from([0u8; 32]);

    check_group_op!(GOLDEN_GROUP_OP_NOOP, GroupOp::Noop, 0);
    check_group_op!(
        GOLDEN_GROUP_OP_MEMBER_ADDED,
        GroupOp::MemberAdded { member, ref role } if member == zero_account && *role == GroupMemberRole::Member,
        1
    );
    check_group_op!(
        GOLDEN_GROUP_OP_MEMBER_REMOVED,
        GroupOp::MemberRemoved {
            member,
            expected_group_state_hash,
            ref expected_context_state_hashes,
        } if member == zero_account
            && expected_group_state_hash == [0u8; 32]
            && expected_context_state_hashes.is_empty(),
        2
    );
    check_group_op!(
        GOLDEN_GROUP_OP_MEMBER_LEFT,
        GroupOp::MemberLeft {
            member,
            expected_group_state_hash,
            ref expected_context_state_hashes,
        } if member == zero_account
            && expected_group_state_hash == [0u8; 32]
            && expected_context_state_hashes.is_empty(),
        3
    );
    check_group_op!(
        GOLDEN_GROUP_OP_MEMBER_ROLE_SET,
        GroupOp::MemberRoleSet { member, ref role } if member == zero_account && *role == GroupMemberRole::Admin,
        4
    );
    check_group_op!(
        GOLDEN_GROUP_OP_MEMBER_CAPABILITY_SET,
        GroupOp::MemberCapabilitySet {
            member,
            capabilities,
        } if member == zero_account && capabilities == MemberCapabilities::empty(),
        5
    );
    check_group_op!(
        GOLDEN_GROUP_OP_DEFAULT_CAPABILITIES_SET,
        GroupOp::DefaultCapabilitiesSet { capabilities } if capabilities == MemberCapabilities::empty(),
        6
    );
    check_group_op!(
        GOLDEN_GROUP_OP_TARGET_APPLICATION_SET,
        GroupOp::TargetApplicationSet {
            bytecode_id,
            target_application_id,
            ref package,
            ref version,
        } if bytecode_id == [0u8; 32].into()
            && target_application_id == [0u8; 32].into()
            && package == "app"
            && version == "1.0.0",
        7
    );
    check_group_op!(
        GOLDEN_GROUP_OP_CONTEXT_REGISTERED,
        GroupOp::ContextRegistered {
            context_id,
            ref source,
            ref service_name,
            ref package,
            ref version,
            ..
        } if context_id == zero_context
            && source.is_empty()
            && service_name.is_none()
            && package == "app"
            && version == "1.0.0",
        8
    );
    check_group_op!(
        GOLDEN_GROUP_OP_CONTEXT_DETACHED,
        GroupOp::ContextDetached { context_id } if context_id == zero_context,
        9
    );
    check_group_op!(
        GOLDEN_GROUP_OP_SUBGROUP_VISIBILITY_SET,
        GroupOp::SubgroupVisibilitySet { mode } if mode == VisibilityMode::Open,
        10
    );
    check_group_op!(
        GOLDEN_GROUP_OP_GROUP_METADATA_SET,
        GroupOp::GroupMetadataSet { ref name, ref data } if name.is_none() && data.is_empty(),
        11
    );
    check_group_op!(
        GOLDEN_GROUP_OP_MEMBER_METADATA_SET,
        GroupOp::MemberMetadataSet {
            member,
            ref name,
            ref data,
        } if member == zero_account && name.is_none() && data.is_empty(),
        12
    );
    check_group_op!(
        GOLDEN_GROUP_OP_CONTEXT_METADATA_SET,
        GroupOp::ContextMetadataSet {
            context_id,
            ref name,
            ref data,
        } if context_id == zero_context && name.is_none() && data.is_empty(),
        13
    );
    check_group_op!(GOLDEN_GROUP_OP_GROUP_DELETE, GroupOp::GroupDelete, 14);
    check_group_op!(
        GOLDEN_GROUP_OP_GROUP_MIGRATION_SET,
        GroupOp::GroupMigrationSet { ref migration } if migration.is_none(),
        15
    );
    check_group_op!(
        GOLDEN_GROUP_OP_CONTEXT_CAPABILITY_GRANTED,
        GroupOp::ContextCapabilityGranted {
            context_id,
            member,
            capability,
        } if context_id == zero_context && member == zero_account && capability.get() == 1,
        16
    );
    check_group_op!(
        GOLDEN_GROUP_OP_CONTEXT_CAPABILITY_REVOKED,
        GroupOp::ContextCapabilityRevoked {
            context_id,
            member,
            capability,
        } if context_id == zero_context && member == zero_account && capability.get() == 1,
        17
    );
    check_group_op!(
        GOLDEN_GROUP_OP_TEE_ADMISSION_POLICY_SET,
        GroupOp::TeeAdmissionPolicySet {
            ref allowed_mrtd,
            ref allowed_rtmr0,
            ref allowed_rtmr1,
            ref allowed_rtmr2,
            ref allowed_rtmr3,
            ref allowed_tcb_statuses,
            accept_mock,
        } if allowed_mrtd.is_empty()
            && allowed_rtmr0.is_empty()
            && allowed_rtmr1.is_empty()
            && allowed_rtmr2.is_empty()
            && allowed_rtmr3.is_empty()
            && allowed_tcb_statuses.is_empty()
            && !accept_mock,
        18
    );
    check_group_op!(
        GOLDEN_GROUP_OP_MEMBER_JOINED_VIA_TEE,
        GroupOp::MemberJoinedViaTeeAttestation {
            member,
            quote_hash,
            ref mrtd,
            ref rtmr0,
            ref rtmr1,
            ref rtmr2,
            ref rtmr3,
            ref tcb_status,
            ref role,
        } if member == zero_account
            && quote_hash == [0u8; 32]
            && mrtd.is_empty()
            && rtmr0.is_empty()
            && rtmr1.is_empty()
            && rtmr2.is_empty()
            && rtmr3.is_empty()
            && tcb_status.is_empty()
            && *role == GroupMemberRole::Member,
        19
    );
    check_group_op!(
        GOLDEN_GROUP_OP_MEMBER_SET_AUTO_FOLLOW,
        GroupOp::MemberSetAutoFollow {
            target,
            auto_follow_contexts,
            auto_follow_subgroups,
        } if target == zero_account && !auto_follow_contexts && !auto_follow_subgroups,
        20
    );
    check_group_op!(
        GOLDEN_GROUP_OP_TRANSFER_OWNERSHIP,
        GroupOp::TransferOwnership { new_owner } if new_owner == zero_account,
        21
    );
    check_group_op!(
        GOLDEN_GROUP_OP_CASCADE_UPGRADE,
        GroupOp::CascadeUpgrade {
            from_bytecode_id,
            bytecode_id,
            target_application_id,
            to_state_version,
            ref migration,
            cascade_hlc,
            ref package,
            ref version,
        } if from_bytecode_id == [0u8; 32].into()
            && bytecode_id == [0u8; 32].into()
            && target_application_id == [0u8; 32].into()
            && to_state_version == 0
            && migration.is_none()
            && cascade_hlc == HybridTimestamp::zero()
            && package == "app"
            && version == "1.0.0",
        22
    );
    check_group_op!(
        GOLDEN_GROUP_OP_GROUP_KEY_ROTATED,
        GroupOp::GroupKeyRotated { departed } if departed == zero_account,
        23
    );
    check_group_op!(
        GOLDEN_GROUP_OP_ACCOUNT_DEVICE_LINKED,
        GroupOp::AccountDeviceLinked {
            ref chain,
            ref scope,
            ..
        } if chain.is_empty() && scope.chain.is_empty() && scope.statement.scope_epoch == 0,
        24
    );
    check_group_op!(
        GOLDEN_GROUP_OP_ACCOUNT_DEVICE_UNLINKED,
        GroupOp::AccountDeviceUnlinked {
            account,
            device,
            ref proof,
        } if account == zero_account && device == DeviceId::from([0u8; 32]) && proof.is_none(),
        25
    );
    check_group_op!(
        GOLDEN_GROUP_OP_ACCOUNT_KEYS_ROTATED,
        GroupOp::AccountKeysRotated { ref handoff } if handoff.account == zero_account,
        26
    );
    check_group_op!(
        GOLDEN_GROUP_OP_SHARED_WRITERS_ROTATED,
        GroupOp::SharedWritersRotated {
            context_id,
            cell,
            ref prior,
            nonce,
            ref new,
        } if context_id == ContextId::from([0u8; 32])
            && cell == calimero_storage::address::Id::new([0u8; 32])
            && prior.is_empty()
            && nonce == 0
            && new.is_empty(),
        43
    );

    assert!(
        failures.is_empty(),
        "GroupOp discriminant golden failures ({} total):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// ---- RootOp golden bytes ----
//
// RootOp is always wrapped in NamespaceOp::Root for Borsh serialization,
// so bytes[0] = NamespaceOp discriminant (0 = Root) and bytes[1] = RootOp
// discriminant. All field bytes are zero / empty for determinism.

/// NamespaceOp::Root(RootOp::GroupCreated) — RootOp ordinal 0
const GOLDEN_ROOT_OP_GROUP_CREATED: &[u8] = &[
    0, // NamespaceOp::Root discriminant
    0, // RootOp::GroupCreated discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // group_id
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // parent_id
    0, // restricted = false
    // admin: the creator's ACCOUNT, carried so a receiver folds the principal
    // the rows already name instead of deriving a stand-in from the signer's key.
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // admin [0u8;32]
    // salt: what `group_id` is derived from alongside the fields above
    // (`calimero_account::created_subgroup_id`), so no other account can name it.
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // salt [0u8;32]
];

/// NamespaceOp::Root(RootOp::GroupReparented) — RootOp ordinal 1
const GOLDEN_ROOT_OP_GROUP_REPARENTED: &[u8] = &[
    0, // NamespaceOp::Root
    1, // RootOp::GroupReparented discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // child_group_id
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // new_parent_id
];

/// NamespaceOp::Root(RootOp::GroupDeleted) — RootOp ordinal 2
const GOLDEN_ROOT_OP_GROUP_DELETED: &[u8] = &[
    0, // NamespaceOp::Root
    2, // RootOp::GroupDeleted discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // root_group_id
    0, 0, 0, 0, // cascade_group_ids vec len = 0
    0, 0, 0, 0, // cascade_context_ids vec len = 0
];

/// NamespaceOp::Root(RootOp::AdminChanged) — RootOp ordinal 3
const GOLDEN_ROOT_OP_ADMIN_CHANGED: &[u8] = &[
    0, // NamespaceOp::Root
    3, // RootOp::AdminChanged discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // new_admin PublicKey [0u8;32]
];

/// NamespaceOp::Root(RootOp::PolicyUpdated) — RootOp ordinal 4
const GOLDEN_ROOT_OP_POLICY_UPDATED: &[u8] = &[
    0, // NamespaceOp::Root
    4, // RootOp::PolicyUpdated discriminant
    0, 0, 0, 0, // policy_bytes vec len = 0
];

/// NamespaceOp::Root(RootOp::MemberJoined) — RootOp ordinal 5
///
/// Regenerate with `emit_golden_root_op_vectors` rather than by hand: borsh
/// offsets computed by eye are how a golden ends up asserting the wrong bytes
/// with total confidence.
///
/// Encoding: member (32 bytes) + SignedGroupOpenInvitation — a minimal
/// GroupInvitationFromAdmin (inviter_identity[0;32] + group_id[0;32] +
/// expiration_timestamp 0 (u64) + invitation_nonce[0;32] + invited_role 1 (u8)
/// + admitters len 0 (u32)) + inviter_signature "" + admitter_addrs len 0 (u32)
/// + application_id None + bytecode_id None — then the joiner credential.
const GOLDEN_ROOT_OP_MEMBER_JOINED: &[u8] = &[
    0, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    160, 141, 155, 252, 204, 242, 97, 7, 27, 45, 55, 101, 100, 41, 67, 238, 227, 44, 150, 1, 160,
    95, 101, 189, 216, 79, 108, 214, 26, 179, 18, 251, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// NamespaceOp::Root(RootOp::KeyDelivery) — RootOp ordinal 6
///
/// `KeyEnvelope` field order: recipient, sender, nonce, ciphertext, signature.
/// `recipient` is an [`EnvelopeRecipient`], so it contributes its own
/// discriminant followed by that variant's fields — here `Member` (tag 0) with
/// `identity` then `ephemeral_pk`.
const GOLDEN_ROOT_OP_KEY_DELIVERY: &[u8] = &[
    0, // NamespaceOp::Root
    6, // RootOp::KeyDelivery discriminant
    // group_id [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // envelope.recipient = EnvelopeRecipient::Member discriminant:
    0, // envelope.recipient.identity [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // envelope.recipient.ephemeral_pk [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // envelope.sender [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // envelope.nonce [0u8;12]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // envelope.ciphertext vec len = 0:
    0, 0, 0, 0, // envelope.signature [0u8;64]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// The same op with a **device**-addressed envelope — `EnvelopeRecipient::Device`
/// is tag 1, and that tag is permanent. Pinned separately from the `Member` case
/// because a golden vector for one variant cannot catch a renumbering of the
/// other, and the two are indistinguishable on the wire apart from this byte.
const GOLDEN_ROOT_OP_KEY_DELIVERY_TO_DEVICE: &[u8] = &[
    0, // NamespaceOp::Root
    6, // RootOp::KeyDelivery discriminant
    // group_id [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // envelope.recipient = EnvelopeRecipient::Device discriminant:
    1, // envelope.recipient.device [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // envelope.recipient.ephemeral_pk [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // envelope.sender [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // envelope.nonce [0u8;12]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // envelope.ciphertext vec len = 0:
    0, 0, 0, 0, // envelope.signature [0u8;64]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// NamespaceOp::Root(RootOp::MemberJoinedOpen) — RootOp ordinal 7
const GOLDEN_ROOT_OP_MEMBER_JOINED_OPEN: &[u8] = &[
    0x00, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// NamespaceOp::Root(RootOp::MemberJoinedAt) — RootOp ordinal 8
///
/// Same inner payload as MemberJoined plus joined_at u64 = 0 at the end.
const GOLDEN_ROOT_OP_MEMBER_JOINED_AT: &[u8] = &[
    0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 160, 141, 155, 252, 204, 242, 97, 7, 27, 45, 55, 101, 100, 41, 67, 238,
    227, 44, 150, 1, 160, 95, 101, 189, 216, 79, 108, 214, 26, 179, 18, 251, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0,
];

/// NamespaceOp::Root(RootOp::NamespaceCreatedV2) — RootOp ordinal 9
///
/// The last 32 bytes are the salt. Without them this is the saltless genesis
/// that used to sit at this ordinal, which must no longer decode.
const GOLDEN_ROOT_OP_NAMESPACE_CREATED: &[u8] = &[
    0x00, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    // salt
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// NamespaceOp::Root(RootOp::MemberJoinedViaTeeAttestation) — RootOp ordinal 10
///
/// Hand-written and decoded only, never re-encoded — see this file's header for
/// why. The credential tail is the same 253 bytes the other three join vectors
/// carry (`AccountGenesis` 49 + empty `chain` prefix 4 + `DeviceCert` 200), and
/// `version` is `ACCOUNT_GENESIS_VERSION` rather than 0 so the vector could
/// plausibly be a real op.
const GOLDEN_ROOT_OP_MEMBER_JOINED_VIA_TEE: &[u8] = &[
    0x00, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x01, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

#[test]
fn root_op_discriminants_are_golden() {
    // bytes[0] = NamespaceOp::Root discriminant (always 0).
    // bytes[1] = RootOp variant discriminant (pinned here).
    // Failures are accumulated so ALL mismatches are reported in one run.
    let mut failures: Vec<String> = Vec::new();
    macro_rules! check_root_op {
        ($golden:expr, $pat:pat $(if $guard:expr)?, $root_discriminant:expr) => {{
            match borsh::from_slice::<NamespaceOp>($golden) {
                Err(e) => failures.push(format!(
                    "RootOp ordinal {}: decode failed: {e}",
                    $root_discriminant
                )),
                Ok(decoded) if !matches!(decoded, NamespaceOp::Root($pat) $(if $guard)?) => {
                    failures.push(format!(
                        "RootOp ordinal {}: decoded as {:?} — a variant was inserted \
                         before this one, or a frozen field byte was edited",
                        $root_discriminant, decoded
                    ))
                }
                Ok(decoded) => {
                    let reencoded = borsh::to_vec(&decoded).expect("re-encode");
                    if reencoded != $golden {
                        failures.push(format!(
                            "RootOp ordinal {}: golden is {:?} but re-encoding the \
                             decoded value produced {reencoded:?}",
                            $root_discriminant, $golden
                        ));
                    }
                }
            }
        }};
    }

    // As with the GroupOp vectors: every field is zero/empty, and each guard
    // pins that, so an edited byte cannot survive a decode/re-encode round trip.
    let zero_account = AccountId::from([0u8; 32]);
    let zero_group = ContextGroupId::from([0u8; 32]);
    // Two vectors carry a marker byte instead of a zero id.
    let marked = {
        let mut bytes = [0u8; 32];
        bytes[16] = 1;
        bytes
    };

    check_root_op!(
        GOLDEN_ROOT_OP_GROUP_CREATED,
        RootOp::GroupCreated {
            group_id,
            parent_id,
            restricted,
            admin,
            salt,
        } if group_id == zero_group
            && parent_id == zero_group
            && !restricted
            && admin == zero_account
            && salt == [0u8; 32],
        0
    );
    check_root_op!(
        GOLDEN_ROOT_OP_GROUP_REPARENTED,
        RootOp::GroupReparented {
            child_group_id,
            new_parent_id,
        } if child_group_id == zero_group && new_parent_id == zero_group,
        1
    );
    check_root_op!(
        GOLDEN_ROOT_OP_GROUP_DELETED,
        RootOp::GroupDeleted {
            root_group_id,
            ref cascade_group_ids,
            ref cascade_context_ids,
        } if root_group_id == zero_group
            && cascade_group_ids.is_empty()
            && cascade_context_ids.is_empty(),
        2
    );
    check_root_op!(
        GOLDEN_ROOT_OP_ADMIN_CHANGED,
        RootOp::AdminChanged { new_admin } if new_admin == zero_account,
        3
    );
    check_root_op!(
        GOLDEN_ROOT_OP_POLICY_UPDATED,
        RootOp::PolicyUpdated { ref policy_bytes } if policy_bytes.is_empty(),
        4
    );
    check_root_op!(
        GOLDEN_ROOT_OP_MEMBER_JOINED,
        RootOp::MemberJoined { member, .. } if member == zero_account,
        5
    );
    check_root_op!(
        GOLDEN_ROOT_OP_KEY_DELIVERY,
        RootOp::KeyDelivery {
            group_id,
            ref envelope,
        } if group_id == zero_group
            && matches!(envelope.recipient, EnvelopeRecipient::Member { .. }),
        6
    );
    check_root_op!(
        GOLDEN_ROOT_OP_MEMBER_JOINED_OPEN,
        RootOp::MemberJoinedOpen {
            member, group_id, ..
        } if member == zero_account && group_id == ContextGroupId::from(marked),
        7
    );
    check_root_op!(
        GOLDEN_ROOT_OP_MEMBER_JOINED_AT,
        RootOp::MemberJoinedAt {
            member, joined_at, ..
        } if member == zero_account && joined_at == 0,
        8
    );
    check_root_op!(
        GOLDEN_ROOT_OP_NAMESPACE_CREATED,
        RootOp::NamespaceCreatedV2 { founder, salt, .. }
            if founder == AccountId::from(marked) && salt == [0u8; 32],
        9
    );
    check_root_op!(
        GOLDEN_ROOT_OP_MEMBER_JOINED_VIA_TEE,
        RootOp::MemberJoinedViaTeeAttestation {
            group_id,
            member,
            quote_hash,
            ref mrtd,
            ref rtmr0,
            ref rtmr1,
            ref rtmr2,
            ref rtmr3,
            ref tcb_status,
            ref role,
            ..
        } if group_id == zero_group
            && member == PublicKey::from([0u8; 32])
            && quote_hash == [0u8; 32]
            && mrtd.is_empty()
            && rtmr0.is_empty()
            && rtmr1.is_empty()
            && rtmr2.is_empty()
            && rtmr3.is_empty()
            && tcb_status.is_empty()
            && *role == GroupMemberRole::ReadOnlyTee,
        10
    );

    assert!(
        failures.is_empty(),
        "RootOp discriminant golden failures ({} total):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The saltless genesis that used to sit at ordinal 9 is gone: its bytes are
/// the current genesis minus the salt, and they no longer decode. A namespace
/// id that no founder derives therefore has no genesis that can found it.
#[test]
fn a_saltless_namespace_genesis_does_not_decode() {
    let saltless = &GOLDEN_ROOT_OP_NAMESPACE_CREATED[..GOLDEN_ROOT_OP_NAMESPACE_CREATED.len() - 32];
    assert!(
        borsh::from_slice::<NamespaceOp>(saltless).is_err(),
        "a genesis without a salt must not decode as any namespace op"
    );
}

/// Freeze the [`EnvelopeRecipient`] discriminants.
///
/// Decode-only, like every golden test here: re-encoding in the same binary
/// cannot catch a swap, because both sides would agree on the swapped ordinals.
/// The consequence of a swap is specific and severe — a `Member` envelope's
/// 32-byte identity would be read as a `DeviceId` and vice versa — so the tags
/// are pinned rather than left to source order.
#[test]
fn envelope_recipient_discriminants_are_frozen() {
    let member = borsh::from_slice::<NamespaceOp>(GOLDEN_ROOT_OP_KEY_DELIVERY).expect("decode");
    let NamespaceOp::Root(RootOp::KeyDelivery { envelope, .. }) = member else {
        panic!("golden vector is not a KeyDelivery");
    };
    assert!(
        matches!(envelope.recipient, EnvelopeRecipient::Member { .. }),
        "tag 0 must stay EnvelopeRecipient::Member, got {:?}",
        envelope.recipient
    );

    let device =
        borsh::from_slice::<NamespaceOp>(GOLDEN_ROOT_OP_KEY_DELIVERY_TO_DEVICE).expect("decode");
    let NamespaceOp::Root(RootOp::KeyDelivery { envelope, .. }) = device else {
        panic!("golden vector is not a KeyDelivery");
    };
    assert!(
        matches!(envelope.recipient, EnvelopeRecipient::Device { .. }),
        "tag 1 must stay EnvelopeRecipient::Device, got {:?}",
        envelope.recipient
    );
}

/// The addressing tag is inside the signed payload, so the two modes cannot be
/// swapped by rewriting the borsh discriminant and reusing the signature.
#[test]
fn the_signing_payload_separates_the_two_addressing_modes() {
    let zero = PublicKey::from([0u8; 32]);
    let as_member = KeyEnvelope::signing_payload(
        &[0u8; 32],
        &EnvelopeRecipient::Member {
            identity: zero,
            ephemeral_pk: zero,
        },
        &zero,
        &[0u8; 12],
        &[],
    );
    let as_device = KeyEnvelope::signing_payload(
        &[0u8; 32],
        &EnvelopeRecipient::Device {
            device: DeviceId::from([0u8; 32]),
            ephemeral_pk: KemPublicKey::from([0u8; 32]),
        },
        &zero,
        &[0u8; 12],
        &[],
    );

    assert_ne!(
        as_member, as_device,
        "identical key bytes under the two addressing modes must not sign the same payload"
    );
}

fn sample_group_id() -> ContextGroupId {
    let mut g = [0u8; 32];
    g[0] = 7;
    g[31] = 3;
    g.into()
}

#[test]
fn sign_and_verify_round_trip() {
    let mut rng = UnwrapErr(SysRng);
    let sk = PrivateKey::random(&mut rng);
    let member = calimero_account::AccountId::from(*PrivateKey::random(&mut rng).public_key());

    let op = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::MemberAdded {
            member,
            role: GroupMemberRole::Member,
        },
    )
    .expect("sign");

    op.verify_signature().expect("verify");
}

#[test]
fn wrong_key_fails() {
    let mut rng = UnwrapErr(SysRng);
    let sk = PrivateKey::random(&mut rng);
    let other = PrivateKey::random(&mut rng);
    let member = calimero_account::AccountId::from(*PrivateKey::random(&mut rng).public_key());

    let mut op = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::MemberAdded {
            member,
            role: GroupMemberRole::Admin,
        },
    )
    .expect("sign");

    // Swap signer to another key without re-signing
    op.signer = other.public_key();

    assert!(op.verify_signature().is_err());
}

#[test]
fn tampered_op_fails() {
    let mut rng = UnwrapErr(SysRng);
    let sk = PrivateKey::random(&mut rng);
    let member = calimero_account::AccountId::from(*PrivateKey::random(&mut rng).public_key());

    let mut op = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::MemberAdded {
            member,
            role: GroupMemberRole::Member,
        },
    )
    .expect("sign");

    op.nonce = 2;
    assert!(op.verify_signature().is_err());
}

/// A writer-set rotation's signature covers the step it names and nothing else.
#[test]
fn a_rotation_signature_does_not_verify_another_writer_set() {
    use calimero_storage::address::Id;
    use calimero_storage::entities::OpMask;

    let mut rng = UnwrapErr(SysRng);
    let alice = PrivateKey::random(&mut rng);
    let account = |sk: &PrivateKey| calimero_account::AccountId::from(*sk.public_key());
    let bob = account(&PrivateKey::random(&mut rng));
    let mallory = account(&PrivateKey::random(&mut rng));
    let set = |who: &[calimero_account::AccountId]| -> BTreeMap<_, _> {
        who.iter().map(|a| (*a, OpMask::FULL)).collect()
    };
    let rotation = |context_id, cell, prior, new| GroupOp::SharedWritersRotated {
        context_id,
        cell,
        prior,
        nonce: 7,
        new,
    };
    let (context, cell) = (ContextId::from([0xC7; 32]), Id::new([0xC0; 32]));
    let genuine = SignedGroupOp::sign(
        &alice,
        sample_group_id(),
        vec![],
        1,
        rotation(
            context,
            cell,
            set(&[account(&alice)]),
            set(&[account(&alice), bob]),
        ),
    )
    .expect("sign");
    genuine.verify_signature().expect("control");

    for (what, op) in [
        (
            "another writer set",
            rotation(context, cell, set(&[account(&alice)]), set(&[mallory])),
        ),
        (
            "the step from another set",
            rotation(context, cell, set(&[mallory]), set(&[account(&alice), bob])),
        ),
        (
            "another cell",
            rotation(
                context,
                Id::new([0xC1; 32]),
                set(&[account(&alice)]),
                set(&[account(&alice), bob]),
            ),
        ),
        (
            "another context",
            rotation(
                ContextId::from([0xC8; 32]),
                cell,
                set(&[account(&alice)]),
                set(&[account(&alice), bob]),
            ),
        ),
    ] {
        let forged = SignedGroupOp {
            op,
            ..genuine.clone()
        };
        assert!(forged.verify_signature().is_err(), "nor {what}");
    }
}

#[test]
fn replay_distinct_content_hash() {
    let mut rng = UnwrapErr(SysRng);
    let sk = PrivateKey::random(&mut rng);
    let member = calimero_account::AccountId::from(*PrivateKey::random(&mut rng).public_key());

    let op1 = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::MemberAdded {
            member,
            role: GroupMemberRole::Member,
        },
    )
    .expect("sign");

    let op2 = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        2,
        GroupOp::MemberAdded {
            member,
            role: GroupMemberRole::Member,
        },
    )
    .expect("sign");

    let h1 = op1.content_hash().expect("hash");
    let h2 = op2.content_hash().expect("hash");
    assert_ne!(
        h1, h2,
        "different nonces must yield different content hashes"
    );
}

#[test]
fn signable_bytes_deterministic() {
    let mut rng = UnwrapErr(SysRng);
    let sk = PrivateKey::random(&mut rng);
    let pk = sk.public_key();
    let s = SignableGroupOp {
        version: SIGNED_GROUP_OP_SCHEMA_VERSION,
        group_id: [1u8; 32].into(),
        parent_op_hashes: vec![],
        signer: pk,
        nonce: 42,
        op: GroupOp::Noop,
    };
    let a = signable_bytes(&s).expect("bytes");
    let b = signable_bytes(&s).expect("bytes");
    assert_eq!(a, b);
    assert!(a.starts_with(GROUP_GOVERNANCE_SIGN_DOMAIN));
}

// --- Namespace op tests ---

fn sample_namespace_id() -> NamespaceId {
    let mut ns = [0u8; 32];
    ns[0] = 0xAA;
    ns[31] = 0xBB;
    ns.into()
}

#[test]
fn namespace_op_sign_verify_root() {
    let mut rng = UnwrapErr(SysRng);
    let sk = PrivateKey::random(&mut rng);

    let op = SignedNamespaceOp::sign(
        &sk,
        sample_namespace_id(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            admin: calimero_account::AccountId::from([0x5C; 32]),
            group_id: sample_group_id(),
            parent_id: sample_namespace_id().to_bytes().into(),
            restricted: true,
            salt: [0; 32],
        }),
    )
    .expect("sign");

    op.verify_signature().expect("verify");
    assert!(op.group_id().is_none());
}

#[test]
fn namespace_op_sign_verify_group() {
    let mut rng = UnwrapErr(SysRng);
    let sk = PrivateKey::random(&mut rng);

    let encrypted = EncryptedGroupOp {
        nonce: [42u8; 12],
        ciphertext: vec![1, 2, 3, 4],
    };

    let op = SignedNamespaceOp::sign(
        &sk,
        sample_namespace_id(),
        vec![],
        1,
        NamespaceOp::Group {
            group_id: sample_group_id(),
            key_id: [0u8; 32].into(),
            encrypted,
            key_rotation: None,
        },
    )
    .expect("sign");

    op.verify_signature().expect("verify");
    assert_eq!(op.group_id(), Some(sample_group_id()));
}

#[test]
fn namespace_op_tampered_fails() {
    let mut rng = UnwrapErr(SysRng);
    let sk = PrivateKey::random(&mut rng);

    let mut op = SignedNamespaceOp::sign(
        &sk,
        sample_namespace_id(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::AdminChanged {
            new_admin: calimero_account::AccountId::from(*sk.public_key()),
        }),
    )
    .expect("sign");

    op.nonce = 999;
    assert!(op.verify_signature().is_err());
}

#[test]
fn namespace_op_content_hash_distinct() {
    let mut rng = UnwrapErr(SysRng);
    let sk = PrivateKey::random(&mut rng);

    let op1 = SignedNamespaceOp::sign(
        &sk,
        sample_namespace_id(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            admin: calimero_account::AccountId::from([0x5C; 32]),
            group_id: sample_group_id(),
            parent_id: sample_namespace_id().to_bytes().into(),
            restricted: true,
            salt: [0; 32],
        }),
    )
    .expect("sign");

    let op2 = SignedNamespaceOp::sign(
        &sk,
        sample_namespace_id(),
        vec![],
        2,
        NamespaceOp::Root(RootOp::GroupCreated {
            admin: calimero_account::AccountId::from([0x5C; 32]),
            group_id: sample_group_id(),
            parent_id: sample_namespace_id().to_bytes().into(),
            restricted: true,
            salt: [0; 32],
        }),
    )
    .expect("sign");

    assert_ne!(
        op1.content_hash().unwrap(),
        op2.content_hash().unwrap(),
        "different nonces must yield different content hashes"
    );
}

#[test]
fn namespace_signable_bytes_deterministic() {
    let mut rng = UnwrapErr(SysRng);
    let sk = PrivateKey::random(&mut rng);
    let pk = sk.public_key();
    let s = SignableNamespaceOp {
        version: SIGNED_NAMESPACE_OP_SCHEMA_VERSION,
        namespace_id: sample_namespace_id(),
        parent_op_hashes: vec![],
        signer: pk,
        nonce: 42,
        op: NamespaceOp::Root(RootOp::GroupCreated {
            admin: calimero_account::AccountId::from([0x5C; 32]),
            group_id: sample_group_id(),
            parent_id: sample_namespace_id().to_bytes().into(),
            restricted: true,
            salt: [0; 32],
        }),
    };
    let a = namespace_signable_bytes(&s).expect("bytes");
    let b = namespace_signable_bytes(&s).expect("bytes");
    assert_eq!(a, b);
    assert!(a.starts_with(NAMESPACE_GOVERNANCE_SIGN_DOMAIN));
}

// --- Cascade op variants (Option C in cascade design doc) ---

fn sample_application_id(seed: u8) -> ApplicationId {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    bytes[31] = !seed;
    ApplicationId::from(bytes)
}

#[test]
fn cascade_upgrade_sign_verify() {
    let mut rng = UnwrapErr(SysRng);
    let sk = PrivateKey::random(&mut rng);

    let op = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::CascadeUpgrade {
            from_bytecode_id: [9u8; 32].into(),
            bytecode_id: [10u8; 32].into(),
            target_application_id: sample_application_id(0x42),
            to_state_version: 3,
            migration: Some(b"migrate_v1_to_v2".to_vec()),
            cascade_hlc: HybridTimestamp::zero(),
            package: "com.acme.app".to_owned(),
            version: "1.2.3".to_owned(),
        },
    )
    .expect("sign");

    op.verify_signature().expect("verify");
    assert_eq!(
        op.op.op_kind_label(),
        "cascade_upgrade",
        "op_kind_label must distinguish cascade variant for metrics"
    );
}

#[test]
fn cascade_upgrade_distinct_from_single_group_target() {
    // A cascade op and a non-cascade op with the same new bytecode_id/target
    // must produce DIFFERENT content hashes -- otherwise replay/dedup
    // would conflate the two distinct governance intents.
    let mut rng = UnwrapErr(SysRng);
    let sk = PrivateKey::random(&mut rng);
    let new_bytecode_id = [11u8; 32];
    let target = sample_application_id(0x77);

    let single = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::TargetApplicationSet {
            bytecode_id: new_bytecode_id.into(),
            target_application_id: target,
            package: "com.acme.app".to_owned(),
            version: "1.2.3".to_owned(),
        },
    )
    .expect("sign");

    let cascade = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::CascadeUpgrade {
            from_bytecode_id: [9u8; 32].into(),
            bytecode_id: new_bytecode_id.into(),
            target_application_id: target,
            to_state_version: 0,
            migration: None,
            cascade_hlc: HybridTimestamp::zero(),
            package: "com.acme.app".to_owned(),
            version: "1.2.3".to_owned(),
        },
    )
    .expect("sign");

    assert_ne!(
        single.content_hash().expect("hash single"),
        cascade.content_hash().expect("hash cascade"),
        "cascade and single-group target ops must hash distinctly"
    );
}

#[test]
fn cascade_upgrade_from_bytecode_id_changes_hash() {
    // The Borsh-discriminant guarantees distinctness from the
    // single-group variant (covered by
    // `cascade_upgrade_distinct_from_single_group_target`). This test
    // covers the stronger invariant: `from_bytecode_id` is itself part of
    // the signed bytes, so two cascade ops that agree on every field
    // EXCEPT `from_bytecode_id` must still hash differently. Otherwise a
    // refactor that accidentally collapses `from_bytecode_id` (e.g. by
    // defaulting it or excluding it from signable bytes) would silently
    // break dedup of intent-different cascades.
    let mut rng = UnwrapErr(SysRng);
    let sk = PrivateKey::random(&mut rng);
    let new_bytecode_id = [11u8; 32];
    let target = sample_application_id(0x77);

    let a = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::CascadeUpgrade {
            from_bytecode_id: [9u8; 32].into(),
            bytecode_id: new_bytecode_id.into(),
            target_application_id: target,
            to_state_version: 0,
            migration: None,
            cascade_hlc: HybridTimestamp::zero(),
            package: "com.acme.app".to_owned(),
            version: "1.2.3".to_owned(),
        },
    )
    .expect("sign");

    let b = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::CascadeUpgrade {
            from_bytecode_id: [8u8; 32].into(), // only this differs
            bytecode_id: new_bytecode_id.into(),
            target_application_id: target,
            to_state_version: 0,
            migration: None,
            cascade_hlc: HybridTimestamp::zero(),
            package: "com.acme.app".to_owned(),
            version: "1.2.3".to_owned(),
        },
    )
    .expect("sign");

    assert_ne!(
        a.content_hash().expect("hash a"),
        b.content_hash().expect("hash b"),
        "from_bytecode_id must be covered by the signed content hash"
    );
}

// --- CascadeUpgrade wire-format back-compat (schema v11) ---

#[test]
fn cascade_upgrade_back_compat_discriminant_fixed() {
    // GOLDEN byte-vector guard on CascadeUpgrade's Borsh discriminant (ordinal
    // 22 at v11). We decode these EXTERNALLY-FIXED bytes with the CURRENT enum
    // and never re-encode them here: a same-binary serialize -> deserialize
    // round-trip would NOT catch a mid-enum insertion, because both sides would
    // use the shifted layout and still agree. Decoding frozen bytes is what
    // actually catches it: insert or remove a variant before CascadeUpgrade and
    // byte `22` here decodes as a DIFFERENT variant (or fails).
    //
    // Golden encoding of:
    //   GroupOp::CascadeUpgrade {
    //       from_bytecode_id: [3u8; 32].into(),
    //       bytecode_id: [4u8; 32].into(),
    //       target_application_id: sample_application_id(5),
    //       to_state_version: 2,
    //       migration: Some(b"migrate".to_vec()),
    //       cascade_hlc: HybridTimestamp::zero(),
    //       package: "com.acme.app".to_owned(),
    //       version: "1.2.3".to_owned(),
    //   }
    const GOLDEN_CASCADE_UPGRADE: &[u8] = &[
        22, // <- CascadeUpgrade's fixed Borsh discriminant (ordinal 22 at v11)
        3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
        3, 3, // from_bytecode_id
        4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
        4, 4, // bytecode_id
        5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 250, // target_application_id = sample_application_id(5)
        2, 0, 0, 0, // to_state_version = 2
        1, 7, 0, 0, 0, 109, 105, 103, 114, 97, 116, 101, // migration = Some("migrate")
        0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0,
        0, // cascade_hlc = HybridTimestamp::zero()
        12, 0, 0, 0, 99, 111, 109, 46, 97, 99, 109, 101, 46, 97, 112,
        112, // package = "com.acme.app"
        5, 0, 0, 0, 49, 46, 50, 46, 51, // version = "1.2.3"
    ];

    // Up-front: the leading discriminant byte must equal CascadeUpgrade's
    // known ordinal, so a mid-enum insertion (which shifts it) is caught.
    assert_eq!(
        GOLDEN_CASCADE_UPGRADE[0], 22,
        "CascadeUpgrade's Borsh discriminant must stay at ordinal 22; a \
         changed leading byte means a prior variant moved"
    );

    let decoded: GroupOp =
        borsh::from_slice(GOLDEN_CASCADE_UPGRADE).expect("decode frozen CascadeUpgrade bytes");
    match decoded {
        GroupOp::CascadeUpgrade {
            from_bytecode_id,
            bytecode_id,
            target_application_id,
            to_state_version,
            migration,
            cascade_hlc,
            package,
            version,
        } => {
            assert_eq!(from_bytecode_id.to_bytes(), [3u8; 32]);
            assert_eq!(bytecode_id.to_bytes(), [4u8; 32]);
            assert_eq!(target_application_id, sample_application_id(5));
            assert_eq!(to_state_version, 2);
            assert_eq!(migration, Some(b"migrate".to_vec()));
            assert_eq!(cascade_hlc, HybridTimestamp::zero());
            assert_eq!(
                (package.as_str(), version.as_str()),
                ("com.acme.app", "1.2.3")
            );
        }
        other => panic!(
            "frozen CascadeUpgrade bytes (discriminant 22) decoded as {other:?}; a \
             variant was inserted mid-enum, shifting prior variant tags"
        ),
    }
}

// C5.S3b schema boundary: an op signed under the OLD schema must be REJECTED on
// the new build, never silently misparsed. The `version` field is the first borsh
// field, so it survives the layout change and the version check fires before any
// signable-bytes reconstruction. These tests pin that boundary so a future refactor
// can't re-open the window.
#[test]
fn pre_flag_day_group_op_version_is_rejected() {
    let signer = PrivateKey::random(&mut UnwrapErr(SysRng)).public_key();
    // A struct-shaped op carrying the immediately-previous schema version.
    // `verify_signature` must reject on the version check alone - before
    // touching the (here bogus) signature.
    let stale = SignedGroupOp {
        version: SIGNED_GROUP_OP_SCHEMA_VERSION - 1,
        group_id: sample_group_id(),
        parent_op_hashes: vec![],
        signer,
        nonce: 1,
        op: GroupOp::Noop,
        signature: [0u8; 64],
    };
    assert!(
        matches!(
            stale.verify_signature(),
            Err(GovernanceError::SchemaVersion { .. })
        ),
        "a prior-version group op must be rejected with SchemaVersion, got {:?}",
        stale.verify_signature()
    );
}

#[test]
fn pre_flag_day_namespace_op_version_is_rejected() {
    let signer = PrivateKey::random(&mut UnwrapErr(SysRng)).public_key();
    let stale = SignedNamespaceOp {
        version: SIGNED_NAMESPACE_OP_SCHEMA_VERSION - 1,
        namespace_id: sample_group_id().to_bytes().into(),
        parent_op_hashes: vec![],
        signer,
        nonce: 1,
        op: NamespaceOp::Root(RootOp::PolicyUpdated {
            policy_bytes: vec![],
        }),
        signature: [0u8; 64],
        admitter_endorsement: None,
    };
    assert!(
        matches!(
            stale.verify_signature(),
            Err(GovernanceError::SchemaVersion { .. })
        ),
        "a prior-version namespace op must be rejected with SchemaVersion, got {:?}",
        stale.verify_signature()
    );
}

#[test]
fn v7_borsh_layout_group_op_is_rejected_not_misparsed() {
    // A v7-shaped op still carries the removed `state_hash` field in its borsh bytes,
    // between `parent_op_hashes` and `signer`. borsh is a flat format with no field
    // names, so decoding these bytes as the v8 struct will (most likely) SUCCEED —
    // consuming the 32 `state_hash` bytes as the start of `signer` and shifting the
    // rest into a garbage `signer`/`nonce`/`op`. That successful-but-garbage decode
    // IS a misparse; the only thing that saves us is that `version` is the FIRST
    // byte, read intact as the old value, so `verify_signature` rejects on the
    // version check. This test pins exactly that: the old version survives in byte 0,
    // and the decoded op is rejected with `SchemaVersion` rather than verifying.
    #[derive(::borsh::BorshSerialize)]
    struct V7SignedGroupOp {
        version: u8,
        group_id: [u8; 32],
        parent_op_hashes: Vec<[u8; 32]>,
        state_hash: [u8; 32],
        signer: PublicKey,
        nonce: u64,
        op: GroupOp,
        signature: [u8; 64],
    }
    let signer = PrivateKey::random(&mut UnwrapErr(SysRng)).public_key();
    let v7 = V7SignedGroupOp {
        version: SIGNED_GROUP_OP_SCHEMA_VERSION - 1,
        group_id: sample_group_id().to_bytes(),
        parent_op_hashes: vec![],
        state_hash: [0xAB; 32],
        signer,
        nonce: 1,
        op: GroupOp::Noop,
        signature: [0u8; 64],
    };
    let bytes = ::borsh::to_vec(&v7).expect("encode v7");
    // Deterministic: byte 0 is the version, untouched by the layout shift.
    assert_eq!(
        bytes[0],
        SIGNED_GROUP_OP_SCHEMA_VERSION - 1,
        "v7 bytes must begin with the old schema version"
    );
    // If borsh decodes the misaligned bytes (the likely case — it doesn't validate
    // field counts), the decode misparsed the shifted bytes but the version survived;
    // assert that dependency explicitly so a future refactor checking the signature
    // before the version can't let a real misparse slip through. If borsh instead
    // rejects the old layout outright, that is also a clean rejection (nothing to do).
    if let Ok(op) = ::borsh::from_slice::<SignedGroupOp>(&bytes) {
        assert_eq!(
            op.version,
            SIGNED_GROUP_OP_SCHEMA_VERSION - 1,
            "decoded version must be the old schema version (byte 0)"
        );
        assert!(
            matches!(
                op.verify_signature(),
                Err(GovernanceError::SchemaVersion { .. })
            ),
            "a v7-decoded op must be rejected on the version check, got {:?}",
            op.verify_signature()
        );
    }
}

// ---------------------------------------------------------------------------
// Namespace governance op storage encoding
//
// The op-log persists each op as a `StoredNamespaceEntry::Signed(op)`, borsh-
// encoded into the `skeleton_bytes` of its store value; the serving, retry, and
// projection-backfill paths read it back with the equivalent of
// `decode_signed_namespace_op`. A silent encode/decode asymmetry in any op
// variant (e.g. an ill-considered field type or a hand-rolled codec) would make
// the affected op un-servable: a peer that needs it as a causal ancestor could
// never fold the cut, stranding every state delta authored against it. These
// tests pin the round-trip so such a regression fails here, in isolation,
// rather than as an opaque convergence stall.
// ---------------------------------------------------------------------------
mod governance_op_storage_roundtrip {
    use super::*;
    use calimero_context_config::types::{
        ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
    };

    /// A joiner's account credential for wire tests. Values are structurally valid
    /// but not cryptographically meaningful — these tests exercise the codec, and
    /// signature verification has its own coverage.
    fn sample_join_account() -> Box<JoinAccountCredential> {
        let root = PrivateKey::random(&mut UnwrapErr(SysRng)).public_key();

        let genesis = calimero_account::AccountGenesis::new(root);

        Box::new(JoinAccountCredential {
            statement: calimero_account::DeviceCert {
                account: genesis.account_id(),

                device: calimero_account::DeviceId::from([0x3E; 32]),

                sign_pk: PrivateKey::random(&mut UnwrapErr(SysRng)).public_key(),

                kem_pk: calimero_account::KemPublicKey::from([0x2B; 32]),

                key_epoch: 0,

                device_epoch: 0,

                signature: [0x11; 64],
            },

            genesis,

            chain: vec![],
        })
    }

    fn sample_invitation() -> SignedGroupOpenInvitation {
        SignedGroupOpenInvitation {
            inviter_account: None,
            invitation: GroupInvitationFromAdmin {
                inviter_identity: SignerId::from([0xA1; 32]),
                group_id: ContextGroupId::from([0x22; 32]),
                expiration_timestamp: 1_900_000_000,
                invitation_nonce: [0x33; 32],
                invited_role: 1,
                admitters: Vec::new(),
            },
            inviter_signature: "deadbeef".to_string(),
            application_id: Some([0x44; 32]),
            bytecode_id: Some([0x55; 32]),
            admitter_addrs: Vec::new(),
        }
    }

    fn signed(op: NamespaceOp) -> SignedNamespaceOp {
        let sk = PrivateKey::random(&mut UnwrapErr(SysRng));
        SignedNamespaceOp::sign(&sk, [0x77; 32].into(), vec![[0x01; 32], [0x02; 32]], 7, op)
            .expect("sign namespace op")
    }

    /// Mirror of `decode_signed_namespace_op` in
    /// `calimero-governance-store::namespace::op_log` (the read-back used by the
    /// serving / retry / opaque walks): try the tagged wrapper first, then the
    /// legacy raw fallback.
    fn decode_signed_namespace_op(bytes: &[u8]) -> Option<SignedNamespaceOp> {
        if let Ok(StoredNamespaceEntry::Signed(op)) =
            ::borsh::from_slice::<StoredNamespaceEntry>(bytes)
        {
            return Some(op);
        }
        ::borsh::from_slice::<SignedNamespaceOp>(bytes).ok()
    }

    fn assert_roundtrips(op: &SignedNamespaceOp) {
        // `SignedNamespaceOp` has no `PartialEq`; compare canonical bytes.
        let skeleton_bytes =
            ::borsh::to_vec(&StoredNamespaceEntry::Signed(op.clone())).expect("encode entry");
        let decoded = decode_signed_namespace_op(&skeleton_bytes)
            .expect("entry must decode back from StoredNamespaceEntry::Signed");
        assert_eq!(
            ::borsh::to_vec(&decoded).unwrap(),
            ::borsh::to_vec(op).unwrap(),
            "round-trip through StoredNamespaceEntry::Signed must be lossless"
        );
    }

    #[test]
    fn an_endorsed_envelope_roundtrips_and_keeps_the_op_it_was() {
        // The property the endorsement design rests on: `content_hash` is taken
        // over the SIGNABLE form, which does not carry the endorsement, so an
        // admitter can attach consent to an op it did not author and every
        // replica still agrees which op it is — and the joiner's signature
        // still verifies over what the joiner actually signed.
        //
        // If the field ever reaches `to_signable`, attaching consent becomes a
        // fork: the relaying node's copy and the author's copy would have
        // different ids for the same op. That failure would surface far from
        // here, as peers disagreeing about history.
        let unendorsed = signed(NamespaceOp::Root(RootOp::MemberJoinedAt {
            member: calimero_account::AccountId::from(
                *PrivateKey::random(&mut UnwrapErr(SysRng)).public_key(),
            ),
            signed_invitation: sample_invitation(),
            joined_at: 1_800_000_000,
            account: sample_join_account(),
        }));

        let id_before = unendorsed.content_hash().expect("hash the unendorsed op");
        unendorsed
            .verify_signature()
            .expect("the author's signature verifies");

        let mut endorsed = unendorsed.clone();
        endorsed.admitter_endorsement = Some(Box::new(deterministic_endorsement()));

        assert_eq!(
            endorsed.content_hash().expect("hash the endorsed op"),
            id_before,
            "attaching an endorsement must not change the op's id"
        );
        assert_eq!(
            endorsed.signature, unendorsed.signature,
            "attaching an endorsement must not disturb the author's signature"
        );
        endorsed
            .verify_signature()
            .expect("the author's signature still verifies once consent is attached");

        // Non-vacuity: the endorsement really is in the encoded envelope. Without
        // this, the id assertion above would also hold if the field were being
        // silently dropped, and the test would pass while proving nothing.
        let bytes_unendorsed = ::borsh::to_vec(&unendorsed).expect("encode unendorsed");
        let bytes_endorsed = ::borsh::to_vec(&endorsed).expect("encode endorsed");
        assert_ne!(
            bytes_unendorsed, bytes_endorsed,
            "the endorsement must actually be encoded, or the id assertion above \
             is trivially true"
        );
        assert_eq!(
            bytes_endorsed.len(),
            // borsh writes `Some` as a 1-byte tag, then the 32-byte signer and
            // the 64-byte signature.
            bytes_unendorsed.len() + 96,
            "an endorsed envelope is exactly the signer and signature longer"
        );

        // And it survives the store round trip, which is what carries it to
        // every peer that has to re-check it.
        assert_roundtrips(&endorsed);
    }

    #[test]
    fn member_joined_at_roundtrips_through_stored_signed_entry() {
        // The invitation join carries a nested `SignedGroupOpenInvitation`, the
        // largest and most field-rich op payload — the one most exposed to a
        // codec asymmetry.
        assert_roundtrips(&signed(NamespaceOp::Root(RootOp::MemberJoinedAt {
            member: calimero_account::AccountId::from(
                *PrivateKey::random(&mut UnwrapErr(SysRng)).public_key(),
            ),
            signed_invitation: sample_invitation(),
            joined_at: 1_800_000_000,
            account: sample_join_account(),
        })));
    }

    #[test]
    fn every_root_op_roundtrips_through_stored_signed_entry() {
        let ops = [
            RootOp::GroupCreated {
                admin: calimero_account::AccountId::from([0x5C; 32]),
                group_id: [1; 32].into(),
                parent_id: [2; 32].into(),
                restricted: true,
                salt: [0; 32],
            },
            RootOp::GroupReparented {
                child_group_id: [1; 32].into(),
                new_parent_id: [2; 32].into(),
            },
            RootOp::GroupDeleted {
                root_group_id: [1; 32].into(),
                cascade_group_ids: vec![[3; 32].into()],
                cascade_context_ids: vec![[4; 32].into()],
            },
            RootOp::AdminChanged {
                new_admin: calimero_account::AccountId::from(
                    *PrivateKey::random(&mut UnwrapErr(SysRng)).public_key(),
                ),
            },
            RootOp::PolicyUpdated {
                policy_bytes: vec![9, 8, 7],
            },
            RootOp::MemberJoined {
                member: calimero_account::AccountId::from(
                    *PrivateKey::random(&mut UnwrapErr(SysRng)).public_key(),
                ),
                signed_invitation: sample_invitation(),
                account: sample_join_account(),
            },
            RootOp::MemberJoinedOpen {
                member: calimero_account::AccountId::from(
                    *PrivateKey::random(&mut UnwrapErr(SysRng)).public_key(),
                ),
                group_id: [7; 32].into(),
                account: sample_join_account(),
            },
            RootOp::MemberJoinedAt {
                member: calimero_account::AccountId::from(
                    *PrivateKey::random(&mut UnwrapErr(SysRng)).public_key(),
                ),
                signed_invitation: sample_invitation(),
                joined_at: 42,
                account: sample_join_account(),
            },
        ];
        for root in ops {
            assert_roundtrips(&signed(NamespaceOp::Root(root)));
        }
    }

    /// The op-log shares a column family with other key types, so its walk can
    /// read a foreign value under a colliding key. The store value wraps the
    /// entry in a length-prefixed `Vec<u8>` (`NamespaceGovOpValue.skeleton_bytes`),
    /// so a raw 32-byte id read as that wrapper has its first 4 bytes misread as
    /// an enormous length — borsh rejects it with "Unexpected length of input"
    /// rather than silently producing a bogus op. Pin that loud-failure
    /// behaviour so the walk's skip-and-continue stays correct.
    #[test]
    fn foreign_column_value_is_rejected_not_misdecoded() {
        // Structural stand-in for `calimero_store::key::NamespaceGovOpValue`
        // (a single length-prefixed `Vec<u8>` field); that type lives in
        // `calimero-store`, which is not a dependency here.
        #[derive(Debug, ::borsh::BorshDeserialize)]
        struct GovOpValueShape {
            #[allow(dead_code)]
            skeleton_bytes: Vec<u8>,
        }

        // A raw 32-byte id (e.g. a group key_id) whose leading bytes form a
        // length far beyond the 28 trailing bytes.
        let foreign = [0xFEu8; 32];
        let err = ::borsh::from_slice::<GovOpValueShape>(&foreign)
            .expect_err("a foreign shared-column value must not decode as the op-log wrapper");
        assert!(
            err.to_string().contains("Unexpected length of input"),
            "expected a borsh length error, got: {err}"
        );
    }

    #[test]
    fn validate_bounds_encrypted_ciphertext() {
        let oversized = EncryptedGroupOp {
            nonce: [0u8; 12],
            ciphertext: vec![0u8; bounds::MAX_CIPHERTEXT_BYTES + 1],
        };
        assert!(
            oversized.validate().is_err(),
            "ciphertext over the bound must be rejected"
        );

        let ok = EncryptedGroupOp {
            nonce: [0u8; 12],
            ciphertext: vec![0u8; 64],
        };
        assert!(ok.validate().is_ok(), "a normal ciphertext must pass");
    }

    /// Build a `MemberJoinedAt` around `invitation`, the shape a joiner sends.
    fn join_op_with(invitation: SignedGroupOpenInvitation) -> SignedNamespaceOp {
        let account = sample_join_account();
        SignedNamespaceOp {
            version: SIGNED_NAMESPACE_OP_SCHEMA_VERSION,
            namespace_id: NamespaceId::from([0u8; 32]),
            parent_op_hashes: vec![[0u8; 32]],
            signer: PublicKey::from([0u8; 32]),
            nonce: 1,
            op: NamespaceOp::Root(RootOp::MemberJoinedAt {
                member: account.statement.account,
                signed_invitation: invitation,
                joined_at: 1_900_000_000,
                account,
            }),
            signature: [0u8; 64],
            admitter_endorsement: None,
        }
    }

    #[test]
    fn validate_bounds_admitter_addrs() {
        // `admitter_addrs` is outside the inviter's signature, so anyone
        // relaying an invitation may rewrite it — and a joiner acts on it by
        // dialing. Unbounded, that is a way to point somebody else's node at a
        // list of the sender's choosing.
        let mut invitation = sample_invitation();
        invitation.admitter_addrs =
            vec!["/ip4/10.0.0.1/tcp/1".to_owned(); bounds::MAX_ADMITTER_ADDRS + 1];
        assert!(
            join_op_with(invitation).validate().is_err(),
            "an invitation offering more addresses than the bound must be rejected"
        );

        let mut invitation = sample_invitation();
        invitation.admitter_addrs = vec!["x".repeat(bounds::MAX_ADMITTER_ADDR_LEN + 1)];
        assert!(
            join_op_with(invitation).validate().is_err(),
            "a single oversized address must be rejected too — a short list of \
             enormous strings is the same attack"
        );

        let mut invitation = sample_invitation();
        invitation.admitter_addrs = vec![
            "/ip4/203.0.113.7/tcp/2528/p2p/12D3KooWExample".to_owned(),
            "/ip4/198.51.100.4/tcp/4001/p2p/12D3KooWRelay/p2p-circuit/p2p/12D3KooWTarget"
                .to_owned(),
        ];
        assert!(
            join_op_with(invitation).validate().is_ok(),
            "a realistic pair of addresses, including a relay circuit, must pass"
        );
    }

    #[test]
    fn validate_bounds_admitters() {
        let mut invitation = sample_invitation();
        invitation.invitation.admitters =
            vec![calimero_account::AccountId::from([1u8; 32]); bounds::MAX_ADMITTERS + 1];
        assert!(
            join_op_with(invitation).validate().is_err(),
            "an invitation naming more admitters than the bound must be rejected"
        );
    }

    #[test]
    fn validate_bounds_parent_op_hashes() {
        let mut op = SignedNamespaceOp {
            version: SIGNED_NAMESPACE_OP_SCHEMA_VERSION,
            namespace_id: NamespaceId::from([0u8; 32]),
            parent_op_hashes: vec![[0u8; 32]; bounds::MAX_PARENT_OP_HASHES + 1],
            signer: PublicKey::from([0u8; 32]),
            nonce: 1,
            op: NamespaceOp::Root(RootOp::AdminChanged {
                new_admin: calimero_account::AccountId::from([1u8; 32]),
            }),
            signature: [0u8; 64],
            admitter_endorsement: None,
        };
        assert!(
            op.validate().is_err(),
            "too many parent_op_hashes must be rejected"
        );

        op.parent_op_hashes = vec![[0u8; 32]; 2];
        assert!(op.validate().is_ok(), "a small parent set must pass");
    }
}

// --- Registry coordinates on the upgrade ops (schema v11) ---

#[test]
fn target_application_set_round_trips_registry_coordinates() {
    // Coordinates are what let a receiver resolve the version from its OWN
    // registry, so they must survive the borsh round trip the DAG stores.
    let op = GroupOp::TargetApplicationSet {
        bytecode_id: BytecodeId::from([0x33; 32]),
        target_application_id: sample_application_id(0x44),
        package: "com.calimero.migration-suite".to_owned(),
        version: "2.0.0".to_owned(),
    };
    let bytes = borsh::to_vec(&op).expect("serialize");
    let back: GroupOp = borsh::from_slice(&bytes).expect("deserialize");
    match back {
        GroupOp::TargetApplicationSet {
            package, version, ..
        } => {
            assert_eq!(package, "com.calimero.migration-suite");
            assert_eq!(version, "2.0.0");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn cascade_upgrade_round_trips_registry_coordinates() {
    let op = GroupOp::CascadeUpgrade {
        from_bytecode_id: BytecodeId::from([0x11; 32]),
        bytecode_id: BytecodeId::from([0x22; 32]),
        target_application_id: sample_application_id(0x44),
        to_state_version: 2,
        migration: Some(b"migrate_v1_to_v2".to_vec()),
        cascade_hlc: HybridTimestamp::zero(),
        package: "com.calimero.migration-suite".to_owned(),
        version: "2.0.0".to_owned(),
    };
    let bytes = borsh::to_vec(&op).expect("serialize");
    let back: GroupOp = borsh::from_slice(&bytes).expect("deserialize");
    match back {
        GroupOp::CascadeUpgrade {
            package, version, ..
        } => {
            assert_eq!(package, "com.calimero.migration-suite");
            assert_eq!(version, "2.0.0");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

/// One `TargetApplicationSet` naming `package`@`version`, for the assertions below.
fn sample_target_application_set(package: &str, version: &str) -> GroupOp {
    GroupOp::TargetApplicationSet {
        bytecode_id: BytecodeId::from([0x33; 32]),
        target_application_id: sample_application_id(0x44),
        package: package.to_owned(),
        version: version.to_owned(),
    }
}

/// One `ContextRegistered` naming `coords`, for the wire assertions below.
fn sample_context_registered((package, version): (&str, &str)) -> GroupOp {
    GroupOp::ContextRegistered {
        context_id: ContextId::from([0x55; 32]),
        application_id: sample_application_id(0x44),
        blob_id: calimero_primitives::blobs::BlobId::from([0x66; 32]),
        source: "https://reg.example/app-1.0.0.mpk".to_owned(),
        service_name: None,
        package: package.to_owned(),
        version: version.to_owned(),
    }
}

#[test]
fn context_registered_round_trips_registry_coordinates() {
    // A joiner resolves the application from its OWN registry, so the pair the
    // registering node announced has to survive the DAG's borsh round trip.
    let bytes =
        borsh::to_vec(&sample_context_registered(("com.acme.app", "1.2.3"))).expect("serialize");
    let back: GroupOp = borsh::from_slice(&bytes).expect("deserialize");
    match back {
        GroupOp::ContextRegistered {
            package, version, ..
        } => {
            assert_eq!(package, "com.acme.app");
            assert_eq!(version, "1.2.3");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn v10_target_application_set_bytes_are_rejected_not_misparsed() {
    // Half one: a v10 `TargetApplicationSet` is two coordinate strings short of
    // the v11 layout, so the reader runs off the end rather than misreading a
    // signature.
    #[derive(::borsh::BorshSerialize)]
    struct V10TargetApplicationSet {
        discriminant: u8, // `GroupOp` ordinal 7; v10 carried no coordinates
        bytecode_id: [u8; 32],
        target_application_id: [u8; 32],
    }
    #[derive(::borsh::BorshSerialize)]
    struct V10SignedGroupOp {
        version: u8,
        group_id: [u8; 32],
        parent_op_hashes: Vec<[u8; 32]>,
        signer: PublicKey,
        nonce: u64,
        op: V10TargetApplicationSet,
        signature: [u8; 64],
    }

    let signer = PrivateKey::random(&mut UnwrapErr(SysRng)).public_key();
    let bytes = ::borsh::to_vec(&V10SignedGroupOp {
        version: 10,
        group_id: sample_group_id().to_bytes(),
        parent_op_hashes: vec![],
        signer,
        nonce: 1,
        op: V10TargetApplicationSet {
            discriminant: 7,
            bytecode_id: [0x33; 32],
            target_application_id: [0x44; 32],
        },
        signature: [0u8; 64],
    })
    .expect("encode v10");
    assert!(
        ::borsh::from_slice::<SignedGroupOp>(&bytes).is_err(),
        "v10 TargetApplicationSet bytes must not decode as a v11 op"
    );

    // Half two: a v10 op whose variant IS layout-compatible decodes fine, so the
    // strictly-equal version check is the only thing standing between it and a
    // signature verification. Pinned to the literal 10 the wire actually carries.
    let compatible = SignedGroupOp {
        version: 10,
        group_id: sample_group_id(),
        parent_op_hashes: vec![],
        signer,
        nonce: 1,
        op: GroupOp::Noop,
        signature: [0u8; 64],
    };
    let decoded: SignedGroupOp =
        ::borsh::from_slice(&::borsh::to_vec(&compatible).expect("encode v10 noop"))
            .expect("a layout-compatible v10 op must decode");
    assert_eq!(
        decoded.version, 10,
        "the version must survive the round trip"
    );
    assert!(
        matches!(
            decoded.verify_signature(),
            Err(GovernanceError::SchemaVersion { .. })
        ),
        "a v10 op must be rejected on the version check, got {:?}",
        decoded.verify_signature()
    );
}

#[test]
fn oversized_registry_coordinates_are_rejected_before_apply() {
    // `validate()` is the anti-amplification gate the apply path runs before any
    // crypto; the coordinates must be capped there, not at URL-build time.
    let long = "a".repeat(bounds::MAX_COORD_BYTES + 1);
    assert!(sample_target_application_set(&long, "1.0.0")
        .validate()
        .is_err());
    assert!(sample_target_application_set("pkg", &long)
        .validate()
        .is_err());
    assert!(sample_target_application_set("pkg", "1.0.0")
        .validate()
        .is_ok());

    let cascade = |package: &str, version: &str, migration| GroupOp::CascadeUpgrade {
        from_bytecode_id: BytecodeId::from([0x11; 32]),
        bytecode_id: BytecodeId::from([0x22; 32]),
        target_application_id: sample_application_id(0x44),
        to_state_version: 2,
        migration,
        cascade_hlc: HybridTimestamp::zero(),
        package: package.to_owned(),
        version: version.to_owned(),
    };
    assert!(cascade(&long, "1.0.0", None).validate().is_err());
    assert!(cascade("pkg", &long, None).validate().is_err());
    // The migration bound this arm was restructured around must still fire.
    assert!(
        cascade("pkg", "1.0.0", Some(vec![0u8; bounds::MAX_BLOB_BYTES + 1]))
            .validate()
            .is_err()
    );
    assert!(cascade("pkg", "1.0.0", None).validate().is_ok());

    assert!(sample_context_registered((&long, "1.0.0"))
        .validate()
        .is_err());
    assert!(sample_context_registered(("pkg", &long))
        .validate()
        .is_err());
    assert!(sample_context_registered(("pkg", "1.0.0"))
        .validate()
        .is_ok());
}

#[test]
fn an_empty_coordinate_fails_validation() {
    // An empty half addresses no registry, so it is a decode-gate rejection
    // rather than a fetch that quietly resolves nothing.
    assert!(
        sample_target_application_set("", "1.0.0")
            .validate()
            .is_err(),
        "empty package"
    );
    assert!(
        sample_target_application_set("pkg", "").validate().is_err(),
        "empty version"
    );
    assert!(sample_target_application_set("pkg", "1.0.0")
        .validate()
        .is_ok());
}

/// The applications ride in from the wire before any authorization runs, and each
/// one is 32 bytes a hostile op can repeat, so the list is capped at decode.
#[test]
fn an_oversized_device_scope_application_list_is_refused() {
    let sk = PrivateKey::from([0x21; 32]);
    let root = PrivateKey::from([0x22; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0x23; 16]);
    let cert = DeviceCert::sign(
        &root,
        account,
        device,
        &sk.public_key(),
        &KemPublicKey::from([0x24; 32]),
        0,
        0,
    )
    .expect("sign the cert");
    let applications =
        vec![ApplicationId::from([0x25; 32]); bounds::MAX_DEVICE_SCOPE_APPLICATIONS + 1];
    let scope = DeviceScope::sign(&root, account, device, applications, 0, 0).expect("sign");

    let op = SignedGroupOp::sign(
        &sk,
        ContextGroupId::from([0x26; 32]),
        vec![],
        1,
        GroupOp::AccountDeviceCertified {
            certificate: Box::new(AccountProof {
                genesis,
                chain: vec![],
                statement: cert,
            }),
            scope: Box::new(AccountProof {
                genesis,
                chain: vec![],
                statement: scope,
            }),
        },
    )
    .expect("sign the op");

    assert!(matches!(op.validate(), Err(GovernanceError::Bounds(_))));
}

/// The narrowing op carries the same root-signed scope off the same wire, so an
/// uncapped application list would be the same amplification one variant along.
#[test]
fn an_oversized_descoped_application_list_is_refused() {
    let root = PrivateKey::from([0x27; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::from([0x28; 32]);
    let applications =
        vec![ApplicationId::from([0x29; 32]); bounds::MAX_DEVICE_SCOPE_APPLICATIONS + 1];
    let scope = DeviceScope::sign(&root, account, device, applications, 1, 0).expect("sign");

    let op = SignedGroupOp::sign(
        &PrivateKey::from([0x2A; 32]),
        ContextGroupId::from([0x2B; 32]),
        vec![],
        1,
        GroupOp::AccountDeviceDescoped {
            account,
            device,
            application: None,
            scope: Box::new(AccountProof {
                genesis,
                chain: vec![],
                statement: scope,
            }),
        },
    )
    .expect("sign the op");

    assert!(matches!(op.validate(), Err(GovernanceError::Bounds(_))));
}

/// Appended last, so every ordinal before it is untouched and the narrowing op
/// decodes back to the scope its publisher signed.
#[test]
fn the_descoped_op_is_the_last_ordinal_and_round_trips() {
    let root = PrivateKey::from([0x2C; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::from([0x2D; 32]);
    let scope = DeviceScope::sign(
        &root,
        account,
        device,
        vec![ApplicationId::from([0x2E; 32])],
        3,
        0,
    )
    .expect("sign");
    let target = ApplicationId::from([0x2F; 32]);
    let op = GroupOp::AccountDeviceDescoped {
        account,
        device,
        application: Some(target),
        scope: Box::new(AccountProof {
            genesis,
            chain: vec![],
            statement: scope,
        }),
    };

    let bytes = borsh::to_vec(&op).expect("encode");
    assert_eq!(bytes[0], 31, "the variant must be appended, never inserted");
    let decoded: GroupOp = borsh::from_slice(&bytes).expect("decode");
    assert!(matches!(
        decoded,
        GroupOp::AccountDeviceDescoped { ref scope, application, .. }
            if scope.statement.scope_epoch == 3 && application == Some(target)
    ));
    assert_eq!(op.op_kind_label(), "account_device_descoped");
}

/// All-zero credential, so the printed vector is reproducible.
/// A fixed endorsement, so the golden vector below is reproducible.
fn deterministic_endorsement() -> AdmitterEndorsement {
    AdmitterEndorsement::sign(
        &PrivateKey::from([9u8; 32]),
        &[0u8; 32],
        &AccountId::from([0u8; 32]),
        &[0u8; 32],
    )
    .expect("sign endorsement")
}

fn deterministic_credential() -> Box<JoinAccountCredential> {
    let genesis = calimero_account::AccountGenesis::new(PublicKey::from([0u8; 32]));
    Box::new(JoinAccountCredential {
        statement: calimero_account::DeviceCert {
            account: genesis.account_id(),
            device: calimero_account::DeviceId::from([0u8; 32]),
            sign_pk: PublicKey::from([0u8; 32]),
            kem_pk: calimero_account::KemPublicKey::from([0u8; 32]),
            key_epoch: 0,
            device_epoch: 0,
            signature: [0u8; 64],
        },
        genesis,
        chain: Vec::new(),
    })
}

#[test]
#[ignore = "regeneration helper: run with --ignored to print golden vectors"]
fn emit_golden_root_op_vectors() {
    use calimero_context_config::types::{
        GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
    };

    // Regenerating by hand means computing borsh offsets by hand, which is how
    // a golden vector ends up asserting the wrong thing confidently. This
    // prints them instead.
    let minimal_invitation = SignedGroupOpenInvitation {
        inviter_account: None,
        invitation: GroupInvitationFromAdmin {
            inviter_identity: SignerId::from([0u8; 32]),
            group_id: ContextGroupId::from([0u8; 32]),
            expiration_timestamp: 0,
            invitation_nonce: [0u8; 32],
            invited_role: 1,
            admitters: Vec::new(),
        },
        inviter_signature: String::new(),
        admitter_addrs: Vec::new(),
        application_id: None,
        bytecode_id: None,
    };

    for (name, op) in [
        (
            "GOLDEN_ROOT_OP_MEMBER_JOINED",
            NamespaceOp::Root(RootOp::MemberJoined {
                member: AccountId::from([0u8; 32]),
                signed_invitation: minimal_invitation.clone(),
                account: deterministic_credential(),
            }),
        ),
        (
            "GOLDEN_ROOT_OP_MEMBER_JOINED_AT",
            NamespaceOp::Root(RootOp::MemberJoinedAt {
                member: AccountId::from([0u8; 32]),
                signed_invitation: minimal_invitation.clone(),
                joined_at: 0,
                account: deterministic_credential(),
            }),
        ),
    ] {
        let bytes = borsh::to_vec(&op).expect("encode");
        println!("{name} ({} bytes):", bytes.len());
        println!("{bytes:?}");
    }
}

// ---------------------------------------------------------------------------
// E1: sealing root ops
// ---------------------------------------------------------------------------

/// The claim that makes appending `RootSealed` safe: existing ops encode exactly
/// as before.
///
/// Borsh numbers enum variants by declaration position, so appending is only
/// safe while nothing is inserted ahead of the existing two. Pinned as bytes
/// rather than trusted, because the failure is silent in the worst way — every
/// op id in the namespace changes, and the first symptom is peers disagreeing
/// about history rather than anything refusing to decode.
#[test]
fn appending_root_sealed_left_the_existing_discriminants_alone() {
    let root = NamespaceOp::Root(RootOp::AdminChanged {
        new_admin: calimero_account::AccountId::from([3u8; 32]),
    });
    let group = NamespaceOp::Group {
        group_id: ContextGroupId::from([4u8; 32]),
        key_id: KeyId::from([5u8; 32]),
        encrypted: EncryptedGroupOp {
            nonce: [6u8; 12],
            ciphertext: vec![7, 8, 9],
        },
        key_rotation: None,
    };
    let sealed = NamespaceOp::RootSealed {
        key_id: KeyId::from([5u8; 32]),
        encrypted: EncryptedRootOp {
            nonce: [6u8; 12],
            ciphertext: vec![7, 8, 9],
        },
    };

    assert_eq!(borsh::to_vec(&root).unwrap()[0], 0, "Root must stay 0");
    assert_eq!(borsh::to_vec(&group).unwrap()[0], 1, "Group must stay 1");
    assert_eq!(
        borsh::to_vec(&sealed).unwrap()[0],
        2,
        "RootSealed must be appended, never inserted"
    );
}

/// A sealed op and a group op with identical payloads must not encode alike.
///
/// They carry the same fields in the same order, so only the discriminant tells
/// them apart. A receiver that read one as the other would look up the right key
/// id in the wrong keyring and report a missing key rather than a mismatch.
#[test]
fn a_sealed_root_op_is_distinguishable_from_a_group_op() {
    let key_id = KeyId::from([5u8; 32]);
    let nonce = [6u8; 12];
    let ciphertext = vec![7, 8, 9];

    let sealed = borsh::to_vec(&NamespaceOp::RootSealed {
        key_id,
        encrypted: EncryptedRootOp {
            nonce,
            ciphertext: ciphertext.clone(),
        },
    })
    .unwrap();
    let group = borsh::to_vec(&NamespaceOp::Group {
        group_id: ContextGroupId::from([5u8; 32]),
        key_id,
        encrypted: EncryptedGroupOp { nonce, ciphertext },
        key_rotation: None,
    })
    .unwrap();

    assert_ne!(sealed, group);
}

/// Every variant's sealability, asserted against the reasons rather than the
/// implementation.
///
/// `root_op_is_sealable` is an exhaustive match, so a twelfth variant fails to
/// compile until it is classified. This test states what the classification has
/// to be, so a future edit that reclassifies one has to argue with a test rather
/// than only with a reviewer.
#[test]
fn only_the_two_bootstrap_variants_travel_in_the_clear() {
    use calimero_context_config::types::{
        GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
    };

    let invitation = SignedGroupOpenInvitation {
        inviter_account: None,
        invitation: GroupInvitationFromAdmin {
            inviter_identity: SignerId::from([0u8; 32]),
            group_id: ContextGroupId::from([0u8; 32]),
            expiration_timestamp: 0,
            invitation_nonce: [0u8; 32],
            invited_role: 1,
            admitters: Vec::new(),
        },
        inviter_signature: String::new(),
        admitter_addrs: Vec::new(),
        application_id: None,
        bytecode_id: None,
    };

    assert!(root_op_is_sealable(&RootOp::AdminChanged {
        new_admin: calimero_account::AccountId::from([1u8; 32]),
    }));
    assert!(root_op_is_sealable(&RootOp::PolicyUpdated {
        policy_bytes: vec![1, 2, 3],
    }));
    assert!(root_op_is_sealable(&RootOp::GroupReparented {
        child_group_id: ContextGroupId::from([2u8; 32]),
        new_parent_id: ContextGroupId::from([3u8; 32]),
    }));
    assert!(root_op_is_sealable(&RootOp::GroupDeleted {
        root_group_id: ContextGroupId::from([2u8; 32]),
        cascade_group_ids: vec![],
        cascade_context_ids: vec![],
    }));
    assert!(root_op_is_sealable(&RootOp::GroupCreated {
        group_id: ContextGroupId::from([2u8; 32]),
        parent_id: ContextGroupId::from([3u8; 32]),
        restricted: true,
        admin: calimero_account::AccountId::from([4u8; 32]),
        salt: [0; 32],
    }));

    // Published by a key-holder like the five above, so it seals. The variant
    // that looks least sealable, because its RECIPIENT holds no key — but the
    // recipient no longer reads it from the DAG: a participant with no key
    // pulls one over the direct-stream path, which authenticates the key by
    // content rather than trusting a publisher's signature. Sealing it keeps
    // the delivery metadata (which account, at which causal position) off the
    // namespace topic.
    assert!(root_op_is_sealable(&RootOp::KeyDelivery {
        group_id: ContextGroupId::from([6u8; 32]),
        envelope: KeyEnvelope {
            // Member-addressed: the bootstrap form, which `EnvelopeRecipient`
            // documents as un-retirable because a device link is itself an
            // encrypted op. Sealing the carrier does not change the addressing.
            recipient: EnvelopeRecipient::Member {
                identity: PublicKey::from([7u8; 32]),
                ephemeral_pk: PublicKey::from([8u8; 32]),
            },
            sender: PublicKey::from([9u8; 32]),
            nonce: [0u8; 12],
            ciphertext: vec![1, 2, 3],
            signature: [0u8; 64],
        },
    }));

    // The joins whose PUBLISHER holds the namespace key, which is what sealing
    // needs -- not the joiner's own key state, which is what makes them read
    // like exceptions.
    //
    // `MemberJoinedOpen` is published by an inherited member reaching into an
    // Open subgroup: it lacks the SUBGROUP key, which is the whole point of the
    // op, and holds the namespace key the seal uses.
    // `MemberJoinedViaTeeAttestation` is published by the ADMITTER, never by the
    // attested node.
    assert!(root_op_is_sealable(&RootOp::MemberJoinedOpen {
        member: calimero_account::AccountId::from([5u8; 32]),
        group_id: ContextGroupId::from([6u8; 32]),
        account: deterministic_credential(),
    }));
    assert!(root_op_is_sealable(
        &RootOp::MemberJoinedViaTeeAttestation {
            group_id: ContextGroupId::from([6u8; 32]),
            member: PublicKey::from([7u8; 32]),
            quote_hash: [0u8; 32],
            mrtd: String::new(),
            rtmr0: String::new(),
            rtmr1: String::new(),
            rtmr2: String::new(),
            rtmr3: String::new(),
            tcb_status: String::new(),
            role: calimero_primitives::context::GroupMemberRole::ReadOnlyTee,
            account: deterministic_credential(),
        }
    ));

    // The three that cannot be sealed, for two different reasons.
    //
    // The invitation joins are published by the JOINER, before any key has
    // reached it -- and publishing one is what causes an admin to send a key.
    // Sealing them under a key the publisher obtains BY publishing them is
    // circular, and the failure is total: the publish is refused, no admin ever
    // learns of the join, and nobody joins a namespace at all. `MemberJoined` is
    // the non-expiring form and is emitted by mero-js's `signMemberJoinOp`, so
    // sealing it locks out browser clients holding a non-expiring invitation.
    // Sealing either needs a different publisher, not a reclassification here.
    assert!(!root_op_is_sealable(&RootOp::MemberJoined {
        member: calimero_account::AccountId::from([5u8; 32]),
        signed_invitation: invitation.clone(),
        account: deterministic_credential(),
    }));
    assert!(!root_op_is_sealable(&RootOp::MemberJoinedAt {
        member: calimero_account::AccountId::from([5u8; 32]),
        signed_invitation: invitation,
        joined_at: 0,
        account: deterministic_credential(),
    }));

    // `NamespaceCreatedV2` is genesis: there is no namespace key yet to seal it
    // under, and this op's own apply is what establishes the founder.
    assert!(!root_op_is_sealable(&RootOp::NamespaceCreatedV2 {
        founder: calimero_account::AccountId::from([5u8; 32]),
        account: deterministic_credential(),
        salt: [6u8; 32],
    }));
}

/// A label rides in from the wire before any authorization runs and is rendered
/// verbatim in a listing, so the shape is decided here, not at the handler.
#[test]
fn a_device_label_is_refused_unless_it_is_trimmed_non_empty_and_printable() {
    let root = PrivateKey::from([0x30; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::from([0x31; 32]);

    let labelled = |label: &str| {
        SignedGroupOp::sign(
            &PrivateKey::from([0x32; 32]),
            ContextGroupId::from([0x33; 32]),
            vec![],
            1,
            GroupOp::AccountDeviceLabelled {
                account,
                device,
                label: label.to_owned(),
                label_epoch: 0,
                root_proof: None,
            },
        )
        .expect("sign the op")
    };

    assert!(labelled("Work laptop").validate().is_ok());
    for refused in [
        "",
        "   ",
        " untrimmed",
        "untrimmed ",
        "two\nlines",
        "bell\u{7}",
        &"x".repeat(bounds::MAX_DEVICE_LABEL_BYTES + 1),
    ] {
        assert!(
            matches!(
                labelled(refused).validate(),
                Err(GovernanceError::Bounds(_))
            ),
            "{refused:?} is not a usable device name"
        );
    }
}

/// Appended last, so every ordinal before it is untouched and the root-signed
/// authority a holder attaches survives the round trip.
#[test]
fn the_labelled_op_is_the_last_ordinal_and_round_trips() {
    let root = PrivateKey::from([0x34; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::from([0x35; 32]);
    let statement =
        calimero_account::DeviceLabel::sign(&root, account, device, "Phone".to_owned(), 2, 0)
            .expect("sign the label");
    let op = GroupOp::AccountDeviceLabelled {
        account,
        device,
        label: "Phone".to_owned(),
        label_epoch: 2,
        root_proof: Some(Box::new(AccountProof {
            genesis,
            chain: vec![],
            statement,
        })),
    };

    let bytes = borsh::to_vec(&op).expect("encode");
    assert_eq!(bytes[0], 32, "the variant must be appended, never inserted");
    let decoded: GroupOp = borsh::from_slice(&bytes).expect("decode");
    assert!(matches!(
        decoded,
        GroupOp::AccountDeviceLabelled { ref label, label_epoch, ref root_proof, .. }
            if label == "Phone"
                && label_epoch == 2
                && root_proof.as_ref().is_some_and(|proof| proof
                    .authorises(account, device)
                    .is_ok())
    ));
    assert_eq!(op.op_kind_label(), "account_device_labelled");
}

/// The two mode-carrying policy ops are appended after `TeeVaultKeyDelivered`,
/// so no stored op's discriminant moves and the unversioned policy ops still
/// decode — which is what lets a policy set before the mode existed read as
/// `replica`.
#[test]
fn tee_admission_mode_ops_are_appended_and_the_mode_is_frozen() {
    let vault = borsh::to_vec(&GroupOp::TeeVaultKeyDelivered {
        vault_key: PublicKey::from([0u8; 32]),
        recipient_key: PublicKey::from([0u8; 32]),
        envelope: vec![],
    })
    .expect("encode");
    let list = borsh::to_vec(&GroupOp::TeeAdmissionPolicySetV2 {
        allowed_mrtd: vec![],
        allowed_rtmr0: vec![],
        allowed_rtmr1: vec![],
        allowed_rtmr2: vec![],
        allowed_rtmr3: vec![],
        allowed_tcb_statuses: vec![],
        accept_mock: false,
        mode: crate::TeeAdmissionMode::Relay,
    })
    .expect("encode");
    let release = borsh::to_vec(&GroupOp::TeeReleaseAdmissionPolicySetV2 {
        allowed_profiles: vec![],
        min_release_version: None,
        allowed_tcb_statuses: vec![],
        accept_mock: false,
        mode: crate::TeeAdmissionMode::Replica,
    })
    .expect("encode");
    assert_eq!(
        list[0],
        vault[0] + 1,
        "TeeAdmissionPolicySetV2 follows the vault op"
    );
    assert_eq!(
        release[0],
        vault[0] + 2,
        "and its release sibling follows it"
    );
    // The mode is the op's last byte: Replica = 0, Relay = 1.
    assert_eq!(list.last(), Some(&1));
    assert_eq!(release.last(), Some(&0));

    assert_eq!(
        crate::TeeAdmissionMode::default(),
        crate::TeeAdmissionMode::Replica,
        "an absent mode is a replica"
    );
    assert_eq!(
        crate::TeeAdmissionMode::Replica.role(),
        calimero_primitives::context::GroupMemberRole::ReadOnlyTee
    );
    assert_eq!(
        crate::TeeAdmissionMode::Relay.role(),
        calimero_primitives::context::GroupMemberRole::RelayTee
    );
}

/// `ContextRegisteredOnBehalf` is appended after the V2 policy ops, so no stored
/// op's discriminant moves, and it round-trips with its delegation intact — the
/// bundle is what every peer re-verifies, so a lossy encoding would make every
/// peer refuse what the relay applied.
#[test]
fn context_registered_on_behalf_is_appended_and_round_trips() {
    let release = borsh::to_vec(&GroupOp::TeeReleaseAdmissionPolicySetV2 {
        allowed_profiles: vec![],
        min_release_version: None,
        allowed_tcb_statuses: vec![],
        accept_mock: false,
        mode: crate::TeeAdmissionMode::Replica,
    })
    .expect("encode");

    let author = PrivateKey::from([0x0A; 32]);
    let root = PrivateKey::from([0x0C; 32]);
    let genesis = calimero_account::AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = calimero_account::DeviceId::mint(account, [0x01; 16]);
    let cert = calimero_account::DeviceCert::sign(
        &root,
        account,
        device,
        &author.public_key(),
        &calimero_account::KemPublicKey::from([0x02; 32]),
        0,
        0,
    )
    .expect("cert");
    let proof = Box::new(calimero_account::AccountProof {
        genesis,
        chain: vec![],
        statement: cert,
    });
    let warrant = calimero_account::ContextCreationWarrant::sign(
        &author,
        calimero_account::ContextCreationTerms {
            group: [0x11; 32],
            seed: [0x12; 32],
            author_account: account,
            executor: account,
            application_id: calimero_primitives::application::ApplicationId::from([0x13; 32]),
            service_name: None,
            name: Some("general".to_owned()),
            init_hash: calimero_account::ContextCreationWarrant::init_hash(b"{}"),
            account_heads: vec![],
            governance_floor: vec![],
            nonce: 1,
            not_after: 2,
        },
    )
    .expect("sign");
    let delegation = calimero_account::ContextCreationDelegation {
        warrant: Box::new(warrant),
        author_proof: proof.clone(),
        executor_proof: proof,
        executor_key: author.public_key(),
    };
    let op = GroupOp::ContextRegisteredOnBehalf {
        context_id: ContextId::from([0x14; 32]),
        application_id: calimero_primitives::application::ApplicationId::from([0x13; 32]),
        blob_id: calimero_primitives::blobs::BlobId::from([0x15; 32]),
        source: "https://example.test/app.mpk".to_owned(),
        service_name: None,
        package: "com.example.chat".to_owned(),
        version: "1.0.0".to_owned(),
        name: Some("general".to_owned()),
        delegation: Box::new(delegation.clone()),
    };
    let bytes = borsh::to_vec(&op).expect("encode");
    assert_eq!(
        bytes[0],
        release[0] + 1,
        "ContextRegisteredOnBehalf follows TeeReleaseAdmissionPolicySetV2"
    );
    assert_eq!(op.op_kind_label(), "context_registered_on_behalf");

    let decoded: GroupOp = borsh::from_slice(&bytes).expect("decode");
    let GroupOp::ContextRegisteredOnBehalf {
        delegation: decoded_delegation,
        name,
        ..
    } = decoded
    else {
        panic!("decoded as another variant");
    };
    assert_eq!(*decoded_delegation, delegation);
    assert_eq!(name.as_deref(), Some("general"));
    let _verified = decoded_delegation
        .verify()
        .expect("a decoded bundle still verifies");
}

/// The bytes a non-Rust client signs for a real op, end to end: the op's borsh
/// (the `op` field of `/governance-intents`), its `op_hash`, for a group op and
/// a root op. Pins the variant indices (`MemberAdded` = 1, `GroupCreated` = 0)
/// and the delegable form of a removal (hashes cleared), so mero-js can assert
/// its encoder and its commitment against core.
#[test]
fn delegated_governance_op_vectors_are_stable() {
    use calimero_account::{GovernanceOpKind, GovernanceWarrant};
    use calimero_primitives::context::GroupMemberRole;

    let added = GroupOp::MemberAdded {
        member: AccountId::from([0x44; 32]),
        role: GroupMemberRole::Member,
    };
    let bytes = borsh::to_vec(&added).expect("encode");
    assert_eq!(
        hex::encode(&bytes),
        "01444444444444444444444444444444444444444444444444444444444444444401"
    );
    assert_eq!(
        hex::encode(GovernanceWarrant::op_hash(GovernanceOpKind::Group, &bytes)),
        "c48dce4ba9da20980a86832b133e7040ec67c293987c0f131140291a71041cd6"
    );

    let removed = GroupOp::MemberRemoved {
        member: AccountId::from([0x44; 32]),
        expected_group_state_hash: [0x42; 32],
        expected_context_state_hashes: vec![],
    };
    let form = borsh::to_vec(&removed.delegable_form().expect("delegable")).expect("encode");
    assert_eq!(hex::encode(&form), "024444444444444444444444444444444444444444444444444444444444444444000000000000000000000000000000000000000000000000000000000000000000000000");

    let created = RootOp::GroupCreated {
        group_id: ContextGroupId::from([0x55; 32]),
        parent_id: ContextGroupId::from([0x11; 32]),
        restricted: true,
        admin: AccountId::from([0x22; 32]),
        salt: [0x33; 32],
    };
    let bytes = borsh::to_vec(&created).expect("encode");
    assert_eq!(hex::encode(&bytes), "00555555555555555555555555555555555555555555555555555555555555555511111111111111111111111111111111111111111111111111111111111111110122222222222222222222222222222222222222222222222222222222222222223333333333333333333333333333333333333333333333333333333333333333");
    assert_eq!(
        hex::encode(GovernanceWarrant::op_hash(GovernanceOpKind::Root, &bytes)),
        "3a30eacf28687109de2b63b649cb0d8a532f344212066897547f52a416a1be0e"
    );
}

// ---------------------------------------------------------------------------
// Root-guarded owner-level ops (schema 20)
// ---------------------------------------------------------------------------

/// A root proof for `op`, signed by the account rooted at `root_seed`.
fn owner_proof(
    root_seed: u8,
    group: [u8; 32],
    kind: calimero_account::OwnerOpKind,
    digest: [u8; 32],
    counter: u64,
) -> calimero_account::SignedOwnerOp {
    let root = PrivateKey::from([root_seed; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let terms = calimero_account::OwnerOpTerms {
        account: genesis.account_id(),
        namespace_id: [0x11; 32],
        group_id: group,
        kind,
        op_digest: digest,
        counter,
        key_epoch: 0,
    };
    calimero_account::SignedOwnerOp {
        genesis,
        chain: vec![],
        statement: OwnerOpAuthorization::sign(&root, terms).expect("sign"),
    }
}

#[test]
fn root_guarded_is_appended_after_every_existing_ordinal() {
    let inner = GroupOp::TransferOwnership {
        new_owner: AccountId::from([0x44; 32]),
    };
    let proof = owner_proof(
        7,
        [0x22; 32],
        OwnerOpKind::TransferOwnership,
        inner.owner_op_digest().expect("digest"),
        0,
    );
    let op = GroupOp::RootGuarded {
        op: Box::new(inner),
        proof: Box::new(proof.clone()),
    };
    let bytes = borsh::to_vec(&op).expect("encode");
    assert_eq!(
        bytes[0], 42,
        "GroupOp::RootGuarded follows FoundingRelayAttested (41)"
    );
    assert_eq!(op.op_kind_label(), "root_guarded");
    let decoded: GroupOp = borsh::from_slice(&bytes).expect("decode");
    let GroupOp::RootGuarded { proof: decoded, .. } = decoded else {
        panic!("decoded as another variant");
    };
    assert_eq!(*decoded, proof);

    let root = RootOp::RootGuarded {
        op: Box::new(RootOp::AdminChanged {
            new_admin: AccountId::from([0x44; 32]),
        }),
        proof: Box::new(proof),
    };
    let bytes = borsh::to_vec(&NamespaceOp::Root(root)).expect("encode");
    assert_eq!(bytes[0], 0, "NamespaceOp::Root");
    assert_eq!(bytes[1], 12, "RootOp::RootGuarded follows OnBehalf (11)");
}

#[test]
fn owner_op_kinds_are_exactly_the_guarded_ops() {
    let account = AccountId::from([0x44; 32]);
    let guarded = [
        (
            GroupOp::TransferOwnership { new_owner: account },
            OwnerOpKind::TransferOwnership,
        ),
        (GroupOp::GroupDelete, OwnerOpKind::GroupDelete),
        (
            GroupOp::TeeAuthoringPolicySet {
                allowed_mrtd: vec![],
            },
            OwnerOpKind::TeeAuthoringPolicy,
        ),
        (
            GroupOp::TeeAdmissionPolicySetV2 {
                allowed_mrtd: vec![],
                allowed_rtmr0: vec![],
                allowed_rtmr1: vec![],
                allowed_rtmr2: vec![],
                allowed_rtmr3: vec![],
                allowed_tcb_statuses: vec![],
                accept_mock: false,
                mode: TeeAdmissionMode::Replica,
            },
            OwnerOpKind::TeeAdmissionPolicy,
        ),
        (
            GroupOp::TeeReleaseAdmissionPolicySetV2 {
                allowed_profiles: vec![],
                min_release_version: None,
                allowed_tcb_statuses: vec![],
                accept_mock: false,
                mode: TeeAdmissionMode::Replica,
            },
            OwnerOpKind::TeeReleaseAdmissionPolicy,
        ),
    ];
    for (op, kind) in &guarded {
        assert_eq!(op.owner_op_kind(), Some(*kind), "{}", op.op_kind_label());
        // None of them may travel through a relay: a relay is exactly the
        // party the guard exists to keep out.
        assert!(op.delegable_form().is_none(), "{}", op.op_kind_label());
    }

    // Member-level governance needs no proof. Removing and demoting members is
    // what an attacker does AFTER taking ownership; guarding the transfer is
    // what stops it.
    for op in [
        GroupOp::MemberAdded {
            member: account,
            role: GroupMemberRole::Admin,
        },
        GroupOp::MemberRoleSet {
            member: account,
            role: GroupMemberRole::Member,
        },
        GroupOp::Noop,
    ] {
        assert_eq!(op.owner_op_kind(), None, "{}", op.op_kind_label());
    }

    assert_eq!(
        RootOp::AdminChanged { new_admin: account }.owner_op_kind(),
        Some(OwnerOpKind::AdminChanged)
    );
    // `GroupDeleted` cannot target the namespace root and is admin moderation;
    // it stays unguarded by decision.
    assert_eq!(
        RootOp::GroupDeleted {
            root_group_id: ContextGroupId::from([0x22; 32]),
            cascade_group_ids: vec![],
            cascade_context_ids: vec![],
        }
        .owner_op_kind(),
        None
    );
}

#[test]
fn a_wrapper_is_not_itself_a_guarded_kind_and_is_never_delegable() {
    let inner = GroupOp::GroupDelete;
    let proof = owner_proof(
        7,
        [0x22; 32],
        OwnerOpKind::GroupDelete,
        inner.owner_op_digest().expect("digest"),
        0,
    );
    let wrapped = GroupOp::RootGuarded {
        op: Box::new(inner),
        proof: Box::new(proof.clone()),
    };
    // `None` is what makes a wrapper-in-a-wrapper refusable on apply: the
    // inner op of a wrapper must name a guarded kind.
    assert_eq!(wrapped.owner_op_kind(), None);
    assert!(wrapped.delegable_form().is_none());
    assert!(matches!(wrapped.unguarded(), GroupOp::GroupDelete));

    let root = RootOp::RootGuarded {
        op: Box::new(RootOp::AdminChanged {
            new_admin: AccountId::from([0x44; 32]),
        }),
        proof: Box::new(proof),
    };
    assert_eq!(root.owner_op_kind(), None);
    assert!(root.delegable_form().is_none());
    assert!(root_op_is_sealable(&root), "sealed as the op it guards is");
}

#[test]
fn a_guarded_ops_proof_chain_is_bounded() {
    let inner = GroupOp::GroupDelete;
    let mut proof = owner_proof(
        7,
        [0x22; 32],
        OwnerOpKind::GroupDelete,
        inner.owner_op_digest().expect("digest"),
        0,
    );
    let handoff = RootKeyHandoff {
        account: proof.statement.account,
        from_epoch: 0,
        new_root_sign_pk: PrivateKey::from([8; 32]).public_key(),
        signature: [0; 64],
    };
    proof.chain = vec![handoff; bounds::MAX_ROOT_KEY_HANDOFFS + 1];
    let op = GroupOp::RootGuarded {
        op: Box::new(inner),
        proof: Box::new(proof),
    };
    assert!(matches!(op.validate(), Err(GovernanceError::Bounds(_))));
}

/// The bytes mero-js must reproduce for a guarded op: the inner op's digest,
/// the proof, and the whole wrapper. See `mero-js` `owner-op` tests.
#[test]
fn root_guarded_vectors_are_stable() {
    let inner = GroupOp::TransferOwnership {
        new_owner: AccountId::from([0x44; 32]),
    };
    let digest = inner.owner_op_digest().expect("digest");
    let proof = owner_proof(7, [0x22; 32], OwnerOpKind::TransferOwnership, digest, 5);
    let op = GroupOp::RootGuarded {
        op: Box::new(inner),
        proof: Box::new(proof),
    };
    let root_inner = RootOp::AdminChanged {
        new_admin: AccountId::from([0x44; 32]),
    };
    let root_digest = root_inner.owner_op_digest().expect("digest");

    assert_eq!(hex::encode(digest), GUARDED_TRANSFER_DIGEST);
    assert_eq!(
        hex::encode(borsh::to_vec(&op).expect("encode")),
        GUARDED_TRANSFER_OP
    );
    assert_eq!(hex::encode(root_digest), GUARDED_ADMIN_CHANGED_DIGEST);

    // The inner ops mero-js encodes, byte for byte: its digest is taken over
    // these, so one wrong byte is a proof the node refuses as "another op".
    let inner_ops = [
        (
            GroupOp::TeeAuthoringPolicySet {
                allowed_mrtd: vec!["aa".to_owned()],
            },
            "2101000000020000006161",
        ),
        (
            GroupOp::TeeAdmissionPolicySetV2 {
                allowed_mrtd: vec!["aa".to_owned()],
                allowed_rtmr0: vec![],
                allowed_rtmr1: vec![],
                allowed_rtmr2: vec![],
                allowed_rtmr3: vec!["bb".to_owned()],
                allowed_tcb_statuses: vec!["UpToDate".to_owned()],
                accept_mock: false,
                mode: TeeAdmissionMode::Relay,
            },
            "25010000000200000061610000000000000000000000000100000002000000626201000000\
             080000005570546f446174650001",
        ),
        (
            GroupOp::TeeReleaseAdmissionPolicySetV2 {
                allowed_profiles: vec!["locked-read-only".to_owned()],
                min_release_version: Some("2.3.72".to_owned()),
                allowed_tcb_statuses: vec![],
                accept_mock: true,
                mode: TeeAdmissionMode::Replica,
            },
            "2601000000100000006c6f636b65642d726561642d6f6e6c790106000000322e332e3732\
             000000000100",
        ),
        (GroupOp::GroupDelete, "0e"),
    ];
    for (op, expected) in inner_ops {
        assert_eq!(
            hex::encode(borsh::to_vec(&op).expect("encode")),
            expected,
            "{}",
            op.op_kind_label()
        );
    }
}

const GUARDED_TRANSFER_DIGEST: &str =
    "a8d53a53c6d1007d055fbfd972aa7a99b27d5a816defdf826d49d1b0799b2c8f";
const GUARDED_TRANSFER_OP: &str =
    "2a15444444444444444444444444444444444444444444444444444444444444444402ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c000000009f9d3474c108b0bc7809ba35b5434980dd5b34f4a5042bdbb4de5519102324fb1111111111111111111111111111111111111111111111111111111111111111222222222222222222222222222222222222222222222222222222222222222200a8d53a53c6d1007d055fbfd972aa7a99b27d5a816defdf826d49d1b0799b2c8f050000000000000000000000d9fefff69e034d07a996f2f0dac5052865a93a780b58f935b3ff7a509ab88ed1f8171ee9c65b2b66a946ec53d8aba76d53c4fd41553973d2fc87a05215f09502";
const GUARDED_ADMIN_CHANGED_DIGEST: &str =
    "bf73aaf8507327573528f38fdf5b003399a4773a7272539548188bc844d8a598";

/// One pinned delegable-op vector: the borsh bytes of the op's
/// `delegable_form()` and `GovernanceWarrant::op_hash(kind, bytes)`.
///
/// `op_hash` is `domain_hash("calimero.governance-warrant.op.v1", [[kind], bytes])`
/// with `kind` = 0 for a `GroupOp`, 1 for a `RootOp`.
struct DelegableVector {
    name: &'static str,
    kind: calimero_account::GovernanceOpKind,
    bytes: Vec<u8>,
    bytes_hex: &'static str,
    op_hash_hex: &'static str,
}

fn group_delegable_bytes(op: &GroupOp) -> Vec<u8> {
    borsh::to_vec(&op.delegable_form().expect("op must be delegable")).expect("encode")
}

fn root_delegable_bytes(op: &RootOp) -> Vec<u8> {
    borsh::to_vec(&op.delegable_form().expect("op must be delegable")).expect("encode")
}

/// Golden vectors for every delegable governance op a non-Rust client (mero-js)
/// has to encode, beyond the three pinned in
/// `delegated_governance_op_vectors_are_stable`.
///
/// Borsh conventions a non-Rust encoder must reproduce: an enum is a `u8`
/// variant index (by position in the enum) followed by its fields in declaration
/// order; `AccountId` / `ContextGroupId` / `ContextId` / `PublicKey` / `[u8; 32]`
/// are 32 raw bytes; `u32` / `u64` little-endian; `Vec<T>` / `String` a `u32` LE
/// length then the elements / UTF-8 bytes; `Option<T>` a `0` byte for `None`, or
/// `1` then `T`; `BTreeMap<String, String>` a `u32` LE length then the entries
/// sorted by key, each key then value. `Box<T>` encodes as `T`.
///
/// Leaf types:
/// - `GroupMemberRole`: `u8` — Admin 0, Member 1, ReadOnly 2, ReadOnlyTee 3, RelayTee 4.
/// - `MemberCapabilities`: `u32` LE bitmask.
/// - `ContextCapabilityBits`: `u8` bitmask, never zero.
/// - `VisibilityMode`: `u8` — Open 0, Restricted 1.
/// - `JoinAccountCredential` (= `AccountProof<DeviceCert>`): `genesis`
///   (`AccountGenesis { version: u8, root_sign_pk: [u8; 32] }`), `chain`
///   (`Vec<RootKeyHandoff>`), `statement` (`DeviceCert { account: [u8; 32],
///   device: [u8; 32], sign_pk: [u8; 32], kem_pk: [u8; 32], key_epoch: u32,
///   device_epoch: u32, signature: [u8; 64] }`).
///
/// `GroupOp` (kind `Group` = 0) variants pinned, with their index and fields:
/// - 3 `MemberLeft { member: AccountId, expected_group_state_hash: [u8; 32],
///   expected_context_state_hashes: Vec<(ContextId, [u8; 32])> }` — delegable
///   form zeroes the hash and empties the list.
/// - 2 `MemberRemoved` — same layout and delegable form as `MemberLeft`.
/// - 4 `MemberRoleSet { member: AccountId, role: GroupMemberRole }`
/// - 5 `MemberCapabilitySet { member: AccountId, capabilities: MemberCapabilities }`
/// - 6 `DefaultCapabilitiesSet { capabilities: MemberCapabilities }`
/// - 9 `ContextDetached { context_id: ContextId }`
/// - 10 `SubgroupVisibilitySet { mode: VisibilityMode }`
/// - 11 `GroupMetadataSet { name: Option<String>, data: BTreeMap<String, String> }`
/// - 12 `MemberMetadataSet { member: AccountId, name: Option<String>,
///   data: BTreeMap<String, String> }`
/// - 13 `ContextMetadataSet { context_id: ContextId, name: Option<String>,
///   data: BTreeMap<String, String> }`
/// - 16 `ContextCapabilityGranted { context_id: ContextId, member: AccountId,
///   capability: ContextCapabilityBits }`
/// - 17 `ContextCapabilityRevoked` — same layout as `ContextCapabilityGranted`.
///
/// `RootOp` (kind `Root` = 1) variants pinned:
/// - 1 `GroupReparented { child_group_id: ContextGroupId, new_parent_id: ContextGroupId }`
/// - 2 `GroupDeleted { root_group_id: ContextGroupId, cascade_group_ids:
///   Vec<ContextGroupId>, cascade_context_ids: Vec<ContextId> }` — delegable
///   form empties both cascade lists.
/// - 7 `MemberJoinedOpen { member: AccountId, group_id: ContextGroupId,
///   account: Box<JoinAccountCredential> }` — delegable as-is. The credential
///   here is a fixed-byte fixture that exercises the layout, not one that
///   verifies.
#[test]
fn delegable_governance_op_vectors_for_non_rust_encoders_are_stable() {
    use calimero_account::{
        AccountGenesis, DeviceCert, DeviceId, GovernanceOpKind, GovernanceWarrant, KemPublicKey,
    };

    let member = AccountId::from([0x44; 32]);
    let group = ContextGroupId::from([0x55; 32]);
    let parent = ContextGroupId::from([0x11; 32]);
    let context = ContextId::from([0x66; 32]);
    let caps_one = MemberCapabilities::from_bits(0b1).expect("defined bits");
    let caps_231 = MemberCapabilities::from_bits(231).expect("defined bits");
    let ctx_cap_one = ContextCapabilityBits::new(0b1).expect("non-zero");
    let ctx_cap_231 = ContextCapabilityBits::new(231).expect("non-zero");
    let data: BTreeMap<String, String> = [("topic".to_owned(), "rust".to_owned())]
        .into_iter()
        .collect();

    let credential = Box::new(JoinAccountCredential {
        genesis: AccountGenesis::new(PublicKey::from([0x77; 32])),
        chain: Vec::new(),
        statement: DeviceCert {
            account: member,
            device: DeviceId::from([0x88; 32]),
            sign_pk: PublicKey::from([0x99; 32]),
            kem_pk: KemPublicKey::from([0xaa; 32]),
            key_epoch: 0,
            device_epoch: 1,
            signature: [0xbb; 64],
        },
    });

    let group_vector = |name, op: GroupOp, bytes_hex, op_hash_hex| DelegableVector {
        name,
        kind: GovernanceOpKind::Group,
        bytes: group_delegable_bytes(&op),
        bytes_hex,
        op_hash_hex,
    };
    let root_vector = |name, op: RootOp, bytes_hex, op_hash_hex| DelegableVector {
        name,
        kind: GovernanceOpKind::Root,
        bytes: root_delegable_bytes(&op),
        bytes_hex,
        op_hash_hex,
    };

    let vectors = [
        group_vector(
            "MemberRemoved (delegable form)",
            GroupOp::MemberRemoved {
                member,
                expected_group_state_hash: [0x42; 32],
                expected_context_state_hashes: vec![(context, [0x42; 32])],
            },
            "024444444444444444444444444444444444444444444444444444444444444444000000000000000000000000000000000000000000000000000000000000000000000000",
            "fdc08345155864f7a9481194c7f12b313994b1a95955ee0969422f0147ab2779",
        ),
        group_vector(
            "MemberLeft (delegable form)",
            GroupOp::MemberLeft {
                member,
                expected_group_state_hash: [0x42; 32],
                expected_context_state_hashes: vec![(context, [0x42; 32])],
            },
            "034444444444444444444444444444444444444444444444444444444444444444000000000000000000000000000000000000000000000000000000000000000000000000",
            "05392aaa36e0e7d08fc9150536d1c9906cd6f5445b0afb6cedec2f03938c6b3d",
        ),
        group_vector(
            "MemberRoleSet Admin",
            GroupOp::MemberRoleSet {
                member,
                role: GroupMemberRole::Admin,
            },
            "04444444444444444444444444444444444444444444444444444444444444444400",
            "79a70a143fb990f2f92478812b0c142ec8e94a75769d415b3e3bbf769168b2bb",
        ),
        group_vector(
            "MemberRoleSet ReadOnly",
            GroupOp::MemberRoleSet {
                member,
                role: GroupMemberRole::ReadOnly,
            },
            "04444444444444444444444444444444444444444444444444444444444444444402",
            "843978d9ed8c38856a33df18ed3ca9a1e60800b3c0985949ce0db831dd57c75d",
        ),
        group_vector(
            "MemberCapabilitySet 0b1",
            GroupOp::MemberCapabilitySet {
                member,
                capabilities: caps_one,
            },
            "05444444444444444444444444444444444444444444444444444444444444444401000000",
            "4266c138e10d5aa8837fcb1d1e7bb81b88c215d4ddea5f7ec413c7a87c995758",
        ),
        group_vector(
            "MemberCapabilitySet 231",
            GroupOp::MemberCapabilitySet {
                member,
                capabilities: caps_231,
            },
            "054444444444444444444444444444444444444444444444444444444444444444e7000000",
            "9225ce03739ea127393321b4b517b5badd05290131e865bd6bb2a5a34af57287",
        ),
        group_vector(
            "DefaultCapabilitiesSet 231",
            GroupOp::DefaultCapabilitiesSet {
                capabilities: caps_231,
            },
            "06e7000000",
            "0bc00f5f7627b34a104b6aa059887c2cf30f59f468711462176f35592fcf95fd",
        ),
        group_vector(
            "ContextDetached",
            GroupOp::ContextDetached {
                context_id: context,
            },
            "096666666666666666666666666666666666666666666666666666666666666666",
            "363e49ed17f870151deed61caa14f493fad3f4e1d9c4781f35b21b833a4f2bf3",
        ),
        group_vector(
            "SubgroupVisibilitySet Open",
            GroupOp::SubgroupVisibilitySet {
                mode: VisibilityMode::Open,
            },
            "0a00",
            "99d379e4656d5711132d0d44491446ab93480b6ad58bc216aba9358bf693d57b",
        ),
        group_vector(
            "SubgroupVisibilitySet Restricted",
            GroupOp::SubgroupVisibilitySet {
                mode: VisibilityMode::Restricted,
            },
            "0a01",
            "42b90a291fb2b5e5d8a5da5a1096facde5d34c2d3ad92903507e1709434020a1",
        ),
        group_vector(
            "GroupMetadataSet Some",
            GroupOp::GroupMetadataSet {
                name: Some("general".to_owned()),
                data: data.clone(),
            },
            "0b010700000067656e6572616c0100000005000000746f7069630400000072757374",
            "2d789be70265d50780fc8cac9b1c3ff847352fccd2ad36e76ffdc464134d4bc9",
        ),
        group_vector(
            "GroupMetadataSet None",
            GroupOp::GroupMetadataSet {
                name: None,
                data: BTreeMap::new(),
            },
            "0b0000000000",
            "4fc9cfd6f2af5690ff47fc685c8ef1408c2c49f41aac2a2a8151223e5dfe2e1d",
        ),
        group_vector(
            "MemberMetadataSet Some",
            GroupOp::MemberMetadataSet {
                member,
                name: Some("alice".to_owned()),
                data: data.clone(),
            },
            "0c44444444444444444444444444444444444444444444444444444444444444440105000000616c6963650100000005000000746f7069630400000072757374",
            "58a5290db3c57d34bb40679071c4739eff0ff81e0e728ff98d707ed570c1eeab",
        ),
        group_vector(
            "MemberMetadataSet None",
            GroupOp::MemberMetadataSet {
                member,
                name: None,
                data: BTreeMap::new(),
            },
            "0c44444444444444444444444444444444444444444444444444444444444444440000000000",
            "d682f73eacf1b2b04aa85e1abde1b37c5e16399a5f41b2d385581a0c401db32e",
        ),
        group_vector(
            "ContextMetadataSet Some",
            GroupOp::ContextMetadataSet {
                context_id: context,
                name: Some("general".to_owned()),
                data,
            },
            "0d6666666666666666666666666666666666666666666666666666666666666666010700000067656e6572616c0100000005000000746f7069630400000072757374",
            "81768a47949bec9245da34acbfc9d90ff8152e65df38026e99691f2d0f4e3f86",
        ),
        group_vector(
            "ContextMetadataSet None",
            GroupOp::ContextMetadataSet {
                context_id: context,
                name: None,
                data: BTreeMap::new(),
            },
            "0d66666666666666666666666666666666666666666666666666666666666666660000000000",
            "4de9794df28cbcfb3fae1c960eaf4a8244b5f415a1a7506b963ec82c1bf1033d",
        ),
        group_vector(
            "ContextCapabilityGranted 0b1",
            GroupOp::ContextCapabilityGranted {
                context_id: context,
                member,
                capability: ctx_cap_one,
            },
            "106666666666666666666666666666666666666666666666666666666666666666444444444444444444444444444444444444444444444444444444444444444401",
            "af5093cfd553f9431d4c2c6eacc8c5ac90c709d520122fa5420982d2594f0ff1",
        ),
        group_vector(
            "ContextCapabilityRevoked 231",
            GroupOp::ContextCapabilityRevoked {
                context_id: context,
                member,
                capability: ctx_cap_231,
            },
            "1166666666666666666666666666666666666666666666666666666666666666664444444444444444444444444444444444444444444444444444444444444444e7",
            "1ba014f559da55169bdcb663f3cd79b5b6519aecf7a09dc5976b9c28e3cd94fd",
        ),
        root_vector(
            "GroupReparented",
            RootOp::GroupReparented {
                child_group_id: group,
                new_parent_id: parent,
            },
            "0155555555555555555555555555555555555555555555555555555555555555551111111111111111111111111111111111111111111111111111111111111111",
            "a52c81b6b15c534a10db72373a4213f0ea75c7b99dbea1c93d510169b804d427",
        ),
        root_vector(
            "GroupDeleted (delegable form)",
            RootOp::GroupDeleted {
                root_group_id: group,
                cascade_group_ids: vec![parent],
                cascade_context_ids: vec![context],
            },
            "0255555555555555555555555555555555555555555555555555555555555555550000000000000000",
            "b2dd53fba1c8b83ef704de1fcc868cc68d4f74cd2a54cd1db4998213500fddd0",
        ),
        root_vector(
            "MemberJoinedOpen",
            RootOp::MemberJoinedOpen {
                member,
                group_id: group,
                account: credential,
            },
            "074444444444444444444444444444444444444444444444444444444444444444555555555555555555555555555555555555555555555555555555555555555502777777777777777777777777777777777777777777777777777777777777777700000000444444444444444444444444444444444444444444444444444444444444444488888888888888888888888888888888888888888888888888888888888888889999999999999999999999999999999999999999999999999999999999999999aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa0000000001000000bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "b9642890aae8b920d6bd9376e532423b6074becd9eb568843b2de025fd294192",
        ),
    ];

    for vector in &vectors {
        let op_hash = hex::encode(GovernanceWarrant::op_hash(vector.kind, &vector.bytes));
        assert_eq!(
            hex::encode(&vector.bytes),
            vector.bytes_hex,
            "{} bytes",
            vector.name
        );
        assert_eq!(op_hash, vector.op_hash_hex, "{} op hash", vector.name);
    }
}

/// The delegable form of a group's first `TargetApplicationSet`: variant 7,
/// `bytecode_id` cleared (the relay fills it with the blob id of the bundle it
/// resolves for `package@version`), then the application id, package and
/// version the member signs. Pinned with its `op_hash` so mero-js can assert
/// the exact bytes it asks a member to sign.
#[test]
fn delegable_target_application_set_vector_is_stable() {
    use calimero_account::{GovernanceOpKind, GovernanceWarrant};
    use calimero_context_config::types::BytecodeId;
    use calimero_primitives::application::ApplicationId;

    let published = GroupOp::TargetApplicationSet {
        bytecode_id: BytecodeId::from([0x77; 32]),
        target_application_id: ApplicationId::from([0x88; 32]),
        package: "com.example.app".to_owned(),
        version: "1.2.3".to_owned(),
    };
    let form = published
        .delegable_form()
        .expect("a first choice is delegable");
    let bytes = borsh::to_vec(&form).expect("encode");
    let signed_form = GroupOp::TargetApplicationSet {
        bytecode_id: BytecodeId::from([0u8; 32]),
        target_application_id: ApplicationId::from([0x88; 32]),
        package: "com.example.app".to_owned(),
        version: "1.2.3".to_owned(),
    };
    assert_eq!(
        bytes,
        borsh::to_vec(&signed_form).expect("encode"),
        "only bytecode_id is the relay's to fill"
    );
    assert_eq!(
        borsh::to_vec(&form.delegable_form().expect("delegable")).expect("encode"),
        bytes,
        "the form is its own form, so what a member signs is a fixed point"
    );
    assert_eq!(hex::encode(&bytes), "07000000000000000000000000000000000000000000000000000000000000000088888888888888888888888888888888888888888888888888888888888888880f000000636f6d2e6578616d706c652e61707005000000312e322e33");
    assert_eq!(
        hex::encode(GovernanceWarrant::op_hash(GovernanceOpKind::Group, &bytes)),
        "904984c8f39e4172ea8864a65511faead68baa76af18d31e1d442a0b9fcb656b"
    );
}

#[test]
fn a_shared_writers_rotation_bounds_its_writer_sets() {
    use calimero_storage::address::Id;
    use calimero_storage::entities::OpMask;

    let sk = PrivateKey::random(&mut UnwrapErr(SysRng));
    let writers = |n: usize| -> BTreeMap<calimero_account::AccountId, OpMask> {
        (0..n)
            .map(|i| {
                let mut bytes = [0u8; 32];
                bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
                (calimero_account::AccountId::from(bytes), OpMask::FULL)
            })
            .collect()
    };
    let validate = |prior, new| {
        SignedGroupOp::sign(
            &sk,
            sample_group_id(),
            vec![],
            1,
            GroupOp::SharedWritersRotated {
                context_id: ContextId::from([0xC7; 32]),
                cell: Id::new([0xC0; 32]),
                prior,
                nonce: 1,
                new,
            },
        )
        .expect("sign")
        .validate()
    };
    validate(writers(256), writers(256)).expect("at the bound");
    assert!(matches!(
        validate(writers(257), writers(1)),
        Err(GovernanceError::Bounds(_))
    ));
    assert!(matches!(
        validate(writers(1), writers(257)),
        Err(GovernanceError::Bounds(_))
    ));
    assert!(matches!(
        validate(writers(0), writers(1)),
        Err(GovernanceError::Bounds(_))
    ));
}

#[test]
fn a_shared_writers_rotation_is_neither_delegable_nor_owner_level() {
    let op = GroupOp::SharedWritersRotated {
        context_id: ContextId::from([0xC7; 32]),
        cell: calimero_storage::address::Id::new([0xC0; 32]),
        prior: BTreeMap::new(),
        nonce: 1,
        new: BTreeMap::new(),
    };
    assert!(op.delegable_form().is_none(), "a relay cannot publish it");
    assert_eq!(op.owner_op_kind(), None, "it needs no owner proof");
    let bytes = borsh::to_vec(&op).expect("encode");
    assert_eq!(bytes[0], 43, "it follows RootGuarded (42)");
}

/// Wrapper tags stacked by the nesting tests: about what one 64 KiB gossip
/// message holds.
const NESTED_WRAPPERS: usize = 60_000;

/// A stack far below a node thread's. A decode that took a frame per wrapper
/// would overflow it and abort the test binary.
const SMALL_STACK: usize = 256 * 1024;

/// Decodes `bytes` as `T` on a thread with [`SMALL_STACK`].
fn decode_on_small_stack<T: BorshDeserialize + Send + 'static>(
    bytes: Vec<u8>,
) -> Result<T, String> {
    std::thread::Builder::new()
        .stack_size(SMALL_STACK)
        .spawn(move || borsh::from_slice::<T>(&bytes).map_err(|err| err.to_string()))
        .expect("spawn the decode thread")
        .join()
        .expect("the decode thread finished")
}

fn assert_nesting_refused<T: BorshDeserialize + Send + 'static>(what: &str, bytes: Vec<u8>) {
    let Err(err) = decode_on_small_stack::<T>(bytes) else {
        panic!("{what}: a chain of wrappers decoded");
    };
    assert!(
        err.contains("may not carry another wrapper"),
        "{what}: refused for another reason: {err}"
    );
}

#[test]
fn a_chain_of_group_op_wrappers_is_refused_without_exhausting_the_stack() {
    for (what, pattern) in [
        ("OnBehalf", vec![40u8]),
        ("RootGuarded", vec![42u8]),
        ("alternating", vec![40u8, 42]),
    ] {
        let bytes = pattern
            .iter()
            .copied()
            .cycle()
            .take(NESTED_WRAPPERS)
            .collect();
        assert_nesting_refused::<GroupOp>(what, bytes);
    }
}

#[test]
fn a_chain_of_root_op_wrappers_is_refused_without_exhausting_the_stack() {
    for (what, pattern) in [
        ("OnBehalf", vec![11u8]),
        ("RootGuarded", vec![12u8]),
        ("alternating", vec![11u8, 12]),
    ] {
        // `NamespaceOp::Root` is tag 0.
        let bytes = std::iter::once(0u8)
            .chain(pattern.iter().copied().cycle().take(NESTED_WRAPPERS))
            .collect();
        assert_nesting_refused::<NamespaceOp>(what, bytes);
    }
}

/// The payload a peer gossips on `ns/<id>`, decoded the way the node decodes
/// it, before any signature or membership check.
#[test]
fn a_gossiped_chain_of_wrappers_is_refused_without_exhausting_the_stack() {
    let signer = PrivateKey::from([0x31; 32]).public_key();
    let mut bytes = Vec::new();
    bytes.push(0u8); // NamespaceTopicMsg::Op
    bytes.extend(borsh::to_vec(&SIGNED_NAMESPACE_OP_SCHEMA_VERSION).expect("version"));
    bytes.extend(borsh::to_vec(&NamespaceId::from([0x11; 32])).expect("namespace"));
    bytes.extend(borsh::to_vec(&Vec::<[u8; 32]>::new()).expect("parents"));
    bytes.extend(borsh::to_vec(&signer).expect("signer"));
    bytes.extend(borsh::to_vec(&1u64).expect("nonce"));
    bytes.push(0u8); // NamespaceOp::Root
    bytes.extend(std::iter::repeat_n(11u8, NESTED_WRAPPERS)); // RootOp::OnBehalf
    assert!(bytes.len() <= 64 * 1024, "fits one gossip message");

    assert_nesting_refused::<crate::wire::NamespaceTopicMsg>("NamespaceTopicMsg", bytes);
}

/// A wrapper an encoder did nest, one level deep, is refused on decode, so the
/// tags the decoder refuses are the wrappers' real ordinals.
#[test]
fn a_wrapper_carrying_a_wrapper_does_not_decode() {
    let inner = GroupOp::GroupDelete;
    let proof = owner_proof(
        7,
        [0x22; 32],
        OwnerOpKind::GroupDelete,
        inner.owner_op_digest().expect("digest"),
        0,
    );
    let single = GroupOp::RootGuarded {
        op: Box::new(inner),
        proof: Box::new(proof.clone()),
    };
    let bytes = borsh::to_vec(&single).expect("encode");
    let decoded: GroupOp = borsh::from_slice(&bytes).expect("one wrapper decodes");
    assert_eq!(borsh::to_vec(&decoded).expect("re-encode"), bytes);

    let nested = GroupOp::RootGuarded {
        op: Box::new(single),
        proof: Box::new(proof.clone()),
    };
    let err = borsh::from_slice::<GroupOp>(&borsh::to_vec(&nested).expect("encode"))
        .expect_err("a wrapper in a wrapper");
    assert!(err.to_string().contains("may not carry another wrapper"));

    let root_single = RootOp::RootGuarded {
        op: Box::new(RootOp::AdminChanged {
            new_admin: AccountId::from([0x44; 32]),
        }),
        proof: Box::new(proof.clone()),
    };
    let bytes = borsh::to_vec(&root_single).expect("encode");
    let decoded: RootOp = borsh::from_slice(&bytes).expect("one wrapper decodes");
    assert_eq!(borsh::to_vec(&decoded).expect("re-encode"), bytes);

    let root_nested = RootOp::RootGuarded {
        op: Box::new(root_single),
        proof: Box::new(proof),
    };
    let err = borsh::from_slice::<RootOp>(&borsh::to_vec(&root_nested).expect("encode"))
        .expect_err("a wrapper in a wrapper");
    assert!(err.to_string().contains("may not carry another wrapper"));
}
