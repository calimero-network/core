//! The `Deprecation` header (RFC 9745) on the answers of every route
//! `api-deprecations.toml` deprecates, with a `Link` to the policy (`rel="deprecation"`).
//! `build.rs` turns the file's `[[deprecated]]` routes into [`DEPRECATED`];
//! `scripts/check-api-compat.py` keeps each route served until its `remove_after` release.

use axum::extract::{MatchedPath, Request};
use axum::http::header::LINK;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

/// `(method, route template, when it was deprecated in Unix seconds)`.
type Deprecated = (&'static str, &'static str, i64);

/// The deprecated routes, from `api-deprecations.toml`.
const DEPRECATED: &[Deprecated] = include!(concat!(env!("OUT_DIR"), "/deprecated_routes.rs"));

/// The policy, which also says what replaces a deprecated route.
const POLICY_LINK: &str =
    "<https://calimero-network.github.io/core/operate/compatibility/>; rel=\"deprecation\"";

static DEPRECATION: HeaderName = HeaderName::from_static("deprecation");

/// When `method route` was deprecated, if it was.
fn deprecated_at(list: &[Deprecated], method: &str, route: &str) -> Option<i64> {
    list.iter()
        .find(|(m, r, _)| *m == method && *r == route)
        .map(|(_, _, at)| *at)
}

/// The layer: a `route_layer`, so it sees the template the request matched.
pub(crate) async fn mark(route: MatchedPath, request: Request, next: Next) -> Response {
    mark_from(DEPRECATED, &route, request, next).await
}

async fn mark_from(
    list: &[Deprecated],
    route: &MatchedPath,
    request: Request,
    next: Next,
) -> Response {
    let at = deprecated_at(list, request.method().as_str(), route.as_str());
    let mut response = next.run(request).await;
    if let Some(at) = at {
        let headers = response.headers_mut();
        if let Ok(value) = HeaderValue::from_str(&format!("@{at}")) {
            let _prev = headers.insert(DEPRECATION.clone(), value);
        }
        headers.append(LINK, HeaderValue::from_static(POLICY_LINK));
    }
    response
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::routing::{get, post};
    use axum::Router;
    use tower::ServiceExt as _;

    use super::*;

    /// 2026-11-12, as `build.rs` turns `date = 2026-11-12` into seconds.
    const AT: i64 = 1_794_441_600;
    const LIST: &[Deprecated] = &[("POST", "/old/{id}", AT)];

    #[test]
    fn a_route_is_deprecated_by_its_method_and_template() {
        assert_eq!(deprecated_at(LIST, "POST", "/old/{id}"), Some(AT));
        assert_eq!(deprecated_at(LIST, "GET", "/old/{id}"), None);
        assert_eq!(
            deprecated_at(LIST, "POST", "/old/7"),
            None,
            "the template, not the path"
        );
    }

    #[tokio::test]
    async fn a_deprecated_routes_answers_carry_the_header_and_others_do_not() {
        let app = Router::new()
            .route("/old/{id}", post(|| async { "ok" }).get(|| async { "ok" }))
            .route("/new", get(|| async { "ok" }))
            .route_layer(axum::middleware::from_fn(
                |route: MatchedPath, request: Request, next: Next| async move {
                    mark_from(LIST, &route, request, next).await
                },
            ));
        let ask = |method: &str, uri: &str| {
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap()
        };
        let old = app.clone().oneshot(ask("POST", "/old/7")).await.unwrap();
        assert_eq!(old.status(), 200);
        assert_eq!(old.headers()["deprecation"], format!("@{AT}"));
        assert!(old.headers()[LINK]
            .to_str()
            .unwrap()
            .contains("rel=\"deprecation\""));
        for (method, uri) in [("GET", "/old/7"), ("GET", "/new")] {
            let answer = app.clone().oneshot(ask(method, uri)).await.unwrap();
            assert!(
                !answer.headers().contains_key("deprecation"),
                "{method} {uri}"
            );
        }
    }
}
