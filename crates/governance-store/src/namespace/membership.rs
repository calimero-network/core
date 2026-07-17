use crate::{MembershipRepository, NamespaceRepository, ReentryRepository};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::types::SignedGroupOpenInvitation;
use calimero_context_config::MemberCapabilities;
use calimero_governance_types::NamespaceId;
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
    namespace_id: NamespaceId,
}

impl<'a> NamespaceMembershipService<'a> {
    pub fn new(store: &'a Store, namespace_id: NamespaceId) -> Self {
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
        joined_at: Option<u64>,
    ) -> EyreResult<Option<crate::op_events::OpEvent>> {
        let inv = &signed_invitation.invitation;
        let group_id = inv.group_id;

        self.verify_member_join_signature(signer, member, signed_invitation)?;

        // Deterministic expiry gate: reject when the joiner's signed
        // claimed join time is past expiry, comparing the op's own field
        // (not a local clock) so every node reaches the same verdict.
        // Runs after signature verification (so `expiration` is authentic)
        // but before the permission lookup, to reject expired ops cheaply.
        // `expiration == 0` is the canonical sentinel for "no expiry".
        let expiration = inv.expiration_timestamp;
        if expiration != 0 {
            // A missing joined_at with a non-zero expiration is a malformed op,
            // not merely an expired one — distinguish the two failure modes.
            let Some(joined_at) = joined_at else {
                bail!("invalid op: expiration {expiration} is set but joined_at is absent (use MemberJoinedAt for expiring invitations)");
            };
            if joined_at > expiration {
                bail!("invitation expired: joined_at {joined_at} > expiration {expiration}");
            }
        }
        // When expiration == 0 (no-expiry sentinel), joined_at is not checked:
        // both None and any Some(_) value are accepted and the field is ignored.
        // Do not add joined_at lookups below this point without handling the
        // None case explicitly.

        let inviter_pk = PublicKey::from(inv.inviter_identity.to_bytes());
        self.require_inviter_permission(&group_id, &inviter_pk)?;

        // Direct-row dedup: a `MemberJoined` op materializes the joiner's
        // direct membership row. An identity that already inherits
        // membership from an Open parent (#2256) is *not* the same as
        // having a direct row — they still need the explicit row written
        // so subsequent direct-membership lookups (e.g. removal,
        // capability writes, list_group_members) reflect their join.
        if MembershipRepository::new(self.store).has_direct_member(&group_id, member)? {
            return Ok(None);
        }

        // Re-entry gate. Rejects two cases: an identity an admin REMOVED (no
        // invitation readmits them — only an admin `MemberAdded` does), and an
        // identity replaying an invitation they have already consumed (they
        // left, and the invitation they joined with is spent for them; they need
        // a freshly issued one). A first-time joiner has neither row, so this is
        // a no-op for them.
        //
        // Placement is load-bearing, and it belongs AFTER the direct-row dedup
        // above, not before it. The block governs RE-ENTRY; someone who already
        // holds a member row is not re-entering, so the gate has nothing to say
        // about them — and this handler MUST be idempotent on re-apply, which the
        // whole apply pipeline relies on (the mutation re-runs on every gossip
        // re-delivery, sync backfill, and post-restart DAG replay, before the
        // op-log dedup fires; a handler that bails on a second run leaves the
        // nonce unpersisted and wedges the node). Put the gate ahead of the dedup
        // and the first apply consumes the invitation, then the second apply sees
        // that consumption row and bails — a failing apply does not advance the
        // namespace governance head, so every later governance op stalls behind it
        // and the namespace stops converging. Behind the dedup, a re-apply hits
        // `has_direct_member` and returns early, so the gate runs exactly once.
        //
        // It must still come after `verify_member_join_signature`, so we never
        // read state on the say-so of an unauthenticated op.
        //
        // Nothing is smuggled past the gate by sitting behind the dedup: a
        // blocked identity cannot acquire a direct row out of band. Every writer
        // of one is itself gated — the sync responder's pre-register, and
        // `join_group`'s local self-add — so a row existing here means the gate
        // already passed for them.
        let reentry = ReentryRepository::new(self.store);
        reentry.require_invitation_admits(&group_id, member, inv.invitation_nonce)?;

        let role = role_from_invited_role(inv.invited_role);
        if role == GroupMemberRole::Admin
            && !MembershipRepository::new(self.store).is_admin(&group_id, &inviter_pk)?
        {
            bail!("only admins can invite new admins");
        }

        let resolved_ns = NamespaceRepository::new(self.store).resolve(&group_id)?;
        if resolved_ns.to_bytes() != self.namespace_id.to_bytes() {
            bail!("group does not belong to this namespace");
        }

        MembershipRepository::new(self.store).add_member(&group_id, member, role)?;
        // Spend the invitation for this identity: presenting it again after they
        // next exit cannot readmit them. Recorded per `(group, identity, nonce)`,
        // so the same open-invite link still admits everyone else who has not
        // used it — an invitation is a bearer token with no invitee field, and
        // burning it globally on first use would break the shared join link.
        reentry.mark_invitation_consumed(&group_id, member, inv.invitation_nonce)?;
        // They are a member again, so the block that stopped them re-entering is
        // spent. Reaching this line means the gate above already established the
        // block was `Left` and this invitation was fresh — a `Removed` block
        // bails there and never gets here.
        reentry.clear_block(&group_id, member)?;
        // #2422 Option 2: synthesize an `AutoFollowSet` so the auto-follow
        // handler backfills any pre-existing contexts in this group. Same
        // rationale as the `GroupOp::MemberAdded` arm in `apply_group_op_
        // mutations` — the handler doesn't subscribe to `MemberJoined`/
        // `MemberJoinedOpen` events, so without this synthesized event an
        // Open-subgroup self-joiner with `contexts: true` (the post-#2422
        // default) would only auto-follow FUTURE contexts, not the ones
        // already registered when they joined.
        build_auto_follow_set_if_enabled(self.store, &group_id, member)
    }

