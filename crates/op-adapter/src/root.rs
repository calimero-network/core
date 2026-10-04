//! Admin/namespace plane: a namespace root governance op ([`RootOp`]) → its
//! `OpPayload`.

use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_governance_types::RootOp;
use calimero_op::{OpPayload, ScopeId, SeatDevice};
use calimero_primitives::context::GroupMemberRole;

use crate::credential::{credential_binds_the_member, join_credential_binds};

/// Encode a namespace root governance op ([`RootOp`]) as an [`OpPayload`].
///
/// **Coverage (admin + membership + scope-tree planes):**
/// `RootGuarded { AdminChanged }` → `RootGuarded { AdminChanged }` (a bare
/// `AdminChanged` folds to nothing, see `crate::guard`); `PolicyUpdated` → `PolicyUpdated`; `MemberJoinedOpen` →
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
/// Every arm reads the account off the OP. Nothing is derived from the signing
/// key any more, which is why this takes no signer: a fold that derives a
/// principal puts a second id space into the view, and whichever space the
/// resolver prefers, the other one mismatches.
#[must_use]
pub fn payload_from_root_op(op: &RootOp) -> Option<OpPayload> {
    match op {
        // A member's root op published by a relay folds as the op it carries,
        // like `GroupOp::OnBehalf` — plus the seat the apply gives the relay
        // for a creation, which the fold has to see as well as the rows.
        RootOp::OnBehalf { op, delegation } if op.delegable_form().is_some() => {
            relay_seat(op, delegation, payload_from_root_op(op)?)
        }
        // Owner-level: folds only with its root proof, whose group is the
        // namespace root. A bare `AdminChanged` falls through to `None`; see
        // `crate::guard`.
        RootOp::RootGuarded { op: inner, proof } => {
            let RootOp::AdminChanged { new_admin } = inner.as_ref() else {
                return None;
            };
            let kind = inner.owner_op_kind()?;
            let digest = inner.owner_op_digest().ok()?;
            // A root op names no group of its own: the proof's must be the
            // namespace root, which is the one it names as its namespace.
            if proof.statement.group_id != proof.statement.namespace_id {
                return None;
            }
            crate::guard::guarded_payload(
                ContextGroupId::from(proof.statement.namespace_id),
                kind,
                digest,
                proof,
                OpPayload::AdminChanged {
                    new_admin: *new_admin,
                },
            )
        }
        RootOp::PolicyUpdated { policy_bytes } => Some(OpPayload::PolicyUpdated {
            policy_bytes: policy_bytes.clone(),
        }),
        // `member` is DELIBERATELY still the stand-in, even though the credential
        // beside it names the joiner's real account.
        //
        // Switching the membership key to `account.cert.account` is correct and is
        // slice B's job, not this one — because it cannot be done alone. The
        // projection would then fold membership keyed by the REAL account while
        // `MembershipRepository` still keys its rows by member key, so the two
        // planes would disagree about who is a member: the membership-equivalence
        // suite fails with projection `Some(false)` against live `Some(true)`.
        //
        // The credential still has to be folded HERE, and that is not the same
        // question. A binding is written by the apply path the moment a join
        // lands, and `env::account_id()` reads those materialized rows — so
        // withholding the credential from the projection does not keep the
        // principal still, it just makes the peer that resolves the joiner's
        // signature disagree with the joiner about who wrote. Both planes have to
        // move together; that is exactly why the device half travels with the
        // membership half instead of waiting for it.
        RootOp::MemberJoined {
            member,
            signed_invitation,
            account,
        }
        | RootOp::MemberJoinedAt {
            member,
            signed_invitation,
            account,
            ..
        } => {
            let group = signed_invitation.invitation.group_id;
            let role =
                GroupMemberRole::from_invited_role(signed_invitation.invitation.invited_role);
            let member = *member;
            Some(if credential_binds_the_member(op) {
                OpPayload::MemberJoinedWithDevice {
                    group,
                    member,
                    role,
                    genesis: account.genesis,
                    chain: account.chain.clone(),
                    cert: account.statement,
                }
            } else {
                // Folds as a plain add, not as a join: the apply refuses a join
                // whose credential does not bind, so none here is a repeat join.
                OpPayload::MemberAdded {
                    group,
                    member,
                    role,
                }
            })
        }
        // No membership half: an open-subgroup self-join is a PROOF of
        // inheritance, never a direct row (see `op_from_namespace_op`, which
        // folds this op's membership as a graph-only node for that reason). The
        // credential it carries is still a real device link and folds as one —
        // or, if it is not the joiner's, as the graph-only node it used to be.
        RootOp::MemberJoinedOpen { account, .. } => Some(if credential_binds_the_member(op) {
            OpPayload::DeviceLinked {
                genesis: account.genesis,
                chain: account.chain.clone(),
                cert: account.statement,
                // A join op carries no scope statement; it binds at epoch 0,
                // as the live apply does.
                scope_epoch: 0,
            }
        } else {
            OpPayload::Noop
        }),
        // A TEE admission is a direct membership like an invitation join — the
        // replica gets a real row, not an inherited path — so it folds the same
        // way: membership and device as one indivisible fact, or membership
        // alone if the credential is not the attested key's.
        RootOp::MemberJoinedViaTeeAttestation {
            group_id,
            role,
            account,
            ..
        } => Some(if credential_binds_the_member(op) {
            OpPayload::MemberJoinedWithDevice {
                group: *group_id,
                // The op names the attested KEY, so the account comes from the
                // credential — which the branch condition has just confirmed
                // certifies that key.
                member: account.statement.account,
                role: role.clone(),
                genesis: account.genesis,
                chain: account.chain.clone(),
                cert: account.statement,
            }
        } else {
            // A credential that does not certify the attested key names no
            // principal this admission could be recorded under — the apply
            // refuses outright for the same reason, so the fold must not
            // invent a membership the rows will not have.
            OpPayload::Noop
        }),
        RootOp::GroupCreated {
            group_id,
            parent_id,
            restricted,
            admin,
            // The id's derivation input; apply checks it, the projection has
            // no use for it.
            salt: _,
        } => Some(OpPayload::SubgroupCreated {
            child: ScopeId::from(group_id.to_bytes()),
            parent: ScopeId::from(parent_id.to_bytes()),
            // Visibility is now carried atomically on the live op (#2771):
            // `restricted: true` = Restricted (default), `false` = born-Open.
            // This aligns the projection-plane `SubgroupCreated.restricted`
            // with the live op instead of hardcoding Restricted.
            restricted: *restricted,
            // The account the op carries, NOT one derived from the signer's key.
            // A derived id names no principal the account-keyed rows know, and
            // folding one here made the creator lose its own admin authority at
            // every cut after this op.
            admin: *admin,
        }),
        RootOp::GroupReparented {
            child_group_id,
            new_parent_id,
        } => Some(OpPayload::SubgroupReparented {
            child: ScopeId::from(child_group_id.to_bytes()),
            new_parent: ScopeId::from(new_parent_id.to_bytes()),
        }),
        RootOp::GroupDeleted { root_group_id, .. } => Some(OpPayload::SubgroupDeleted {
            scope: ScopeId::from(root_group_id.to_bytes()),
        }),
        // Genesis is where the founder's device becomes known, and it is the ONLY
        // place: no join op ever admits the founder. Folding it as a Noop — which
        // is what an unhandled arm does — leaves this plane unable to turn the
        // founder's signing key into the account the live rows are keyed by, so
        // every op the founder later signs is judged "not admin" at the cut and
        // rejected by every receiver while the publisher accepts it.
        //
        // The admin half needs no arm: the root's `admin_identity` reaches the
        // cut through `auth_cut_context`, which reads it from the root meta that
        // genesis wrote. Only the device link is missing here.
        RootOp::NamespaceCreatedV2 { account, .. } => {
            Some(if credential_binds_the_member(op) {
                OpPayload::DeviceLinked {
                    genesis: account.genesis,
                    chain: account.chain.clone(),
                    cert: account.statement,
                    // A join op carries no scope statement; it binds at epoch 0,
                    // as the live apply does.
                    scope_epoch: 0,
                }
            } else {
                OpPayload::Noop
            })
        }
        // Out-of-model: `KeyDelivery` is key transport, not authorization
        // state. (`RootOp` is `#[non_exhaustive]`, so a `_` arm is mandatory.)
        _ => None,
    }
}

