//! Authorization classification of every signed governance op variant.
//! `GroupOp` and `NamespaceOp` are `#[non_exhaustive]`, so only this crate can match them without a wildcard.

use super::{GroupOp, NamespaceOp, RootOp};

enum Authz {
    /// Covered by the authorization-matrix rows of these `GatedOp` variants.
    Gated(&'static [&'static str]),
    /// Confers or removes no standing, capability, key or membership.
    Ungated(&'static str),
    /// Makes an authorization decision, or grants something, that no matrix row exercises.
    Uncovered(&'static str),
}

macro_rules! gated {
    ($($op:literal),+) => {{
        $(const _: () = assert!(!$op.is_empty());)+
        Authz::Gated(&[$($op),+])
    }};
}

macro_rules! ungated {
    ($reason:literal) => {{
        const _: () = assert!(!$reason.is_empty());
        Authz::Ungated($reason)
    }};
}

macro_rules! uncovered {
    ($grants:literal) => {{
        const _: () = assert!(!$grants.is_empty());
        Authz::Uncovered($grants)
    }};
}

fn classify_group_op(op: &GroupOp) -> Authz {
    match op {
        GroupOp::Noop => ungated!("changes nothing; only a member may append it"),
        GroupOp::MemberAdded { .. } => {
            uncovered!("adds a member and may grant admin, behind manage-members")
        }
        GroupOp::MemberRemoved { .. } => uncovered!("removes a member, behind manage-members"),
        GroupOp::MemberLeft { .. } => ungated!("a member removes only itself"),
        GroupOp::MemberRoleSet { .. } => uncovered!("promotes or demotes a member, admin only"),
        GroupOp::MemberCapabilitySet { .. } => {
            uncovered!("grants a member capabilities, admin only")
        }
        GroupOp::DefaultCapabilitiesSet { .. } => {
            uncovered!("sets the capabilities every new member gets")
        }
        GroupOp::TargetApplicationSet { .. } => {
            uncovered!("points the group at an application, behind manage-application")
        }
        GroupOp::ContextRegistered { .. } => uncovered!("registers a context under the group"),
        GroupOp::ContextDetached { .. } => uncovered!("detaches a registered context"),
        GroupOp::SubgroupVisibilitySet { .. } => {
            uncovered!("opens or restricts a subgroup to inherited members")
        }
        GroupOp::GroupMetadataSet { .. }
        | GroupOp::MemberMetadataSet { .. }
        | GroupOp::ContextMetadataSet { .. } => ungated!("metadata only; grants no authority"),
        GroupOp::GroupDelete => uncovered!("deletes the group, owner only"),
        GroupOp::GroupMigrationSet { .. } => uncovered!("sets the group's migration payload"),
        GroupOp::ContextCapabilityGranted { .. } => {
            uncovered!("grants a member a capability on a context")
        }
        GroupOp::ContextCapabilityRevoked { .. } => {
            uncovered!("revokes a member's capability on a context")
        }
        GroupOp::TeeAdmissionPolicySet { .. }
        | GroupOp::TeeAdmissionPolicySetV2 { .. }
        | GroupOp::TeeReleaseAdmissionPolicySet { .. }
        | GroupOp::TeeReleaseAdmissionPolicySetV2 { .. } => {
            uncovered!("sets which TEE attestations admit a member, owner only")
        }
        GroupOp::TeeAuthoringPolicySet { .. } => {
            uncovered!("sets which TEE images may author, owner only")
        }
        GroupOp::MemberJoinedViaTeeAttestation { .. } => {
            uncovered!("admits a TEE member on a verifier's attestation")
        }
        GroupOp::MemberSetAutoFollow { .. } => {
            ungated!("an admin or the member itself toggles follow flags")
        }
        GroupOp::TransferOwnership { .. } => uncovered!("moves group ownership, owner only"),
        GroupOp::CascadeUpgrade { .. } => {
            uncovered!("upgrades the group and its descendants, behind manage-application")
        }
        GroupOp::GroupKeyRotated { .. } | GroupOp::GroupKeyRotatedForDevice { .. } => {
            uncovered!("rotates the group key, admin only")
        }
        GroupOp::AccountDeviceLinked { .. } => gated!("DeviceLink"),
        GroupOp::AccountDeviceUnlinked { .. } => gated!("DeviceRevoke"),
        GroupOp::AccountDeviceDescoped { .. } => gated!("DeviceDescope", "ForeignDescope"),
        GroupOp::AccountKeysRotated { .. } => uncovered!("hands an account's root to a new key"),
        GroupOp::AccountDeviceCertified { .. } => {
            uncovered!("records a device certificate and scope for an account")
        }
        GroupOp::AccountNamespaceGained { .. } | GroupOp::AccountNamespaceLeft { .. } => {
            uncovered!("changes the namespaces an account's devices follow")
        }
        GroupOp::AccountDeviceLabelled { .. } => ungated!("names a device; grants no authority"),
        GroupOp::TeeAuthorityEvidence { .. } => {
            uncovered!("records evidence that makes a key a TEE authority")
        }
        GroupOp::TeeVaultKeyDelivered { .. } => {
            uncovered!("delivers the namespace TEE key, signer must hold a TEE role")
        }
        GroupOp::ContextRegisteredOnBehalf { .. } => {
            uncovered!("a relay registers a context for an author under a delegation")
        }
        GroupOp::OnBehalf { .. } => uncovered!("a relay applies a delegated group op, relay seat"),
        GroupOp::FoundingRelayAttested { .. } => {
            uncovered!("grants the founding relay its attested standing")
        }
        GroupOp::RootGuarded { .. } => {
            uncovered!("applies an owner-level op on the account root's proof")
        }
        GroupOp::SharedWritersRotated { .. } => uncovered!("replaces a shared cell's writer set"),
    }
}

fn classify_root_op(op: &RootOp) -> Authz {
    match op {
        RootOp::GroupCreated { .. } => uncovered!("creates a subgroup under a parent, namespace admin or the creator capability"),
        RootOp::GroupDeleted { .. } => uncovered!("deletes a subgroup tree, owner or delete-subgroup capability"),
        RootOp::GroupReparented { .. } => uncovered!("moves a subgroup, changing who inherits into it"),
        RootOp::AdminChanged { .. } => uncovered!("changes the namespace admin, owner level"),
        RootOp::PolicyUpdated { .. } => uncovered!("replaces the namespace policy, namespace admin"),
        RootOp::MemberJoinedViaTeeAttestation { .. } => uncovered!("admits a TEE member on a verifier's attestation"),
        RootOp::MemberJoined { .. } | RootOp::MemberJoinedAt { .. } => uncovered!("admits a member on a signed invitation; the join-key rows judge the sync request, not this apply"),
        RootOp::MemberJoinedOpen { .. } => uncovered!("joins an Open subgroup by inheritance; the join-key rows judge the key request, not this apply"),
        RootOp::NamespaceCreatedV2 { .. } => uncovered!("founds a namespace whose id must derive from the signer's founder and salt"),
        RootOp::KeyDelivery { .. } => ungated!("applies as a no-op; the key moved to the pull exchange"),
        RootOp::OnBehalf { .. } => uncovered!("a relay applies a delegated namespace op, relay seat"),
        RootOp::RootGuarded { .. } => uncovered!("applies an owner-level op on the account root's proof"),
    }
}

fn classify_namespace_op(op: &NamespaceOp) -> Authz {
    match op {
        NamespaceOp::Root(_) => ungated!("carrier; judged as the RootOp it holds"),
        NamespaceOp::Group { .. } => ungated!("carrier; judged as the GroupOp it decrypts to"),
        NamespaceOp::RootSealed { .. } | NamespaceOp::RootSealedForGroup { .. } => {
            ungated!("carrier; judged as the RootOp it opens to")
        }
        NamespaceOp::RootRelaySealed { .. } => {
            ungated!("carrier; judged as the joiner's own signed op it holds")
        }
    }
}

fn text(authz: &Authz) -> String {
    match authz {
        Authz::Gated(ops) => ops.join(","),
        Authz::Ungated(reason) | Authz::Uncovered(reason) => (*reason).to_owned(),
    }
}

#[test]
fn a_variant_that_makes_a_decision_no_row_exercises_is_listed_as_uncovered() {
    let delete = classify_group_op(&GroupOp::GroupDelete);
    let policy = classify_root_op(&RootOp::PolicyUpdated {
        policy_bytes: vec![],
    });
    assert!(matches!(delete, Authz::Uncovered(_)));
    assert!(matches!(policy, Authz::Uncovered(_)));
    assert!(!text(&delete).is_empty() && !text(&policy).is_empty());
}

#[test]
fn a_variant_that_confers_nothing_is_ungated_and_a_carrier_defers_to_its_payload() {
    let noop = classify_group_op(&GroupOp::Noop);
    let carrier = classify_namespace_op(&NamespaceOp::Root(RootOp::PolicyUpdated {
        policy_bytes: vec![],
    }));
    assert!(matches!(noop, Authz::Ungated(_)));
    assert!(matches!(carrier, Authz::Ungated(_)));
    assert!(!text(&noop).is_empty() && !text(&carrier).is_empty());
}
