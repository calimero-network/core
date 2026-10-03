use std::sync::Arc;

use axum::body::Body;
use axum::extract::rejection::RawPathParamsRejection;
use axum::extract::RawPathParams;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use calimero_primitives::context::ContextId;
use tracing::warn;

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::service::ApiError;
use crate::auth::ClientKeyScope;
use crate::AdminState;

const SYNC_ALL_REFUSAL: &str = "This key may not sync every context";
const CONTEXT_REFUSAL: &str = "This key is not permitted to act on this context";
const GROUP_REFUSAL: &str = "This key is not permitted to act on this group";

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
            let params: Vec<(&str, &str)> = params
                .as_ref()
                .map(|p| p.iter().collect())
                .unwrap_or_default();
            refusal(
                request.uri().path(),
                &params,
                |raw| {
                    raw.parse::<ContextId>()
                        .is_ok_and(|id| scope.permits_context(&state.ctx_client, &id))
                },
                |raw| {
                    parse_group_id(raw)
                        .is_ok_and(|id| scope.permits_group(state.ctx_client.datastore(), &id))
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

fn refusal(
    path: &str,
    params: &[(&str, &str)],
    may_act_on_context: impl Fn(&str) -> bool,
    may_act_on_group: impl Fn(&str) -> bool,
) -> Option<&'static str> {
    if let Some(target) = sync_target(path) {
        return match target {
            None => Some(SYNC_ALL_REFUSAL),
            Some(context_id) => (!may_act_on_context(context_id)).then_some(CONTEXT_REFUSAL),
        };
    }

    params.iter().find_map(|&(name, value)| match name {
        "context_id" => (!may_act_on_context(value)).then_some(CONTEXT_REFUSAL),
        "group_id" | "namespace_id" => (!may_act_on_group(value)).then_some(GROUP_REFUSAL),
        _ => None,
    })
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
    use super::{refusal, sync_target, CONTEXT_REFUSAL, GROUP_REFUSAL, SYNC_ALL_REFUSAL};

    const MINE: &str = "aa";
    const OTHER: &str = "bb";

    fn decide(path: &str, params: &[(&str, &str)]) -> Option<&'static str> {
        refusal(path, params, |c| c == MINE, |g| g == MINE)
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
    fn routes_naming_nothing_pass() {
        assert_eq!(decide("/contexts", &[]), None);
        assert_eq!(
            decide("/applications/x", &[("application_id", OTHER)]),
            None
        );
        assert_eq!(decide("/peers", &[]), None);
    }

    #[test]
    fn sync_target_reads_only_the_sync_routes() {
        assert_eq!(sync_target("/contexts/sync"), Some(None));
        assert_eq!(sync_target("/contexts/sync/abc"), Some(Some("abc")));
        assert_eq!(sync_target("/contexts/abc/resync"), None);
        assert_eq!(sync_target("/contexts/sync/abc/extra"), None);
        assert_eq!(sync_target("/groups/abc/sync"), None);
    }
}
