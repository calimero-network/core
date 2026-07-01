use super::*;

use calimero_primitives::identity::{PrivateKey, PublicKey};
use rand::rngs::OsRng;

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
// validation — the frozen bytes are stable across builds.
// ---------------------------------------------------------------------------

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

/// GroupOp ordinal 7 — UpgradePolicySet { policy: LazyOnAccess(1) }
const GOLDEN_GROUP_OP_UPGRADE_POLICY_SET: &[u8] = &[
    7, // discriminant
    1, // UpgradePolicy::LazyOnAccess (ordinal 1, the Default)
];

/// GroupOp ordinal 8 — TargetApplicationSet { app_key: [0;32].into(), target: [0;32] }
const GOLDEN_GROUP_OP_TARGET_APPLICATION_SET: &[u8] = &[
    8, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // app_key [0u8;32]
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // target_application_id [0u8;32]
];

/// GroupOp ordinal 9 — ContextRegistered (all empty/zero fields)
const GOLDEN_GROUP_OP_CONTEXT_REGISTERED: &[u8] = &[
    9, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // context_id
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // application_id
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // blob_id
    0, 0, 0, 0, // source String (len = 0)
    0, // service_name = None
];

/// GroupOp ordinal 10 — ContextDetached { context_id: [0;32] }
const GOLDEN_GROUP_OP_CONTEXT_DETACHED: &[u8] = &[
    10, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // context_id
];

/// GroupOp ordinal 11 — SubgroupVisibilitySet { mode: 0 }
const GOLDEN_GROUP_OP_SUBGROUP_VISIBILITY_SET: &[u8] = &[
    11, // discriminant
    0,  // mode = 0
];

/// GroupOp ordinal 12 — GroupMetadataSet { name: None, data: {} }
const GOLDEN_GROUP_OP_GROUP_METADATA_SET: &[u8] = &[
    12, // discriminant
    0,  // name = None
    0, 0, 0, 0, // data BTreeMap len = 0
];

/// GroupOp ordinal 13 — MemberMetadataSet { member: [0;32], name: None, data: {} }
const GOLDEN_GROUP_OP_MEMBER_METADATA_SET: &[u8] = &[
    13, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // member
    0, // name = None
    0, 0, 0, 0, // data len = 0
];

/// GroupOp ordinal 14 — ContextMetadataSet { context_id: [0;32], name: None, data: {} }
const GOLDEN_GROUP_OP_CONTEXT_METADATA_SET: &[u8] = &[
    14, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // context_id
    0, // name = None
    0, 0, 0, 0, // data len = 0
];

/// GroupOp ordinal 15 — GroupDelete (no fields; full encoding = discriminant only)
const GOLDEN_GROUP_OP_GROUP_DELETE: &[u8] = &[15];

/// GroupOp ordinal 16 — GroupMigrationSet { migration: None }
const GOLDEN_GROUP_OP_GROUP_MIGRATION_SET: &[u8] = &[
    16, // discriminant
    0,  // migration = None
];

/// GroupOp ordinal 17 — ContextCapabilityGranted { context_id: [0;32], member: [0;32], capability: 1 }
const GOLDEN_GROUP_OP_CONTEXT_CAPABILITY_GRANTED: &[u8] = &[
    17, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // context_id
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // member
    1, // capability (must be non-zero: ContextCapabilityBits rejects 0 on the wire)
];

/// GroupOp ordinal 18 — ContextCapabilityRevoked (same shape as Granted)
const GOLDEN_GROUP_OP_CONTEXT_CAPABILITY_REVOKED: &[u8] = &[
    18, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // context_id
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // member
    1, // capability (must be non-zero: ContextCapabilityBits rejects 0 on the wire)
];

/// GroupOp ordinal 19 — TeeAdmissionPolicySet (6 empty Vec<String> + accept_mock=false)
const GOLDEN_GROUP_OP_TEE_ADMISSION_POLICY_SET: &[u8] = &[
    19, // discriminant
    0, 0, 0, 0, // allowed_mrtd vec len = 0
    0, 0, 0, 0, // allowed_rtmr0 vec len = 0
    0, 0, 0, 0, // allowed_rtmr1 vec len = 0
    0, 0, 0, 0, // allowed_rtmr2 vec len = 0
    0, 0, 0, 0, // allowed_rtmr3 vec len = 0
    0, 0, 0, 0, // allowed_tcb_statuses vec len = 0
    0, // accept_mock = false
];

