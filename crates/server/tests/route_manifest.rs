//! Route-manifest coverage guard.
//!
//! `endpoints.json` is the committed list of admin HTTP routes. This test
//! re-extracts the routes from `admin/service.rs` and asserts they match — so a
//! new `.route(...)` reddens here until it's added to the manifest (and, by
//! convention, exercised by an SDK e2e test). Regenerate after an intended change:
//!   UPDATE_MANIFEST=1 cargo test -p calimero-server --test route_manifest
//!
//! Phase 1 covers the top-level admin service routes (the bulk + where SDK drift
//! matters most). Nested sub-services (alias/tee) and the jsonrpc/ws/sse mounts
//! are a mechanical follow-up.

use std::collections::BTreeSet;

const ADMIN_PREFIX: &str = "/admin-api";

/// Extract the path literal of every `.route("<path>", ...)` in the admin service
/// source. Pure string scan (no regex dep); spans multi-line `.route(` calls.
fn extract_routes(src: &str) -> BTreeSet<String> {
    let mut routes = BTreeSet::new();
    for chunk in src.split(".route(").skip(1) {
        let trimmed = chunk.trim_start();
        if let Some(rest) = trimmed.strip_prefix('"') {
            if let Some(end) = rest.find('"') {
                routes.insert(format!("{ADMIN_PREFIX}{}", &rest[..end]));
            }
        }
    }
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
