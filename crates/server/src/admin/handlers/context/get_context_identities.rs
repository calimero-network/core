//! `GET /admin-api/contexts/:id/identities` and `…/identities-owned`.
//!
//! # What "owned" means, and why it had to change
//!
//! `owned` was a statement about the NODE: `get_context_members(.., Some(true))`
//! yields the rows whose private key this node holds (or the keyless marker for
//! its namespace identity). On a node the caller owns that is the right answer
//! and it stays the answer.
//!
//! For a delegated caller it is the wrong question twice over. It reports which
//! identities a machine they do not own can sign as — no use to them — and it
//! discloses that set to anyone who asks, the same unscoped shape #3941 closed
//! for contexts and namespaces.
//!
//! So `owned` is now relative to the CALLER: "the identities you can act as in
//! this context". A node-owner session (and a node running without the auth
//! guard) keeps the node-wide reading unchanged.
//!
//! # What a delegated caller gets, and what it does not
//!
//! Its account's live device bindings in the group owning this context, as
//! `sign_pk` — the key a warrant is signed with and the replica a write is
//! attributed to (`Warrant::author_device_key`, `Principal::device`).
//!
//! Not a `ContextIdentity` row, deliberately: those exist for identities the
//! node holds a key for, and a delegated device has none by definition. Asking
//! for one would return empty for every delegated caller and read as "you are
//! not a member here".
//!
//! The node cannot verify POSSESSION — it cannot know which private keys a
//! client actually holds, only which the account has certified and which are
//! still live (`devices_of` reads live bindings, so a revoked or superseded
//! device is already gone). So the promise is exactly that: these are your
//! account's certified, unrevoked devices that this group knows. A client with
//! several devices intersects the list with its own keystore.
//!
//! # Why reads do not need this
//!
//! The delegated read (#3931) runs with the node's own signer as the device half
//! precisely because "a read writes nothing for a replica to own". This endpoint
//! answers the write question: which key may sign a warrant here.

use std::sync::Arc;

use axum::extract::{Path, Request};
use axum::response::IntoResponse;
use axum::Extension;
use calimero_governance_store::AccountBindingRepository;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::GetContextIdentitiesResponse;
use futures_util::TryStreamExt;
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::caller_scope::{list_scope_for, ListScope};
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::auth::{AuthenticatedAccount, AuthenticatedNodeOwner};
use crate::AdminState;

pub async fn handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
    node_owner: Option<Extension<AuthenticatedNodeOwner>>,
    account: Option<Extension<AuthenticatedAccount>>,
    req: Request,
) -> impl IntoResponse {
    let owned = req.uri().path().ends_with("identities-owned");

    info!(context_id=%context_id, owned=%owned, "Getting context identities");

    let context = match state.ctx_client.get_context(&context_id) {
        Ok(Some(context)) => context,
        Ok(None) => {
            info!(context_id=%context_id, "Context not found");
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Context not found".into(),
            }
            .into_response();
        }
        Err(err) => {
            error!(context_id=%context_id, error=?err, "Failed to get context");
            return parse_api_error(err).into_response();
        }
    };

    // Resolved per request, never cached on the session — the same rule the
    // delegated read follows. A membership change is a governance op this node
    // has already applied, and asking at the moment of the call is the only way
    // it reaches the answer.
    let scope = match list_scope_for(&state.ctx_client, node_owner, account) {
        Ok(scope) => scope,
        Err(err) => {
            error!(context_id=%context_id, error=?err, "Failed to resolve caller scope");
            return parse_api_error(err).into_response();
        }
    };

    let group = match calimero_governance_store::get_group_for_context(
        state.ctx_client.datastore(),
        &context.id,
    ) {
        Ok(group) => group,
        Err(err) => {
            error!(context_id=%context_id, error=?err, "Failed to resolve the context's group");
            return parse_api_error(err).into_response();
        }
    };

    let (account, group) = match identities_view(owned, &scope, group.as_ref()) {
        IdentitiesView::Refused => {
            info!(context_id=%context_id, "Refusing identities: caller is not a member of this context's group");
            return ApiError {
                status_code: StatusCode::FORBIDDEN,
                message: "account is not a member of the group owning this context".to_owned(),
            }
            .into_response();
        }
        IdentitiesView::CallerDevices { account, group } => (account, group),
        IdentitiesView::NodeMembers { owned } => {
            let stream = state
                .ctx_client
                .get_context_members(&context.id, Some(owned));

            return match stream.map_ok(|(id, _)| id).try_collect::<Vec<_>>().await {
                Ok(identities) => {
                    info!(context_id=%context_id, count=%identities.len(), "Context identities retrieved successfully");
                    ApiResponse {
                        payload: GetContextIdentitiesResponse::new(identities),
                    }
                    .into_response()
                }
                Err(err) => {
                    error!(context_id=%context_id, error=?err, "Failed to process identities");
                    ApiError {
                        status_code: StatusCode::INTERNAL_SERVER_ERROR,
                        message: "Failed to process identities".into(),
                    }
                    .into_response()
                }
            };
        }
    };

    let devices = match AccountBindingRepository::new(state.ctx_client.datastore())
        .devices_of(&group, account)
    {
        Ok(devices) => devices,
        Err(err) => {
            error!(context_id=%context_id, error=?err, "Failed to resolve the caller's devices");
            return parse_api_error(err).into_response();
        }
    };
    let identities: Vec<_> = devices.into_iter().map(|binding| binding.sign_pk).collect();
    info!(
        context_id=%context_id,
        count=%identities.len(),
        "Caller's context identities retrieved successfully",
    );
    ApiResponse {
        payload: GetContextIdentitiesResponse::new(identities),
    }
    .into_response()
}

