//! `GroupOp::TransferOwnership` apply handler. Extracted from
//! `apply_group_op_mutations` in #2304.

use super::context::GroupApplyCtx;
use crate::{MembershipError, MembershipRepository, MetaRepository};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result as EyreResult};

pub(crate) fn apply(ctx: &mut GroupApplyCtx<'_>, new_owner: &PublicKey) -> EyreResult<()> {
    let signer = ctx.signer();
    let group_id = ctx.group_id();
    let store = ctx.store();

    // Owner-only — current owner is the only signer who can transfer.
    let mut meta = MetaRepository::new(store)
        .load(group_id)?
        .ok_or_else(|| MembershipError::UnknownGroup(hex::encode(group_id.to_bytes())))?;

    if meta.owner_identity != *signer {
        bail!(MembershipError::OnlyOwnerCanTransfer(hex::encode(
            group_id.to_bytes()
        )));
    }

    // The new owner must already be an Admin of the group. Transfer
    // does not implicitly invite or promote — the successor must
    // already be in place at admin tier. This prevents two awkward
    // states:
    //   * Transferring to a non-member: would create an absentee
    //     owner.
    //   * Transferring to a plain Member: Owner has all Admin
    //     privileges by design (see doc § 7 privilege matrix), so
    //     a plain-Member owner would have a confusing "owner with
    //     reduced capabilities" status. Require Admin first;
    //     promote then transfer if needed.
    match MembershipRepository::new(store).role_of(group_id, new_owner)? {
        Some(GroupMemberRole::Admin) => {}
        Some(other) => bail!(
            "new owner of group {} must be an Admin, but is currently {:?}; \
             promote them to Admin before transferring ownership",
            hex::encode(group_id.to_bytes()),
            other
        ),
        None => bail!(
            "new owner is not a member of group {}; invite and promote them first",
            hex::encode(group_id.to_bytes())
        ),
    }

    meta.owner_identity = *new_owner;
    MetaRepository::new(store).save(group_id, &meta)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::GroupApplyCtx;
    use crate::test_fixtures::{test_group_id, test_meta, test_store};
    use crate::{MembershipRepository, MetaRepository, LIVE_FALLBACK_AUTHORIZER};
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PublicKey;

    // test_meta() sets owner_identity = PublicKey::from([0x01; 32])
    const OWNER_SEED: u8 = 0x01;

    #[test]
    fn non_owner_signer_is_rejected() {
        let store = test_store();
        let gid = test_group_id();
        let signer = PublicKey::from([0x02; 32]); // different from owner
        let new_owner = PublicKey::from([0x03; 32]);

        MetaRepository::new(&store).save(&gid, &test_meta()).unwrap();
        MembershipRepository::new(&store)
            .add_member(&gid, &new_owner, GroupMemberRole::Admin)
            .unwrap();

        let mut ctx = GroupApplyCtx::new_with_apply_auth(
            &store,
            &gid,
            &signer,
            &[],
            &LIVE_FALLBACK_AUTHORIZER,
        );
        let err = super::apply(&mut ctx, &new_owner).unwrap_err();
        assert!(
            err.to_string().contains("only the current owner"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn new_owner_that_is_plain_member_is_rejected() {
        let store = test_store();
        let gid = test_group_id();
        let owner = PublicKey::from([OWNER_SEED; 32]);
        let new_owner = PublicKey::from([0x03; 32]);

        MetaRepository::new(&store).save(&gid, &test_meta()).unwrap();
        MembershipRepository::new(&store)
            .add_member(&gid, &new_owner, GroupMemberRole::Member) // Member, not Admin
            .unwrap();

        let mut ctx = GroupApplyCtx::new_with_apply_auth(
            &store,
            &gid,
            &owner,
            &[],
            &LIVE_FALLBACK_AUTHORIZER,
        );
        let err = super::apply(&mut ctx, &new_owner).unwrap_err();
        assert!(
            err.to_string().contains("must be an Admin"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn new_owner_that_is_not_a_member_is_rejected() {
        let store = test_store();
        let gid = test_group_id();
        let owner = PublicKey::from([OWNER_SEED; 32]);
        let outsider = PublicKey::from([0x03; 32]); // never added to group

        MetaRepository::new(&store).save(&gid, &test_meta()).unwrap();

        let mut ctx = GroupApplyCtx::new_with_apply_auth(
            &store,
            &gid,
            &owner,
            &[],
            &LIVE_FALLBACK_AUTHORIZER,
        );
        let err = super::apply(&mut ctx, &outsider).unwrap_err();
        assert!(
            err.to_string().contains("not a member"),
            "unexpected error: {err}"
        );
    }
}
