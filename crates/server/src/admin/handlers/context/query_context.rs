//! `POST /admin-api/contexts/:context_id/query` — let an authenticated account
//! read a context it is a member of, without minting a warrant.
//!
//! # Why a read needs no warrant
//!
//! A warrant proves, to peers who never saw this HTTP request, that the author
//! consented to an operation. A read has no peer to convince: it publishes
//! nothing — no delta, no envelope, no DAG entry — so there is nobody downstream
//! whose acceptance depends on evidence of consent. What a read does need is
//! proof that *this* caller may see *this* context's state, and that is a
//! question about the caller's membership here and now, which a session answers
//! and a warrant does not.
//!
//! Before this existed the only way an account that runs no node could observe
//! state was as a side effect of a write: `PerformIntentApiResponseData` carries
//! the method's return value, so reading meant spending a warrant nonce, holding
//! `CAN_AUTHOR_ON_BEHALF`, and publishing a delta — to ask a question that
//! changes nothing.
//!
//! # What this handler does not decide
//!
//! Two refusals that look like they belong here live deeper, and deliberately:
//!
//! **Whether the method is read-only** is settled in the execute handler, after
//! the module has loaded, against the ABI's declared set. Deciding it here would
//! mean either duplicating the ABI lookup or reusing the read-only set computed
//! for *lock selection* — which is allowed to answer conservatively, so a cold
//! module cache would refuse a perfectly read-only method and the refusal would
//! depend on cache warmth.
//!
//! **Whether the caller is a member** is re-evaluated per call at the same
//! depth, under the execution lock, against the group that owns the context. It
//! is not cached from the session: one relay serves several tenants, and a
//! session carrying a standing right to read would keep serving a member after
//! the removal op this node has already applied.
//!
//! So this handler's whole job is to turn an authenticated identity and a JSON
//! body into a call, and to map what comes back onto statuses a client can act
//! on distinctly.

use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_context_client::client::ContextClient;
use calimero_context_client::messages::{ExecuteError, ReadAs};
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{
    QueryContextApiRequest, QueryContextApiResponse, QueryContextApiResponseData,
};
use calimero_server_primitives::jsonrpc::ExecutionError;
use futures_util::StreamExt as _;
use tracing::{debug, warn};

use crate::admin::service::{method_error_response, parse_api_error, ApiError, ApiResponse};
use crate::auth::{AuthenticatedAccount, AuthenticatedDevice};
use crate::AdminState;

pub async fn handler(
    Path(context_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    account: Option<Extension<AuthenticatedAccount>>,
    device: Option<Extension<AuthenticatedDevice>>,
    Json(req): Json<QueryContextApiRequest>,
) -> impl IntoResponse {
    let context_id: ContextId = match context_id_str.parse() {
        Ok(id) => id,
        Err(err) => {
            return parse_api_error(eyre::eyre!(
                "context id '{context_id_str}' is not valid: {err}"
            ))
            .into_response()
        }
    };

    // The account comes from the auth layer, never from the body. A caller may
    // choose which account it authenticates as and nothing more; there is no
    // field here it could set to read as somebody else.
    //
    // Absent means the session is not account-anchored — a node-owner or client
    // key session, or no auth guard at all. Those are not refused because they
    // lack permission; they are refused because this route answers "what may
    // THIS account see", and there is no account to answer for. A node owner
    // reads through the ordinary execute path.
    let Some(Extension(AuthenticatedAccount(account))) = account else {
        return ApiError {
            status_code: StatusCode::UNAUTHORIZED,
            message: "this endpoint requires an account-authenticated session".to_owned(),
        }
        .into_response();
    };

    // A device the namespace withdrew reads nothing in it. A session that names
    // no device is judged by its account alone.
    let device = device.map(|Extension(AuthenticatedDevice(device))| device);
    if let Some(device) = device {
        if crate::caller_account::device_withdrawn_for_context(
            &state.ctx_client,
            &context_id,
            account,
            device,
        ) {
            return ApiError {
                status_code: StatusCode::FORBIDDEN,
                message: "this device is no longer authorized in the namespace owning this context"
                    .to_owned(),
            }
            .into_response();
        }
    }

    answer(
        context_id,
        account,
        &req.method,
        query(
            &state.ctx_client,
            context_id,
            ReadAs { account, device },
            &req,
        )
        .await,
    )
}

/// The response for a read's result.
fn answer(
    context_id: ContextId,
    account: calimero_account::AccountId,
    method: &str,
    result: eyre::Result<Option<serde_json::Value>>,
) -> axum::response::Response {
    match result {
        Ok(returns) => ApiResponse {
            payload: QueryContextApiResponse {
                data: QueryContextApiResponseData { returns },
            },
        }
        .into_response(),
        Err(err) => {
            // The method ran and returned an error: not a refusal. The execute
            // path already warned with the app's message redacted; this is not.
            if let Some(method_error) = err.downcast_ref::<ExecutionError>() {
                debug!(%context_id, %account, %method, %err, "read's method returned an error");
                return method_error_response(method_error);
            }
            // The method is named here rather than inside `ExecuteError`, which
            // derives `Copy` and so cannot carry a `String`.
            warn!(%context_id, %account, %method, %err, "refusing read");
            refusal_status(err, method).into_response()
        }
    }
}

/// Map a refusal onto a status a client can branch on without parsing prose.
///
/// The three are genuinely different instructions to a client, which is why they
/// are not collapsed into one 403:
///
/// * `403` — you are not a member. Nothing the client can retry; a person has to
///   be added to the group.
/// * `409` — you asked for a method that is not a read. The client picked the
///   wrong call and should mint a warrant instead. A bug in the client, not a
///   permission problem, and telling the two apart is the difference between
///   "ask an admin" and "fix your code".
/// * anything else — an ordinary execution failure, mapped as elsewhere.
fn refusal_status(err: eyre::Report, method: &str) -> ApiError {
    match err.downcast_ref::<ExecuteError>() {
        Some(ExecuteError::NotAMember { .. }) => ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: "account is not a member of the group owning this context".to_owned(),
        },
        Some(ExecuteError::NotReadOnly { .. }) => ApiError {
            status_code: StatusCode::CONFLICT,
            message: format!(
                "method '{method}' is not declared read-only; a session authorizes reads \
                 only, so this call needs a warrant"
            ),
        },
        // By value, not `eyre!("{err}")`: formatting drops the type, and with it
        // every mapping `parse_api_error` has for the cause.
        _ => parse_api_error(err),
    }
}

