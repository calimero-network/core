//! Browser `WebSocket` and `EventSource` cannot set headers, so only the
//! streaming routes take `?token=`; a URL token elsewhere ends up in logs.

/// Whether `path` (without a node prefix) is `/ws`, `/sse` or `/sse/...`. Dot segments
/// and escapes are refused: a proxy that normalizes them routes elsewhere.
#[must_use]
pub fn is_query_token_route(path: &str) -> bool {
    if path
        .split('/')
        .any(|segment| segment == "." || segment == ".." || segment.contains('%'))
    {
        return false;
    }
    let path = path.trim_end_matches('/');
    path == "/ws" || path == "/sse" || path.starts_with("/sse/")
}

/// The `token` parameter of a query string, matched by exact name.
#[must_use]
pub fn query_token(query: &str) -> Option<&str> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "token").then_some(value)
    })
}

#[cfg(test)]
mod tests {
    use super::{is_query_token_route, query_token};

    #[test]
    fn streaming_routes_may_carry_the_token_in_the_url() {
        for path in [
            "/ws",
            "/ws/",
            "/sse",
            "/sse/",
            "/sse/subscription",
            "/sse/session/abc",
        ] {
            assert!(is_query_token_route(path), "{path}");
        }
    }

    #[test]
    fn nothing_else_may() {
        for path in [
            "/admin-api/blobs/ab12",
            "/admin-api/contexts",
            "/jsonrpc",
            "/admin-api/sse",
            "/wss",
            "/sse-evil",
            "/admin-dashboard/",
            "/sse/../admin-api/contexts",
            "/sse/session/../../admin-api/contexts",
            "/sse/./x",
            "/sse/%2e%2e/admin-api/contexts",
            "/sse/x%2f..%2fadmin-api",
        ] {
            assert!(!is_query_token_route(path), "{path}");
        }
    }

    #[test]
    fn only_a_parameter_named_token_is_read() {
        assert_eq!(query_token("a=1&token=t&b=2"), Some("t"));
        assert_eq!(query_token("xtoken=t&tokens=u"), None);
        assert_eq!(query_token("token"), None);
    }
}
