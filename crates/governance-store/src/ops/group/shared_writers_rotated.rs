//! `GroupOp::SharedWritersRotated` apply handler.
//!
//! Checks what the op and its signer's standing at the cut decide; the fold decides which
//! steps take effect. It records only that the context has rotated cells.

use std::collections::BTreeMap;

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::bounds::MAX_SHARED_WRITERS;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_storage::address::Id;
use calimero_storage::collections::is_cell_id;
use calimero_storage::entities::OpMask;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::context::GroupApplyCtx;
use crate::{
    get_group_for_context, ContextRegistrationError, MembershipRepository, NamespaceRepository,
    SharedWritersRotatedRejection as Rejection,
};

/// `Generic` scope of the rotated-context records, keyed by context id.
const ROTATED_SCOPE: [u8; 16] = *b"calimero-swrotat";

pub(crate) fn apply(
    ctx: &mut GroupApplyCtx<'_>,
    context_id: &ContextId,
    cell: &Id,
    prior: &BTreeMap<AccountId, OpMask>,
    new: &BTreeMap<AccountId, OpMask>,
) -> EyreResult<()> {
    let group_id = ctx.group_id();
    if !is_cell_id(*cell) {
        bail!(Rejection::NotACell(cell.to_string()));
    }
    if new.is_empty() {
        bail!(Rejection::EmptyWriterSet);
    }
    let max = MAX_SHARED_WRITERS;
    if prior.is_empty() || prior.len() > max || new.len() > max {
        bail!(Rejection::WriterSetSize { max });
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
    // Everything above is decided by the op and its cut. Whether the context is in the
    // group depends on the arrival order of a concurrent detach, so it only gates the record.
    if get_group_for_context(ctx.store(), context_id)? != Some(*group_id) {
        return Ok(());
    }
    // Only a step from the cell's genesis set pins the context, which takes an
    // admin of that set.
    if calimero_storage::collections::cell_id_binds(*cell, prior) {
        let data = GenericData::from(Slice::from(group_id.to_bytes().to_vec()));
        ctx.store().handle().put(&rotated_key(context_id), &data)?;
    }
    Ok(())
}

/// Refuse to detach a context whose cells have rotated (`into` is `None`), or to
/// register it in a group other than the one they rotated in.
pub(super) fn refuse_moving_rotated_context(
    ctx: &GroupApplyCtx<'_>,
    context_id: &ContextId,
    into: Option<&ContextGroupId>,
) -> EyreResult<()> {
    let rotated_in =
        match ctx
            .authorizer()
            .context_rotation_group_at_cut(ctx.group_id(), context_id, ctx.cut())
        {
            Some(answer) => answer,
            None => {
                ctx.ensure_live_fallback_is_sound(ctx.signer())?;
                rotated_in_live(ctx.store(), context_id)?
            }
        };
    match rotated_in {
        Some(group) if into != Some(&group) => bail!(ContextRegistrationError::HasRotatedCells {
            group_id: hex::encode(group.to_bytes()),
            context_id: context_id.to_string(),
        }),
        _ => Ok(()),
    }
}

/// Refuse, before anything is done for a deletion, a context that cannot be
/// detached because its cells rotated.
pub fn require_context_not_rotated(store: &Store, context_id: &ContextId) -> EyreResult<()> {
    match rotated_in_live(store, context_id)? {
        Some(group) => bail!(ContextRegistrationError::HasRotatedCells {
            group_id: hex::encode(group.to_bytes()),
            context_id: context_id.to_string(),
        }),
        None => Ok(()),
    }
}

fn rotated_key(context_id: &ContextId) -> GenericKey {
    GenericKey::new(ROTATED_SCOPE, **context_id)
}

/// The group this node applied a rotation of `context_id`'s cells in.
fn rotated_in_live(store: &Store, context_id: &ContextId) -> EyreResult<Option<ContextGroupId>> {
    let handle = store.handle();
    let Some(data) = handle.get(&rotated_key(context_id))? else {
        return Ok(None);
    };
    let bytes: [u8; 32] = data
        .as_ref()
        .try_into()
        .map_err(|_| eyre::eyre!("rotated-context record for {context_id} is not a group id"))?;
    Ok(Some(ContextGroupId::from(bytes)))
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
    use super::*;
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use calimero_primitives::identity::PublicKey;

    use crate::test_fixtures::{
        enrol_member, nest_for_test, sample_meta_with_admin, test_group_id, test_store, TEST_CUT,
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

    fn refusal(result: EyreResult<()>) -> eyre::Report {
        result.expect_err("must be refused")
    }

    fn rejected(err: &eyre::Report) -> Option<&Rejection> {
        err.chain()
            .find_map(|cause| cause.downcast_ref::<Rejection>())
    }

    fn registration(err: &eyre::Report) -> Option<&ContextRegistrationError> {
        err.chain()
            .find_map(|cause| cause.downcast_ref::<ContextRegistrationError>())
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
    fn a_rotation_of_a_context_outside_the_group_has_no_effect() {
        let w = world(test_group_id(), test_group_id());
        let genesis = full(&[w.admin]);
        let elsewhere = ContextId::from([0x45; 32]);
        let op = w.rotation(elsewhere, cell(&genesis), genesis.clone(), genesis);
        w.rotate(&w.admin_sk, op).expect("applies as a no-op");
        assert_eq!(rotated_in_live(&w.store, &elsewhere).unwrap(), None);
    }

    /// The cut decides, so a node that applied a concurrent rotation first
    /// agrees with one that did not.
    #[test]
    fn a_detach_whose_cut_holds_no_rotation_is_allowed_whatever_this_node_recorded() {
        let w = world(test_group_id(), test_group_id());
        let genesis = full(&[w.admin]);
        let op = w.rotation(context(), cell(&genesis), genesis.clone(), genesis);
        w.rotate(&w.admin_sk, op).expect("rotate");
        let signed =
            SignedGroupOp::sign(&w.admin_sk, w.group, TEST_CUT.to_vec(), 9, detach()).unwrap();
        let authorizer = AtCut {
            store: &w.store,
            roles: BTreeMap::new(),
            rotated: Some(None),
        };
        crate::apply_local_signed_group_op_at_cut(&w.store, &signed, &authorizer)
            .expect("the cut holds no rotation");
    }

    #[test]
    fn a_refusal_does_not_depend_on_whether_the_context_is_in_the_group() {
        let w = world(test_group_id(), test_group_id());
        let (reader_sk, reader) = w.member(0x0C, GroupMemberRole::ReadOnly);
        let genesis = full(&[reader]);
        let elsewhere = ContextId::from([0x45; 32]);
        let op = w.rotation(elsewhere, cell(&genesis), genesis, full(&[w.admin]));
        let err = refusal(w.rotate(&reader_sk, op));
        assert!(matches!(rejected(&err), Some(Rejection::SignerReadOnly(_))));
    }

    #[test]
    fn a_rotation_with_an_oversized_or_empty_prior_set_is_refused() {
        let w = world(test_group_id(), test_group_id());
        let many = |n: usize| -> BTreeMap<AccountId, OpMask> {
            let mut set = full(&[w.admin]);
            set.extend((1..n).map(|i| (AccountId::from([i as u8; 32]), OpMask::WRITE)));
            set
        };
        for (what, prior, new) in [
            ("oversized prior", many(257), full(&[w.admin])),
            ("oversized new", full(&[w.admin]), many(257)),
            ("empty prior", BTreeMap::new(), full(&[w.admin])),
        ] {
            let op = w.rotation(context(), cell(&prior), prior, new);
            // The namespace path applies a decrypted op without validating it.
            let err = refusal(
                crate::apply_group_op_mutations(
                    &w.store,
                    &w.group,
                    &w.admin_sk.public_key(),
                    &op,
                    &[],
                    &crate::authorizer::LIVE_FALLBACK_AUTHORIZER,
                )
                .map(|_| ()),
            );
            assert!(
                matches!(rejected(&err), Some(Rejection::WriterSetSize { .. })),
                "{what}"
            );
        }
    }

    #[test]
    fn a_rotated_context_is_reported_before_a_deletion_starts() {
        let w = world(test_group_id(), test_group_id());
        require_context_not_rotated(&w.store, &context()).expect("never rotated");
        let genesis = full(&[w.admin]);
        let op = w.rotation(context(), cell(&genesis), genesis.clone(), genesis);
        w.rotate(&w.admin_sk, op).expect("rotate");
        let err = require_context_not_rotated(&w.store, &context()).expect_err("rotated");
        assert!(matches!(
            registration(&err),
            Some(ContextRegistrationError::HasRotatedCells { .. })
        ));
    }

    #[test]
    fn a_rotation_not_resting_on_the_cell_genesis_does_not_pin_its_context() {
        let w = world(test_group_id(), test_group_id());
        let (_, mallory) = w.member(0x0E, GroupMemberRole::Member);
        // A cell made with another set: the admin's own prior set does not bind it.
        let op = w.rotation(
            context(),
            cell(&full(&[mallory])),
            full(&[w.admin]),
            full(&[w.admin]),
        );
        w.rotate(&w.admin_sk, op).expect("applies");
        w.rotate(&w.admin_sk, detach())
            .expect("the context is not pinned");
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
        let err = refusal(w.rotate(&w.admin_sk, not_a_cell));
        assert!(matches!(rejected(&err), Some(Rejection::NotACell(_))));
        let emptied = w.rotation(context(), cell(&genesis), genesis, BTreeMap::new());
        let err = refusal(w.rotate(&w.admin_sk, emptied));
        assert!(matches!(rejected(&err), Some(Rejection::EmptyWriterSet)));
    }

    #[test]
    fn a_signer_without_admin_in_the_prior_set_is_refused() {
        let w = world(test_group_id(), test_group_id());
        let (mallory_sk, mallory) = w.member(0x0E, GroupMemberRole::Member);
        let mut prior = full(&[w.admin]);
        let _ = prior.insert(mallory, OpMask::WRITE);
        for prior in [prior, full(&[w.admin])] {
            let op = w.rotation(context(), cell(&prior), prior, full(&[mallory]));
            let err = refusal(w.rotate(&mallory_sk, op));
            assert!(matches!(
                rejected(&err),
                Some(Rejection::SignerNotAdminOfPrior(_))
            ));
        }
    }

    #[test]
    fn a_read_only_member_or_a_stranger_is_refused() {
        let w = world(test_group_id(), test_group_id());
        let (reader_sk, reader) = w.member(0x0C, GroupMemberRole::ReadOnly);
        let genesis = full(&[reader]);
        let op = w.rotation(context(), cell(&genesis), genesis, full(&[w.admin]));
        let err = refusal(w.rotate(&reader_sk, op));
        assert!(matches!(rejected(&err), Some(Rejection::SignerReadOnly(_))));

        let stranger_sk = PrivateKey::from([0x0D; 32]);
        let stranger = enrol_member(&w.store, &w.namespace, &stranger_sk.public_key());
        let genesis = full(&[stranger]);
        let op = w.rotation(context(), cell(&genesis), genesis, full(&[w.admin]));
        let err = refusal(w.rotate(&stranger_sk, op));
        assert!(matches!(
            rejected(&err),
            Some(Rejection::SignerNotMember(_))
        ));
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
        let err = refusal(w.rotate(&tee_sk, op));
        assert!(matches!(rejected(&err), Some(Rejection::SignerIsTee(_))));
    }

    /// Answers every question from `live` except the role, which it gives from
    /// `roles` per group, and a rotated context's group from `rotated`.
    struct AtCut<'a> {
        store: &'a Store,
        roles: BTreeMap<ContextGroupId, Option<GroupMemberRole>>,
        rotated: Option<Option<ContextGroupId>>,
    }

    impl crate::authorizer::AtCutAuthorizer for AtCut<'_> {
        fn is_admin_at_cut(
            &self,
            _: &ContextGroupId,
            _: &PublicKey,
            _: &[[u8; 32]],
        ) -> Option<bool> {
            None
        }
        fn is_admin_or_capability_at_cut(
            &self,
            _: &ContextGroupId,
            _: &PublicKey,
            _: u32,
            _: &[[u8; 32]],
        ) -> Option<bool> {
            None
        }
        fn is_admin_or_capability_account_at_cut(
            &self,
            _: &ContextGroupId,
            _: &AccountId,
            _: u32,
            _: &[[u8; 32]],
        ) -> Option<bool> {
            None
        }
        fn is_admin_account_at_cut(
            &self,
            _: &ContextGroupId,
            _: &AccountId,
            _: &[[u8; 32]],
        ) -> Option<bool> {
            None
        }
        fn is_last_admin_at_cut(
            &self,
            _: &ContextGroupId,
            _: &AccountId,
            _: &[[u8; 32]],
        ) -> Option<bool> {
            None
        }
        fn membership_path_at_cut(
            &self,
            _: &ContextGroupId,
            _: &AccountId,
            _: &[[u8; 32]],
        ) -> Option<crate::authorizer::AtCutMembershipPath> {
            None
        }
        fn effective_role_at_cut(
            &self,
            group: &ContextGroupId,
            member: &AccountId,
            _: &[[u8; 32]],
        ) -> Option<Option<GroupMemberRole>> {
            self.roles.get(group).cloned().or_else(|| {
                Some(
                    MembershipRepository::new(self.store)
                        .effective_role(group, member)
                        .ok()?
                        .map(|(role, _)| role),
                )
            })
        }
        fn context_rotation_group_at_cut(
            &self,
            _: &ContextGroupId,
            _: &ContextId,
            _: &[[u8; 32]],
        ) -> Option<Option<ContextGroupId>> {
            self.rotated
        }
    }

    /// Receivers read the signer's role at the op's cut, not from live rows.
    #[test]
    fn the_signers_standing_is_read_at_the_cut() {
        let w = world(
            ContextGroupId::from([0x70; 32]),
            ContextGroupId::from([0x71; 32]),
        );
        let genesis = full(&[w.admin]);
        let op = w.rotation(context(), cell(&genesis), genesis, full(&[w.admin]));
        let signed = SignedGroupOp::sign(&w.admin_sk, w.group, TEST_CUT.to_vec(), 1, op).unwrap();
        let apply = |roles: &[(ContextGroupId, Option<GroupMemberRole>)]| {
            let authorizer = AtCut {
                store: &w.store,
                roles: roles.iter().cloned().collect(),
                rotated: None,
            };
            crate::apply_local_signed_group_op_at_cut(&w.store, &signed, &authorizer)
        };
        let err = refusal(apply(&[(w.group, Some(GroupMemberRole::ReadOnly))]));
        assert!(matches!(rejected(&err), Some(Rejection::SignerReadOnly(_))));
        let err = refusal(apply(&[(w.group, None)]));
        assert!(matches!(
            rejected(&err),
            Some(Rejection::SignerNotMember(_))
        ));
        let err = refusal(apply(&[(w.namespace, Some(GroupMemberRole::ReadOnlyTee))]));
        assert!(matches!(rejected(&err), Some(Rejection::SignerIsTee(_))));
        apply(&[]).expect("control: the live rows say admin");
    }

    fn detach() -> GroupOp {
        GroupOp::ContextDetached {
            context_id: context(),
        }
    }

    fn register() -> GroupOp {
        GroupOp::ContextRegistered {
            context_id: context(),
            application_id: calimero_primitives::application::ApplicationId::from([0u8; 32]),
            blob_id: calimero_primitives::blobs::BlobId::from([0u8; 32]),
            source: String::new(),
            service_name: None,
            package: "com.example.app".to_owned(),
            version: "1.0.0".to_owned(),
        }
    }

    /// A context whose cells rotated keeps its group, so its rotations stay
    /// with the group readers fold them from.
    #[test]
    fn a_rotated_context_cannot_leave_its_group() {
        let w = world(test_group_id(), test_group_id());
        w.rotate(&w.admin_sk, detach())
            .expect("control: never rotated");
        w.rotate(&w.admin_sk, register()).expect("and back");

        let genesis = full(&[w.admin]);
        let op = w.rotation(context(), cell(&genesis), genesis.clone(), genesis);
        w.rotate(&w.admin_sk, op).expect("rotate");
        let err = refusal(w.rotate(&w.admin_sk, detach()));
        assert!(matches!(
            registration(&err),
            Some(ContextRegistrationError::HasRotatedCells { .. })
        ));

        // Unregistered by other means (a group deletion), it may not join another.
        let other = ContextGroupId::from([0x72; 32]);
        crate::unregister_context_from_group(&w.store, &w.group, &context()).unwrap();
        MetaRepository::new(&w.store)
            .save(&other, &sample_meta_with_admin(w.admin))
            .unwrap();
        MembershipRepository::new(&w.store)
            .add_member(&other, &w.admin, GroupMemberRole::Admin)
            .unwrap();
        let signed = SignedGroupOp::sign(&w.admin_sk, other, vec![], 99, register()).unwrap();
        let err = refusal(apply_local_signed_group_op(&w.store, &signed).map(|_| ()));
        assert!(matches!(
            registration(&err),
            Some(ContextRegistrationError::HasRotatedCells { .. })
        ));
    }

    /// A receiver decides by the rotations in the detach's own cut.
    #[test]
    fn a_detach_is_judged_by_the_rotations_at_its_cut() {
        let w = world(test_group_id(), test_group_id());
        let signed =
            SignedGroupOp::sign(&w.admin_sk, w.group, TEST_CUT.to_vec(), 1, detach()).unwrap();
        let authorizer = AtCut {
            store: &w.store,
            roles: BTreeMap::new(),
            rotated: Some(Some(w.group)),
        };
        let err = refusal(crate::apply_local_signed_group_op_at_cut(
            &w.store,
            &signed,
            &authorizer,
        ));
        assert!(matches!(
            registration(&err),
            Some(ContextRegistrationError::HasRotatedCells { .. })
        ));
    }
}
