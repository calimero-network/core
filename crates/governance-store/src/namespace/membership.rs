use crate::{MembershipRepository, NamespaceRepository};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::types::SignedGroupOpenInvitation;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};
use sha2::Digest;

use super::super::build_auto_follow_set_if_enabled;
use super::super::membership::role_from_invited_role;
/// Namespace-scoped service for handling `RootOp::MemberJoined`.
pub struct NamespaceMembershipService<'a> {
    store: &'a Store,
    namespace_id: [u8; 32],
}

impl<'a> NamespaceMembershipService<'a> {
    pub fn new(store: &'a Store, namespace_id: [u8; 32]) -> Self {
        Self {
            store,
            namespace_id,
        }
    }

    pub fn apply_member_joined(
        &self,
        signer: &PublicKey,
        member: &PublicKey,
        signed_invitation: &SignedGroupOpenInvitation,
    ) -> EyreResult<()> {
        let inv = &signed_invitation.invitation;
        let group_id = inv.group_id;

        self.verify_member_join_signature(signer, member, signed_invitation)?;
        let inviter_pk = PublicKey::from(inv.inviter_identity.to_bytes());
        self.require_inviter_permission(&group_id, &inviter_pk)?;

        // Direct-row dedup: a `MemberJoined` op materializes the joiner's
        // direct membership row. An identity that already inherits
        // membership from an Open parent (#2256) is *not* the same as
        // having a direct row — they still need the explicit row written
        // so subsequent direct-membership lookups (e.g. removal,
        // capability writes, list_group_members) reflect their join.
        if MembershipRepository::new(self.store).has_direct_member(&group_id, member)? {
            return Ok(());
        }

        let role = role_from_invited_role(inv.invited_role);
        if role == GroupMemberRole::Admin
            && !MembershipRepository::new(self.store).is_admin(&group_id, &inviter_pk)?
        {
            bail!("only admins can invite new admins");
        }

        let resolved_ns = NamespaceRepository::new(self.store).resolve(&group_id)?;
        if resolved_ns.to_bytes() != self.namespace_id {
            bail!("group does not belong to this namespace");
        }

        MembershipRepository::new(self.store).add_member(&group_id, member, role)?;
        // #2422 Option 2: synthesize an `AutoFollowSet` so the auto-follow
        // handler backfills any pre-existing contexts in this group. Same
        // rationale as the `GroupOp::MemberAdded` arm in `apply_group_op_
        // mutations` — the handler doesn't subscribe to `MemberJoined`/
        // `MemberJoinedOpen` events, so without this synthesized event an
        // Open-subgroup self-joiner with `contexts: true` (the post-#2422
        // default) would only auto-follow FUTURE contexts, not the ones
        // already registered when they joined.
        if let Some(event) = build_auto_follow_set_if_enabled(self.store, &group_id, member)? {
            crate::op_events::notify(event);
        }
        Ok(())
    }

    fn verify_member_join_signature(
        &self,
        signer: &PublicKey,
        member: &PublicKey,
        signed_invitation: &SignedGroupOpenInvitation,
    ) -> EyreResult<()> {
        if *signer != *member {
            bail!(
                "MemberJoined signer ({}) does not match member ({})",
                signer,
                member
            );
        }

        let inv = &signed_invitation.invitation;
        let inviter_pk = PublicKey::from(inv.inviter_identity.to_bytes());
        let invitation_bytes = borsh::to_vec(inv).map_err(|e| eyre::eyre!("borsh: {e}"))?;
        let hash = sha2::Sha256::digest(&invitation_bytes);
        let sig_bytes = hex::decode(&signed_invitation.inviter_signature)
            .map_err(|e| eyre::eyre!("bad invitation signature hex: {e}"))?;
        let sig_arr: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| eyre::eyre!("invitation signature wrong length"))?;
        inviter_pk
            .verify_raw_signature(&hash, &sig_arr)
            .map_err(|e| eyre::eyre!("invalid invitation signature: {e}"))?;
        Ok(())
    }

    fn require_inviter_permission(
        &self,
        group_id: &ContextGroupId,
        inviter_pk: &PublicKey,
    ) -> EyreResult<()> {
        if !MembershipRepository::new(self.store).is_admin_or_has_capability(
            group_id,
            inviter_pk,
            MemberCapabilities::CAN_INVITE_MEMBERS,
        )? {
            bail!(
                "invitation inviter {} lacks permission for group {:?}",
                inviter_pk,
                group_id
            );
        }
        Ok(())
    }
}
