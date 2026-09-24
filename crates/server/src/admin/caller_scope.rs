//! What a node-wide list endpoint is allowed to show the caller.
//!
//! `GET /admin-api/contexts`, `/namespaces` and `/blobs` returned everything the
//! *node* held, with no scoping to the caller (#3941). On a single-tenant node
//! that is right. On a Calimero-operated relay it disclosed one tenant's context
//! ids, namespace ids and blobs to another.
//!
//! One module rather than a filter per handler, for the same reason
//! `execute::caller_authorized_for_context` is shared: three copies of "whose
//! rows are these" drift, and the drift is invisible until the endpoint that
//! kept the old copy is the one that leaks.

use std::collections::BTreeSet;

use calimero_account::AccountId;
use calimero_context_client::client::ContextClient;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::MembershipRepository;

use calimero_governance_store::AccountBindingRepository;
use calimero_primitives::identity::DeviceId;

use crate::auth::{AuthenticatedAccount, AuthenticatedDevice, AuthenticatedNodeOwner};

/// Which rows a caller may see.
#[derive(Clone, Debug)]
pub(crate) enum ListScope {
    /// Everything the node holds — unchanged from before #3941.
    ///
    /// This is the answer for a node owner, and for a node running without the
    /// auth guard at all. The second case is the one worth stating: under
    /// `AuthMode::Proxy` core installs no guard, so no extension is injected and
    /// every caller looks anonymous. Narrowing *there* would not be a security
    /// win — the proxy is what decides who gets through — it would just empty
    /// the endpoint on every default-configured node.
    NodeWide,
    /// Only what this account's groups reach.
    Account {
        account: AccountId,
        groups: BTreeSet<ContextGroupId>,
    },
}

impl ListScope {
    /// Whether `group_id` is inside the scope.
    ///
    /// A context whose group could not be resolved is **excluded** from an
    /// account scope and included in the node-wide one. Failing closed is the
    /// only safe direction: an unresolvable group is not evidence of membership,
    /// and treating it as one would make the leak conditional on a lookup
    /// failure rather than on the rule.
    pub(crate) fn admits(&self, group_id: Option<&ContextGroupId>) -> bool {
        match self {
            Self::NodeWide => true,
            Self::Account { groups, .. } => group_id.is_some_and(|g| groups.contains(g)),
        }
    }

    /// The account this scope is for, when it is an account scope.
    pub(crate) const fn account(&self) -> Option<&AccountId> {
        match self {
            Self::NodeWide => None,
            Self::Account { account, .. } => Some(account),
        }
    }
}

/// Which account, if any, a caller's listing should be narrowed to.
///
/// Split from [`list_scope`] so the decision is testable without a store: it is
/// three lines and every one of them is a security boundary, while resolving the
/// group set needs a populated node.
///
/// Node-owner wins over account. A session can carry both — the node's owner may
/// also be account-anchored — and the owner's view is the wider one; narrowing
/// them would be a regression in single-tenant operation, which is what the
/// "node-owner keeps today's view" criterion protects.
pub(crate) const fn narrow_to(
    node_owner: Option<&AuthenticatedNodeOwner>,
    account: Option<&AuthenticatedAccount>,
) -> Option<AccountId> {
    match (node_owner, account) {
        (Some(_), _) | (None, None) => None,
        (None, Some(AuthenticatedAccount(account))) => Some(*account),
    }
}

/// Resolve what this caller may list.
///
/// `device` is the one that proved this caller, when one was named. `None` is
/// not "no device" — it is "we were not told which". A request-carried proof
/// names the device its certificate covers; a session does not, because
/// `account_proof` mints a token whose subject is the account and the device
/// that logged in is dropped there.
///
/// So the revocation filter below applies to proofs and not to sessions, which
/// means a session survives the revocation of the very device that opened it.
/// That is a gap in the session path rather than a decision here, and closing
/// it means carrying the device through the login — a change in the auth store,
/// not in this function.
///
/// The group set is resolved **per request**, never cached on the session — the
/// same rule the delegated read follows (#3931). A membership change is a
/// governance op this node has already applied, and the only way it reaches the
/// answer is by asking at the moment of the call; a set captured at login would
/// keep listing a group after the caller was removed from it.
pub(crate) fn list_scope(
    ctx_client: &ContextClient,
    node_owner: Option<&AuthenticatedNodeOwner>,
    account: Option<&AuthenticatedAccount>,
    device: Option<DeviceId>,
) -> eyre::Result<ListScope> {
    let Some(account) = narrow_to(node_owner, account) else {
        return Ok(ListScope::NodeWide);
    };

    let store = ctx_client.datastore();
    let mut groups = MembershipRepository::new(store).effective_groups_for_account(&account)?;

    // Revocation is per device AND per group, so it can only be applied where
    // both are in hand — which is here, and not at authentication. A device
    // revoked in one group may be live in another, so this removes groups
    // rather than refusing the caller: the account is still itself, and still a
    // member everywhere the revocation does not reach.
    //
    // A read that fails removes the group. An unreadable revocation row is not
    // evidence of a live device, and treating it as one would make the check
    // conditional on the store answering.
    if let Some(device) = device {
        let bindings = AccountBindingRepository::new(store);
        groups.retain(|group| !bindings.is_revoked(group, device).unwrap_or(true));
    }

    Ok(ListScope::Account { account, groups })
}