/// GroupOp ordinal 20 — MemberJoinedViaTeeAttestation (all empty/zero)
const GOLDEN_GROUP_OP_MEMBER_JOINED_VIA_TEE: &[u8] = &[
    20, // discriminant
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

/// GroupOp ordinal 21 — MemberSetAutoFollow { target: [0;32], auto_follow_contexts: false, auto_follow_subgroups: false }
const GOLDEN_GROUP_OP_MEMBER_SET_AUTO_FOLLOW: &[u8] = &[
    21, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // target
    0, // auto_follow_contexts = false
    0, // auto_follow_subgroups = false
];

/// GroupOp ordinal 22 — TransferOwnership { new_owner: [0;32] }
const GOLDEN_GROUP_OP_TRANSFER_OWNERSHIP: &[u8] = &[
    22, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // new_owner
];

/// GroupOp ordinal 23 — CascadeTargetApplicationSet { from_app_key: [0;32].into(), app_key: [0;32].into(), target: [0;32] }
const GOLDEN_GROUP_OP_CASCADE_TARGET_APPLICATION_SET: &[u8] = &[
    23, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // from_app_key
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // app_key
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // target_application_id
];

/// GroupOp ordinal 24 — CascadeGroupMigrationSet { from_app_key: [0;32].into(), migration: None }
const GOLDEN_GROUP_OP_CASCADE_GROUP_MIGRATION_SET: &[u8] = &[
    24, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // from_app_key
    0, // migration = None
];

/// Borsh encoding of `HybridTimestamp::zero()` — 24 bytes.
///
/// Verified by `hlc_zero_golden_bytes_are_self_consistent` below; kept as a
/// named constant so both the CascadeUpgrade golden vector and the verifier
/// test reference the same source of truth.
const GOLDEN_HLC_ZERO: &[u8] = &[
    0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

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
    assert_eq!(
        &GOLDEN_GROUP_OP_CASCADE_UPGRADE[GOLDEN_GROUP_OP_CASCADE_UPGRADE.len() - 24..],
        GOLDEN_HLC_ZERO,
        "HLC bytes embedded in GOLDEN_GROUP_OP_CASCADE_UPGRADE diverged from \
         GOLDEN_HLC_ZERO — update both constants together"
    );
}

/// GroupOp ordinal 25 — CascadeUpgrade (all zero fields; HybridTimestamp::zero() via GOLDEN_HLC_ZERO)
const GOLDEN_GROUP_OP_CASCADE_UPGRADE: &[u8] = &[
    25, // discriminant
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // from_app_key
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // app_key
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // target_application_id
    0, // migration = None
    // HybridTimestamp::zero() — same bytes as GOLDEN_HLC_ZERO (verified by hlc_zero_golden_bytes_are_self_consistent)
    0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

#[test]
fn group_op_discriminants_are_golden() {
    // Decode each frozen byte vector and verify the correct variant is returned.
    // A mid-enum insertion shifts ordinals so the wrong variant is decoded (or
    // decoding fails). Failures are accumulated so ALL mismatches are reported
    // in one run rather than stopping at the first panic.
    let mut failures: Vec<String> = Vec::new();
    macro_rules! check_group_op {
        ($golden:expr, $pat:pat, $discriminant:expr) => {{
            match borsh::from_slice::<GroupOp>($golden) {
                Err(e) => failures.push(format!(
                    "GroupOp ordinal {}: decode failed: {e}",
                    $discriminant
                )),
                Ok(decoded) if !matches!(decoded, $pat) => failures.push(format!(
                    "GroupOp ordinal {}: decoded as {:?} — \
                     a variant was inserted before this one, shifting its ordinal",
                    $discriminant, decoded
                )),
                Ok(decoded) => {
                    let reencoded = borsh::to_vec(&decoded).expect("re-encode");
                    if reencoded.len() != $golden.len() {
                        failures.push(format!(
                            "GroupOp ordinal {}: golden has {} bytes but re-encoding \
                             produced {} — golden vector has unexpected trailing bytes",
                            $discriminant,
                            $golden.len(),
                            reencoded.len()
                        ));
                    }
                }
            }
        }};
    }

    check_group_op!(GOLDEN_GROUP_OP_NOOP, GroupOp::Noop, 0);
    check_group_op!(GOLDEN_GROUP_OP_MEMBER_ADDED, GroupOp::MemberAdded { .. }, 1);
    check_group_op!(
        GOLDEN_GROUP_OP_MEMBER_REMOVED,
        GroupOp::MemberRemoved { .. },
        2
    );
    check_group_op!(GOLDEN_GROUP_OP_MEMBER_LEFT, GroupOp::MemberLeft { .. }, 3);
    check_group_op!(
        GOLDEN_GROUP_OP_MEMBER_ROLE_SET,
        GroupOp::MemberRoleSet { .. },
        4
    );
    check_group_op!(
        GOLDEN_GROUP_OP_MEMBER_CAPABILITY_SET,
        GroupOp::MemberCapabilitySet { .. },
        5
    );
    check_group_op!(
        GOLDEN_GROUP_OP_DEFAULT_CAPABILITIES_SET,
        GroupOp::DefaultCapabilitiesSet { .. },
        6
    );
    check_group_op!(
        GOLDEN_GROUP_OP_UPGRADE_POLICY_SET,
        GroupOp::UpgradePolicySet { .. },
        7
    );
    check_group_op!(
        GOLDEN_GROUP_OP_TARGET_APPLICATION_SET,
        GroupOp::TargetApplicationSet { .. },
        8
    );
    check_group_op!(
        GOLDEN_GROUP_OP_CONTEXT_REGISTERED,
        GroupOp::ContextRegistered { .. },
        9
    );
    check_group_op!(
        GOLDEN_GROUP_OP_CONTEXT_DETACHED,
        GroupOp::ContextDetached { .. },
        10
    );
    check_group_op!(
        GOLDEN_GROUP_OP_SUBGROUP_VISIBILITY_SET,
        GroupOp::SubgroupVisibilitySet { .. },
        11
    );
    check_group_op!(
        GOLDEN_GROUP_OP_GROUP_METADATA_SET,
        GroupOp::GroupMetadataSet { .. },
        12
    );
    check_group_op!(
        GOLDEN_GROUP_OP_MEMBER_METADATA_SET,
        GroupOp::MemberMetadataSet { .. },
        13
    );
    check_group_op!(
        GOLDEN_GROUP_OP_CONTEXT_METADATA_SET,
        GroupOp::ContextMetadataSet { .. },
        14
    );
    check_group_op!(GOLDEN_GROUP_OP_GROUP_DELETE, GroupOp::GroupDelete, 15);
    check_group_op!(
        GOLDEN_GROUP_OP_GROUP_MIGRATION_SET,
        GroupOp::GroupMigrationSet { .. },
        16
    );
    check_group_op!(
        GOLDEN_GROUP_OP_CONTEXT_CAPABILITY_GRANTED,
        GroupOp::ContextCapabilityGranted { .. },
        17
    );
    check_group_op!(
        GOLDEN_GROUP_OP_CONTEXT_CAPABILITY_REVOKED,
        GroupOp::ContextCapabilityRevoked { .. },
        18
    );
    check_group_op!(
        GOLDEN_GROUP_OP_TEE_ADMISSION_POLICY_SET,
        GroupOp::TeeAdmissionPolicySet { .. },
        19
    );
    check_group_op!(
        GOLDEN_GROUP_OP_MEMBER_JOINED_VIA_TEE,
        GroupOp::MemberJoinedViaTeeAttestation { .. },
        20
    );
    check_group_op!(
        GOLDEN_GROUP_OP_MEMBER_SET_AUTO_FOLLOW,
        GroupOp::MemberSetAutoFollow { .. },
        21
    );
    check_group_op!(
        GOLDEN_GROUP_OP_TRANSFER_OWNERSHIP,
        GroupOp::TransferOwnership { .. },
        22
    );
    check_group_op!(
        GOLDEN_GROUP_OP_CASCADE_TARGET_APPLICATION_SET,
        GroupOp::CascadeTargetApplicationSet { .. },
        23
    );
    check_group_op!(
        GOLDEN_GROUP_OP_CASCADE_GROUP_MIGRATION_SET,
        GroupOp::CascadeGroupMigrationSet { .. },
        24
    );
    check_group_op!(
        GOLDEN_GROUP_OP_CASCADE_UPGRADE,
        GroupOp::CascadeUpgrade { .. },
        25
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
/// Encoding: member (32 bytes) + SignedGroupOpenInvitation with a minimal
/// GroupInvitationFromAdmin (inviter_identity[0;32] + group_id[0;32] +
/// expiration_timestamp 0 (u64) + secret_salt[0;32] + invited_role 1 (u8))
/// + inviter_signature "" + application_id None + app_key None.
const GOLDEN_ROOT_OP_MEMBER_JOINED: &[u8] = &[
    0, // NamespaceOp::Root
    5, // RootOp::MemberJoined discriminant
    // member PublicKey [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // signed_invitation.invitation.inviter_identity [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // signed_invitation.invitation.group_id [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // signed_invitation.invitation.expiration_timestamp u64 = 0:
    0, 0, 0, 0, 0, 0, 0, 0, // signed_invitation.invitation.secret_salt [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // signed_invitation.invitation.invited_role u8 = 1:
    1, // signed_invitation.inviter_signature String len = 0:
    0, 0, 0, 0, // signed_invitation.application_id = None:
    0, // signed_invitation.app_key = None:
    0,
];

/// NamespaceOp::Root(RootOp::KeyDelivery) — RootOp ordinal 6
const GOLDEN_ROOT_OP_KEY_DELIVERY: &[u8] = &[
    0, // NamespaceOp::Root
    6, // RootOp::KeyDelivery discriminant
    // group_id [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // envelope.recipient [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // envelope.ephemeral_pk [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // envelope.nonce [0u8;12]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // envelope.ciphertext vec len = 0:
    0, 0, 0, 0,
];

/// NamespaceOp::Root(RootOp::MemberJoinedOpen) — RootOp ordinal 7
const GOLDEN_ROOT_OP_MEMBER_JOINED_OPEN: &[u8] = &[
    0, // NamespaceOp::Root
    7, // RootOp::MemberJoinedOpen discriminant
    // member [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // group_id [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// NamespaceOp::Root(RootOp::MemberJoinedAt) — RootOp ordinal 8
///
/// Same inner payload as MemberJoined plus joined_at u64 = 0 at the end.
const GOLDEN_ROOT_OP_MEMBER_JOINED_AT: &[u8] = &[
    0, // NamespaceOp::Root
    8, // RootOp::MemberJoinedAt discriminant
    // member [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // signed_invitation (same encoding as MemberJoined):
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // inviter_identity
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // group_id
    0, 0, 0, 0, 0, 0, 0, 0, // expiration_timestamp
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // secret_salt
    1, // invited_role = 1
    0, 0, 0, 0, // inviter_signature len = 0
    0, // application_id = None
    0, // app_key = None
    0, 0, 0, 0, 0, 0, 0, 0, // joined_at u64 = 0
];

/// NamespaceOp::Root(RootOp::NamespaceCreated) — RootOp ordinal 9
const GOLDEN_ROOT_OP_NAMESPACE_CREATED: &[u8] = &[
    0, // NamespaceOp::Root
    9, // RootOp::NamespaceCreated discriminant
    // founder [0u8;32]:
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

#[test]
fn root_op_discriminants_are_golden() {
    // bytes[0] = NamespaceOp::Root discriminant (always 0).
    // bytes[1] = RootOp variant discriminant (pinned here).
    // Failures are accumulated so ALL mismatches are reported in one run.
    let mut failures: Vec<String> = Vec::new();
    macro_rules! check_root_op {
        ($golden:expr, $pat:pat, $root_discriminant:expr) => {{
            match borsh::from_slice::<NamespaceOp>($golden) {
                Err(e) => failures.push(format!(
                    "RootOp ordinal {}: decode failed: {e}",
                    $root_discriminant
                )),
                Ok(decoded) if !matches!(decoded, NamespaceOp::Root($pat)) => {
                    failures.push(format!(
                        "RootOp ordinal {}: decoded as {:?} — \
                         a variant was inserted before this one, shifting its ordinal",
                        $root_discriminant, decoded
                    ))
                }
                Ok(decoded) => {
                    let reencoded = borsh::to_vec(&decoded).expect("re-encode");
                    if reencoded.len() != $golden.len() {
                        failures.push(format!(
                            "RootOp ordinal {}: golden has {} bytes but re-encoding \
                             produced {} — golden vector has unexpected trailing bytes",
                            $root_discriminant,
                            $golden.len(),
                            reencoded.len()
                        ));
                    }
                }
            }
        }};
    }

    check_root_op!(GOLDEN_ROOT_OP_GROUP_CREATED, RootOp::GroupCreated { .. }, 0);
    check_root_op!(
        GOLDEN_ROOT_OP_GROUP_REPARENTED,
        RootOp::GroupReparented { .. },
        1
    );
    check_root_op!(GOLDEN_ROOT_OP_GROUP_DELETED, RootOp::GroupDeleted { .. }, 2);
    check_root_op!(GOLDEN_ROOT_OP_ADMIN_CHANGED, RootOp::AdminChanged { .. }, 3);
    check_root_op!(
        GOLDEN_ROOT_OP_POLICY_UPDATED,
        RootOp::PolicyUpdated { .. },
        4
    );
    check_root_op!(GOLDEN_ROOT_OP_MEMBER_JOINED, RootOp::MemberJoined { .. }, 5);
    check_root_op!(GOLDEN_ROOT_OP_KEY_DELIVERY, RootOp::KeyDelivery { .. }, 6);
    check_root_op!(
        GOLDEN_ROOT_OP_MEMBER_JOINED_OPEN,
        RootOp::MemberJoinedOpen { .. },
        7
    );
    check_root_op!(
        GOLDEN_ROOT_OP_MEMBER_JOINED_AT,
        RootOp::MemberJoinedAt { .. },
        8
    );
    check_root_op!(
        GOLDEN_ROOT_OP_NAMESPACE_CREATED,
        RootOp::NamespaceCreated { .. },
        9
    );

    assert!(
        failures.is_empty(),
        "RootOp discriminant golden failures ({} total):\n{}",
        failures.len(),
        failures.join("\n")
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
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);
    let member = PrivateKey::random(&mut rng).public_key();

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
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);
    let other = PrivateKey::random(&mut rng);
    let member = PrivateKey::random(&mut rng).public_key();

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
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);
    let member = PrivateKey::random(&mut rng).public_key();

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

#[test]
fn replay_distinct_content_hash() {
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);
    let member = PrivateKey::random(&mut rng).public_key();

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
    let mut rng = OsRng;
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
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);

    let op = SignedNamespaceOp::sign(
        &sk,
        sample_namespace_id(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: sample_group_id(),
            parent_id: sample_namespace_id().to_bytes().into(),
            restricted: true,
        }),
    )
    .expect("sign");

    op.verify_signature().expect("verify");
    assert!(op.group_id().is_none());
}

#[test]
fn namespace_op_sign_verify_group() {
    let mut rng = OsRng;
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
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);

    let mut op = SignedNamespaceOp::sign(
        &sk,
        sample_namespace_id(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::AdminChanged {
            new_admin: sk.public_key(),
        }),
    )
    .expect("sign");

    op.nonce = 999;
    assert!(op.verify_signature().is_err());
}

#[test]
fn namespace_op_content_hash_distinct() {
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);

    let op1 = SignedNamespaceOp::sign(
        &sk,
        sample_namespace_id(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: sample_group_id(),
            parent_id: sample_namespace_id().to_bytes().into(),
            restricted: true,
        }),
    )
    .expect("sign");

    let op2 = SignedNamespaceOp::sign(
        &sk,
        sample_namespace_id(),
        vec![],
        2,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: sample_group_id(),
            parent_id: sample_namespace_id().to_bytes().into(),
            restricted: true,
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
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);
    let pk = sk.public_key();
    let s = SignableNamespaceOp {
        version: SIGNED_NAMESPACE_OP_SCHEMA_VERSION,
        namespace_id: sample_namespace_id(),
        parent_op_hashes: vec![],
        signer: pk,
        nonce: 42,
        op: NamespaceOp::Root(RootOp::GroupCreated {
            group_id: sample_group_id(),
            parent_id: sample_namespace_id().to_bytes().into(),
            restricted: true,
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
fn cascade_target_application_set_sign_verify() {
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);

    let op = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::CascadeTargetApplicationSet {
            from_app_key: [9u8; 32].into(),
            app_key: [10u8; 32].into(),
            target_application_id: sample_application_id(0x42),
        },
    )
    .expect("sign");

    op.verify_signature().expect("verify");
    assert_eq!(
        op.op.op_kind_label(),
        "cascade_target_application_set",
        "op_kind_label must distinguish cascade variant for metrics"
    );
}

#[test]
fn cascade_group_migration_set_sign_verify() {
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);

    let op = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::CascadeGroupMigrationSet {
            from_app_key: [9u8; 32].into(),
            migration: Some(b"migrate_v1_to_v2".to_vec()),
        },
    )
    .expect("sign");

    op.verify_signature().expect("verify");
    assert_eq!(
        op.op.op_kind_label(),
        "cascade_group_migration_set",
        "op_kind_label must distinguish cascade migration variant for metrics"
    );
}

