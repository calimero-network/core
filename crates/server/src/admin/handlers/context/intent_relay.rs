//! `GET /admin-api/contexts/:context_id/intents` — what a keyholder needs to
//! know before it mints a warrant for this node.
//!
//! # Why this is a read on the same path as the write
//!
//! Minting a warrant is an offline act, and every input to it is something the
//! author already holds — except one. `Warrant::executor` names *this node's*
//! account, which is a content address the client has no way to derive, and
//! whether the node may act here at all is a row in the owning group's
//! capabilities that the client cannot read either.
//!
//! Both facts belong to the same question — "can this relay run my intent, and
//! whose name do I put in the warrant?" — so they are one answer, on the path
//! the intent will be presented to. A client that had to compose them from
//! `/admin-api/identity` plus a group read would need two calls, a group id it
//! does not have, and a credential on this node to make the second one.
//!
//! # Why it must be readable without the grant already existing
//!
//! `canAuthorOnBehalf: false` is the default state of every context: authorship
//! is deliberately not implied by membership, not implied by admin, and not
//! propagated by the subgroup cascade. So the common first answer here is "no",
//! and a client has to be able to *get* that answer in order to say "ask an
//! admin of this group to grant it" rather than presenting a warrant that will
//! be refused — after the author has already spent a nonce from its monotonic
//! sequence on it.

use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{IntentRelayApiResponse, IntentRelayApiResponseData};
use reqwest::StatusCode;
use tracing::error;

use crate::admin::handlers::identity::get_node_identity::node_identity;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(context_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let context_id: ContextId = match context_id_str.parse() {
        Ok(id) => id,
        Err(err) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: format!("context id '{context_id_str}' is not valid: {err}"),
            }
            .into_response()
        }
    };

    let store = state.ctx_client.datastore();

    // The group first: a context registered to none has nothing to authorize a
    // delegated write in, and reporting `canAuthorOnBehalf: false` for it would
    // send a client to look for an admin of a group that does not exist.
    let group_id = match calimero_governance_store::get_group_for_context(store, &context_id) {
        Ok(Some(group_id)) => group_id,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "this context belongs to no group, so a delegated write has no \
                          group to be authorized in"
                    .to_owned(),
            }
            .into_response()
        }
        Err(err) => {
            error!(error = ?err, %context_id, "Failed to resolve the group owning this context");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to resolve the group owning this context".to_owned(),
            }
            .into_response();
        }
    };

    // A failed read is not a missing row: telling a client "this node has no
    // account" when the truth is "could not look" sends it to mint a warrant
    // naming nobody.
    let executor_account = match node_identity(store) {
        Ok(Some((account, ..))) => account,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "this node holds neither a usable device nor an account root yet, \
                          so it can be named as no warrant's executor"
                    .to_owned(),
            }
            .into_response()
        }
        Err(err) => {
            error!(error = ?err, "Failed to read this node's identity");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to read this node's identity".to_owned(),
            }
            .into_response();
        }
    };

    // The same question `POST .../intents` asks before it executes, and the same
    // one every peer asks at the cut — read here so a client learns the answer
    // before it signs rather than from a 403 after it has.
    let can_author_on_behalf = match calimero_governance_store::warrant_gate::account_may_author(
        store,
        &context_id,
        executor_account,
    ) {
        Ok(may) => may,
        Err(err) => {
            error!(error = ?err, %context_id, "Failed to read this node's authorship grant");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to read this node's authorship grant".to_owned(),
            }
            .into_response();
        }
    };

    // Where the grant lives, which is a different question from whether it
    // applies — though the two now resolve through the same function, so this
    // being `Some` and `can_author_on_behalf` cannot disagree.
    //
    // Still read separately, rather than derived as `granted_on.is_some()`,
    // because `account_may_author` is what the gate and every applying peer
    // actually call. Deriving the bool would make this endpoint report the
    // descriptor's opinion of the gate; calling it reports the gate. If the two
    // are ever made to differ again, the endpoint should tell the truth about
    // the one that decides.
    let granted_on = match calimero_governance_store::warrant_gate::authorship_grant_source(
        store,
        &group_id,
        executor_account,
    ) {
        Ok(source) => source,
        Err(err) => {
            error!(error = ?err, %context_id, "Failed to locate this node's authorship grant");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to locate this node's authorship grant".to_owned(),
            }
            .into_response();
        }
    };

    ApiResponse {
        payload: IntentRelayApiResponse {
            data: IntentRelayApiResponseData {
                executor_account: hex::encode(executor_account.as_bytes()),
                can_author_on_behalf,
                group_id: hex::encode(group_id.to_bytes()),
                granted_on_group_id: granted_on.map(|g| hex::encode(g.to_bytes())),
            },
        },
    }
    .into_response()
}
