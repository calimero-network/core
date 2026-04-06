use calimero_context_config::types::ContextGroupId;
use calimero_context_config::types::SignedGroupOpenInvitation;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};
use sha2::Digest;

use super::{
    add_group_member, check_group_membership, is_group_admin, is_group_admin_or_has_capability,
    resolve_namespace,
};

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

        if check_group_membership(self.store, &group_id, member)? {
            return Ok(());
        }

        let role = Self::map_invited_role(inv.invited_role);
        if role == GroupMemberRole::Admin && !is_group_admin(self.store, &group_id, &inviter_pk)? {
            bail!("only admins can invite new admins");
        }

        let resolved_ns = resolve_namespace(self.store, &group_id)?;
        if resolved_ns.to_bytes() != self.namespace_id {
            bail!("group does not belong to this namespace");
        }

        add_group_member(self.store, &group_id, member, role)?;
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
        if !is_group_admin_or_has_capability(
            self.store,
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

    fn map_invited_role(invited_role: u8) -> GroupMemberRole {
        match invited_role {
            0 => GroupMemberRole::Admin,
            2 => GroupMemberRole::ReadOnly,
            _ => GroupMemberRole::Member,
        }
    }
}