#[test]
fn cascade_target_distinct_from_single_group_target() {
    // A cascade op and a non-cascade op with the same new app_key/target
    // must produce DIFFERENT content hashes -- otherwise replay/dedup
    // would conflate the two distinct governance intents.
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);
    let new_app_key = [11u8; 32];
    let target = sample_application_id(0x77);

    let single = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::TargetApplicationSet {
            app_key: new_app_key.into(),
            target_application_id: target,
        },
    )
    .expect("sign");

    let cascade = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::CascadeTargetApplicationSet {
            from_app_key: [9u8; 32].into(),
            app_key: new_app_key.into(),
            target_application_id: target,
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
fn cascade_target_from_app_key_changes_hash() {
    // The Borsh-discriminant guarantees distinctness from the
    // single-group variant (covered by
    // `cascade_target_distinct_from_single_group_target`). This test
    // covers the stronger invariant: `from_app_key` is itself part of
    // the signed bytes, so two cascade ops that agree on every field
    // EXCEPT `from_app_key` must still hash differently. Otherwise a
    // refactor that accidentally collapses `from_app_key` (e.g. by
    // defaulting it or excluding it from signable bytes) would silently
    // break dedup of intent-different cascades.
    let mut rng = OsRng;
    let sk = PrivateKey::random(&mut rng);
    let new_app_key = [11u8; 32];
    let target = sample_application_id(0x77);

    let a = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::CascadeTargetApplicationSet {
            from_app_key: [9u8; 32].into(),
            app_key: new_app_key.into(),
            target_application_id: target,
        },
    )
    .expect("sign");

    let b = SignedGroupOp::sign(
        &sk,
        sample_group_id(),
        vec![],
        1,
        GroupOp::CascadeTargetApplicationSet {
            from_app_key: [8u8; 32].into(), // only this differs
            app_key: new_app_key.into(),
            target_application_id: target,
        },
    )
    .expect("sign");

    assert_ne!(
        a.content_hash().expect("hash a"),
        b.content_hash().expect("hash b"),
        "from_app_key must be covered by the signed content hash"
    );
}

