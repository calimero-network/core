//! Transitional adapter for the unified causal log (core#2716, Phase 5
//! migration). Additive: nothing depends on this crate yet; it (and the legacy
//! source types it reads) is deleted once the migration completes.
//!
//! Two roles:
//!
//! - **Encoders** — map the legacy per-plane operation types onto the one
//!   [`OpPayload`], so the migration can prove the unified projection
//!   faithfully represents the current system across all four planes: data
//!   (`Action` → `Put`/`Delete`), access-control (`RotationLogEntry` →
//!   `SetWriters`), membership (`GroupOp` → `MemberAdded`/`MemberRemoved`), and
//!   admin (`RootOp` → `AdminChanged`/`PolicyUpdated`/`SubgroupCreated`/open-join).
//!   Coverage gaps (ops with no current `OpPayload` arm) are documented per
//!   encoder as migration inputs.
//!
//! - **Shadow comparison** ([`shadow_data_delta`]) — the slice-2 cutover aid:
//!   compare the unified `authorize` decision to the legacy one, off the
//!   current resolvers, so the live path can run both behind a divergence
//!   metric and act on the old decision until divergence is proven zero.

use std::collections::BTreeMap;

use calimero_authz::AclView;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::{GroupOp, RootOp};
use calimero_op::{OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::entities::OpMask;
use calimero_storage::rotation_log::RotationLogEntry;

/// Result of comparing the legacy per-delta authorization to the unified
/// per-write [`authorize`](calimero_authz::authorize) in **shadow mode**
/// (slice-2 S2.2). The caller runs both, records a [`Diverge`](ShadowVerdict::Diverge)
/// behind a metric, and ACTS ON the old decision until the divergence metric is
/// proven zero across e2e — only then cutting over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowVerdict {
    /// Old and new agree — the unified `authorize` is behaviorally identical
    /// here.
    Agree,
    /// They disagree. Expected only where the unified model is *finer* than the
    /// old coarse gate (e.g. a context member writing a restricted object it
    /// isn't a listed writer of — new denies, old allowed); any other
    /// divergence is a bug to investigate before cutover.
    Diverge { old_allows: bool, new_allows: bool },
}

