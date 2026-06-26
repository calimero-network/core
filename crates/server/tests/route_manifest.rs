//! Route-manifest coverage guard (method-aware).
//!
//! `endpoints.json` is the committed list of admin HTTP routes as `METHOD /path`
//! entries. This test re-extracts them from `admin/service.rs` and asserts they
//! match — so a new `.route(...)` (or a new method on an existing path) reddens
//! here until it's added to the manifest (and, by convention, exercised by an SDK
//! e2e test). Method-aware so a broken verb can't hide behind another verb on the
//! same path (e.g. GET vs DELETE /blobs/:id). Regenerate after an intended change:
//!   UPDATE_MANIFEST=1 cargo test -p calimero-server --test route_manifest
//!
//! Inline `.nest("/prefix", Router::new()...)` mounts are resolved to their real
//! paths. External sub-services mounted by reference (`.nest("/tee", tee::service())`,
//! alias) define their routes in other modules and are out of scope here — a
//! follow-up, like the jsonrpc/ws/sse mounts.

use std::collections::BTreeSet;

const ADMIN_PREFIX: &str = "/admin-api";
const VERBS: [&str; 5] = ["get", "post", "put", "delete", "patch"];

/// Find the balanced region inside the `(...)` that begins at `open` (the index of
/// the byte just after the `(`). Returns `(inner, end)` where `inner` is the text
/// between the parens and `end` is the index just past the closing `)`. String
/// literals are skipped so parens/quotes inside them don't unbalance the scan.
fn balanced(src: &str, open: usize) -> (&str, usize) {
    let bytes = src.as_bytes();
    let mut depth = 1usize;
    let mut j = open;
    let mut in_str = false;
    while j < bytes.len() && depth > 0 {
        match bytes[j] {
            b'\\' if in_str => j += 1,
            b'"' => in_str = !in_str,
            b'(' if !in_str => depth += 1,
            b')' if !in_str => depth -= 1,
            _ => {}
        }
        j += 1;
    }
    (&src[open..j.saturating_sub(1)], j)
}

/// First string literal in `s` (its content + the index just past its close quote).
/// Honors `\"` escapes so a quote inside the literal doesn't end it early.
fn first_string(s: &str) -> Option<(&str, usize)> {
    let bytes = s.as_bytes();
    let q1 = s.find('"')?;
    let mut i = q1 + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2, // skip the escaped byte
            b'"' => return Some((&s[q1 + 1..i], i + 1)),
            _ => i += 1,
        }
    }
    None
}

/// Join a nest `prefix` with a route `path`. Returns `None` for the top-level
/// bare-root / catch-all literals (`/`, `/*path`) — those are the static-file /
/// SPA handlers, not gated API. A nested `"/"` collapses to the nest root.
fn join_path(prefix: &str, path: &str) -> Option<String> {
    if prefix.is_empty() {
        if path == "/" || path.starts_with("/*") {
            return None;
        }
        return Some(path.to_string());
    }
    if path == "/" {
        return Some(prefix.to_string());
    }
    Some(format!("{prefix}{path}"))
}

/// Recursively collect `METHOD /admin-api/<path>` from a router-builder region,
/// accumulating `.nest("<prefix>", Router::new()...)` prefixes so nested routes
/// resolve to their real paths (e.g. `/contexts/sync/:context_id`). Paren-balanced
/// + string-aware, so multi-line calls and trailing code don't confuse it.
fn collect(region: &str, prefix: &str, out: &mut BTreeSet<String>) {
    let mut cursor = 0;
    loop {
        let route_at = region[cursor..].find(".route(").map(|i| cursor + i);
        let nest_at = region[cursor..].find(".nest(").map(|i| cursor + i);
        let (is_nest, at, tok_len) = match (route_at, nest_at) {
            (Some(r), Some(n)) if n < r => (true, n, ".nest(".len()),
            (Some(r), _) => (false, r, ".route(".len()),
            (None, Some(n)) => (true, n, ".nest(".len()),
            (None, None) => break,
        };
        let (inner, end) = balanced(region, at + tok_len);
        cursor = end;

        let Some((path, after_idx)) = first_string(inner) else {
            continue;
        };

        if is_nest {
            // Recurse into the nested router with the accumulated prefix. A nest
            // whose body is an external `service()` call has no inline `.route(`
            // and simply contributes nothing here.
            collect(&inner[after_idx..], &format!("{prefix}{path}"), out);
            continue;
        }

        let Some(full) = join_path(prefix, path) else {
            continue;
        };
        // Methods = verb tokens (`get(`, `post(`, …) where the verb isn't part of
        // a longer identifier (so `get_foo(` doesn't count).
        let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        for verb in VERBS {
            let token = format!("{verb}(");
            let mut from = 0;
            while let Some(p) = inner[from..].find(&token) {
                let abs = from + p;
                // A token at the very start, or one preceded by a non-identifier
                // byte, is a real verb call (not the tail of `get_foo(`).
                let boundary = abs == 0 || !is_ident(inner.as_bytes()[abs - 1]);
                if boundary {
                    out.insert(format!("{} {ADMIN_PREFIX}{full}", verb.to_uppercase()));
                }
                from = abs + token.len();
            }
        }
    }
}

fn extract_routes(src: &str) -> BTreeSet<String> {
    let mut routes = BTreeSet::new();
    collect(src, "", &mut routes);
    routes
}

#[test]
fn route_manifest_matches_source() {
    let src = include_str!("../src/admin/service.rs");
    let actual = extract_routes(src);

    if std::env::var_os("UPDATE_MANIFEST").is_some() {
        let list: Vec<&String> = actual.iter().collect();
        let json = serde_json::to_string_pretty(&list).expect("serialize manifest");
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/endpoints.json"),
            format!("{json}\n"),
        )
        .expect("write endpoints.json");
        return;
    }

    let manifest_raw = include_str!("../endpoints.json");
    let expected: BTreeSet<String> = serde_json::from_str::<Vec<String>>(manifest_raw)
        .expect("parse endpoints.json")
        .into_iter()
        .collect();

    let added: Vec<&String> = actual.difference(&expected).collect();
    let removed: Vec<&String> = expected.difference(&actual).collect();

    assert!(
        added.is_empty() && removed.is_empty(),
        "route manifest drift vs admin/service.rs:\n  \
         new in source (add to endpoints.json + cover with an SDK e2e test): {added:?}\n  \
         in manifest but gone from source: {removed:?}\n  \
         regenerate: UPDATE_MANIFEST=1 cargo test -p calimero-server --test route_manifest",
    );
}