#[test]
fn cascade_target_application_set_borsh_round_trip() {
    // Explicit wire-format round-trip for the new variant. The
    // sign/verify tests above implicitly exercise serialization (sign
    // hashes the Borsh bytes; verify rebuilds them) but do not assert
    // that field values survive a standalone serialize -> deserialize
    // round trip on the GroupOp itself. A future enum reordering that
    // shifts variant tags would silently change which variant a stored
    // op decodes as; this guards against that by asserting field
    // equality after a round trip.
    let original = GroupOp::CascadeTargetApplicationSet {
        from_app_key: [9u8; 32].into(),
        app_key: [10u8; 32].into(),
        target_application_id: sample_application_id(0x42),
    };

    let bytes = borsh::to_vec(&original).expect("serialize");
    let decoded: GroupOp = borsh::from_slice(&bytes).expect("deserialize");

    match decoded {
        GroupOp::CascadeTargetApplicationSet {
            from_app_key,
            app_key,
            target_application_id,
        } => {
            assert_eq!(from_app_key.to_bytes(), [9u8; 32]);
            assert_eq!(app_key.to_bytes(), [10u8; 32]);
            assert_eq!(target_application_id, sample_application_id(0x42));
        }
        other => panic!("expected CascadeTargetApplicationSet, got {other:?}"),
    }
}