/// Which reading of `owned` this caller gets.
///
/// Split out for the same reason `caller_scope::narrow_to` is: every arm is a
/// security boundary, and a decision that needs a populated store to exercise
/// does not get exercised. The store work each arm implies is the handler's.
#[derive(Debug, Eq, PartialEq)]
enum IdentitiesView {
    /// Stream the context's member rows, `owned` meaning what it always meant —
    /// the identities this NODE holds a key for. The node-owner reading, and the
    /// full roster on `/identities`.
    NodeMembers { owned: bool },
    /// The caller account's certified devices in the group owning this context.
    CallerDevices {
        account: calimero_account::AccountId,
        group: calimero_context_config::types::ContextGroupId,
    },
    /// The caller is not a member of this context's group.
    Refused,
}

fn identities_view(
    owned: bool,
    scope: &ListScope,
    group: Option<&calimero_context_config::types::ContextGroupId>,
) -> IdentitiesView {
    // `admits` fails closed on a group that will not resolve: an account scope
    // refuses rather than falling back to the node-wide answer, while a
    // node-wide scope admits everything and is unaffected.
    if !scope.admits(group) {
        return IdentitiesView::Refused;
    }

    match (owned, scope, group) {
        // Only an account scope asking what it OWNS gets the caller reading.
        // `/identities` stays the context's member roster for everyone: the
        // caller is a member by the check above, and narrowing a roster to the
        // caller's own devices would answer a different question than it asks.
        (true, ListScope::Account { account, .. }, Some(group)) => IdentitiesView::CallerDevices {
            account: *account,
            group: *group,
        },
        (owned, _, _) => IdentitiesView::NodeMembers { owned },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use calimero_account::AccountId;
    use calimero_context_config::types::ContextGroupId;

    use super::{identities_view, IdentitiesView};
    use crate::admin::caller_scope::ListScope;

    fn account() -> AccountId {
        AccountId::from([0x01; 32])
    }

    fn mine() -> ContextGroupId {
        ContextGroupId::from([0xa1; 32])
    }

    fn theirs() -> ContextGroupId {
        ContextGroupId::from([0xb1; 32])
    }

    fn account_scope() -> ListScope {
        ListScope::Account {
            account: account(),
            groups: BTreeSet::from([mine()]),
        }
    }

    /// The single-tenant guarantee. Nothing here may change what a node owner
    /// sees, on either route.
    #[test]
    fn a_node_owner_keeps_the_node_wide_reading() {
        assert_eq!(
            identities_view(true, &ListScope::NodeWide, Some(&mine())),
            IdentitiesView::NodeMembers { owned: true },
        );
        assert_eq!(
            identities_view(false, &ListScope::NodeWide, Some(&mine())),
            IdentitiesView::NodeMembers { owned: false },
        );
    }

    /// The disclosure this change exists to stop: before it, an account caller
    /// asking a context it has no part in was handed the node's own signing
    /// identities.
    #[test]
    fn an_account_outside_the_group_is_refused() {
        assert_eq!(
            identities_view(true, &account_scope(), Some(&theirs())),
            IdentitiesView::Refused,
        );
        assert_eq!(
            identities_view(false, &account_scope(), Some(&theirs())),
            IdentitiesView::Refused,
        );
    }

    /// The point of the change: `owned` answers "what can I act as", not "what
    /// can the node act as".
    #[test]
    fn an_account_asking_what_it_owns_gets_its_own_devices() {
        assert_eq!(
            identities_view(true, &account_scope(), Some(&mine())),
            IdentitiesView::CallerDevices {
                account: account(),
                group: mine(),
            },
        );
    }

    /// `/identities` is the context's member ROSTER and stays that for everyone.
    /// Narrowing it to the caller's own devices would answer a different
    /// question than the route asks — and the caller is a member by the time
    /// this arm is reached, so the roster is not a disclosure to them.
    #[test]
    fn an_account_asking_for_the_roster_still_gets_the_roster() {
        assert_eq!(
            identities_view(false, &account_scope(), Some(&mine())),
            IdentitiesView::NodeMembers { owned: false },
        );
    }

    /// Fail-closed on a context whose group will not resolve. An unresolvable
    /// group is not evidence of membership, and treating it as one would make
    /// the disclosure conditional on a lookup failure rather than on the rule.
    #[test]
    fn an_unresolvable_group_refuses_an_account_but_not_the_node_owner() {
        assert_eq!(
            identities_view(true, &account_scope(), None),
            IdentitiesView::Refused,
            "an account scope must not fall back to the node-wide answer",
        );
        assert_eq!(
            identities_view(true, &ListScope::NodeWide, None),
            IdentitiesView::NodeMembers { owned: true },
            "a node owner's view must not narrow because a group lookup failed",
        );
    }
}
