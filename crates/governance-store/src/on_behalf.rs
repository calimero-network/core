//! Who may sign a storage entry written on another account's behalf.
//!
//! A relay that runs a member's warranted write signs the run's entries with
//! its own key and names the member in `SignatureData::on_behalf`. Storage then
//! judges ownership and writer sets against that account, but only if the node
//! resolves the entry to it, and the node does so only when [`on_behalf_standing`]
//! accepts the pair. This is that rule, written once over [`StandingReads`] so
//! the relay (before it signs), the delta path (at the delta's cut), repair
//! (live) and snapshot apply all ask the same question.
//!
//! # The rule
//!
//! * The signer's account must be a **`RelayTee`** in the context's namespace:
//!   its effective role in the context's group, with a TEE role replaced by the
//!   one on its namespace root row (`namespace_tee_role`, the same reading
//!   [`executor_standing`](crate::warrant_admission::executor_standing) uses, so a
//!   mode switch at the root decides both). The role is the grant, so no
//!   `CAN_AUTHOR_ON_BEHALF` bit is consulted. `Member`, `Admin`, `ReadOnly` and
//!   `ReadOnlyTee` never sign on someone's behalf, whatever their capability row
//!   says.
//! * The on-behalf account must be an **effective member** of the context's group
//!   whose role may write: deny-list aware, inheritance included, never a
//!   read-only role.
//!
//! # Narrower than relaying
//!
//! `executor_standing` also lets an `Admin` or `Member` holding
//! `CAN_AUTHOR_ON_BEHALF` relay a warranted write. Such a relay passes the
//! warrant gate, but the entries it signs fail this rule on every peer. So the
//! relay applies this rule too, before it runs the write, and refuses at the API
//! instead of publishing entries every peer would drop.

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use thiserror::Error as ThisError;

use crate::warrant_admission::{namespace_tee_role, LiveReads};
use crate::{AdmissionCut, StandingReads};

/// Why an entry written on an account's behalf is refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ThisError)]
pub enum OnBehalfRefusal {
    /// The signer's account is not a `RelayTee` in the context's namespace.
    #[error(
        "the signer is not a RelayTee in this namespace, and only a RelayTee writes on a \
         member's behalf"
    )]
    SignerNotARelay,
    /// The account the entry is written for is not a member of the context's
    /// group.
    #[error(
        "the account the entry is written for is not a member of the group owning this context"
    )]
    AccountNotAMember,
    /// The account the entry is written for holds a read-only role there.
    #[error("the account the entry is written for is read-only in the group owning this context")]
    AccountIsReadOnly,
}

/// Whether an entry signed by `signer` (whose account is `signer_account`) may
/// be written on behalf of `on_behalf` in `group_id`, read at `cut`.
///
/// `signer_account` is the caller's resolution of `signer`: the binding at the
/// delta's cut, the live binding on repair, every certificate ever folded on a
/// snapshot, this node's own account on the relay. The rule is about accounts;
/// how a key is placed differs per path and is the caller's business. `signer`
/// itself only names the statement in an undecidable-cut error.
///
/// `Ok(Ok(()))` accepts, `Ok(Err(_))` refuses for the reason given.
///
/// # Errors
/// `ApplyError::AuthorityUndecidable` when `cut` is real but not folded here
/// (the caller defers and retries), or a store failure.
pub fn on_behalf_standing(
    store: &Store,
    group_id: &ContextGroupId,
    signer: &PublicKey,
    signer_account: AccountId,
    on_behalf: AccountId,
    cut: AdmissionCut<'_>,
) -> EyreResult<Result<(), OnBehalfRefusal>> {
    let at_cut = cut.reads(group_id, signer)?;
    let live = LiveReads::new(store);
    let reads: &dyn StandingReads = at_cut.as_deref().unwrap_or(&live);
    standing(store, reads, group_id, signer_account, on_behalf)
}