#[test]
fn cascade_group_migration_set_borsh_round_trip() {
    // Symmetric round-trip guard for the migration variant.
    let original = GroupOp::CascadeGroupMigrationSet {
        from_app_key: [9u8; 32].into(),
        migration: Some(b"migrate_v1_to_v2".to_vec()),
    };

    let bytes = borsh::to_vec(&original).expect("serialize");
    let decoded: GroupOp = borsh::from_slice(&bytes).expect("deserialize");

    match decoded {
        GroupOp::CascadeGroupMigrationSet {
            from_app_key,
            migration,
        } => {
            assert_eq!(from_app_key.to_bytes(), [9u8; 32]);
            assert_eq!(migration.as_deref(), Some(b"migrate_v1_to_v2".as_ref()));
        }
        other => panic!("expected CascadeGroupMigrationSet, got {other:?}"),
    }

    // Also cover migration = None.
    let original_none = GroupOp::CascadeGroupMigrationSet {
        from_app_key: [0u8; 32].into(),
        migration: None,
    };
    let bytes_none = borsh::to_vec(&original_none).expect("serialize none");
    let decoded_none: GroupOp = borsh::from_slice(&bytes_none).expect("deserialize none");
    match decoded_none {
        GroupOp::CascadeGroupMigrationSet {
            from_app_key,
            migration,
        } => {
            assert_eq!(from_app_key.to_bytes(), [0u8; 32]);
            assert!(migration.is_none());
        }
        other => panic!("expected CascadeGroupMigrationSet, got {other:?}"),
    }
}

