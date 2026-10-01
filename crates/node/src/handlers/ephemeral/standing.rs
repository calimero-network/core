//! Whether an account may publish presence in a context, by this node's own
//! governance data. Shared by the receive path and a relay's publish path, so
//! the two cannot disagree.

use calimero_account::DeviceId;
use calimero_governance_store::{AccountBindingRepository, MembershipRepository};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::AccountId;
use calimero_store::Store;

/// An account's standing for presence in one context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Standing {
    Member,
    NotAMember,
    DeviceRevoked,
}

/// Any role counts, read-only included, as for a node's presence. Deny-list
/// aware and inheritance included (`effective_role`), in the context's own
/// group, so a DM's restricted subgroup is asked about its own members.
pub(crate) fn account_standing(
    store: &Store,
    context_id: &ContextId,
    account: AccountId,
    device: DeviceId,
) -> eyre::Result<Standing> {
    let group = calimero_governance_store::get_group_for_context(store, context_id)?
        .ok_or_else(|| eyre::eyre!("context has no group"))?;
    if AccountBindingRepository::new(store).device_is_withdrawn(&group, account, device)? {
        return Ok(Standing::DeviceRevoked);
    }
    Ok(
        match MembershipRepository::new(store).effective_role(&group, &account)? {
            Some(_) => Standing::Member,
            None => Standing::NotAMember,
        },
    )
}

#[cfg(test)]
mod tests {
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::register_context_in_group;
    use calimero_governance_store::test_fixtures::{
        enrol_member, nest_for_test, real_join_account, test_store,
    };
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PublicKey;

    use super::*;

    /// A namespace with a subgroup context, and an account that is a member of
    /// the subgroup and holds one device bound in the namespace.
    fn world() -> (Store, ContextGroupId, ContextId, AccountId, DeviceId) {
        let store = test_store();
        let ns = ContextGroupId::from([0xA0; 32]);
        let sub = ContextGroupId::from([0xA1; 32]);
        let context = ContextId::from([0xA2; 32]);
        let key = PublicKey::from([0x5D; 32]);
        let account = enrol_member(&store, &ns, &key);
        let device = real_join_account(&key).statement.device;
        nest_for_test(&store, &ns, &sub);
        MembershipRepository::new(&store)
            .add_member(&sub, &account, GroupMemberRole::Member)
            .expect("add the account to the subgroup");
        register_context_in_group(&store, &sub, &context).expect("register the context");
        (store, ns, context, account, device)
    }

    #[test]
    fn a_live_device_has_standing_in_a_subgroup_context() {
        let (store, _ns, context, account, device) = world();
        assert_eq!(
            account_standing(&store, &context, account, device).expect("read"),
            Standing::Member
        );
    }

    #[test]
    fn a_device_revoked_in_the_namespace_has_no_standing_in_a_subgroup_context() {
        let (store, ns, context, account, device) = world();
        AccountBindingRepository::new(&store)
            .apply_revocation(&ns, device)
            .expect("revoke");
        assert_eq!(
            account_standing(&store, &context, account, device).expect("read"),
            Standing::DeviceRevoked
        );
    }

    #[test]
    fn a_device_narrowed_out_of_the_namespace_has_no_standing_in_it() {
        let (store, ns, context, account, device) = world();
        AccountBindingRepository::new(&store)
            .narrow(&ns, account, device, 1)
            .expect("narrow");
        assert_eq!(
            account_standing(&store, &context, account, device).expect("read"),
            Standing::DeviceRevoked
        );
    }
}
