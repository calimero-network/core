//! Access grants: let a third party call a few named methods in one context, as
//! this node, with nothing but a token.
//!
//! - `POST   /admin-api/contexts/:context_id/grants` — issue one (protected)
//! - `GET    /admin-api/contexts/:context_id/grants` — list them (protected)
//! - `DELETE /admin-api/contexts/:context_id/grants/:grant_id` — revoke (protected)
//! - `POST   /admin-api/contexts/:context_id/grant-calls` — call under one (public)
//!
//! # What a grant is, and what it is not
//!
//! A grant is this node's consent to run a call for whoever holds the token. The
//! call runs exactly as `/jsonrpc` `execute` runs one for the node owner: as this
//! node's own identity in the context, so what it writes is this node's write and
//! peers see an ordinary self-authored delta. Nothing about a grant reaches the
//! wire, and no peer is asked to honour one, which is why it needs no signature:
//! the only party it binds is the node that issued it and keeps the row.
//!
//! That is also its limit. It is not delegated authorship: the holder is not an
//! author, no peer can tell a granted call from the owner's own, and the
//! application sees this node's account as the caller. An application that needs
//! to tell holders apart takes something from the holder in its arguments; a
//! grant decides only *which methods* may be reached.
//!
//! # Why the call route is public on every node
//!
//! The holder is a third party with no relationship to this node and so no
//! credential it could present to the guard: the token is the credential. A node
//! that issued no grant refuses every call after one hash and one point read, so
//! serving the route grants nothing until its operator issues a grant, which is
//! the opt-in. Rate limiting it belongs to the deployment's reverse proxy, as for
//! the other public routes.
//!
//! # Why a token, not a signature
//!
//! The holder is typically a browser that opened a link. A token in the link's
//! fragment is all it has, and a key pair would have to travel the same way. The
//! row is keyed by a domain-separated hash of the token, so this node stores no
//! secret, a presented token is looked up rather than compared, and the token
//! cannot be shown again after it is issued.
//!
//! # Who may issue one
//!
//! A grant can only narrow what the issuer already holds: every method it names
//! must be one the issuer's own token may execute in this context
//! (`context:execute[<context>,,<method>]`), the check `/jsonrpc` makes for each
//! call. An account-anchored session is refused outright, for the reason
//! `/jsonrpc` refuses it: a granted call runs as this node, which an account
//! session never does.

use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::domain_hash;
use calimero_server_primitives::admin::{
    AccessGrantApiData, AccessGrantCallApiRequest, AccessGrantCallApiResponse,
    AccessGrantCallApiResponseData, CreateAccessGrantApiRequest, CreateAccessGrantApiResponse,
    CreatedAccessGrantApiData, ListAccessGrantsApiResponse, RevokeAccessGrantApiResponse,
    RevokeAccessGrantApiResponseData,
};
use calimero_server_primitives::jsonrpc::ExecutionRequest;
use calimero_store::{key, types};
use rand::RngExt;
use tracing::{debug, info, warn};

use super::perform_intent::now_secs;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{method_error_response, parse_api_error, ApiError, ApiResponse};
use crate::auth::{AuthenticatedAccount, GrantedPermissions};
use crate::execute::{execute_request, may_execute, CallerIdentity};
use crate::AdminState;

/// The domain a grant id is hashed under, so a token can never be confused with
/// any other hashed value this node stores.
const ACCESS_GRANT_ID_DOMAIN: &[u8] = b"calimero.access-grant.id.v1";

/// The id a token is stored under.
fn grant_id_of(token: &[u8; 32]) -> [u8; 32] {
    domain_hash(ACCESS_GRANT_ID_DOMAIN, &[token])
}

/// The permissions a grant's calls run under: one `context:execute` per method,
/// for this context alone. `execute_request` checks the call against them, the
/// same check a node token's call meets.
fn granted_permissions(context_id: &ContextId, methods: &[String]) -> GrantedPermissions {
    GrantedPermissions(
        methods
            .iter()
            .map(|method| format!("context:execute[{context_id},,{method}]"))
            .collect(),
    )
}

fn parse_context(context_id: &str) -> Result<ContextId, ApiError> {
    context_id.parse().map_err(|err| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("context id '{context_id}' is not valid: {err}"),
    })
}

fn parse_hex_32(value: &str, what: &str) -> Result<[u8; 32], ApiError> {
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(value, &mut bytes).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("{what} must be 64 hex characters"),
    })?;
    Ok(bytes)
}

fn api_data(grant_id: [u8; 32], grant: types::ContextAccessGrant) -> AccessGrantApiData {
    AccessGrantApiData {
        grant_id: hex::encode(grant_id),
        methods: grant.methods,
        label: grant.label,
        created_at: grant.created_at,
        not_after: grant.not_after,
    }
}

