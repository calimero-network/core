//! Membership plane: a per-group governance op ([`GroupOp`]) → its `OpPayload`.

use calimero_context_config::types::ContextGroupId;
use calimero_context_config::VisibilityMode;
use calimero_governance_types::GroupOp;
use calimero_op::{OpPayload, ScopeId};

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
/// - app / upgrade / migration config (`TargetApplicationSet`,
///   `GroupMigrationSet`, `CascadeUpgrade`) - owned by
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
        // The account plane. Without these three arms the ops folded to `Noop`,
        // so `crates/projection`'s account plane — built, tested, and complete —
        // never learned that a device existed, and `AclView.devices` stayed empty
        // on the governance path. That is not "dormant until the cutover": it is
        // three missing arms. The payloads have been in `calimero-op` all along.
        //
        // The consequence was concrete: per-device authorization cannot resolve a
        // device's signing key to the account it speaks for at a causal cut, so a
        // second device could receive scope keys and then not author with them.
        // `endorsement` is deliberately dropped here. It is a bridge artifact: the
        // unified plane's membership is keyed by `AccountId`, so `authorize` asks
        // whether the ACCOUNT is a member directly and needs no proxy. The
        // endorsement exists only for the governance path, where membership is
        // still key-keyed and an offline root is a member nowhere.
        GroupOp::AccountDeviceLinked {
            genesis,
            chain,
            cert,
            ..
        } => Some(OpPayload::DeviceLinked {
            genesis: *genesis,
            chain: chain.clone(),
            cert: *cert,
        }),
        // `proof` is dropped, like `endorsement` on the link above and for the
        // same reason: on the unified plane membership is keyed by `AccountId`,
        // so `authorize` asks whether the revoker IS the account rather than
        // needing a self-certifying proxy for it.
        GroupOp::AccountDeviceUnlinked {
            account,
            device,
            proof: _,
        } => Some(OpPayload::DeviceRevoked {
            account: *account,
            device: *device,
        }),
        GroupOp::AccountKeysRotated { handoff } => {
            Some(OpPayload::AccountKeysRotated { handoff: *handoff })
        }
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
            restricted: matches!(mode, VisibilityMode::Restricted),
        }),
        _ => None,
    }
}