/// `list_scope` against the extensions an axum handler receives.
pub(crate) fn list_scope_for(
    ctx_client: &ContextClient,
    node_owner: Option<axum::Extension<AuthenticatedNodeOwner>>,
    account: Option<axum::Extension<AuthenticatedAccount>>,
    device: Option<axum::Extension<AuthenticatedDevice>>,
) -> eyre::Result<ListScope> {
    list_scope(
        ctx_client,
        node_owner.as_ref().map(|e| &e.0),
        account.as_ref().map(|e| &e.0),
        device.map(|e| e.0 .0),
    )
}

#[cfg(test)]
mod tests {
    use super::{narrow_to, ListScope};
    use crate::auth::{AuthenticatedAccount, AuthenticatedNodeOwner};
    use calimero_account::AccountId;
    use calimero_context_config::types::ContextGroupId;
    use std::collections::BTreeSet;

    fn account() -> AccountId {
        AccountId::from([0x01; 32])
    }

    fn account_scope(groups: &[[u8; 32]]) -> ListScope {
        ListScope::Account {
            account: account(),
            groups: groups
                .iter()
                .map(|g| ContextGroupId::from(*g))
                .collect::<BTreeSet<_>>(),
        }
    }

    /// The single-tenant guarantee: nothing about this change may narrow what a
    /// node owner or an unguarded node sees.
    #[test]
    fn only_an_account_anchored_session_is_narrowed() {
        assert_eq!(
            narrow_to(Some(&AuthenticatedNodeOwner), None),
            None,
            "a node owner keeps the node-wide view"
        );
        assert_eq!(
            narrow_to(
                Some(&AuthenticatedNodeOwner),
                Some(&AuthenticatedAccount(account()))
            ),
            None,
            "carrying both is the node's owner who also holds an account; the \
             owner's view is the wider one and narrowing it would regress \
             single-tenant operation"
        );
        assert_eq!(
            narrow_to(None, None),
            None,
            "no extension at all is AuthMode::Proxy, where core installs no \
             guard — narrowing there would empty the endpoint on every \
             default-configured node without closing anything, because the \
             proxy is what decides who gets through"
        );
        assert_eq!(
            narrow_to(None, Some(&AuthenticatedAccount(account()))),
            Some(account()),
            "an account-anchored session is the caller #3941 is about"
        );
    }

    #[test]
    fn a_node_wide_scope_admits_everything_including_an_unresolved_group() {
        let scope = ListScope::NodeWide;
        assert!(scope.admits(Some(&ContextGroupId::from([0xaa; 32]))));
        assert!(scope.admits(None));
        assert_eq!(scope.account(), None);
    }

    #[test]
    fn an_account_scope_admits_only_its_own_groups() {
        let scope = account_scope(&[[0xa1; 32], [0xa2; 32]]);

        assert!(scope.admits(Some(&ContextGroupId::from([0xa1; 32]))));
        assert!(scope.admits(Some(&ContextGroupId::from([0xa2; 32]))));
        assert!(
            !scope.admits(Some(&ContextGroupId::from([0xb1; 32]))),
            "another tenant's group is the disclosure this exists to stop"
        );
        assert_eq!(scope.account(), Some(&account()));
    }

    /// The fail-closed direction, pinned on its own because getting it backwards
    /// makes the leak conditional on a lookup failure rather than on the rule —
    /// and a lookup failure is exactly the state nobody tests by hand.
    #[test]
    fn an_account_scope_excludes_a_context_whose_group_did_not_resolve() {
        assert!(!account_scope(&[[0xa1; 32]]).admits(None));
    }

    /// An account in no groups sees nothing, rather than falling through to
    /// everything. The empty set is a real answer here, not a missing one.
    #[test]
    fn an_account_in_no_groups_admits_nothing() {
        let scope = account_scope(&[]);
        assert!(!scope.admits(Some(&ContextGroupId::from([0xa1; 32]))));
        assert!(!scope.admits(None));
    }
}