/// Compare the legacy per-delta auth (`old_allows`) to the unified per-write
/// `AclView::may` for a data delta, building the [`AclView`] from the
/// **current resolvers'** output (slice-2 S2.2 — shadow off current state, no
/// dependency on the live `ScopeState` yet).
///
/// The live caller (in `delta_store`, where the resolved writers + decoded
/// actions are both available) supplies:
/// - `author` + `member_at_cut`: from the membership resolution
///   (`acl_view_at`).
/// - `restricted_acl`: explicit writer sets for the restricted entities the
///   delta touches (from `writers_at_authenticated`); empty ⇒ all entities are
///   non-restricted (default-write = membership, S2.1).
/// - `writes`: the `(entity, required mask)` each action performs.
///
/// Returns [`ShadowVerdict`]; the caller metrics/logs `Diverge` and acts on
/// `old_allows`.
#[must_use]
pub fn shadow_data_delta(
    author: &PublicKey,
    member_at_cut: bool,
    restricted_acl: &BTreeMap<Id, BTreeMap<PublicKey, OpMask>>,
    writes: &[(Id, OpMask)],
    old_allows: bool,
) -> ShadowVerdict {
    let mut groups = BTreeMap::new();
    if member_at_cut {
        // Any group keying `author` as a member makes `is_scope_member` true —
        // that's all `default-write` (S2.1) consults for non-restricted writes.
        let _ = groups.insert(
            ContextGroupId::from([0u8; 32]),
            [(*author, GroupMemberRole::Member)].into_iter().collect(),
        );
    }
    let view = AclView {
        acl: restricted_acl.clone(),
        groups,
        root_admin: None,
    };
    let new_allows = writes
        .iter()
        .all(|(entity, mask)| view.may(author, *entity, *mask));
    if new_allows == old_allows {
        ShadowVerdict::Agree
    } else {
        ShadowVerdict::Diverge {
            old_allows,
            new_allows,
        }
    }
}

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
/// **Coverage (admin + membership + scope-tree planes):** `AdminChanged` →
/// `AdminChanged`; `PolicyUpdated` → `PolicyUpdated`; `MemberJoinedOpen` →
/// `MemberAdded` (open-subgroup self-join grants `Member`); `GroupCreated` →
/// `SubgroupCreated`; `GroupReparented` → `SubgroupReparented`; `GroupDeleted`
/// → `SubgroupDeleted` (see caveats).
///
/// **Caveats:**
/// - `GroupCreated`: the `restricted` flag isn't carried on the op (it's a
///   policy determination), so this emits `restricted: false`; the live
///   migration resolves real restriction from the group's policy.
/// - `GroupDeleted`: maps only the `root_group_id`; the op's pre-computed
///   `cascade_group_ids` mean the live path emits one `SubgroupDeleted` per
///   cascaded scope.
///
/// **Returns `None`** (tracked gaps): `MemberJoined` (invitation-based — needs
/// the signed-invitation decode for `group_id`/`invited_role`; handled where
/// the invitation is already decoded), `KeyDelivery` (key transport, outside
/// the op model — §9.6).
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
        RootOp::GroupCreated {
            group_id,
            parent_id,
        } => Some(OpPayload::SubgroupCreated {
            child: ScopeId::from(*group_id),
            parent: ScopeId::from(*parent_id),
            restricted: false,
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
        let parent = [0x70; 32]; // placeholder parent id
        assert_eq!(
            payload_from_root_op(&RootOp::GroupCreated {
                group_id: gid,
                parent_id: parent,
            }),
            Some(OpPayload::SubgroupCreated {
                child: ScopeId::from(gid),
                parent: ScopeId::from(parent),
                restricted: false,
            })
        );
        // Scope-tree restructure ops now map to the structural OpPayload arms.
        assert_eq!(
            payload_from_root_op(&RootOp::GroupReparented {
                child_group_id: gid,
                new_parent_id: [9u8; 32],
            }),
            Some(OpPayload::SubgroupReparented {
                child: ScopeId::from(gid),
                new_parent: ScopeId::from([9u8; 32]),
            })
        );
        assert_eq!(
            payload_from_root_op(&RootOp::GroupDeleted {
                root_group_id: gid,
                cascade_group_ids: vec![],
                cascade_context_ids: vec![],
            }),
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

    // ---- slice-2 S2.2: shadow-decision comparison ----

    #[test]
    fn shadow_agrees_for_member_default_write() {
        // Member writes ordinary keys (no restricted ACL); old gate allowed →
        // unified also allows → Agree (the common case; divergence stays 0).
        let bob = PublicKey::from([0xB0; 32]);
        let writes = [
            (Id::new([1; 32]), OpMask::WRITE),
            (Id::new([2; 32]), OpMask::WRITE),
        ];
        assert_eq!(
            shadow_data_delta(&bob, true, &BTreeMap::new(), &writes, true),
            ShadowVerdict::Agree
        );
    }

    #[test]
    fn shadow_agrees_for_non_member_rejection() {
        let carol = PublicKey::from([0xC0; 32]);
        let writes = [(Id::new([1; 32]), OpMask::WRITE)];
        // Non-member: old rejected, unified also rejects (not a scope member).
        assert_eq!(
            shadow_data_delta(&carol, false, &BTreeMap::new(), &writes, false),
            ShadowVerdict::Agree
        );
    }

    #[test]
    fn shadow_diverges_when_member_writes_restricted_nonwriter() {
        // The S2.1 tightening: a context member writes a restricted object it
        // isn't a listed writer of. Old coarse gate allowed (member); unified
        // denies (explicit ACL). The shadow surfaces this expected divergence.
        let bob = PublicKey::from([0xB0; 32]);
        let alice = PublicKey::from([0xA1; 32]);
        let secret = Id::new([0x5E; 32]);
        let restricted: BTreeMap<Id, BTreeMap<PublicKey, OpMask>> =
            [(secret, [(alice, OpMask::FULL)].into_iter().collect())]
                .into_iter()
                .collect();
        let writes = [(secret, OpMask::WRITE)];
        assert_eq!(
            shadow_data_delta(&bob, true, &restricted, &writes, true),
            ShadowVerdict::Diverge {
                old_allows: true,
                new_allows: false,
            }
        );
        // The listed writer agrees (old + new both allow).
        assert_eq!(
            shadow_data_delta(&alice, true, &restricted, &writes, true),
            ShadowVerdict::Agree
        );
    }
}
