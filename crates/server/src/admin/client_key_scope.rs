use std::sync::Arc;

use axum::body::Body;
use axum::extract::rejection::RawPathParamsRejection;
use axum::extract::RawPathParams;
use axum::http::{Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use calimero_context_config::types::{ContextGroupId, SignedGroupOpenInvitation};
use calimero_governance_store::{placeholder_admin_identity, MetaRepository};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_store::Store;
use tracing::warn;

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::service::ApiError;
use crate::auth::ClientKeyScope;
use crate::AdminState;

const SYNC_ALL_REFUSAL: &str = "This key may not sync every context";
const CONTEXT_REFUSAL: &str = "This key is not permitted to act on this context";
const GROUP_REFUSAL: &str = "This key is not permitted to act on this group";
const APPLICATION_REFUSAL: &str = "This key is not permitted to act on this application";
const INSTALL_REFUSAL: &str = "This key may not install applications";
const INSTALL_ROUTE: &str = "/install-application";

pub(crate) async fn refuse_out_of_scope(
    params: Result<RawPathParams, RawPathParamsRejection>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let refused = request
        .extensions()
        .get::<ClientKeyScope>()
        .and_then(|scope| {
            let Some(state) = request.extensions().get::<Arc<AdminState>>() else {
                return Some(CONTEXT_REFUSAL);
            };
            // A route without parameters has none to check; an undecodable one is refused.
            let params: Vec<(&str, &str)> = match params.as_ref() {
                Ok(params) => params.iter().collect(),
                Err(RawPathParamsRejection::MissingPathParams(_)) => Vec::new(),
                Err(_) => return Some(CONTEXT_REFUSAL),
            };
            let store = state.ctx_client.datastore();
            let changes = !matches!(*request.method(), Method::GET | Method::HEAD);
            refusal(
                request.uri().path(),
                &params,
                |raw| {
                    raw.parse::<ContextId>()
                        .is_ok_and(|id| scope.permits_context(&state.ctx_client, &id))
                },
                |raw| {
                    parse_group_id(raw).is_ok_and(|id| {
                        if changes {
                            scope.permits_group_change(store, &id)
                        } else {
                            scope.permits_group(store, &id)
                        }
                    })
                },
                |raw| {
                    raw.parse::<ApplicationId>()
                        .is_ok_and(|id| scope.permits_application(store, &id))
                },
            )
            .inspect(|reason| {
                warn!(
                    path = %request.uri().path(),
                    bindings = ?scope.0,
                    "client key refused: {reason}"
                );
            })
        });

    match refused {
        Some(message) => ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: message.to_owned(),
        }
        .into_response(),
        None => next.run(request).await,
    }
}

/// Refuse a request whose body names a group or application outside a bound
/// key's binding. A bound key naming neither is refused: it would create outside any.
pub(crate) fn refuse_target_outside_binding(
    state: &AdminState,
    client_key: Option<&ClientKeyScope>,
    group_id: Option<&ContextGroupId>,
    application_id: Option<&ApplicationId>,
) -> Option<Response> {
    let scope = client_key?;
    let store = state.ctx_client.datastore();
    let (message, permitted) = match (group_id, application_id) {
        // A context binding creates only inside its context's own group.
        (None, _) if scope.0.context_id.is_some() => (GROUP_REFUSAL, false),
        (None, None) => (GROUP_REFUSAL, false),
        (group, application) => {
            if group.is_some_and(|g| !scope.permits_group_change(store, g)) {
                (GROUP_REFUSAL, false)
            } else {
                (
                    APPLICATION_REFUSAL,
                    application.is_none_or(|a| scope.permits_application(store, a)),
                )
            }
        }
    };
    if permitted {
        return None;
    }
    warn!(bindings = ?scope.0, ?group_id, ?application_id, "client key refused: {message}");
    Some(
        ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: message.to_owned(),
        }
        .into_response(),
    )
}

