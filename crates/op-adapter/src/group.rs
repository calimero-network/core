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
/// - `RootGuarded { TransferOwnership }` → `RootGuarded { AdminChanged }` (owner
///   ⇔ ADMIN; the op is authored in the *group's* scope, so it sets that scope's
///   root admin). A bare `TransferOwnership` folds to nothing: apply refuses it
///   without a root proof, so there is nothing to fold. The same holds for every
///   owner-level op; see `crate::guard`.
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
            scope,
            ..
        } => Some(OpPayload::DeviceLinked {
            genesis: *genesis,
            chain: chain.clone(),
            cert: *cert,
            // The payload carries the epoch, not the proof, so the statement is
            // checked here: an unauthorised one lifts no device over a floor.
            scope_epoch: scope
                .authorises(cert.account, cert.device)
                .map_or(0, |verified| verified.scope_epoch),
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
        // Not `DeviceRevoked`, which is terminal: this records a floor a wider
        // scope can re-cross. `application` is dropped - the epoch alone decides.
        GroupOp::AccountDeviceDescoped {
            account,
            device,
            scope,
            ..
        } => scope
            .authorises(*account, *device)
            .ok()
            .map(|verified| OpPayload::DeviceDescoped {
                account: *account,
                device: *device,
                scope_epoch: verified.scope_epoch,
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
        // TEE authorship plane, folded so the TEE authority resolves at a cut.
        // `TeeAuthorityEvidence` is not mapped here: its payload is what the
        // quote proves, and verifying a quote is `calimero-governance-store`'s
        // job, which decodes that op itself.
        //
        // Only inside `RootGuarded`, like every owner-level op: see below.
        // A member's op published by a relay folds as the op it carries: its
        // effect on membership, capabilities and visibility is the inner op's,
        // and the live apply has already authorized it as the member.
        GroupOp::OnBehalf { op, .. } => payload_from_group_op(group, op),
        // The founding relay's self-admission is a direct TEE membership, like an
        // attestation admission: membership and device as one fact, or nothing
        // if the credential does not bind the account it names.
        GroupOp::FoundingRelayAttested { account, .. } => {
            let member = account.statement.account;
            crate::credential::join_credential_binds(&member, account).then(|| {
                OpPayload::MemberJoinedWithDevice {
                    group,
                    member,
                    role: calimero_primitives::context::GroupMemberRole::RelayTee,
                    genesis: account.genesis,
                    chain: account.chain.clone(),
                    cert: account.statement,
                }
            })
        }
        // An owner-level op with its root proof. The bare forms fall through to
        // `None` below.
        GroupOp::RootGuarded { op: inner, proof } => {
            let kind = inner.owner_op_kind()?;
            let digest = inner.owner_op_digest().ok()?;
            crate::guard::guarded_payload(group, kind, digest, proof, guarded_carried(group, inner))
        }
        _ => None,
    }
}

/// What an owner-level group op folds as once its guard has been checked.
///
/// `Noop` for the ones the projection models nothing about: a group deletion
/// lives in the context↔group index, and an admission policy is read by the
/// admission gate, not the membership fold. They still fold as a `RootGuarded`
/// node, so the projection counts them against the group's guarded-op counter.
fn guarded_carried(group: ContextGroupId, op: &GroupOp) -> OpPayload {
    match op {
        GroupOp::TransferOwnership { new_owner } => OpPayload::AdminChanged {
            new_admin: *new_owner,
        },
        GroupOp::TeeAuthoringPolicySet { allowed_mrtd } => OpPayload::TeeAuthoringPolicySet {
            group,
            allowed_mrtd: allowed_mrtd.clone(),
        },
        _ => OpPayload::Noop,
    }
}

/// [`payload_from_group_op`] for an op signed under a schema from before the
/// root guard (`calimero_governance_types::ROOT_GUARD_SCHEMA_VERSION`).
///
/// Such an op applied under the old rule, which took a bare owner-level op
/// from a device key, so its bare form still folds as it always did. Without
/// this, a replica re-deriving its fold from stored history would silently drop
/// a TEE authoring policy or ownership transfer that its live rows still hold.
/// No op signed under the current schema reaches this: the schema gate refuses
/// an old version before anything is applied.
#[must_use]
pub fn payload_from_pre_guard_group_op(group: ContextGroupId, op: &GroupOp) -> Option<OpPayload> {
    match op {
        GroupOp::TransferOwnership { .. } | GroupOp::TeeAuthoringPolicySet { .. } => {
            Some(guarded_carried(group, op))
        }
        other => payload_from_group_op(group, other),
    }
}