/// This node's own signing identity in the context.
///
/// The execution's EXECUTOR: the replica running the call, which the sealing
/// context is derived from. Not the identity the method observes — that is the
/// session's account and device (`ReadAs`), exactly as a warranted write
/// observes the warrant's author while this node executes it. It used to be
/// both, which made `env::device_id()` answer with the relay for every account.
async fn local_signer(
    ctx_client: &ContextClient,
    context_id: &ContextId,
) -> eyre::Result<calimero_primitives::identity::PublicKey> {
    let members = ctx_client.get_context_members(context_id, Some(true));
    let mut members = std::pin::pin!(members);
    members
        .next()
        .await
        .transpose()?
        .map(|(key, _)| key)
        .ok_or_else(|| eyre::eyre!("this node owns no identity in this context"))
}

async fn query(
    ctx_client: &ContextClient,
    context_id: ContextId,
    read_as: ReadAs,
    req: &QueryContextApiRequest,
) -> eyre::Result<Option<serde_json::Value>> {
    // A context this node does not hold is a typed `404` here as on `/intents`;
    // past this point every failure would read as this node's own fault.
    super::perform_intent::held_context_group(ctx_client, &context_id)?;
    let executor = local_signer(ctx_client, &context_id).await?;
    let payload = serde_json::to_vec(&req.args_json)?;

    let response = ctx_client
        .query_as(&context_id, read_as, &executor, req.method.clone(), payload)
        .await?;

    read_output(response.returns)
}

/// The read's output, with the method's own error kept typed as JSON-RPC
/// `execute` keeps it (`crate::execute::method_output`). `None` is a method that
/// returns unit, which is a legitimate answer and not an error.
fn read_output(returns: eyre::Result<Option<Vec<u8>>>) -> eyre::Result<Option<serde_json::Value>> {
    crate::execute::method_output(returns).map_err(eyre::Report::new)
}

#[cfg(test)]
mod tests {
    use calimero_primitives::context::ContextId;

    use super::*;

    fn ctx() -> ContextId {
        ContextId::from([0x11; 32])
    }

    /// The two refusals must not collapse into one status.
    ///
    /// They are different instructions to whoever gets them: 403 means a person
    /// has to add you to the group, 409 means your code called the wrong method.
    /// Served as one status, a client cannot tell "ask an admin" from "fix your
    /// call", which is the same failure the intent refusals were typed to avoid.
    #[test]
    fn not_a_member_and_not_read_only_are_different_statuses() {
        let member = refusal_status(
            eyre::eyre!(ExecuteError::NotAMember { context_id: ctx() }),
            "get",
        );
        let read_only = refusal_status(
            eyre::eyre!(ExecuteError::NotReadOnly { context_id: ctx() }),
            "set",
        );

        assert_eq!(member.status_code, StatusCode::FORBIDDEN);
        assert_eq!(read_only.status_code, StatusCode::CONFLICT);
        assert_ne!(member.status_code, read_only.status_code);
    }

