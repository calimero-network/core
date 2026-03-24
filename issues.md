# Core PR Review Triage

Scope: only comments from `../comments.md` that apply to the 5 files changed in this PR:

- `crates/client/src/connection.rs`
- `crates/meroctl/src/auth.rs`
- `crates/meroctl/src/cli.rs`
- `crates/meroctl/src/config.rs`
- `crates/node/primitives/src/client/application.rs`

I ignored outdated comments tied to older revisions of the same files.

## Fixed In This Session

- [x] `crates/meroctl/src/cli.rs`: removed the unused `node_path` variable in `prepare_connection`.
- [x] `crates/client/src/connection.rs`: renamed `load_auth_header` to `ensure_auth_header` so the browser-auth side effect is clearer at the call site.
- [x] `crates/client/src/connection.rs`: added a unit test for a plain-text error body (`"Service unavailable"` -> `HTTP 503`).
- [x] `crates/client/src/connection.rs`: `refresh_token()` now propagates token-storage read errors instead of silently converting them into `No tokens available to refresh`.
- [x] `crates/client/src/connection.rs`: the 401 retry loop now fails fast when auth is required but `node_name` is missing, instead of triggering futile retry/auth cycles.
- [x] `crates/node/primitives/src/client/application.rs`: replaced the hard-coded `256` truncation length with `MAX_ERROR_BODY_LEN`.

## Already Resolved On This Branch

- [x] `crates/meroctl/src/config.rs`: config writes use owner-only permissions (`0600`) on Unix from the start.
- [x] `crates/client/src/connection.rs`: auth-mode detection no longer treats `404` as `AuthMode::None`; only `2xx` does.
- [x] `crates/client/src/connection.rs`: the localhost bypass removal is documented, so the behavior change is explicit.
- [x] `crates/client/src/connection.rs`: `extract_error_message` truncates extracted strings to 300 chars and appends an ellipsis.
- [x] `crates/client/src/connection.rs`: `extract_error_message` now has unit tests for the main structured JSON branches and truncation behavior.
- [x] `crates/client/src/connection.rs`: `unwrap()` after a `None` guard was already replaced with `let Some(node_name) = ... else`.
- [x] `crates/meroctl/src/auth.rs`: the config-persistence side effect is now documented in `authenticate_with_session_cache` / `persist_node_in_config`.
- [x] `crates/node/primitives/src/client/application.rs`: registry error bodies are truncated before being included in the error.

## Worth Fixing Later

- [ ] `crates/meroctl/src/auth.rs`: tests still simulate `persist_node_in_config` behavior instead of calling the real async function against a temp config path.
- [ ] `crates/node/primitives/src/client/application.rs`: error handling still reads the full response body into memory before truncating it; bounded reads or streaming would be safer.
- [ ] `crates/client/src/connection.rs`: `detect_auth_mode()` still returns `AuthMode::None` on network errors, which is a trade-off and may deserve a revisit if it causes false negatives in practice.

## Does Not Make Sense To Fix In This PR

- `crates/meroctl/src/auth.rs`: TOCTOU / file-locking comments. They are valid in theory, but this is a low-probability concurrent CLI race and fixing it properly is larger than this auth-focused PR.
- `crates/meroctl/src/auth.rs`: "move large inline HTML out of `start_callback_server`". This is readability-only and not worth mixing into the current change set.
- `crates/client/src/connection.rs`: "double token load after refresh" / "dedupe refresh and re-auth token update paths". Nice cleanup, but not important enough to hold up this PR.
- `crates/client/src/connection.rs`: single-pass string truncation for `extract_error_message`. Micro-optimization only.
- `crates/client/src/connection.rs`: special-casing `404` for older nodes. Current behavior is a deliberate safe default, and the fallback retry path already reduces the practical risk.
- `crates/client/src/connection.rs`: renaming beyond `ensure_auth_header`. The main misleading name was already fixed, and further renaming would be churn without meaningful behavior change.
- `crates/client/src/connection.rs`: "persist tokens when `node_name` is `None`". For authenticated stored connections, `node_name` should exist; if that assumption changes, it should be handled as a broader design decision rather than patched here.