/// Whether the caller's token could have issued a grant naming `methods`: it may
/// call every one of them here itself.
///
/// The one rule for all three management routes, so a token sees and revokes
/// exactly the grants it could have issued. The route check alone admits any
/// `context:execute` for the context, method-scoped ones included, and without
/// this a token minted for one method could list or revoke every grant.
///
/// With no permissions handed over, no guard ran on this node (proxy mode
/// without `X-Auth-Permissions`), and the proxy is what decided this caller may
/// be here, as on every other route.
fn may_grant(granted: Option<&GrantedPermissions>, context_id: &ContextId, methods: &[String]) -> bool {
    granted.is_none_or(|granted| {
        methods
            .iter()
            .all(|method| may_execute(true, Some(granted), context_id, method))
    })
}

/// The refusal for an account-anchored session, on every management route.
fn account_session_refusal() -> ApiError {
    ApiError {
        status_code: StatusCode::FORBIDDEN,
        message: "an access grant runs calls as this node, which an account session may not \
                  delegate; use a node token"
            .to_owned(),
    }
}

/// `POST /admin-api/contexts/:context_id/grants`
pub async fn create_handler(
    Path(context_id): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    account: Option<Extension<AuthenticatedAccount>>,
    granted: Option<Extension<GrantedPermissions>>,
    ValidatedJson(req): ValidatedJson<CreateAccessGrantApiRequest>,
) -> impl IntoResponse {
    let context_id = match parse_context(&context_id) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    if account.is_some() {
        return account_session_refusal().into_response();
    }

    let granted = granted.map(|Extension(granted)| granted);
    if let Some(method) = req
        .methods
        .iter()
        .find(|method| !may_grant(granted.as_ref(), &context_id, std::slice::from_ref(method)))
    {
        return ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: format!(
                "this token may not call '{method}' in this context, so it cannot grant it"
            ),
        }
        .into_response();
    }

    let now = now_secs();
    if req.not_after.is_some_and(|not_after| not_after <= now) {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "notAfter is already in the past".to_owned(),
        }
        .into_response();
    }

    if let Err(err) = super::perform_intent::held_context_group(&state.ctx_client, &context_id) {
        return parse_api_error(err).into_response();
    }

    let mut methods = req.methods;
    methods.sort();
    methods.dedup();

    let token: [u8; 32] = rand::rng().random();
    let grant_id = grant_id_of(&token);
    let grant = types::ContextAccessGrant::new(methods, req.label, now, req.not_after);

    if let Err(err) = state
        .store
        .handle()
        .put(&key::ContextAccessGrant::new(context_id, grant_id), &grant)
    {
        return parse_api_error(err.into()).into_response();
    }
    info!(%context_id, grant_id = %hex::encode(grant_id), methods = ?grant.methods, "issued an access grant");

    ApiResponse {
        payload: CreateAccessGrantApiResponse {
            data: CreatedAccessGrantApiData {
                grant: api_data(grant_id, grant),
                token: hex::encode(token),
            },
        },
    }
    .into_response()
}

/// The grants this node holds for `context_id`, by id.
fn grants_of(
    state: &AdminState,
    context_id: ContextId,
) -> eyre::Result<Vec<([u8; 32], types::ContextAccessGrant)>> {
    let handle = state.store.handle();
    let mut iter = handle.iter::<key::ContextAccessGrant>()?;
    let mut next = iter.seek(key::ContextAccessGrant::new(context_id, [0; 32]))?;
    let mut grants = Vec::new();
    while let Some(row) = next {
        if row.context_id() != context_id {
            break;
        }
        if let Some(grant) = handle.get(&row)? {
            grants.push((row.grant_id(), grant));
        }
        next = iter.next()?;
    }
    Ok(grants)
}

