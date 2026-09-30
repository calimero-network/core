//! `GroupOp::SharedWritersRotated` apply handler.
//!
//! Checks what the op and its signer's standing at the cut decide, and writes
//! nothing: which steps take effect is `calimero_storage::shared_writers::fold`
//! over every step a reader's cut sees.

use std::collections::BTreeMap;

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_storage::address::Id;
use calimero_storage::collections::is_cell_id;
use calimero_storage::entities::OpMask;
use eyre::{bail, Result as EyreResult};

use super::context::GroupApplyCtx;
use crate::{
    get_group_for_context, ContextRegistrationError, MembershipRepository, NamespaceRepository,
    SharedWritersRotatedRejection as Rejection,
};

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    context_id: &ContextId,
    cell: &Id,
    prior: &BTreeMap<AccountId, OpMask>,
    new: &BTreeMap<AccountId, OpMask>,
) -> EyreResult<()> {
    let group_id = ctx.group_id();
    if get_group_for_context(ctx.store(), context_id)? != Some(*group_id) {
        bail!(ContextRegistrationError::NotInGroup {
            group_id: hex::encode(group_id.to_bytes()),
            context_id: context_id.to_string(),
        });
    }
    if !is_cell_id(*cell) {
        bail!(Rejection::NotACell(cell.to_string()));
    }
    if new.is_empty() {
        bail!(Rejection::EmptyWriterSet);
    }
    let Some(account) = ctx.signer_account()? else {
        bail!(Rejection::SignerUnbound);
    };
    match effective_role(ctx, group_id, &account)? {
        None => bail!(Rejection::SignerNotMember(account.to_string())),
        Some(role) if role.is_read_only() => bail!(Rejection::SignerReadOnly(account.to_string())),
        Some(_) => {}
    }
    // A TEE-triggered run signs with the TEE's key, and a TEE's role is read at
    // the namespace root, whatever row it holds in a subgroup.
    let root = NamespaceRepository::new(ctx.store()).resolve(group_id)?;
    if effective_role(ctx, &root, &account)?.is_some_and(|role| role.is_tee()) {
        bail!(Rejection::SignerIsTee(account.to_string()));
    }
    if !prior
        .get(&account)
        .is_some_and(|mask| mask.contains(OpMask::ADMIN))
    {
        bail!(Rejection::SignerNotAdminOfPrior(account.to_string()));
    }
    Ok(())
}

/// `account`'s effective role in `group` at the op's cut, or live when there is
/// no cut to resolve against.
fn effective_role(
    ctx: &GroupApplyCtx<'_>,
    group: &ContextGroupId,
    account: &AccountId,
) -> EyreResult<Option<GroupMemberRole>> {
    if let Some(role) = ctx
        .authorizer()
        .effective_role_at_cut(group, account, ctx.cut())
    {
        return Ok(role);
    }
    ctx.ensure_live_fallback_is_sound(ctx.signer())?;
    Ok(MembershipRepository::new(ctx.store())
        .effective_role(group, account)?
        .map(|(role, _)| role))
}

#[cfg(test)]
mod tests {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::Store;

    use super::*;
    use crate::test_fixtures::{
        enrol_member, nest_for_test, sample_meta_with_admin, test_group_id, test_store,
    };
    use crate::{apply_local_signed_group_op, register_context_in_group, MetaRepository};

    const CONTEXT: [u8; 32] = [0x44; 32];

    struct World {
        store: Store,
        namespace: ContextGroupId,
        group: ContextGroupId,
        admin_sk: PrivateKey,
        admin: AccountId,
        nonce: std::cell::Cell<u64>,
    }

    /// `group`, in `namespace`, owning `CONTEXT`, with an admin.
    fn world(namespace: ContextGroupId, group: ContextGroupId) -> World {
        let store = test_store();
        if namespace != group {
            nest_for_test(&store, &namespace, &group);
        }
        let admin_sk = PrivateKey::from([0x0A; 32]);
        let admin = enrol_member(&store, &namespace, &admin_sk.public_key());
        for g in [namespace, group] {
            MetaRepository::new(&store)
                .save(&g, &sample_meta_with_admin(admin))
                .unwrap();
            MembershipRepository::new(&store)
                .add_member(&g, &admin, GroupMemberRole::Admin)
                .unwrap();
        }
        register_context_in_group(&store, &group, &context()).unwrap();
        World {
            store,
            namespace,
            group,
            admin_sk,
            admin,
            nonce: std::cell::Cell::new(0),
        }
    }

    fn context() -> ContextId {
        ContextId::from(CONTEXT)
    }

    fn full(who: &[AccountId]) -> BTreeMap<AccountId, OpMask> {
        who.iter().map(|a| (*a, OpMask::FULL)).collect()
    }

    fn cell(genesis: &BTreeMap<AccountId, OpMask>) -> Id {
        calimero_storage::collections::cell_id(Id::new([0x11; 32]), genesis)
    }

    impl World {
        fn member(&self, seed: u8, role: GroupMemberRole) -> (PrivateKey, AccountId) {
            let sk = PrivateKey::from([seed; 32]);
            let account = enrol_member(&self.store, &self.namespace, &sk.public_key());
            MembershipRepository::new(&self.store)
                .add_member(&self.group, &account, role)
                .unwrap();
            (sk, account)
        }