/// [`payload_from_root_op`] for an op signed under a schema from before the
/// root guard. See [`crate::payload_from_pre_guard_group_op`] for why.
#[must_use]
pub fn payload_from_pre_guard_root_op(op: &RootOp) -> Option<OpPayload> {
    match op {
        RootOp::AdminChanged { new_admin } => Some(OpPayload::AdminChanged {
            new_admin: *new_admin,
        }),
        other => payload_from_root_op(other),
    }
}

/// `carried` — what a relay-published root op folds as — together with the seat
/// the apply gives that relay, when it gives one.
///
/// Two creations seat their relay, and each must fold its seat or the relay is
/// a member in the rows and a stranger at every cut, refused by every read the
/// projection gates:
///
/// - a subgroup's creation seats the relay in the subgroup (`seat_creating_relay`
///   in `calimero-governance-store`) — with its namespace-root TEE role and no
///   capabilities when it is a TEE there, since that role is its attestation
///   verdict and a `RelayTee` relays by role;
/// - a namespace's founding seats the relay in the namespace root and binds its
///   device there (`seat_founding_relay`), since it has no earlier op that
///   could have.
///
/// Otherwise both seats are `Member` holding `CAN_AUTHOR_ON_BEHALF`. The relay is the
/// warrant's executor and the namespace is the warrant's scope — the apply
/// refuses a root warrant scoped anywhere else. The device is folded only when
/// the credential binds the executor, the op-local half of the check the apply
/// makes before it links it.
fn relay_seat(
    op: &RootOp,
    delegation: &calimero_account::GovernanceDelegation,
    carried: OpPayload,
) -> Option<OpPayload> {
    let relay = delegation.warrant.executor;
    let namespace = ContextGroupId::from(delegation.warrant.scope);
    let (group, device, tee_role_from) = match op {
        RootOp::GroupCreated { group_id, .. } => (
            ContextGroupId::from(group_id.to_bytes()),
            None,
            Some(namespace),
        ),
        RootOp::NamespaceCreatedV2 { .. } => {
            let proof = &delegation.executor_proof;
            let device = join_credential_binds(&relay, proof).then(|| {
                Box::new(SeatDevice {
                    genesis: proof.genesis,
                    chain: proof.chain.clone(),
                    cert: proof.statement,
                })
            });
            (namespace, device, None)
        }
        _ => return Some(carried),
    };
    Some(OpPayload::RelaySeated {
        carried: Box::new(carried),
        group,
        relay,
        capabilities: MemberCapabilities::CAN_AUTHOR_ON_BEHALF,
        device,
        tee_role_from,
    })
}