/// [`on_behalf_standing`] over this replica's live rows: for the relay before it
/// signs, and for the repair and snapshot paths, which carry no cut.
///
/// # Errors
/// A store failure.
pub fn on_behalf_standing_live(
    store: &Store,
    group_id: &ContextGroupId,
    signer_account: AccountId,
    on_behalf: AccountId,
) -> EyreResult<Result<(), OnBehalfRefusal>> {
    standing(
        store,
        &LiveReads::new(store),
        group_id,
        signer_account,
        on_behalf,
    )
}

fn standing(
    store: &Store,
    reads: &dyn StandingReads,
    group_id: &ContextGroupId,
    signer_account: AccountId,
    on_behalf: AccountId,
) -> EyreResult<Result<(), OnBehalfRefusal>> {
    let Some((role, role_group)) = reads.effective_role(group_id, &signer_account)? else {
        return Ok(Err(OnBehalfRefusal::SignerNotARelay));
    };
    let (role, _) = namespace_tee_role(store, reads, group_id, &signer_account, role, role_group)?;
    if role != GroupMemberRole::RelayTee {
        return Ok(Err(OnBehalfRefusal::SignerNotARelay));
    }
    Ok(match reads.effective_role(group_id, &on_behalf)? {
        None => Err(OnBehalfRefusal::AccountNotAMember),
        Some((role, _)) if role.is_read_only() => Err(OnBehalfRefusal::AccountIsReadOnly),
        Some(_) => Ok(()),
    })
}

#[cfg(test)]
mod tests {
    use calimero_account::AccountId;
    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::{MemberCapabilities, VisibilityMode};
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PublicKey;
    use calimero_store::Store;

    use super::{on_behalf_standing, on_behalf_standing_live, OnBehalfRefusal};
    use crate::authorizer::{AtCutAuthorizer, AtCutMembershipPath};
    use crate::test_fixtures::{
        nest_for_test, sample_meta_with_admin, test_store, UnresolvableAuthorizer, TEST_CUT,
    };
    use crate::warrant_admission::LiveReads;
    use crate::{
        AdmissionCut, ApplyError, CapabilitiesRepository, DenyListRepository, MembershipRepository,
        MetaRepository,
    };

    const RELAY: [u8; 32] = [0x7E; 32];
    const AUTHOR: [u8; 32] = [0xA1; 32];
    const SIGNER_KEY: [u8; 32] = [0x0B; 32];

    /// A namespace with an Open subgroup (where the context lives), a relay
    /// admitted at the root only, and an author admitted at the root, both
    /// inheriting into the subgroup.
    struct World {
        store: Store,
        namespace: ContextGroupId,
        subgroup: ContextGroupId,
        relay: AccountId,
        author: AccountId,
    }

    fn world(relay_role: GroupMemberRole) -> World {
        let store = test_store();
        let namespace = ContextGroupId::from([0xB1; 32]);
        let subgroup = ContextGroupId::from([0xB2; 32]);
        for gid in [namespace, subgroup] {
            MetaRepository::new(&store)
                .save(&gid, &sample_meta_with_admin(AccountId::from([0xEE; 32])))
                .expect("save meta");
        }
        nest_for_test(&store, &namespace, &subgroup);
        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
            .expect("open the subgroup");

        let relay = AccountId::from(RELAY);
        let author = AccountId::from(AUTHOR);
        let membership = MembershipRepository::new(&store);
        membership
            .add_member(&namespace, &relay, relay_role)
            .expect("admit the relay at the root");
        membership
            .add_member(&namespace, &author, GroupMemberRole::Member)
            .expect("admit the author at the root");
        for account in [relay, author] {
            CapabilitiesRepository::new(&store)
                .set_member_capability(
                    &namespace,
                    &account,
                    MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
                )
                .expect("let it inherit");
        }
        World {
            store,
            namespace,
            subgroup,
            relay,
            author,
        }
    }

    fn live(w: &World) -> Result<(), OnBehalfRefusal> {
        on_behalf_standing_live(&w.store, &w.subgroup, w.relay, w.author).expect("read standing")
    }

    /// The accept direction, which every refusal below assumes.
    #[test]
    fn a_relay_tee_writes_for_a_member() {
        assert_eq!(live(&world(GroupMemberRole::RelayTee)), Ok(()));
    }