        fn rotate(&self, signer: &PrivateKey, op: GroupOp) -> EyreResult<()> {
            self.nonce.set(self.nonce.get() + 1);
            let signed = SignedGroupOp::sign(signer, self.group, vec![], self.nonce.get(), op)?;
            apply_local_signed_group_op(&self.store, &signed).map(|_| ())
        }

        fn rotation(
            &self,
            context_id: ContextId,
            cell: Id,
            prior: BTreeMap<AccountId, OpMask>,
            new: BTreeMap<AccountId, OpMask>,
        ) -> GroupOp {
            GroupOp::SharedWritersRotated {
                context_id,
                cell,
                prior,
                nonce: 10,
                new,
            }
        }
    }

    fn rejection(result: EyreResult<()>) -> String {
        let err = result.expect_err("must be refused");
        err.chain()
            .find_map(|cause| cause.downcast_ref::<Rejection>())
            .map_or_else(
                || {
                    err.chain()
                        .find_map(|cause| cause.downcast_ref::<ContextRegistrationError>())
                        .map(|e| format!("{e:?}"))
                        .unwrap_or_else(|| panic!("not a rotation refusal: {err:?}"))
                },
                |e| format!("{e:?}"),
            )
    }

    #[test]
    fn an_admin_of_the_prior_set_may_rotate_it() {
        let w = world(test_group_id(), test_group_id());
        let genesis = full(&[w.admin]);
        let (_, bob) = w.member(0x0B, GroupMemberRole::Member);
        let op = w.rotation(context(), cell(&genesis), genesis, full(&[w.admin, bob]));
        let signed = SignedGroupOp::sign(&w.admin_sk, w.group, vec![], 1, op).unwrap();
        apply_local_signed_group_op(&w.store, &signed).expect("applies");
        apply_local_signed_group_op(&w.store, &signed).expect("and re-applies");
    }

    #[test]
    fn a_rotation_of_a_context_outside_the_group_is_refused() {
        let w = world(test_group_id(), test_group_id());
        let genesis = full(&[w.admin]);
        let op = w.rotation(
            ContextId::from([0x45; 32]),
            cell(&genesis),
            genesis.clone(),
            genesis,
        );
        assert!(rejection(w.rotate(&w.admin_sk, op)).starts_with("NotInGroup"));
    }

    #[test]
    fn only_a_cell_with_writers_left_may_be_rotated() {
        let w = world(test_group_id(), test_group_id());
        let genesis = full(&[w.admin]);
        let not_a_cell = w.rotation(
            context(),
            Id::new([0x11; 32]),
            genesis.clone(),
            genesis.clone(),
        );
        assert!(rejection(w.rotate(&w.admin_sk, not_a_cell)).starts_with("NotACell"));
        let emptied = w.rotation(context(), cell(&genesis), genesis, BTreeMap::new());
        assert_eq!(rejection(w.rotate(&w.admin_sk, emptied)), "EmptyWriterSet");
    }

    #[test]
    fn a_signer_without_admin_in_the_prior_set_is_refused() {
        let w = world(test_group_id(), test_group_id());
        let (mallory_sk, mallory) = w.member(0x0E, GroupMemberRole::Member);
        let mut prior = full(&[w.admin]);
        let _ = prior.insert(mallory, OpMask::WRITE);
        for prior in [prior, full(&[w.admin])] {
            let op = w.rotation(context(), cell(&prior), prior, full(&[mallory]));
            assert!(rejection(w.rotate(&mallory_sk, op)).starts_with("SignerNotAdminOfPrior"));
        }
    }

    #[test]
    fn a_read_only_member_or_a_stranger_is_refused() {
        let w = world(test_group_id(), test_group_id());
        let (reader_sk, reader) = w.member(0x0C, GroupMemberRole::ReadOnly);
        let genesis = full(&[reader]);
        let op = w.rotation(context(), cell(&genesis), genesis.clone(), full(&[w.admin]));
        assert!(rejection(w.rotate(&reader_sk, op.clone())).starts_with("SignerReadOnly"));

        let stranger_sk = PrivateKey::from([0x0D; 32]);
        let stranger = enrol_member(&w.store, &w.namespace, &stranger_sk.public_key());
        let genesis = full(&[stranger]);
        let op = w.rotation(context(), cell(&genesis), genesis, full(&[w.admin]));
        assert!(rejection(w.rotate(&stranger_sk, op)).starts_with("SignerNotMember"));
    }

    /// What a TEE-triggered run signs with is the TEE's key, and a TEE of the
    /// namespace is refused even where it holds an ordinary row.
    #[test]
    fn a_tee_of_the_namespace_is_refused() {
        let w = world(
            ContextGroupId::from([0x70; 32]),
            ContextGroupId::from([0x71; 32]),
        );
        let (tee_sk, tee) = w.member(0x0F, GroupMemberRole::Member);
        MembershipRepository::new(&w.store)
            .add_member(&w.namespace, &tee, GroupMemberRole::ReadOnlyTee)
            .unwrap();
        let genesis = full(&[tee]);
        let op = w.rotation(context(), cell(&genesis), genesis, full(&[w.admin]));
        assert!(rejection(w.rotate(&tee_sk, op)).starts_with("SignerIsTee"));
    }
}