/// Refuse a bound key's join unless the binding may change the group, or the
/// group has no signed admin here yet and an invitation's unsigned hint names the bound application.
pub(crate) fn refuse_join_outside_binding(
    state: &AdminState,
    client_key: Option<&ClientKeyScope>,
    group_id: &ContextGroupId,
    invitation: Option<&SignedGroupOpenInvitation>,
) -> Option<Response> {
    let scope = client_key?;
    let store = state.ctx_client.datastore();
    if scope.permits_group_change(store, group_id)
        || invitation.is_some_and(|invitation| hints_bound_application(scope, store, invitation))
    {
        return None;
    }
    warn!(bindings = ?scope.0, ?group_id, "client key refused: join outside its binding");
    Some(
        ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: GROUP_REFUSAL.to_owned(),
        }
        .into_response(),
    )
}

fn hints_bound_application(
    scope: &ClientKeyScope,
    store: &Store,
    invitation: &SignedGroupOpenInvitation,
) -> bool {
    let unverified = MetaRepository::new(store)
        .load(&invitation.invitation.group_id)
        .is_ok_and(|meta| {
            meta.is_none_or(|meta| meta.admin_identity == placeholder_admin_identity())
        });
    let hint = invitation.application_id.map(ApplicationId::from);
    unverified
        && scope.0.context_id.is_none()
        && scope
            .0
            .application_id
            .as_deref()
            .zip(hint)
            .is_some_and(|(bound, hint)| bound.eq_ignore_ascii_case(&hint.to_string()))
}

fn refusal(
    path: &str,
    params: &[(&str, &str)],
    may_act_on_context: impl Fn(&str) -> bool,
    may_act_on_group: impl Fn(&str) -> bool,
    may_name_application: impl Fn(&str) -> bool,
) -> Option<&'static str> {
    if path.trim_end_matches('/').ends_with(INSTALL_ROUTE) {
        return Some(INSTALL_REFUSAL);
    }

    if let Some(target) = sync_target(path) {
        return match target {
            None => Some(SYNC_ALL_REFUSAL),
            Some(context_id) => (!may_act_on_context(context_id)).then_some(CONTEXT_REFUSAL),
        };
    }

    params.iter().find_map(|&(name, value)| match name {
        "context_id" => (!may_act_on_context(value)).then_some(CONTEXT_REFUSAL),
        "group_id" => (!may_act_on_group(value)).then_some(GROUP_REFUSAL),
        // A join is judged by its handler, which weighs the invitation's hint for a namespace not held yet.
        "namespace_id" if is_join(path) => None,
        "namespace_id" => (!may_act_on_group(value)).then_some(GROUP_REFUSAL),
        "application_id" => (!may_name_application(value)).then_some(APPLICATION_REFUSAL),
        _ => None,
    })
}

fn is_join(path: &str) -> bool {
    path.trim_end_matches('/').ends_with("/join")
}