    /// No other role signs for someone, whatever its capability row holds: the
    /// role is the grant, and `CAN_AUTHOR_ON_BEHALF` is not consulted.
    #[test]
    fn no_other_role_writes_for_a_member_even_holding_the_authorship_bit() {
        for role in [
            GroupMemberRole::Member,
            GroupMemberRole::Admin,
            GroupMemberRole::ReadOnly,
            GroupMemberRole::ReadOnlyTee,
        ] {
            let w = world(role.clone());
            CapabilitiesRepository::new(&w.store)
                .set_member_capability(
                    &w.namespace,
                    &w.relay,
                    (MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS
                        | MemberCapabilities::CAN_AUTHOR_ON_BEHALF)
                        .bits(),
                )
                .expect("grant authorship");
            assert_eq!(
                live(&w),
                Err(OnBehalfRefusal::SignerNotARelay),
                "{role:?} must not sign on a member's behalf"
            );
        }
    }

    /// The namespace root row decides a TEE's role, as it does for relaying: a
    /// subgroup copy left as a replica does not stop a relay the root says is one.
    #[test]
    fn a_tee_role_is_read_at_the_namespace_root() {
        let w = world(GroupMemberRole::RelayTee);
        MembershipRepository::new(&w.store)
            .add_member(&w.subgroup, &w.relay, GroupMemberRole::ReadOnlyTee)
            .expect("a stale replica copy in the subgroup");
        assert_eq!(live(&w), Ok(()));

        let w = world(GroupMemberRole::ReadOnlyTee);
        MembershipRepository::new(&w.store)
            .add_member(&w.subgroup, &w.relay, GroupMemberRole::RelayTee)
            .expect("a stale relay copy in the subgroup");
        assert_eq!(live(&w), Err(OnBehalfRefusal::SignerNotARelay));
    }

    /// A relay deny-listed off the subgroup is no member there, so it is no
    /// relay there either.
    #[test]
    fn a_deny_listed_relay_does_not_write_there() {
        let w = world(GroupMemberRole::RelayTee);
        DenyListRepository::new(&w.store)
            .mark(&w.subgroup, &w.relay)
            .expect("deny-list the relay");
        assert_eq!(live(&w), Err(OnBehalfRefusal::SignerNotARelay));
    }

    #[test]
    fn a_relay_does_not_write_for_a_stranger() {
        let w = world(GroupMemberRole::RelayTee);
        assert_eq!(
            on_behalf_standing_live(&w.store, &w.subgroup, w.relay, AccountId::from([0x51; 32]))
                .expect("read standing"),
            Err(OnBehalfRefusal::AccountNotAMember)
        );
    }

    #[test]
    fn a_relay_does_not_write_for_a_deny_listed_member() {
        let w = world(GroupMemberRole::RelayTee);
        DenyListRepository::new(&w.store)
            .mark(&w.subgroup, &w.author)
            .expect("deny-list the author");
        assert_eq!(live(&w), Err(OnBehalfRefusal::AccountNotAMember));
    }

    /// Read-only is the account's own rule, and a relay is not a way round it;
    /// the role is the inherited one, which a direct-row read would miss.
    #[test]
    fn a_relay_does_not_write_for_a_read_only_member() {
        let w = world(GroupMemberRole::RelayTee);
        MembershipRepository::new(&w.store)
            .set_role(&w.namespace, &w.author, GroupMemberRole::ReadOnly)
            .expect("make the author read-only at the root");
        assert_eq!(live(&w), Err(OnBehalfRefusal::AccountIsReadOnly));
    }

    /// Answers standing from a store holding the state as of the cut.
    struct StateAt(Store);