/// `GET /admin-api/contexts/:context_id/grants`
pub async fn list_handler(
    Path(context_id): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    account: Option<Extension<AuthenticatedAccount>>,
    granted: Option<Extension<GrantedPermissions>>,
) -> impl IntoResponse {
    let context_id = match parse_context(&context_id) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    if account.is_some() {
        return account_session_refusal().into_response();
    }
    match grants_of(&state, context_id) {
        Ok(grants) => ApiResponse {
            payload: ListAccessGrantsApiResponse {
                data: grants
                    .into_iter()
                    .filter(|(_, grant)| {
                        may_grant(
                            granted.as_ref().map(|Extension(granted)| granted),
                            &context_id,
                            &grant.methods,
                        )
                    })
                    .map(|(id, grant)| api_data(id, grant))
                    .collect(),
            },
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}

/// `DELETE /admin-api/contexts/:context_id/grants/:grant_id`
pub async fn revoke_handler(
    Path((context_id, grant_id)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
    account: Option<Extension<AuthenticatedAccount>>,
    granted: Option<Extension<GrantedPermissions>>,
) -> impl IntoResponse {
    let context_id = match parse_context(&context_id) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    if account.is_some() {
        return account_session_refusal().into_response();
    }
    let grant_id = match parse_hex_32(&grant_id, "grant id") {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let row = key::ContextAccessGrant::new(context_id, grant_id);
    let mut handle = state.store.handle();
    // A grant this token could not have issued is answered as absent, as the
    // listing hides it: a 403 would confirm the id names a grant.
    match handle.get(&row) {
        Ok(Some(grant))
            if may_grant(
                granted.as_ref().map(|Extension(granted)| granted),
                &context_id,
                &grant.methods,
            ) => {}
        Ok(_) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "no such access grant in this context".to_owned(),
            }
            .into_response()
        }
        Err(err) => return parse_api_error(err.into()).into_response(),
    }
    if let Err(err) = handle.delete(&row) {
        return parse_api_error(err.into()).into_response();
    }
    info!(%context_id, grant_id = %hex::encode(grant_id), "revoked an access grant");

    ApiResponse {
        payload: RevokeAccessGrantApiResponse {
            data: RevokeAccessGrantApiResponseData {
                grant_id: hex::encode(grant_id),
            },
        },
    }
    .into_response()
}

/// A refusal of a granted call. An unknown token and a revoked one get the same
/// message: the row is gone either way, and telling them apart would tell a
/// guesser which tokens once existed.
fn refuse(status_code: StatusCode, message: &str) -> axum::response::Response {
    ApiError {
        status_code,
        message: message.to_owned(),
    }
    .into_response()
}

/// `POST /admin-api/contexts/:context_id/grant-calls`
pub async fn call_handler(
    Path(context_id): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<AccessGrantCallApiRequest>,
) -> impl IntoResponse {
    let context_id = match parse_context(&context_id) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    let token = match parse_hex_32(&req.token, "token") {
        Ok(token) => token,
        Err(err) => return err.into_response(),
    };
    let grant_id = grant_id_of(&token);

    let grant = match state
        .store
        .handle()
        .get(&key::ContextAccessGrant::new(context_id, grant_id))
    {
        Ok(Some(grant)) => grant,
        Ok(None) => {
            debug!(%context_id, "refusing a call under an unknown access grant");
            return refuse(
                StatusCode::FORBIDDEN,
                "this access grant is not valid for this context",
            );
        }
        Err(err) => return parse_api_error(err.into()).into_response(),
    };

    // One clock, on the one node the grant binds: nothing has converged yet,
    // so a wall-clock bound means the same thing to everyone it can affect.
    if grant.not_after.is_some_and(|not_after| now_secs() > not_after) {
        return refuse(StatusCode::FORBIDDEN, "this access grant has expired");
    }

    if !grant.methods.iter().any(|method| *method == req.method) {
        warn!(%context_id, grant_id = %hex::encode(grant_id), method = %req.method, "refusing a method the access grant does not name");
        return refuse(
            StatusCode::FORBIDDEN,
            "this access grant does not allow this method",
        );
    }

    let permissions = granted_permissions(&context_id, &grant.methods);
    match execute_request(
        &state.ctx_client,
        CallerIdentity::NodeOwner,
        true,
        Some(&permissions),
        ExecutionRequest::new(context_id, req.method, req.args_json),
    )
    .await
    {
        Ok(response) => ApiResponse {
            payload: AccessGrantCallApiResponse {
                data: AccessGrantCallApiResponseData {
                    returns: response.output,
                },
            },
        }
        .into_response(),
        Err(err) => method_error_response(&err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A token sees and revokes only the grants it could have issued.
    #[test]
    fn a_method_scoped_token_may_grant_only_its_own_methods() {
        let ctx = ContextId::from([1u8; 32]);
        let scoped = GrantedPermissions(Arc::from([format!("context:execute[{ctx},,book]")]));
        let wide = GrantedPermissions(Arc::from([format!("context:execute[{ctx}]")]));
        let both = ["book".to_owned(), "cancel".to_owned()];

        assert!(may_grant(Some(&scoped), &ctx, &both[..1]));
        assert!(!may_grant(Some(&scoped), &ctx, &both));
        assert!(may_grant(Some(&wide), &ctx, &both));
        assert!(!may_grant(Some(&wide), &ContextId::from([2u8; 32]), &both));
        // No guard ran: the proxy decided.
        assert!(may_grant(None, &ctx, &both));
    }

    #[test]
    fn a_grant_id_is_not_the_bare_hash_of_its_token() {
        use sha2::{Digest, Sha256};

        let token = [7u8; 32];
        let bare: [u8; 32] = Sha256::digest(token).into();
        assert_ne!(grant_id_of(&token), bare);
        assert_ne!(grant_id_of(&token), grant_id_of(&[8u8; 32]));
    }

    /// A grant's calls meet the same per-method check a node token's call does,
    /// so a method the grant does not name is refused even past the handler.
    #[test]
    fn granted_permissions_cover_exactly_the_named_methods_in_one_context() {
        let ctx = ContextId::from([1u8; 32]);
        let other = ContextId::from([2u8; 32]);
        let permissions = granted_permissions(&ctx, &["book".to_owned(), "free".to_owned()]);

        assert!(may_execute(true, Some(&permissions), &ctx, "book"));
        assert!(may_execute(true, Some(&permissions), &ctx, "free"));
        assert!(!may_execute(true, Some(&permissions), &ctx, "delete_event"));
        assert!(!may_execute(true, Some(&permissions), &other, "book"));
    }
}
