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
    if AccountBindingRepository::new(store).is_revoked(&group, device)? {
        return Ok(Standing::DeviceRevoked);
    }
    Ok(
        match MembershipRepository::new(store).effective_role(&group, &account)? {
            Some(_) => Standing::Member,
            None => Standing::NotAMember,
        },
    )
}
