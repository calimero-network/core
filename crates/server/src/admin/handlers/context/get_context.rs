use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::GetContextResponse;
use reqwest::StatusCode;
use tracing::{debug, error, info};

use crate::admin::caller_scope::{admits_context, list_scope_for};
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::auth::{AuthenticatedAccount, AuthenticatedDevice, AuthenticatedNodeOwner};
use crate::AdminState;

pub async fn handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
    node_owner: Option<Extension<AuthenticatedNodeOwner>>,
    account: Option<Extension<AuthenticatedAccount>>,
    device: Option<Extension<AuthenticatedDevice>>,
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
    let scope = match list_scope_for(&state.ctx_client, node_owner, account, device) {
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