// --- CascadeUpgrade wire-format back-compat (schema v7) ---

#[test]
fn cascade_upgrade_back_compat_discriminant_fixed() {
    // `CascadeUpgrade` is the LAST variant of `GroupOp`, so its Borsh
    // discriminant must stay fixed at ordinal 25. This is a GOLDEN
    // byte-vector guard: the bytes below were produced by the enum at the
    // v7 layout (CascadeUpgrade at ordinal 25, its leading discriminant
    // byte). We decode these EXTERNALLY-FIXED bytes with the CURRENT enum —
    // we never re-encode them here. A same-binary serialize -> deserialize
    // round-trip would NOT catch a mid-enum insertion, because both sides
    // would use the shifted layout and still agree. Decoding frozen bytes is
    // what actually catches it: insert a variant in the MIDDLE of `GroupOp`
    // and CascadeUpgrade's ordinal shifts off 25, so byte `25` here decodes
    // as a DIFFERENT variant (or fails).
    //
    // Golden encoding of:
    //   GroupOp::CascadeUpgrade {
    //       from_app_key: [3u8; 32].into(),
    //       app_key: [4u8; 32].into(),
    //       target_application_id: sample_application_id(5),
    //       migration: Some(b"migrate".to_vec()),
    //       cascade_hlc: HybridTimestamp::zero(),
    //   }
    const GOLDEN_CASCADE_UPGRADE: &[u8] = &[
        25, // <- CascadeUpgrade's fixed Borsh discriminant (ordinal 25)
        3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
        3, 3, // from_app_key
        4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
        4, 4, // app_key
        5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 250, // target_application_id = sample_application_id(5)
        1, 7, 0, 0, 0, 109, 105, 103, 114, 97, 116, 101, // migration = Some("migrate")
        0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, // cascade_hlc = HybridTimestamp::zero()
    ];

    // Up-front: the leading discriminant byte must equal CascadeUpgrade's
    // known ordinal, so a mid-enum insertion (which shifts it) is caught.
    assert_eq!(
        GOLDEN_CASCADE_UPGRADE[0], 25,
        "CascadeUpgrade's Borsh discriminant must stay at ordinal 25; a \
         changed leading byte means a prior variant moved"
    );

    let decoded: GroupOp =
        borsh::from_slice(GOLDEN_CASCADE_UPGRADE).expect("decode frozen CascadeUpgrade bytes");
    match decoded {
        GroupOp::CascadeUpgrade {
            from_app_key,
            app_key,
            target_application_id,
            migration,
            cascade_hlc,
        } => {
            assert_eq!(from_app_key.to_bytes(), [3u8; 32]);
            assert_eq!(app_key.to_bytes(), [4u8; 32]);
            assert_eq!(target_application_id, sample_application_id(5));
            assert_eq!(migration, Some(b"migrate".to_vec()));
            assert_eq!(cascade_hlc, HybridTimestamp::zero());
        }
        other => panic!(
            "frozen CascadeUpgrade bytes (discriminant 25) decoded as {other:?}; a \
             variant was inserted mid-enum, shifting prior variant tags"
        ),
    }
}