    /// A refused read is the caller's problem, never a 500.
    ///
    /// Both must be client errors: a 5xx tells a client the node is broken and
    /// invites a retry, and neither of these gets better on retry.
    #[test]
    fn both_refusals_are_client_errors() {
        for err in [
            ExecuteError::NotAMember { context_id: ctx() },
            ExecuteError::NotReadOnly { context_id: ctx() },
        ] {
            let mapped = refusal_status(eyre::eyre!(err), "m");
            assert!(
                mapped.status_code.is_client_error(),
                "{err:?} mapped to {}, which tells the client to retry a request \
                 that can never succeed",
                mapped.status_code
            );
        }
    }

    /// `ExecuteError` is `Copy` and so cannot carry the method name; the handler
    /// supplies it. If that ever stops happening the 409 becomes "some method
    /// was not read-only", which is the least actionable form of the one error a
    /// client is most likely to hit.
    #[test]
    fn the_read_only_refusal_names_the_method() {
        let mapped = refusal_status(
            eyre::eyre!(ExecuteError::NotReadOnly { context_id: ctx() }),
            "set_value",
        );
        assert!(
            mapped.message.contains("set_value"),
            "the refusal should name the method the caller asked for; got: {}",
            mapped.message
        );
    }

    /// The fall-through keeps the error's type. It used to go through
    /// `eyre!("{err}")`, which made every other typed refusal (an unknown
    /// context, a pending group key) a 500.
    #[test]
    fn a_typed_refusal_keeps_its_status_through_the_fall_through() {
        let mapped = refusal_status(eyre::eyre!(ExecuteError::ContextNotFound), "get");
        assert_eq!(mapped.status_code, StatusCode::NOT_FOUND);
    }

    /// A read whose method returned `Err` answers as JSON-RPC `execute` does:
    /// `FunctionCallError` with the method's message, as a `400`. It used to be
    /// an untyped report, so a read of a missing key was `500 Internal server
    /// error` with the app's reason nowhere but the node's log.
    #[tokio::test]
    async fn a_method_error_is_a_function_call_error() {
        let response = answer(
            ctx(),
            calimero_account::AccountId::from([0x22; 32]),
            "get_result",
            read_output(Err(eyre::eyre!("key not found"))),
        );
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(body["type"], "FunctionCallError");
        assert_eq!(body["data"], "key not found");
    }

    /// Anything that is not one of the two typed refusals falls through to the
    /// ordinary mapping rather than being reported as a read-specific problem.
    #[test]
    fn an_unrelated_failure_is_not_reported_as_a_read_refusal() {
        let mapped = refusal_status(eyre::eyre!("the datastore is on fire"), "get");
        assert_ne!(mapped.status_code, StatusCode::FORBIDDEN);
        assert_ne!(mapped.status_code, StatusCode::CONFLICT);
    }
    /// The same path as `/intents`: a session reading a context this node does
    /// not hold is told `404` naming the context, not that the node is broken.
    #[actix::test]
    async fn a_query_on_a_context_this_node_does_not_hold_is_a_404_not_a_500() {
        use axum::body::Body;
        use axum::http::Request;
        use axum::routing::post;
        use axum::{Extension, Router};
        use calimero_store::db::InMemoryDB;
        use calimero_store::Store;
        use tower::ServiceExt as _;

        let store = Store::new(std::sync::Arc::new(InMemoryDB::owned()));
        let (state, _blobs) = crate::test_support::admin_state(&store).await;
        let account = calimero_account::AccountGenesis::new(
            calimero_primitives::identity::PrivateKey::from([7u8; 32]).public_key(),
        )
        .account_id();
        let app = Router::new()
            .route("/contexts/{context_id}/query", post(super::handler))
            .layer(Extension(state))
            .layer(Extension(AuthenticatedAccount(account)));

        let context = ctx();
        let body = serde_json::json!({ "method": "get", "argsJson": {} });
        let response = app
            .oneshot(
                Request::post(format!("/contexts/{context}/query"))
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("a request"),
            )
            .await
            .expect("the query route answers");

        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read the response");
        let body = String::from_utf8_lossy(&body).into_owned();
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert!(
            body.contains(&context.to_string()) && body.contains("not held by this node"),
            "{body}"
        );
    }
}
