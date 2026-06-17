//! Transitional encoders: current domain operations → unified [`OpPayload`]
//! (core#2716, Phase 5 migration — slice 1).
//!
//! These map the legacy per-plane operation types onto the one `Op` model so
//! the migration can prove the unified projection faithfully represents — and
//! resolves identically to — the current system **before** any live apply/sync
//! path is rewired. Additive: nothing depends on this crate yet; it (and the
//! legacy source types it reads) is deleted once the migration completes.
//!
//! Covered here (slice 1): the **data plane** (`Action` → `Put`/`Delete`) and
//! the **access-control plane** (`RotationLogEntry` → `SetWriters`). The
//! membership/admin plane (`GroupOp`/`NamespaceOp` → `MemberAdded`/… /
//! `AdminChanged`/…) lands in the next slice.

use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::{GroupOp, RootOp};
use calimero_op::{OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::rotation_log::RotationLogEntry;

/// Encode a storage data [`Action`] as an [`OpPayload`].
///
/// Returns `None` for [`Action::Compare`] — a sync reconciliation *hint*, not
/// a state change, so it has no op-model representation.
#[must_use]
pub fn payload_from_action(action: &Action) -> Option<OpPayload> {
    match action {
        Action::Add { id, data, .. } | Action::Update { id, data, .. } => Some(OpPayload::Put {
            entity: *id,
            value: data.clone(),
        }),
        Action::DeleteRef { id, .. } => Some(OpPayload::Delete { entity: *id }),
        Action::Compare { .. } => None,
    }
}

/// Encode a writer-set rotation ([`RotationLogEntry`]) as a `SetWriters` op for
/// `object` (the Shared anchor whose ACL is being rotated).
///
/// The op's `parents` carry the rotation's causal position and its author is
/// `entry.signer`; this function captures only the payload — the caller
/// assembles the full `Op` (id/parents/author/hlc/signature) from the entry's
/// `delta_id`/`delta_hlc`/`signer`/`signature`.
#[must_use]
pub fn set_writers_payload(object: Id, entry: &RotationLogEntry) -> OpPayload {
    OpPayload::SetWriters {
        object,
        writers: entry.new_writers.clone(),
    }
}

/// Encode a per-group governance op ([`GroupOp`], already decrypted) as an
/// [`OpPayload`] for `group`.
///
/// **Coverage (membership plane):** `MemberAdded`/`MemberRoleSet` →
/// `MemberAdded` (a role change is a re-assert; `ScopeState`'s per-`(group,
/// member)` LWW keeps the latest role); `MemberRemoved`/`MemberLeft` →
/// `MemberRemoved`.
///
/// **Returns `None`** for ops with no current `OpPayload` arm — a tracked
/// coverage gap for the migration to resolve (either extend `OpPayload` or
/// handle outside the op model): `Noop`, `MemberCapabilitySet`,
/// `DefaultCapabilitiesSet`, `UpgradePolicySet`, `TargetApplicationSet`,
/// `ContextRegistered`, `ContextDetached`.
#[must_use]
pub fn payload_from_group_op(group: ContextGroupId, op: &GroupOp) -> Option<OpPayload> {
    match op {
        GroupOp::MemberAdded { member, role } | GroupOp::MemberRoleSet { member, role } => {
            Some(OpPayload::MemberAdded {
                group,
                member: *member,
                role: role.clone(),
            })
        }
        GroupOp::MemberRemoved { member, .. } | GroupOp::MemberLeft { member, .. } => {
            Some(OpPayload::MemberRemoved {
                group,
                member: *member,
            })
        }
        _ => None,
    }
}

/// Encode a namespace root governance op ([`RootOp`]) as an [`OpPayload`].
///
/// **Coverage (admin + membership planes):** `AdminChanged` → `AdminChanged`;
/// `PolicyUpdated` → `PolicyUpdated`; `MemberJoinedOpen` → `MemberAdded`
/// (open-subgroup self-join grants `Member`); `GroupCreated` →
/// `SubgroupCreated` (see caveat).
///
/// **Caveat — `GroupCreated`:** the `restricted` flag isn't carried on the op
/// (it's a policy determination), so this emits `restricted: false`; the live
/// migration must resolve real restriction from the group's policy.
///
/// **Returns `None`** (tracked gaps): `MemberJoined` (invitation-based — needs
/// the signed-invitation decode for `group_id`/`invited_role`; handled where
/// the invitation is already decoded), `GroupReparented`/`GroupDeleted`
/// (scope-tree structure, not member/admin state — needs `OpPayload`
/// extension), `KeyDelivery` (key transport, outside the op model — §9.6).
#[must_use]
pub fn payload_from_root_op(op: &RootOp) -> Option<OpPayload> {
    match op {
        RootOp::AdminChanged { new_admin } => Some(OpPayload::AdminChanged {
            new_admin: *new_admin,
        }),
        RootOp::PolicyUpdated { policy_bytes } => Some(OpPayload::PolicyUpdated {
            policy_bytes: policy_bytes.clone(),
        }),
        RootOp::MemberJoinedOpen { member, group_id } => Some(OpPayload::MemberAdded {
            group: ContextGroupId::from(*group_id),
            member: *member,
            role: GroupMemberRole::Member,
        }),
        RootOp::GroupCreated { group_id, .. } => Some(OpPayload::SubgroupCreated {
            child: ScopeId::from(*group_id),
            restricted: false,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use core::num::NonZeroU128;
    use std::collections::BTreeMap;

    use calimero_op::{Op, OpPayload, ScopeId};
    use calimero_primitives::identity::PublicKey;
    use calimero_projection::ScopeState;
    use calimero_storage::entities::{Metadata, OpMask};
    use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
    use calimero_storage::rotation_log::{RotationLog, RotationLogEntry};

    fn hlc(ns: u64) -> HybridTimestamp {
        HybridTimestamp::new(Timestamp::new(
            NTP64(ns),
            ID::from(NonZeroU128::new(1).unwrap()),
        ))
    }

    #[test]
    fn data_plane_action_mapping() {
        let id = Id::new([1u8; 32]);
        let add = Action::Add {
            id,
            data: vec![1, 2, 3],
            ancestors: Vec::new(),
            metadata: Metadata::default(),
        };
        let upd = Action::Update {
            id,
            data: vec![4, 5],
            ancestors: Vec::new(),
            metadata: Metadata::default(),
        };
        let del = Action::DeleteRef {
            id,
            deleted_at: 0,
            metadata: Metadata::default(),
        };
        let cmp = Action::Compare { id };

        assert_eq!(
            payload_from_action(&add),
            Some(OpPayload::Put {
                entity: id,
                value: vec![1, 2, 3]
            })
        );
        assert_eq!(
            payload_from_action(&upd),
            Some(OpPayload::Put {
                entity: id,
                value: vec![4, 5]
            })
        );
        assert_eq!(
            payload_from_action(&del),
            Some(OpPayload::Delete { entity: id })
        );
        // Compare is a sync hint, not a state change.
        assert_eq!(payload_from_action(&cmp), None);
    }

    /// Build a `SetWriters` op chain from a rotation log and assert the unified
    /// projection resolves the **same writer set** the current
    /// `rotation_log::resolve_local` does — the equivalence that de-risks
    /// routing the live ACL resolution through `ScopeState` in a later slice.
    ///
    /// Scope: sequential rotations (strictly increasing HLC), the common case.
    /// Genuinely-concurrent (equal-HLC) rotations tie-break by `op_id` in
    /// `ScopeState` vs signer-digest in `resolve_local`; aligning that tiebreak
    /// is a tracked migration detail, exercised once the live path is wired.
    #[test]
    fn acl_plane_matches_resolve_local_for_sequential_rotations() {
        let object = Id::new([0xA0; 32]);
        let scope = ScopeId::from([0u8; 32]);
        let admin = PublicKey::from([1u8; 32]);
        let w1 = PublicKey::from([0x11; 32]);
        let w2 = PublicKey::from([0x22; 32]);

        // Three sequential rotations: {w1} → {w1,w2} → {w2}.
        let sets: Vec<BTreeMap<PublicKey, OpMask>> = vec![
            [(w1, OpMask::FULL)].into_iter().collect(),
            [(w1, OpMask::FULL), (w2, OpMask::FULL)]
                .into_iter()
                .collect(),
            [(w2, OpMask::FULL)].into_iter().collect(),
        ];

        let mut entries = Vec::new();
        let mut ops = Vec::new();
        let mut prev_id: Option<[u8; 32]> = None;
        for (i, writers) in sets.iter().enumerate() {
            let delta_id = [i as u8 + 1; 32];
            let h = hlc((i as u64 + 1) * 10);
            entries.push(RotationLogEntry {
                delta_id,
                delta_hlc: h,
                signer: Some(admin),
                signature: None,
                signed_payload: None,
                new_writers: writers.clone(),
                writers_nonce: i as u64 + 1,
            });
            let payload = OpPayload::SetWriters {
                object,
                writers: writers.clone(),
            };
            let parents: Vec<[u8; 32]> = prev_id.into_iter().collect();
            let id = Op::compute_id(scope, &parents, &admin, &h, &payload);
            ops.push(Op {
                id,
                scope,
                parents,
                author: admin,
                hlc: h,
                payload,
                expected_scope_root: [0u8; 32],
                signature: [0u8; 64],
            });
            prev_id = Some(id);
        }

        let log = RotationLog {
            snapshot: None,
            entries,
        };
        let expected =
            calimero_storage::rotation_log::resolve_local(&log).expect("non-empty log resolves");

        let projected = ScopeState::from_ops(&ops);
        let resolved = projected
            .acl_view()
            .acl
            .get(&object)
            .cloned()
            .unwrap_or_default();

        assert_eq!(
            resolved, expected,
            "ScopeState ACL fold must resolve the same writer set as resolve_local"
        );
        // Sanity: the latest rotation ({w2}) wins.
        assert_eq!(resolved, sets[2]);
    }

    /// Encoding a rotation's payload then folding it yields the rotation's
    /// writer set verbatim.
    #[test]
    fn set_writers_payload_round_trips_through_projection() {
        let object = Id::new([0xB0; 32]);
        let scope = ScopeId::from([0u8; 32]);
        let admin = PublicKey::from([1u8; 32]);
        let writers: BTreeMap<PublicKey, OpMask> = [(PublicKey::from([7u8; 32]), OpMask::FULL)]
            .into_iter()
            .collect();

        let entry = RotationLogEntry {
            delta_id: [9u8; 32],
            delta_hlc: hlc(5),
            signer: Some(admin),
            signature: None,
            signed_payload: None,
            new_writers: writers.clone(),
            writers_nonce: 1,
        };
        let payload = set_writers_payload(object, &entry);
        let id = Op::compute_id(scope, &[], &admin, &entry.delta_hlc, &payload);
        let op = Op {
            id,
            scope,
            parents: vec![],
            author: admin,
            hlc: entry.delta_hlc,
            payload,
            expected_scope_root: [0u8; 32],
            signature: [0u8; 64],
        };

        let resolved = ScopeState::from_ops([&op])
            .acl_view()
            .acl
            .get(&object)
            .cloned()
            .unwrap_or_default();
        assert_eq!(resolved, writers);
    }

    #[test]
    fn group_op_encoder_mapping() {
        let group = ContextGroupId::from([3u8; 32]);
        let m = PublicKey::from([0x55; 32]);

        assert_eq!(
            payload_from_group_op(
                group,
                &GroupOp::MemberAdded {
                    member: m,
                    role: GroupMemberRole::Member,
                },
            ),
            Some(OpPayload::MemberAdded {
                group,
                member: m,
                role: GroupMemberRole::Member,
            })
        );
        // A role change re-asserts membership (ScopeState LWW keeps the latest).
        assert_eq!(
            payload_from_group_op(
                group,
                &GroupOp::MemberRoleSet {
                    member: m,
                    role: GroupMemberRole::Admin,
                },
            ),
            Some(OpPayload::MemberAdded {
                group,
                member: m,
                role: GroupMemberRole::Admin,
            })
        );
        // Non-membership ops have no current OpPayload arm (tracked gap).
        assert_eq!(payload_from_group_op(group, &GroupOp::Noop), None);
    }

    #[test]
    fn root_op_encoder_mapping() {
        let admin = PublicKey::from([1u8; 32]);
        let m = PublicKey::from([0x55; 32]);
        let gid = [3u8; 32];

        assert_eq!(
            payload_from_root_op(&RootOp::AdminChanged { new_admin: admin }),
            Some(OpPayload::AdminChanged { new_admin: admin })
        );
        assert_eq!(
            payload_from_root_op(&RootOp::PolicyUpdated {
                policy_bytes: vec![1, 2, 3],
            }),
            Some(OpPayload::PolicyUpdated {
                policy_bytes: vec![1, 2, 3],
            })
        );
        assert_eq!(
            payload_from_root_op(&RootOp::MemberJoinedOpen {
                member: m,
                group_id: gid,
            }),
            Some(OpPayload::MemberAdded {
                group: ContextGroupId::from(gid),
                member: m,
                role: GroupMemberRole::Member,
            })
        );
        assert_eq!(
            payload_from_root_op(&RootOp::GroupCreated {
                group_id: gid,
                parent_id: [0u8; 32],
            }),
            Some(OpPayload::SubgroupCreated {
                child: ScopeId::from(gid),
                restricted: false,
            })
        );
        // Scope-tree restructure has no member/admin-plane representation yet.
        assert_eq!(
            payload_from_root_op(&RootOp::GroupReparented {
                child_group_id: gid,
                new_parent_id: [9u8; 32],
            }),
            None
        );
    }

    /// A membership op sequence folds through `ScopeState` to the same final
    /// membership the governance state machine (what `membership_status_at`
    /// resolves) produces: last write wins per member, a removal drops them.
    #[test]
    fn membership_plane_fold_add_remove_readd() {
        let scope = ScopeId::from([0u8; 32]);
        let group = ContextGroupId::from([3u8; 32]);
        let admin = PublicKey::from([1u8; 32]);
        let m = PublicKey::from([0x55; 32]);

        let build = |ns: u64, payload: OpPayload| -> Op {
            let h = hlc(ns);
            let id = Op::compute_id(scope, &[], &admin, &h, &payload);
            Op {
                id,
                scope,
                parents: vec![],
                author: admin,
                hlc: h,
                payload,
                expected_scope_root: [0u8; 32],
                signature: [0u8; 64],
            }
        };

        // Add(Member)@10 → Remove@20 → Add(Admin)@30 → present as Admin.
        let ops = vec![
            build(
                10,
                OpPayload::MemberAdded {
                    group,
                    member: m,
                    role: GroupMemberRole::Member,
                },
            ),
            build(20, OpPayload::MemberRemoved { group, member: m }),
            build(
                30,
                OpPayload::MemberAdded {
                    group,
                    member: m,
                    role: GroupMemberRole::Admin,
                },
            ),
        ];
        let groups = ScopeState::from_ops(&ops).acl_view().groups;
        assert_eq!(
            groups.get(&group).and_then(|g| g.get(&m)),
            Some(&GroupMemberRole::Admin),
            "re-add after remove wins with the new role"
        );

        // Same set ending in Remove@40 → member absent.
        let mut ops2 = ops;
        ops2.push(build(40, OpPayload::MemberRemoved { group, member: m }));
        let groups2 = ScopeState::from_ops(&ops2).acl_view().groups;
        assert_eq!(
            groups2.get(&group).and_then(|g| g.get(&m)),
            None,
            "final removal drops the member"
        );
    }
}