    impl AtCutAuthorizer for StateAt {
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
        ) -> Option<AtCutMembershipPath> {
            None
        }
        fn effective_role_at_cut(
            &self,
            _: &ContextGroupId,
            _: &AccountId,
            _: &[[u8; 32]],
        ) -> Option<Option<GroupMemberRole>> {
            None
        }
        fn context_rotation_group_at_cut(
            &self,
            _: &ContextGroupId,
            _: &calimero_primitives::context::ContextId,
            _: &[[u8; 32]],
        ) -> Option<Option<ContextGroupId>> {
            None
        }
        fn can_resolve_cut(&self, _: &ContextGroupId, _: &[[u8; 32]]) -> bool {
            true
        }
        fn standing_reads_at_cut<'s>(
            &'s self,
            _: &ContextGroupId,
            _: &[[u8; 32]],
        ) -> Option<Box<dyn crate::StandingReads + 's>> {
            Some(Box::new(LiveReads::new(&self.0)))
        }
    }

    /// At a cut the cut's state decides, not this replica's rows: a relay demoted
    /// after the cut still wrote as a relay at it, and one this replica holds as
    /// a relay but the cut does not is refused.
    #[test]
    fn at_a_cut_the_cut_decides() {
        let now = world(GroupMemberRole::ReadOnlyTee);
        let then = StateAt(world(GroupMemberRole::RelayTee).store);
        let at = |w: &World, cut: &StateAt| {
            on_behalf_standing(
                &w.store,
                &w.subgroup,
                &PublicKey::from(SIGNER_KEY),
                w.relay,
                w.author,
                AdmissionCut::at(cut, &TEST_CUT),
            )
            .expect("read standing at the cut")
        };
        assert_eq!(live(&now), Err(OnBehalfRefusal::SignerNotARelay));
        assert_eq!(at(&now, &then), Ok(()));

        let now = world(GroupMemberRole::RelayTee);
        let then = StateAt(world(GroupMemberRole::ReadOnlyTee).store);
        assert_eq!(live(&now), Ok(()));
        assert_eq!(at(&now, &then), Err(OnBehalfRefusal::SignerNotARelay));
    }

    /// A cut this replica has not folded is undecidable, never answered live.
    #[test]
    fn an_unfolded_cut_is_undecidable() {
        let w = world(GroupMemberRole::RelayTee);
        let err = on_behalf_standing(
            &w.store,
            &w.subgroup,
            &PublicKey::from(SIGNER_KEY),
            w.relay,
            w.author,
            AdmissionCut::at(&UnresolvableAuthorizer, &TEST_CUT),
        )
        .expect_err("an unfolded cut must not be answered");
        assert!(matches!(
            err.downcast_ref::<ApplyError>(),
            Some(ApplyError::AuthorityUndecidable { .. })
        ));
    }

    /// A member removed from the namespace, kicked or gone of its own accord
    /// (both delete the row), is no account a relay writes for.
    #[test]
    fn a_relay_does_not_write_for_a_member_that_left_or_was_removed() {
        let w = world(GroupMemberRole::RelayTee);
        MembershipRepository::new(&w.store)
            .remove_member(&w.namespace, &w.author)
            .expect("remove the author");
        assert_eq!(live(&w), Err(OnBehalfRefusal::AccountNotAMember));
    }

    /// A relay removed from the namespace no longer signs for anyone in it.
    #[test]
    fn a_relay_that_left_or_was_removed_writes_for_no_one() {
        let w = world(GroupMemberRole::RelayTee);
        MembershipRepository::new(&w.store)
            .remove_member(&w.namespace, &w.relay)
            .expect("remove the relay");
        assert_eq!(live(&w), Err(OnBehalfRefusal::SignerNotARelay));
    }

    /// A `RelayTee` of another namespace is not one here: the role is read in
    /// the namespace owning the context, not wherever the signer holds it.
    #[test]
    fn a_relay_tee_of_another_namespace_does_not_write_here() {
        let w = world(GroupMemberRole::Member);
        let elsewhere = ContextGroupId::from([0xC1; 32]);
        MetaRepository::new(&w.store)
            .save(
                &elsewhere,
                &sample_meta_with_admin(AccountId::from([0xEE; 32])),
            )
            .expect("save meta");
        MembershipRepository::new(&w.store)
            .add_member(&elsewhere, &w.relay, GroupMemberRole::RelayTee)
            .expect("a relay in another namespace");
        assert_eq!(live(&w), Err(OnBehalfRefusal::SignerNotARelay));
    }
}