    /// Validate an open invitation for the responder key-delivery path:
    /// inviter signature, namespace ownership, inviter permission, and
    /// expiry against `now_secs`. Reads a wall clock (supplied by the
    /// caller) rather than an op field because key delivery is
    /// point-to-point, not folded governance state, so responders
    /// disagreeing cannot diverge membership.
    pub fn validate_open_invitation(
        &self,
        signed_invitation: &SignedGroupOpenInvitation,
        now_secs: u64,
    ) -> EyreResult<()> {
        let inv = &signed_invitation.invitation;
        let group_id = inv.group_id;

        self.verify_inviter_signature(signed_invitation)?;

        let resolved_ns = NamespaceRepository::new(self.store).resolve(&group_id)?;
        if resolved_ns.to_bytes() != self.namespace_id.to_bytes() {
            bail!("group does not belong to this namespace");
        }

        let inviter_pk = PublicKey::from(inv.inviter_identity.to_bytes());
        self.require_inviter_permission(&group_id, &inviter_pk)?;

        let expiration = inv.expiration_timestamp;
        if expiration != 0 && now_secs > expiration {
            bail!("invitation expired: now {now_secs} > expiration {expiration}");
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
        self.verify_inviter_signature(signed_invitation)
    }

    fn verify_inviter_signature(
        &self,
        signed_invitation: &SignedGroupOpenInvitation,
    ) -> EyreResult<()> {
        Self::verify_open_invitation_signature(signed_invitation)
    }

    /// Store-free verification of an open invitation's inviter signature:
    /// `sha256(borsh(invitation))` checked against `inviter_identity`.
    ///
    /// Exposed so trust-seeding paths that run before governance state is
    /// available (e.g. a fresh joiner writing its local `admin_identity` in
    /// `join_namespace` / the `join_group` handler) can reject a forged
    /// invitation up front. This is only the cryptographic check — namespace
    /// ownership, inviter permission, and expiry still require DAG state and are
    /// enforced by [`Self::validate_open_invitation`] on the responder and by
    /// the deterministic apply-time check.
    pub fn verify_open_invitation_signature(
        signed_invitation: &SignedGroupOpenInvitation,
    ) -> EyreResult<()> {
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
            MemberCapabilities::CAN_INVITE_MEMBERS.bits(),
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
