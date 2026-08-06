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
use sha2::{Digest, Sha256};

use calimero_account::{AccountId, DeviceId};
use calimero_op::{Authorship, OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::rotation_log::RotationLogEntry;

/// Domain separator for the adapter's stand-in account derivation. Distinct
/// from every domain in `calimero-account` so a value minted here can never be
/// mistaken for — or collide with — a real account id.
const LEGACY_ACCOUNT_DOMAIN: &[u8] = b"calimero.op-adapter.legacy-account.v1";

/// The stand-in account for a legacy member key.
///
/// **Adapter-local, and not a protocol concept.** Per-plane ops name a bare
/// member key because they predate accounts; the unified planes are keyed by
/// [`AccountId`]. This derivation is the seam between the two, and it exists
/// only so the fold-equivalence proofs can compare like with like.
///
/// It is deliberately *not* something `calimero-account` offers. A real account
/// is the content address of a genesis whose root key can rotate and whose
/// devices can be revoked; a value derived from a bare key has none of those
/// properties, and exposing one as a first-class account would quietly
/// reintroduce the id-equals-key conflation the account model exists to remove.
/// Living in the adapter — the crate that is deleted at cutover — is what keeps
/// it from outliving the transition.
#[must_use]
pub fn legacy_account_id(member: &PublicKey) -> AccountId {
    let mut hasher = Sha256::new();
    hasher.update((LEGACY_ACCOUNT_DOMAIN.len() as u64).to_le_bytes());
    hasher.update(LEGACY_ACCOUNT_DOMAIN);
    hasher.update(AsRef::<[u8; 32]>::as_ref(member));
    let digest: [u8; 32] = hasher.finalize().into();
    AccountId::from(digest)
}

/// The account a signing key writes as, on the **writer** plane.
///
/// One rule, used at both ends: the node deciding what to put in a writer set
/// (`env::account_id()`), and the peer resolving an incoming signature against
/// one. They have to agree, and they can only agree by sharing this.
///
/// `binding` is that key's device binding — its real [`AccountId`] — if one has
/// been published. Absent that, the key writes as its own stand-in, which is what
/// any peer can derive from the key alone. That fallback is what makes an
/// unenrolled node usable at all: it has an account nobody else can compute (an
/// id derived from its private root), so naming *that* in a writer set would
/// produce a grant no peer could ever match.
///
/// **The precedence is the opposite of the membership plane's, deliberately.**
/// There, a key that is a member in its own right *is* that member and a binding
/// is only a fallthrough — preferring the binding erased members whose rows are
/// keyed by the stand-in. Here the writer set is populated from
/// `env::account_id()`, so resolution has to answer in whatever space that
/// returns, and the binding is what makes a person's second device write under
/// the same principal as their first. The two planes converge when the legacy
/// bridge retires.
///
/// Consequence worth stating: a writer set seeded BEFORE the writer enrolled
/// holds the stand-in, so enrolling afterwards changes what that key writes as and
/// the old grant goes stale. Re-grant after `account create`.
#[must_use]
pub fn writer_account(binding: Option<AccountId>, key: &PublicKey) -> AccountId {
    binding.unwrap_or_else(|| legacy_account_id(key))
}

/// The [`Authorship`] a bridged legacy op carries.
///
/// Legacy ops name only a signing key, so the device is derived from the
/// stand-in account rather than enrolled. Like [`legacy_account_id`], this is a
/// transition artifact: natively authored ops carry a real enrolled device.
#[must_use]
pub fn legacy_authorship(signer: PublicKey) -> Authorship {
    let account = legacy_account_id(&signer);
    Authorship {
        account,
        device: DeviceId::from(*account.as_bytes()),
        device_key: signer,
    }
}

/// Encode a storage data [`Action`] as an [`OpPayload`].
///
/// Every state-changing [`Action`] maps to an op, so this currently always
/// returns `Some`; the `Option` is retained so a future non-state-changing
/// action can encode as `None` without a signature change.
#[must_use]
pub fn payload_from_action(action: &Action) -> Option<OpPayload> {
    match action {
        Action::Add { id, data, .. } | Action::Update { id, data, .. } => Some(OpPayload::Put {
            entity: *id,
            value: data.clone(),
        }),
        Action::DeleteRef { id, .. } => Some(OpPayload::Delete { entity: *id }),
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
        // Passed through, not bridged: a rotation log's writer set is ALREADY
        // account-keyed, so there is no key here to stand in for. The entry's
        // `signer` still needs `legacy_account_id`, because a signature names a
        // key — which is exactly the split the account plane draws.
        writers: entry.new_writers.clone(),
    }
}

/// Whether a join op's credential is certified for the key that is joining.
///
/// The projection has to reach the SAME verdict the apply path does, and for the
/// same reason it exists there: join ops are cleartext, so any node can lift a
/// credential out of another join and present it as its own. If the two planes
/// disagree here, a replayed credential is refused a binding in the materialized
/// rows and granted one in the folded view — the same split this whole change
/// closes, running the other way.
///
/// The certificate names the namespace identity as `sign_pk` because that is the
/// key that signs ops, so for an honest joiner this already holds.
fn credential_is_the_joiners(op: &RootOp) -> bool {
    match op {
        RootOp::MemberJoined {
            member, account, ..
        }
        | RootOp::MemberJoinedAt {
            member, account, ..
        }
        | RootOp::MemberJoinedOpen {
            member, account, ..
        } => account.cert.sign_pk == *member,
        _ => false,
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
                member: legacy_account_id(member),
                role: role.clone(),
            })
        }
        GroupOp::MemberRemoved { member, .. } | GroupOp::MemberLeft { member, .. } => {
            Some(OpPayload::MemberRemoved {
                group,
                member: legacy_account_id(member),
            })
        }
        GroupOp::TransferOwnership { new_owner } => Some(OpPayload::AdminChanged {
            new_admin: legacy_account_id(new_owner),
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
            member: legacy_account_id(member),
            capabilities: *capabilities,
        }),
        // Visibility plane — the Open/Restricted wall that gates inheritance.
        // Live mode byte: 0 = Open, anything else = Restricted.
        GroupOp::SubgroupVisibilitySet { mode } => Some(OpPayload::SubgroupVisibilitySet {
            scope: ScopeId::from(group.to_bytes()),
            restricted: matches!(mode, calimero_context_config::VisibilityMode::Restricted),
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
            new_admin: legacy_account_id(new_admin),
        }),
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
            let member = legacy_account_id(member);
            Some(if credential_is_the_joiners(op) {
                OpPayload::MemberJoinedWithDevice {
                    group,
                    member,
                    role,
                    genesis: account.genesis,
                    chain: account.chain.clone(),
                    cert: account.cert,
                }
            } else {
                // Membership stands, the device does not — the same verdict the
                // apply path reaches, which is the whole requirement.
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
        // BISECT PROBE (temporary): open-join credentials are NOT folded, so this
        // op is the graph-only node it was before this branch. Invitation joins
        // still fold theirs. Reverting this half isolates whether the
        // dm-subgroup-privacy regression comes from the open-join fold.
        RootOp::MemberJoinedOpen { .. } => Some(OpPayload::Noop),
        RootOp::GroupCreated {
            group_id,
            parent_id,
            restricted,
        } => Some(OpPayload::SubgroupCreated {
            child: ScopeId::from(group_id.to_bytes()),
            parent: ScopeId::from(parent_id.to_bytes()),
            // Visibility is now carried atomically on the live op (#2771):
            // `restricted: true` = Restricted (default), `false` = born-Open.
            // This aligns the projection-plane `SubgroupCreated.restricted`
            // with the live op instead of hardcoding Restricted.
            restricted: *restricted,
            admin: legacy_account_id(&signer),
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
        // Out-of-model: `KeyDelivery` is key transport, not authorization
        // state. (`RootOp` is `#[non_exhaustive]`, so a `_` arm is mandatory.)
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    /// A joiner credential for the bridge tests. These assert what
    /// `payload_from_root_op` FOLDS, and the bridge deliberately still keys
    /// membership by `legacy_account_id(member)` until slice B moves the live plane
    /// too — so nothing here reads the credential, it only has to be present.
    fn test_join_account_for(
        sign_pk: PublicKey,
    ) -> Box<calimero_governance_types::JoinAccountCredential> {
        let root = calimero_primitives::identity::PublicKey::from([0x7A; 32]);
        let genesis = calimero_account::AccountGenesis::new(root, [0x5A; 16]);
        Box::new(calimero_governance_types::JoinAccountCredential {
            cert: calimero_account::DeviceCert {
                account: genesis.account_id(),
                device: calimero_account::DeviceId::from([0x3E; 32]),
                sign_pk,
                kem_pk: calimero_account::KemPublicKey::from([0x2B; 32]),
                key_epoch: 0,
                device_epoch: 0,
                signature: [0x11; 64],
            },
            genesis,
            chain: vec![],
        })
    }

    use super::*;

    use core::num::NonZeroU128;
    use std::collections::BTreeMap;

    use calimero_op::{Op, OpPayload, ScopeId};
    use calimero_primitives::identity::PublicKey;
    use calimero_projection::ScopeState;
    use calimero_storage::entities::{Metadata, OpMask};
    use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
    use calimero_storage::rotation_log::{RotationLog, RotationLogEntry};

    /// A governance `AccountDeviceLinked` must reach the projection's account
    /// plane and land as a folded device binding.
    ///
    /// Before the account arms existed this returned `None`, the op folded to
    /// `Noop`, and `AclView.devices` stayed empty on the governance path — so the
    /// complete, tested account plane in `crates/projection` guarded nothing, and
    /// per-device authorization had no way to resolve a signing key to the account
    /// it speaks for at a causal cut.
    #[test]
    fn a_governance_device_link_reaches_the_projection() {
        use calimero_account::{sign_device_cert, AccountGenesis, DeviceId, KemPublicKey};
        use calimero_primitives::identity::PrivateKey;

        let root = PrivateKey::from([1u8; 32]);
        let genesis = AccountGenesis::new(root.public_key(), [1u8; 16]);
        let account = genesis.account_id();
        let device = DeviceId::mint(account, [5u8; 16]);
        let cert = sign_device_cert(
            &root,
            account,
            device,
            &PrivateKey::from([5u8; 32]).public_key(),
            &KemPublicKey::from([5u8; 32]),
            0,
            0,
        )
        .expect("sign cert");

        let group = ContextGroupId::from([9u8; 32]);
        let payload = payload_from_group_op(
            group,
            &GroupOp::AccountDeviceLinked {
                genesis,
                chain: vec![],
                cert,
                endorsement: calimero_account::sign_account_endorsement(&root, account)
                    .expect("sign endorsement"),
            },
        )
        .expect("the account ops must map to a unified payload, not fold to Noop");
        assert!(matches!(payload, OpPayload::DeviceLinked { .. }));

        // And it must actually fold: a payload that never becomes a binding would
        // satisfy the assertion above while leaving the plane just as blind.
        let op = Op::from_parts(
            [7u8; 32],
            ScopeId::from([9u8; 32]),
            vec![],
            legacy_authorship(root.public_key()),
            hlc(1),
            payload,
            [0u8; 32],
            [0u8; 64],
        );
        let mut state = ScopeState::default();
        state.apply(&op);
        let view = state.acl_view();
        assert!(
            view.devices.contains_key(&device),
            "the folded view must know the device, or per-device authorization has \
             nothing to resolve against"
        );
        assert_eq!(
            view.devices.get(&device).map(|b| b.account),
            Some(account),
            "and it must know which account the device speaks for"
        );

        // And the reason the binding has to be consulted at all: an account
        // DERIVED from the device's signing key is a different account, and not a
        // member. Resolving a signer through `legacy_account_id` alone therefore
        // answers about somebody who does not exist, which is exactly why a second
        // device could receive scope keys and then not author with them.
        let device_sign_pk = PrivateKey::from([5u8; 32]).public_key();
        assert_ne!(
            legacy_account_id(&device_sign_pk),
            account,
            "if the derived account happened to equal the real one, the check \
             below would pass for the wrong reason"
        );
        assert!(
            !view.is_scope_member(&legacy_account_id(&device_sign_pk)),
            "the account derived from a device key must not be a member"
        );
    }

    /// **Revoking a device withdraws the authority its key had to write as its
    /// account** — on the writer plane, not just the membership plane.
    ///
    /// The writer plane resolves a signature to a principal through
    /// [`writer_account`], which is what `ScopeProjections::device_account_at_cut`
    /// arms the receiver with. Revocation removes the binding, so the key stops
    /// resolving to the account and a writer set naming that account no longer
    /// matches it.
    ///
    /// The refusal is checked to follow from the revocation and not from some
    /// incidental gap: the tombstone is asserted present while the binding is
    /// asserted gone, which is what distinguishes a revoked device from one whose
    /// link this node simply has not folded yet. Those two must not be confused —
    /// the second is a timing gap that has to defer, the first is terminal.
    #[test]
    fn revoking_a_device_withdraws_its_authority_on_the_writer_plane() {
        use calimero_account::{sign_device_cert, AccountGenesis, DeviceId, KemPublicKey};
        use calimero_primitives::identity::PrivateKey;

        let root = PrivateKey::from([1u8; 32]);
        let genesis = AccountGenesis::new(root.public_key(), [1u8; 16]);
        let account = genesis.account_id();
        let device = DeviceId::mint(account, [5u8; 16]);
        let device_sk = PrivateKey::from([5u8; 32]);
        let device_key = device_sk.public_key();
        let cert = sign_device_cert(
            &root,
            account,
            device,
            &device_key,
            &KemPublicKey::from([5u8; 32]),
            0,
            0,
        )
        .expect("sign cert");
        let group = ContextGroupId::from([9u8; 32]);

        // The receiver's rule, verbatim: find a live binding for the signing key,
        // else fall back to the key's stand-in account.
        let resolve = |view: &calimero_authz::AclView, key: &PublicKey| {
            let binding = view
                .devices
                .values()
                .find(|b| b.sign_pk == *key)
                .map(|b| b.account);
            writer_account(binding, key)
        };

        let link = Op::from_parts(
            [7u8; 32],
            ScopeId::from([9u8; 32]),
            vec![],
            legacy_authorship(root.public_key()),
            hlc(1),
            payload_from_group_op(
                group,
                &GroupOp::AccountDeviceLinked {
                    genesis,
                    chain: vec![],
                    cert,
                    endorsement: calimero_account::sign_account_endorsement(&root, account)
                        .expect("sign endorsement"),
                },
            )
            .expect("device link maps to a payload"),
            [0u8; 32],
            [0u8; 64],
        );
        let revoke = Op::from_parts(
            [8u8; 32],
            ScopeId::from([9u8; 32]),
            vec![[7u8; 32]],
            legacy_authorship(root.public_key()),
            hlc(2),
            OpPayload::DeviceRevoked { account, device },
            [0u8; 32],
            [0u8; 64],
        );

        // At the cut before the revocation the device writes as its account, so a
        // writer set naming the account admits it.
        let mut linked = ScopeState::default();
        linked.apply(&link);
        let before = linked.acl_view();
        assert_eq!(
            resolve(&before, &device_key),
            account,
            "precondition: while bound, the device's key must resolve to its              account, or the assertion below would hold for a device that never              had authority in the first place"
        );

        // Fold the revocation. The binding is gone, the tombstone is there.
        let mut revoked_state = linked;
        revoked_state.apply(&revoke);
        let after = revoked_state.acl_view();
        assert!(
            after.revoked_devices.contains(&device),
            "the revocation must be recorded, or the refusal below proves nothing              about revocation"
        );
        assert!(
            !after.devices.contains_key(&device),
            "and the binding must be withdrawn — a revocation that left the              binding in force would resolve the thief's key to the account"
        );

        let resolved = resolve(&after, &device_key);
        assert_ne!(
            resolved, account,
            "a revoked device must no longer write as the account it was              withdrawn from"
        );
        assert_eq!(
            resolved,
            legacy_account_id(&device_key),
            "it falls back to speaking only for itself"
        );

        // **The caveat, asserted rather than assumed.** The fallback is a stable
        // account, so a writer set that names a device's STAND-IN — as happens
        // when a set is seeded for a key before its account exists — keeps
        // admitting that key after revocation. That is consistent rather than a
        // hole: revoking a device withdraws its authority to speak for an
        // ACCOUNT, and says nothing about a grant made to the key itself, which
        // is undone by rotating the writer set. Worth pinning, because "I revoked
        // the device and it can still write" is a surprising way to learn it.
        assert_eq!(
            resolved,
            resolve(&after, &device_key),
            "the stand-in is stable, so this refusal is permanent rather than a              retryable one — the caller must not treat it as a timing gap"
        );

        // Causal honour on the writer plane: the pre-revocation cut still resolves
        // to the account, so a write authored before the revocation stays
        // authorized when re-judged at its own cut. `device_account_at_cut` takes
        // the write's heads for exactly this reason — resolving at the receiver's
        // latest cut would retroactively invalidate history the sender's root hash
        // already includes, leaving the two unable to agree on a root.
        assert_eq!(
            resolve(&before, &device_key),
            account,
            "an earlier cut must keep its answer after a later revocation folds"
        );
    }

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
        // The admin SIGNS, so it is a key; the writers are granted, so they are
        // accounts. Different domains — the bridge derives a stand-in account for
        // the signer, and passes the writer set through untouched.
        let admin = PublicKey::from([1u8; 32]);
        let w1 = AccountId::from([0x11; 32]);
        let w2 = AccountId::from([0x22; 32]);

        // Three sequential rotations: {w1} → {w1,w2} → {w2}.
        let sets: Vec<BTreeMap<AccountId, OpMask>> = vec![
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
            let payload = set_writers_payload(object, entries.last().expect("just pushed"));
            let parents: Vec<[u8; 32]> = prev_id.into_iter().collect();
            let authorship = legacy_authorship(admin);
            let id = Op::compute_id(scope, &parents, &authorship, &h, &payload);
            ops.push(Op::from_parts(
                id, scope, parents, authorship, h, payload, [0u8; 32], [0u8; 64],
            ));
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

        // Fold-equivalence still holds under the account model, modulo the
        // No mapping any more: both sides speak accounts, because the rotation
        // log's writer set is account-keyed at the source. The equivalence is
        // therefore direct, and still catches the bridge dropping or renaming a
        // writer.
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
        let writers: BTreeMap<AccountId, OpMask> = [(AccountId::from([7u8; 32]), OpMask::FULL)]
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
        let op = Op::new(
            scope,
            vec![],
            legacy_authorship(admin),
            entry.delta_hlc,
            payload,
            [0u8; 32],
            [0u8; 64],
        );

        let resolved = ScopeState::from_ops([&op])
            .acl_view()
            .acl
            .get(&object)
            .cloned()
            .unwrap_or_default();
        // Verbatim: the payload carries the log's own account-keyed set, so a
        // round trip must return exactly it.
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
                member: legacy_account_id(&m),
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
                member: legacy_account_id(&m),
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
                member: legacy_account_id(&m),
                role: GroupMemberRole::ReadOnlyTee,
            })
        );
        // Ownership transfer sets the group scope's root admin (owner ⇔ ADMIN).
        let new_owner = PublicKey::from([0x77; 32]);
        assert_eq!(
            payload_from_group_op(group, &GroupOp::TransferOwnership { new_owner }),
            Some(OpPayload::AdminChanged {
                new_admin: legacy_account_id(&new_owner),
            })
        );
        // Out-of-model ops (metadata, config, …) → None.
        assert_eq!(payload_from_group_op(group, &GroupOp::Noop), None);
        // Capability plane is now folded (gates inherited membership).
        assert_eq!(
            payload_from_group_op(
                group,
                &GroupOp::DefaultCapabilitiesSet {
                    capabilities: calimero_context_config::MemberCapabilities::from_bits_truncate(
                        7
                    )
                }
            ),
            Some(OpPayload::DefaultCapabilitiesSet {
                group,
                capabilities: calimero_context_config::MemberCapabilities::from_bits_truncate(7),
            })
        );
    }

    /// A credential that is not the joiner's folds like the apply path refuses it.
    ///
    /// Both planes have to reach the same verdict. If the projection admitted a
    /// lifted credential the apply path refuses, a replayed device would be bound
    /// in the folded view and absent from the materialized rows — the same split
    /// this encoder exists to close, running the other way.
    #[test]
    fn a_replayed_credential_folds_no_device_on_either_join_shape() {
        use calimero_context_config::types::{GroupInvitationFromAdmin, SignedGroupOpenInvitation};

        let m = PublicKey::from([7u8; 32]);
        let stranger = PublicKey::from([8u8; 32]);
        let gid = [0x44; 32];

        // Open self-join: back to the graph-only node, no device.
        assert_eq!(
            payload_from_root_op(
                &RootOp::MemberJoinedOpen {
                    member: m,
                    group_id: gid.into(),
                    account: test_join_account_for(stranger),
                },
                PublicKey::from([1u8; 32])
            ),
            Some(OpPayload::Noop)
        );

        // Invitation join: the membership still stands, the device does not.
        let signed_invitation = SignedGroupOpenInvitation {
            invitation: GroupInvitationFromAdmin {
                inviter_identity: [0xA1; 32].into(),
                group_id: ContextGroupId::from(gid),
                expiration_timestamp: 1_700_000_000,
                invitation_nonce: [0x33; 32],
                invited_role: 0,
            },
            inviter_signature: "deadbeef".to_string(),
            application_id: None,
            app_key: None,
        };
        assert_eq!(
            payload_from_root_op(
                &RootOp::MemberJoined {
                    member: m,
                    signed_invitation,
                    account: test_join_account_for(stranger),
                },
                PublicKey::from([1u8; 32])
            ),
            Some(OpPayload::MemberAdded {
                group: ContextGroupId::from(gid),
                member: legacy_account_id(&m),
                role: GroupMemberRole::Admin,
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
            Some(OpPayload::AdminChanged {
                new_admin: legacy_account_id(&admin),
            })
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
        // BISECT PROBE (temporary): open joins fold no device while we isolate the
        // dm-subgroup-privacy regression, so this is the graph-only node again.
        assert_eq!(
            payload_from_root_op(
                &RootOp::MemberJoinedOpen {
                    member: m,
                    group_id: gid.into(),
                    account: test_join_account_for(m),
                },
                PublicKey::from([1u8; 32])
            ),
            Some(OpPayload::Noop)
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
                invitation_nonce: [0x33; 32],
                invited_role: 0, // Admin
            },
            inviter_signature: "deadbeef".to_string(),
            application_id: None,
            app_key: None,
        };
        let invited = test_join_account_for(m);
        assert_eq!(
            payload_from_root_op(
                &RootOp::MemberJoined {
                    member: m,
                    signed_invitation: signed_invitation.clone(),
                    account: invited.clone(),
                },
                PublicKey::from([1u8; 32])
            ),
            Some(OpPayload::MemberJoinedWithDevice {
                group: ContextGroupId::from(gid),
                member: legacy_account_id(&m),
                role: GroupMemberRole::Admin,
                genesis: invited.genesis,
                chain: invited.chain.clone(),
                cert: invited.cert,
            })
        );
        // `MemberJoinedAt` (the timestamped invitation join `join_group` emits)
        // decodes identically — it is NOT out-of-model.
        let invited_at = test_join_account_for(m);
        assert_eq!(
            payload_from_root_op(
                &RootOp::MemberJoinedAt {
                    member: m,
                    signed_invitation,
                    joined_at: 42,
                    account: invited_at.clone(),
                },
                PublicKey::from([1u8; 32])
            ),
            Some(OpPayload::MemberJoinedWithDevice {
                group: ContextGroupId::from(gid),
                member: legacy_account_id(&m),
                role: GroupMemberRole::Admin,
                genesis: invited_at.genesis,
                chain: invited_at.chain.clone(),
                cert: invited_at.cert,
            })
        );
        let parent = [0x70; 32]; // placeholder parent id
        assert_eq!(
            payload_from_root_op(
                &RootOp::GroupCreated {
                    group_id: gid.into(),
                    parent_id: parent.into(),
                    restricted: true,
                },
                PublicKey::from([1u8; 32])
            ),
            Some(OpPayload::SubgroupCreated {
                child: ScopeId::from(gid),
                parent: ScopeId::from(parent),
                restricted: true,
                admin: legacy_account_id(&PublicKey::from([1u8; 32])),
            })
        );
        // Scope-tree restructure ops now map to the structural OpPayload arms.
        assert_eq!(
            payload_from_root_op(
                &RootOp::GroupReparented {
                    child_group_id: gid.into(),
                    new_parent_id: [9u8; 32].into(),
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
                    root_group_id: gid.into(),
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
            Op::new(
                scope,
                vec![],
                legacy_authorship(admin),
                h,
                payload,
                [0u8; 32],
                [0u8; 64],
            )
        };

        // Add(Member)@10 → Remove@20 → Add(Admin)@30 → present as Admin.
        let ops = vec![
            build(
                10,
                OpPayload::MemberAdded {
                    group,
                    member: legacy_account_id(&m),
                    role: GroupMemberRole::Member,
                },
            ),
            build(
                20,
                OpPayload::MemberRemoved {
                    group,
                    member: legacy_account_id(&m),
                },
            ),
            build(
                30,
                OpPayload::MemberAdded {
                    group,
                    member: legacy_account_id(&m),
                    role: GroupMemberRole::Admin,
                },
            ),
        ];
        let groups = ScopeState::from_ops(&ops).acl_view().groups;
        assert_eq!(
            groups
                .get(&group)
                .and_then(|g| g.get(&legacy_account_id(&m))),
            Some(&GroupMemberRole::Admin),
            "re-add after remove wins with the new role"
        );

        // Same set ending in Remove@40 → member absent.
        let mut ops2 = ops;
        ops2.push(build(
            40,
            OpPayload::MemberRemoved {
                group,
                member: legacy_account_id(&m),
            },
        ));
        let groups2 = ScopeState::from_ops(&ops2).acl_view().groups;
        assert_eq!(
            groups2
                .get(&group)
                .and_then(|g| g.get(&legacy_account_id(&m))),
            None,
            "final removal drops the member"
        );
    }
}