// C5.S3b flag-day boundary: an op signed under the OLD schema must be REJECTED on
// the new build, never silently misparsed. The `version` field is the first borsh
// field, so it survives the layout change and the version check fires before any
// signable-bytes reconstruction. These tests pin that boundary so a future refactor
// can't re-open the window.
#[test]
fn pre_flag_day_group_op_version_is_rejected() {
    let signer = PrivateKey::random(&mut OsRng).public_key();
    // A struct-shaped op carrying a prior schema version (here v7, the last version
    // that still had `state_hash`). `verify_signature` must reject on the version
    // check alone — before touching the (here bogus) signature.
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
    let signer = PrivateKey::random(&mut OsRng).public_key();
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
    let signer = PrivateKey::random(&mut OsRng).public_key();
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

    fn sample_invitation() -> SignedGroupOpenInvitation {
        SignedGroupOpenInvitation {
            invitation: GroupInvitationFromAdmin {
                inviter_identity: SignerId::from([0xA1; 32]),
                group_id: ContextGroupId::from([0x22; 32]),
                expiration_timestamp: 1_900_000_000,
                secret_salt: [0x33; 32],
                invited_role: 1,
            },
            inviter_signature: "deadbeef".to_string(),
            application_id: Some([0x44; 32]),
            app_key: Some([0x55; 32]),
        }
    }

    fn signed(op: NamespaceOp) -> SignedNamespaceOp {
        let sk = PrivateKey::random(&mut OsRng);
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
    fn member_joined_at_roundtrips_through_stored_signed_entry() {
        // The invitation join carries a nested `SignedGroupOpenInvitation`, the
        // largest and most field-rich op payload — the one most exposed to a
        // codec asymmetry.
        assert_roundtrips(&signed(NamespaceOp::Root(RootOp::MemberJoinedAt {
            member: PrivateKey::random(&mut OsRng).public_key(),
            signed_invitation: sample_invitation(),
            joined_at: 1_800_000_000,
        })));
    }

    #[test]
    fn every_root_op_roundtrips_through_stored_signed_entry() {
        let ops = [
            RootOp::GroupCreated {
                group_id: [1; 32].into(),
                parent_id: [2; 32].into(),
                restricted: true,
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
                new_admin: PrivateKey::random(&mut OsRng).public_key(),
            },
            RootOp::PolicyUpdated {
                policy_bytes: vec![9, 8, 7],
            },
            RootOp::MemberJoined {
                member: PrivateKey::random(&mut OsRng).public_key(),
                signed_invitation: sample_invitation(),
            },
            RootOp::MemberJoinedOpen {
                member: PrivateKey::random(&mut OsRng).public_key(),
                group_id: [7; 32].into(),
            },
            RootOp::MemberJoinedAt {
                member: PrivateKey::random(&mut OsRng).public_key(),
                signed_invitation: sample_invitation(),
                joined_at: 42,
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
}
