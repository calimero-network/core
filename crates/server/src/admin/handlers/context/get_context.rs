use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::GetContextResponse;
use reqwest::StatusCode;
use tracing::{debug, error, info};

use crate::admin::caller_scope::{list_scope_for, ListScope};
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::auth::{AuthenticatedAccount, AuthenticatedNodeOwner};
use crate::AdminState;

pub async fn handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
    node_owner: Option<Extension<AuthenticatedNodeOwner>>,
    account: Option<Extension<AuthenticatedAccount>>,
) -> impl IntoResponse {
    debug!(context_id=%context_id, "Getting context");

    // The single-context read has to answer the same question the listing does,
    // or the listing's scoping is decoration: an account that cannot see a
    // context in `GET /admin-api/contexts` must not be able to read it by naming
    // its id here.
    //
    // Resolved per request, never cached on the session — the rule the delegated
    // read and `/contexts/:id/identities` both follow. A membership change is a
    // governance op this node has already applied, and asking at the moment of
    // the call is the only way it reaches the answer.
    //
    // `caller_scope` rather than `caller_account::for_context`: the latter maps
    // an authenticated *key* to the account it acts as, which is the question a
    // key-anchored session poses. This caller is already an account, so there is
    // nothing to resolve, and `ListScope::admits` is the same predicate the
    // listing applies — one rule, so the two cannot drift apart. A node owner,
    // and a node running with no auth guard, get `NodeWide` and are unaffected.
    let scope = match list_scope_for(&state.ctx_client, node_owner, account) {
        Ok(scope) => scope,
        Err(err) => {
            error!(context_id=%context_id, error=?err, "Failed to resolve the caller's list scope");
            return parse_api_error(err).into_response();
        }
    };

    // todo! experiment with Interior<Store>: WriteLayer<Interior>
    let context = state
        .ctx_client
        .get_context(&context_id)
        .map_err(|err| parse_api_error(err).into_response());

    match context {
        Ok(ctx) => match ctx {
            Some(mut context) => {
                match admits_context(state.ctx_client.datastore(), &context_id, &scope) {
                    Ok(true) => {}
                    Ok(false) => {
                        info!(context_id=%context_id, "Refusing context: caller is not a member of this context's group");
                        return ApiError {
                            status_code: StatusCode::FORBIDDEN,
                            message: "account is not a member of the group owning this context"
                                .to_owned(),
                        }
                        .into_response();
                    }
                    Err(err) => {
                        error!(context_id=%context_id, error=?err, "Failed to resolve the context's group");
                        return parse_api_error(err).into_response();
                    }
                }

                // Per-context executing version (activation marker) wins over
                // the application row's latest-installed version.
                if let Some(v) = state
                    .ctx_client
                    .executing_application_version(&context_id)
                    .await
                {
                    context.application_version = Some(v);
                }
                ApiResponse {
                    payload: GetContextResponse { data: context },
                }
                .into_response()
            }
            None => {
                info!(context_id=%context_id, "Context not found");
                ApiError {
                    status_code: StatusCode::NOT_FOUND,
                    message: "Context not found".into(),
                }
                .into_response()
            }
        },
        Err(err) => {
            error!(context_id=%context_id, "Failed to get context");
            err.into_response()
        }
    }
}

/// Whether `scope` may be served this context.
///
/// The named-id counterpart of `get_context_ids`'s enumeration: there the
/// caller's groups produce the list, here a context produces its group and the
/// scope decides. One predicate either way — `ListScope::admits` — so the two
/// routes cannot answer differently about the same context.
///
/// A store fault is returned rather than swallowed, because an outage must not
/// read as a quiet permissions change. An honest *absence* — a context owned by
/// no group — is a refusal, since `admits` fails closed on `None` for an account
/// scope and there is no membership to check against.
fn admits_context(
    store: &calimero_store::Store,
    context_id: &ContextId,
    scope: &ListScope,
) -> eyre::Result<bool> {
    // Node-wide callers (a node owner, or a node with no auth guard at all) are
    // admitted without the lookup: the answer cannot change and the read would
    // only add a way for the endpoint to fail.
    if matches!(scope, ListScope::NodeWide) {
        return Ok(true);
    }

    let group_id = calimero_governance_store::get_group_for_context(store, context_id)?;
    Ok(scope.admits(group_id.as_ref()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use calimero_account::AccountId;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::ContextTreeService;
    use calimero_primitives::context::ContextId;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::admits_context;
    use crate::admin::caller_scope::ListScope;

    fn mine() -> ContextGroupId {
        ContextGroupId::from([0xa1; 32])
    }

    fn theirs() -> ContextGroupId {
        ContextGroupId::from([0xb1; 32])
    }

    fn my_context() -> ContextId {
        ContextId::from([0x01; 32])
    }

    fn their_context() -> ContextId {
        ContextId::from([0x02; 32])
    }

    /// A context owned by no group at all.
    fn orphan_context() -> ContextId {
        ContextId::from([0x03; 32])
    }

    fn store_with_two_tenants() -> Store {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        ContextTreeService::new(&store, mine())
            .register_context(&my_context())
            .expect("register");
        ContextTreeService::new(&store, theirs())
            .register_context(&their_context())
            .expect("register");
        store
    }

    fn my_scope() -> ListScope {
        ListScope::Account {
            account: AccountId::from([0x01; 32]),
            groups: BTreeSet::from([mine()]),
        }
    }

    #[test]
    fn an_account_is_served_a_context_in_its_own_group() {
        assert!(admits_context(&store_with_two_tenants(), &my_context(), &my_scope()).unwrap());
    }

    /// The criterion this function exists for: scoping the listing is decoration
    /// if naming an id reaches a context the listing would have hidden.
    #[test]
    fn an_account_is_refused_a_context_it_is_not_a_member_of() {
        assert!(!admits_context(&store_with_two_tenants(), &their_context(), &my_scope()).unwrap());
    }

    /// Fails closed where there is no membership to check: "no rule, therefore
    /// allowed" is how a stranger gets in.
    #[test]
    fn a_context_owned_by_no_group_is_refused_an_account_scope() {
        assert!(
            !admits_context(&store_with_two_tenants(), &orphan_context(), &my_scope()).unwrap()
        );
    }

    /// Single-tenant operation is unchanged: a node owner, and a node running
    /// with no auth guard, keep today's view of every context — including one
    /// owned by no group.
    #[test]
    fn a_node_wide_scope_is_served_everything() {
        let store = store_with_two_tenants();
        for context_id in [my_context(), their_context(), orphan_context()] {
            assert!(admits_context(&store, &context_id, &ListScope::NodeWide).unwrap());
        }
    }
}
