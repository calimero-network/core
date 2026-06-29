//! Transitional adapter that bridges the per-plane operation types onto the
//! unified causal log. It (and the per-plane source types it reads) is deleted
//! once everything runs on the unified [`OpPayload`].
//!
//! **Encoders** map each per-plane operation onto the one [`OpPayload`], so we
//! can prove the unified projection faithfully represents the current system
//! across all four planes: data (`Action` → `Put`/`Delete`), access-control
//! (`RotationLogEntry` → `SetWriters`), membership (`GroupOp` →
//! `MemberAdded`/`MemberRemoved`), and admin (`RootOp` →
//! `AdminChanged`/`PolicyUpdated`/`SubgroupCreated`/open-join). In-model vs
//! out-of-model coverage is documented per encoder.
//!
//! The proof of faithfulness is deterministic **fold-equivalence**: the unified
//! projection resolves the same writer set and the same membership as the
//! current resolvers over the same op sequence (`acl_plane_matches_resolve_local_*`
//! here, plus the membership-fold property test in `calimero-governance-store`).

use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::{GroupOp, RootOp};
use calimero_op::{OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
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

/// Map an invitation's `invited_role` byte (0 = Admin, 2 = ReadOnly, else
/// Member) to a [`GroupMemberRole`] — the same mapping governance-store uses
/// (reimplemented here so the adapter doesn't depend on a `pub(crate)` helper).
fn role_from_invited_role(value: u8) -> GroupMemberRole {
    match value {
        0 => GroupMemberRole::Admin,
        2 => GroupMemberRole::ReadOnly,
        _ => GroupMemberRole::Member,
    }
}

/// Encode a per-group governance op ([`GroupOp`], already decrypted) as an
/// [`OpPayload`] for `group`.
///
/// **In-model — the ops that move the unified `authorize` decision:**
/// - `MemberAdded` / `MemberRoleSet` → `MemberAdded` (a role change is a
///   re-assert; `ScopeState`'s per-`(group, member)` LWW keeps the latest).
/// - `MemberRemoved` / `MemberLeft` → `MemberRemoved`.
/// - `MemberJoinedViaTeeAttestation` → `MemberAdded` (a hardware-attested TEE
///   node becomes a member with the granted role; the attestation evidence is
///   consumed by the admission gate, not the membership projection).
/// - `TransferOwnership` → `AdminChanged` (owner ⇔ ADMIN; the op is authored in
///   the *group's* scope, so it sets that scope's root admin).
///
/// **Inheritance-relevant planes (folded — they drive at-cut membership):**
/// - capability: `DefaultCapabilitiesSet` / `MemberCapabilitySet` → the
///   `CAN_JOIN_OPEN_SUBGROUPS` bit gates inheritance into open subgroups, so the
///   projection must resolve it at the cut;
/// - visibility: `SubgroupVisibilitySet` → the Open/Restricted wall that gates
///   the inheritance parent-walk.
///
/// **Out-of-model (`None`, by design — not gaps).** Ops that never enter the
/// authorization decision:
/// - app / upgrade / migration config (`UpgradePolicySet`,
///   `TargetApplicationSet`, `GroupMigrationSet`, the `Cascade*` ops) — owned by
///   the app-version machinery;
/// - metadata (`GroupMetadataSet`, `MemberMetadataSet`, `ContextMetadataSet`),
///   TEE-admission *policy* (`TeeAdmissionPolicySet`), auto-follow
///   (`MemberSetAutoFollow`);
/// - the context↔group binding (`ContextRegistered`/`ContextDetached`,
///   `GroupDelete`) — `authorize` derives a context's group from that binding
///   *at auth time* (the context→group lookup), so it lives in that index, not
///   inside a scope's `ScopeState`.
///
/// The auth-relevant (in-model) variants are armed explicitly; everything else
/// maps to `None`. `GroupOp` is `#[non_exhaustive]`, so a `_` arm is mandatory
/// here (a downstream crate cannot match it exhaustively) — which means a new
/// upstream variant lands in `_ => None` by default. The safety net against a
/// new *auth-relevant* op being silently dropped is the fold-equivalence test
/// (`prefix_walk_resolution_matches_reference_under_random_inputs` in
/// `calimero-governance-store`): if a new variant changes membership in a way
/// the projection doesn't see, that test diverges.
#[must_use]
pub fn payload_from_group_op(group: ContextGroupId, op: &GroupOp) -> Option<OpPayload> {
    match op {
        GroupOp::MemberAdded { member, role }
        | GroupOp::MemberRoleSet { member, role }
        | GroupOp::MemberJoinedViaTeeAttestation { member, role, .. } => {
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
        GroupOp::TransferOwnership { new_owner } => Some(OpPayload::AdminChanged {
            new_admin: *new_owner,
        }),
        // Capability plane — folded so the projection can resolve inherited
        // membership (the `CAN_JOIN_OPEN_SUBGROUPS` bit) at the cut.
        GroupOp::DefaultCapabilitiesSet { capabilities } => {
            Some(OpPayload::DefaultCapabilitiesSet {
                group,
                capabilities: *capabilities,
            })
        }
        GroupOp::MemberCapabilitySet {
            member,
            capabilities,
        } => Some(OpPayload::MemberCapabilitySet {
            group,
            member: *member,
            capabilities: *capabilities,
        }),
        // Visibility plane — the Open/Restricted wall that gates inheritance.
        // Live mode byte: 0 = Open, anything else = Restricted.
        GroupOp::SubgroupVisibilitySet { mode } => Some(OpPayload::SubgroupVisibilitySet {
            scope: ScopeId::from(group.to_bytes()),
            restricted: *mode != 0,
        }),
        _ => None,
    }
}

/// Encode a namespace root governance op ([`RootOp`]) as an [`OpPayload`].
///
/// **Coverage (admin + membership + scope-tree planes):** `AdminChanged` →
/// `AdminChanged`; `PolicyUpdated` → `PolicyUpdated`; `MemberJoinedOpen` →
/// `MemberAdded` (open-subgroup self-join grants `Member`); `GroupCreated` →
/// `SubgroupCreated`; `GroupReparented` → `SubgroupReparented`; `GroupDeleted`
/// → `SubgroupDeleted` (see caveats).
///
/// **Caveats:**
/// - `GroupCreated`: the `restricted` flag isn't carried on the op (it's a
///   policy determination), so this emits `restricted: false`; the live path
///   resolves real restriction from the group's policy.
/// - `GroupDeleted`: maps only the `root_group_id`; the op's pre-computed
///   `cascade_group_ids` mean the live path emits one `SubgroupDeleted` per
///   cascaded scope.
///
/// `MemberJoined` / `MemberJoinedAt` → `MemberAdded`: an invitation-based join
/// (`MemberJoinedAt` is the same join carrying the joiner's observed timestamp).
/// The admin-signed invitation carries the authoritative `group_id` and
/// `invited_role` (the joiner cannot escalate — the role is under the admin's
/// signature), so we decode both straight off it.
///
/// **Returns `None`** (out-of-model by design): `KeyDelivery` — key transport,
/// which rides its own channel and never enters the auth projection.
///
/// `signer` is the op's outer-`SignedNamespaceOp` signer — needed for
/// `GroupCreated`, whose creator becomes the new subgroup's genesis admin
/// (mirrors the live `GroupMeta.admin_identity = GroupCreated.signer`). It is
/// ignored by every other variant.
#[must_use]
pub fn payload_from_root_op(op: &RootOp, signer: PublicKey) -> Option<OpPayload> {
    match op {
        RootOp::AdminChanged { new_admin } => Some(OpPayload::AdminChanged {
            new_admin: *new_admin,
        }),
        RootOp::PolicyUpdated { policy_bytes } => Some(OpPayload::PolicyUpdated {
            policy_bytes: policy_bytes.clone(),
        }),
        RootOp::MemberJoined {
            member,
            signed_invitation,
        }
        | RootOp::MemberJoinedAt {
            member,
            signed_invitation,
            ..
        } => Some(OpPayload::MemberAdded {
            group: signed_invitation.invitation.group_id,
            member: *member,
            role: role_from_invited_role(signed_invitation.invitation.invited_role),
        }),
        RootOp::MemberJoinedOpen { member, group_id } => Some(OpPayload::MemberAdded {
            group: ContextGroupId::from(*group_id),
            member: *member,
            role: GroupMemberRole::Member,
        }),
        RootOp::GroupCreated {
            group_id,
            parent_id,
            restricted,
        } => Some(OpPayload::SubgroupCreated {
            child: ScopeId::from(*group_id),
            parent: ScopeId::from(*parent_id),
            // Visibility is now carried atomically on the live op (#2771):
            // `restricted: true` = Restricted (default), `false` = born-Open.
            // This aligns the projection-plane `SubgroupCreated.restricted`
            // with the live op instead of hardcoding Restricted.
            restricted: *restricted,
            admin: signer,
        }),
        RootOp::GroupReparented {
            child_group_id,
            new_parent_id,
        } => Some(OpPayload::SubgroupReparented {
            child: ScopeId::from(*child_group_id),
            new_parent: ScopeId::from(*new_parent_id),
        }),
        RootOp::GroupDeleted { root_group_id, .. } => Some(OpPayload::SubgroupDeleted {
            scope: ScopeId::from(*root_group_id),
        }),
        // Out-of-model: `KeyDelivery` is key transport, not authorization
        // state. (`RootOp` is `#[non_exhaustive]`, so a `_` arm is mandatory.)
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
    /// `rotation_log::resolve_local` does — the equivalence that lets the live
    /// ACL resolution route through `ScopeState`.
    ///
    /// Scope: sequential rotations (strictly increasing HLC), the common case.
    /// Genuinely-concurrent (equal-HLC) rotations tie-break by `op_id` in
    /// `ScopeState` vs signer-digest in `resolve_local`; once `resolve_local` is
    /// gone the `op_id` tiebreak is canonical and identical on every node, so
    /// there is nothing to align.
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
        // A TEE node admitted via attestation is a member with the granted
        // role; the attestation evidence is consumed by the admission gate.
        assert_eq!(
            payload_from_group_op(
                group,
                &GroupOp::MemberJoinedViaTeeAttestation {
                    member: m,
                    quote_hash: [0u8; 32],
                    mrtd: String::new(),
                    rtmr0: String::new(),
                    rtmr1: String::new(),
                    rtmr2: String::new(),
                    rtmr3: String::new(),
                    tcb_status: String::new(),
                    role: GroupMemberRole::ReadOnlyTee,
                },
            ),
            Some(OpPayload::MemberAdded {
                group,
                member: m,
                role: GroupMemberRole::ReadOnlyTee,
            })
        );
        // Ownership transfer sets the group scope's root admin (owner ⇔ ADMIN).
        let new_owner = PublicKey::from([0x77; 32]);
        assert_eq!(
            payload_from_group_op(group, &GroupOp::TransferOwnership { new_owner }),
            Some(OpPayload::AdminChanged {
                new_admin: new_owner,
            })
        );
        // Out-of-model ops (metadata, config, …) → None.
        assert_eq!(payload_from_group_op(group, &GroupOp::Noop), None);
        // Capability plane is now folded (gates inherited membership).
        assert_eq!(
            payload_from_group_op(group, &GroupOp::DefaultCapabilitiesSet { capabilities: 7 }),
            Some(OpPayload::DefaultCapabilitiesSet {
                group,
                capabilities: 7,
            })
        );
    }

    #[test]
    fn root_op_encoder_mapping() {
        let admin = PublicKey::from([1u8; 32]);
        let m = PublicKey::from([0x55; 32]);
        let gid = [3u8; 32];

        assert_eq!(
            payload_from_root_op(
                &RootOp::AdminChanged { new_admin: admin },
                PublicKey::from([1u8; 32])
            ),
            Some(OpPayload::AdminChanged { new_admin: admin })
        );
        assert_eq!(
            payload_from_root_op(
                &RootOp::PolicyUpdated {
                    policy_bytes: vec![1, 2, 3],
                },
                PublicKey::from([1u8; 32])
            ),
            Some(OpPayload::PolicyUpdated {
                policy_bytes: vec![1, 2, 3],
            })
        );
        assert_eq!(
            payload_from_root_op(
                &RootOp::MemberJoinedOpen {
                    member: m,
                    group_id: gid,
                },
                PublicKey::from([1u8; 32])
            ),
            Some(OpPayload::MemberAdded {
                group: ContextGroupId::from(gid),
                member: m,
                role: GroupMemberRole::Member,
            })
        );
        // Invitation-based join: group_id + role decoded off the admin-signed
        // invitation (invited_role 0 = Admin). The joiner can't escalate — the
        // role is under the admin's signature.
        use calimero_context_config::types::{GroupInvitationFromAdmin, SignedGroupOpenInvitation};
        let signed_invitation = SignedGroupOpenInvitation {
            invitation: GroupInvitationFromAdmin {
                inviter_identity: [0xA1; 32].into(),
                group_id: ContextGroupId::from(gid),
                expiration_timestamp: 1_700_000_000,
                secret_salt: [0x33; 32],
                invited_role: 0, // Admin
            },
            inviter_signature: "deadbeef".to_string(),
            application_id: None,
            app_key: None,
        };
        assert_eq!(
            payload_from_root_op(
                &RootOp::MemberJoined {
                    member: m,
                    signed_invitation: signed_invitation.clone(),
                },
                PublicKey::from([1u8; 32])
            ),
            Some(OpPayload::MemberAdded {
                group: ContextGroupId::from(gid),
                member: m,
                role: GroupMemberRole::Admin,
            })
        );
        // `MemberJoinedAt` (the timestamped invitation join `join_group` emits)
        // decodes identically — it is NOT out-of-model.
        assert_eq!(
            payload_from_root_op(
                &RootOp::MemberJoinedAt {
                    member: m,
                    signed_invitation,
                    joined_at: 42,
                },
                PublicKey::from([1u8; 32])
            ),
            Some(OpPayload::MemberAdded {
                group: ContextGroupId::from(gid),
                member: m,
                role: GroupMemberRole::Admin,
            })
        );
        let parent = [0x70; 32]; // placeholder parent id
        assert_eq!(
            payload_from_root_op(
                &RootOp::GroupCreated {
                    group_id: gid,
                    parent_id: parent,
                    restricted: true,
                },
                PublicKey::from([1u8; 32])
            ),
            Some(OpPayload::SubgroupCreated {
                child: ScopeId::from(gid),
                parent: ScopeId::from(parent),
                restricted: true,
                admin: PublicKey::from([1u8; 32]),
            })
        );
        // Scope-tree restructure ops now map to the structural OpPayload arms.
        assert_eq!(
            payload_from_root_op(
                &RootOp::GroupReparented {
                    child_group_id: gid,
                    new_parent_id: [9u8; 32],
                },
                PublicKey::from([1u8; 32])
            ),
            Some(OpPayload::SubgroupReparented {
                child: ScopeId::from(gid),
                new_parent: ScopeId::from([9u8; 32]),
            })
        );
        assert_eq!(
            payload_from_root_op(
                &RootOp::GroupDeleted {
                    root_group_id: gid,
                    cascade_group_ids: vec![],
                    cascade_context_ids: vec![],
                },
                PublicKey::from([1u8; 32])
            ),
            Some(OpPayload::SubgroupDeleted {
                scope: ScopeId::from(gid),
            })
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