fn sync_target(path: &str) -> Option<Option<&str>> {
    let path = path.trim_end_matches('/');
    if path.ends_with("/contexts/sync") {
        return Some(None);
    }
    let (_, rest) = path.rsplit_once("/contexts/sync/")?;
    (!rest.is_empty() && !rest.contains('/')).then_some(Some(rest))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use calimero_primitives::context::ContextId;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use mero_auth::auth::bindings::ClientKeyBindings;
    use tower::ServiceExt as _;

    use super::{
        refusal, sync_target, APPLICATION_REFUSAL, CONTEXT_REFUSAL, GROUP_REFUSAL, INSTALL_REFUSAL,
        SYNC_ALL_REFUSAL,
    };
    use crate::auth::ClientKeyScope;
    use crate::test_support::protected_router;

    const MINE: &str = "aa";
    const OTHER: &str = "bb";

    fn decide(path: &str, params: &[(&str, &str)]) -> Option<&'static str> {
        refusal(path, params, |c| c == MINE, |g| g == MINE, |a| a == MINE)
    }

    #[test]
    fn a_route_naming_another_context_is_refused() {
        for route in ["join", "leave", "resync", "query", "storage", "identities"] {
            let path = format!("/contexts/{OTHER}/{route}");
            assert_eq!(
                decide(&path, &[("context_id", OTHER)]),
                Some(CONTEXT_REFUSAL),
                "{path}"
            );
        }
        assert_eq!(
            decide(&format!("/contexts/{OTHER}"), &[("context_id", OTHER)]),
            Some(CONTEXT_REFUSAL)
        );
    }

    #[test]
    fn a_route_naming_the_bound_context_passes() {
        assert_eq!(
            decide(&format!("/contexts/{MINE}/leave"), &[("context_id", MINE)]),
            None
        );
        assert_eq!(decide(&format!("/contexts/sync/{MINE}"), &[]), None);
    }

    #[test]
    fn sync_is_checked_from_the_path() {
        assert_eq!(
            decide(&format!("/contexts/sync/{OTHER}"), &[]),
            Some(CONTEXT_REFUSAL)
        );
        assert_eq!(decide("/contexts/sync", &[]), Some(SYNC_ALL_REFUSAL));
        assert_eq!(decide("/contexts/sync/", &[]), Some(SYNC_ALL_REFUSAL));
        assert_eq!(
            decide("/node1/admin-api/contexts/sync", &[]),
            Some(SYNC_ALL_REFUSAL)
        );
    }

    #[test]
    fn group_and_namespace_routes_are_checked() {
        for name in ["group_id", "namespace_id"] {
            assert_eq!(
                decide("/groups/x/members", &[(name, OTHER)]),
                Some(GROUP_REFUSAL),
                "{name}"
            );
            assert_eq!(decide("/groups/x/members", &[(name, MINE)]), None, "{name}");
        }
    }

    #[test]
    fn a_namespace_join_is_left_to_its_handler() {
        assert_eq!(
            decide("/namespaces/x/join", &[("namespace_id", OTHER)]),
            None
        );
        assert_eq!(
            decide("/namespaces/x/leave", &[("namespace_id", OTHER)]),
            Some(GROUP_REFUSAL)
        );
    }

    #[test]
    fn routes_naming_nothing_pass() {
        assert_eq!(decide("/contexts", &[]), None);
        assert_eq!(decide("/peers", &[]), None);
    }

    #[test]
    fn a_route_naming_another_application_is_refused() {
        assert_eq!(
            decide("/applications/x", &[("application_id", OTHER)]),
            Some(APPLICATION_REFUSAL)
        );
        assert_eq!(decide("/applications/x", &[("application_id", MINE)]), None);
    }

    #[test]
    fn installing_is_refused() {
        assert_eq!(decide("/install-application", &[]), Some(INSTALL_REFUSAL));
        assert_eq!(
            decide("/node1/admin-api/install-application/", &[]),
            Some(INSTALL_REFUSAL)
        );
    }

    #[test]
    fn sync_target_reads_only_the_sync_routes() {
        assert_eq!(sync_target("/contexts/sync"), Some(None));
        assert_eq!(sync_target("/contexts/sync/abc"), Some(Some("abc")));
        assert_eq!(sync_target("/contexts/abc/resync"), None);
        assert_eq!(sync_target("/contexts/sync/abc/extra"), None);
        assert_eq!(sync_target("/groups/abc/sync"), None);
    }

    async fn get_as_bound_key(
        router: axum::Router,
        bound: u8,
        context: u8,
    ) -> (StatusCode, String) {
        let scope = ClientKeyScope(ClientKeyBindings::from_permissions(&[format!(
            "context[{},identity]",
            ContextId::from([bound; 32])
        )]));
        let mut request = Request::get(format!("/contexts/{}", ContextId::from([context; 32])))
            .body(Body::empty())
            .expect("a request");
        drop(request.extensions_mut().insert(scope));

        let response = router.oneshot(request).await.expect("the route answers");
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read the response");
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[actix::test]
    async fn the_admin_router_refuses_a_bound_key_naming_another_context() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (router, _blobs) = protected_router(&store).await;

        let (status, body) = get_as_bound_key(router.clone(), 0xA1, 0xB2).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains(CONTEXT_REFUSAL), "{body}");

        let (status, body) = get_as_bound_key(router, 0xA1, 0xA1).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "the bound context reaches its handler: {body}"
        );
    }
}

#[cfg(test)]
mod route_tests;
