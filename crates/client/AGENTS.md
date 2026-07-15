# calimero-client - HTTP Client for the Calimero Admin API and JSON-RPC

Generic, trait-based Rust client for talking to a Calimero node's Admin API, JSON-RPC endpoint, and WebSocket upgrade, with built-in JWT auth/refresh handling.

## Package Identity

- **Crate**: `calimero-client`
- **Entry**: `src/lib.rs`
- **Key deps**: `reqwest` (HTTP transport, `json` feature; `native-tls`/`rustls` feature-gated), `tokio` (async runtime), `serde`/`serde_json` (request/response bodies), `async-trait` (object-safe async traits), `zeroize` (token wiping), `webbrowser` (CLI OAuth flow), `percent-encoding` (path-traversal guard), `calimero-primitives` / `calimero-server-primitives` / `calimero-context-config` (shared domain types and Admin API request/response DTOs)

## Commands

```bash
# Build
cargo build -p calimero-client

# Test (all - unit tests + wiremock-backed integration tests in src/tests.rs)
cargo test -p calimero-client

# Test a single case
cargo test -p calimero-client get_retries_on_401_and_reauthenticates -- --nocapture
```

## Public API

| Item | Kind | Purpose |
| --- | --- | --- |
| `Client<A, S>` | struct | Main entry point; generic over authenticator `A` and storage `S`. Holds a `ConnectionInfo<A, S>` |
| `Client::new(connection)` | fn | Wrap a `ConnectionInfo` |
| `Client::api_url()` / `Client::ws_url()` | fn | Base HTTP URL / derived WebSocket URL (`http`→`ws`, `https`→`wss`, path-preserving) |
| `Client::auth_header()` | fn | Resolve the bearer header for out-of-band use (e.g. attaching to a WS upgrade) |
| `Client::execute_jsonrpc(request)` | fn (`client/jsonrpc.rs`) | POST `jsonrpc`, generic `Request<P>` → `Response` |
| `Client::create_alias` / `delete_alias` / `list_aliases` / `lookup_alias` / `resolve_alias` | fn (`client.rs`) | Generic alias CRUD over any `T: UrlFragment` (`ContextId`, `PublicKey`, `ApplicationId`) |
| ~90 endpoint methods across `client/{contexts,applications,blobs,group,namespace,packages,system}.rs` | fn | One method per Admin API operation (contexts, apps, blobs, groups/subgroups, namespaces, packages, system/network/TEE) - see submodules below |
| `ConnectionInfo<A, S>` | struct | Owns the `reqwest::Client`, `api_url`, optional `node_name`, the `authenticator`, `client_storage`, and an `auth_lock` for single-flight refresh |
| `ConnectionInfo::{get, post, post_no_body, delete, delete_with_body, patch, put_json, put_binary, get_binary, head}` | fn | Low-level verb helpers; every endpoint method in `client/*.rs` is built on these |
| `ConnectionInfo::detect_auth_mode()` | fn | Probes `admin-api/contexts`; `401`→`Required`, `2xx`→`None`, other status→`Required`, network error→`None` |
| `AuthMode` | enum | `Required` / `None` |
| `ClientAuthenticator` | trait (`traits.rs`) | `authenticate`, `refresh_tokens`, `handle_auth_failure`, `check_auth_required`, `get_auth_method`, `supports_refresh` |
| `ClientStorage` | trait (`traits.rs`) | `load_tokens`, `save_tokens`, `update_tokens` (default load-merge-save), `remove_tokens`, `list_nodes` |
| `ClientConfig<A, S>` | trait (`traits.rs`) | Higher-level node-registry abstraction (get/set active node, add/remove node, settings) - not implemented in this crate |
| `CliAuthenticator` | struct (`auth.rs`) | Interactive/browser-based authenticator; pluggable `OutputHandler` |
| `HeadlessAuthenticator`, `ApiKeyAuthenticator` | struct (`auth.rs`) | Non-interactive authenticators (pre-set tokens / static API key) |
| `JwtToken` | struct (`storage.rs`) | Access/refresh token pair; decodes the unsigned `exp` claim on construction; `Zeroize`s on drop |
| `ClientError` | enum (`errors.rs`) | `Network`, `Authentication`, `Storage`, `Http { status, message }`, `Internal`; `is_not_found()` helper |
| `ResolveResponse<T>` / `ResolveResponseValue<T>` | struct/enum (`client.rs`) | Result of `resolve_alias`: either a server-side `Lookup` or a locally `Parsed` value |
| `VERSION` | const | `CARGO_PKG_VERSION` |

Everything public returns `eyre::Result<T>` (re-exported as `Result`), not `ClientError` directly - `ClientError` is the concrete type usually found by downcasting (see `is_not_found`).

## Mental Model

`Client<A, S>` is a thin, cloneable wrapper around `ConnectionInfo<A, S>`; almost all behavior lives in `connection.rs`. Every endpoint method (contexts, applications, blobs, groups, namespaces, packages, system) is a one-liner that calls `self.connection.{get,post,...}(path, body)` and deserializes the JSON response - the submodules under `src/client/` are purely a path/type catalog, not where the interesting logic is.

**Request flow**: `ConnectionInfo::request` (and the binary/head variants) resolve the path against `api_url` via `resolve_path`, decide if the endpoint needs auth (`path_requires_auth` - everything except `admin-api/health`), then call `execute_request_with_auth_retry`, which loads a fresh auth header on *every* attempt (including retries), sends the request, and classifies the response:
- `401` and under `MAX_RETRIES` (2): re-authenticate via `refresh_or_reauth`, then either loop (idempotent verbs: GET/PUT/DELETE/DELETE-with-body) or bail without replay (POST/PATCH - replaying a non-idempotent write after a 401 risks duplicating an already-applied create/governance operation).
- `403`: bailed as "access denied".
- Other non-2xx: turned into `ClientError::Http { status, message }`, with `message` extracted only from known-safe JSON fields (`error.message` / `error` / `message`), truncated to 300 chars, to avoid leaking raw server internals.
- `2xx`: body is read via `read_body_capped` (hard byte ceilings per body class - JSON 16MiB, error bodies 64KiB, token responses 64KiB, binary/blob 512MiB) and `serde_json`-deserialized.

**Auth flow**: `ensure_auth_header` is the gate all of the above goes through. No `node_name` → no auth attempted. A stored token that's usable and not expiring within `TOKEN_REFRESH_SKEW_SECS` (30s) is used as-is. An expired token forces a synchronous refresh before the request goes out; a token expiring soon is refreshed proactively but non-fatally (falls back to the current token on refresh failure). All of this funnels through `refresh_or_reauth`, which takes `auth_lock` (a `tokio::sync::Mutex<()>`) so concurrent callers collapse into a single `/auth/refresh` POST (via `try_refresh_token`) or, on any refresh failure, a single call to `authenticator.authenticate(api_url)` (which for `CliAuthenticator` opens a browser / prompts stdin). An empty access token (the default `remove_tokens` fallback) is always treated as "no credentials", never sent as `Authorization: Bearer `.

**WebSocket**: the crate does not open WS connections itself; `Client::ws_url()` derives the `wss?://.../ws` URL from `api_url` (rejecting unknown schemes, stripping query/fragment) and `Client::auth_header()` exposes the bearer token so a caller (e.g. `meroctl`) can attach it once at the upgrade handshake, since WS auth is connection-level, not per-message.

**Path safety**: all endpoint paths are relative strings (`"admin-api/contexts/{id}"`) resolved by `resolve_path`, which appends segments to `api_url`'s existing path (preserving a reverse-proxy mount point like `/calimero/node1/`) and rejects any segment that percent-decodes to `.`/`..` or contains a control character - stopping an interpolated identifier from escaping its intended admin-api subtree.

## Key Files

| Path | What's there |
| --- | --- |
| `src/connection.rs` | The whole request/auth/retry/body-cap engine; `resolve_path`, `read_body_capped`, `extract_error_message` |
| `src/client.rs` | `Client` struct, `ws_url`, alias CRUD, `UrlFragment` trait (maps `ContextId`/`PublicKey`/`ApplicationId` to alias-kind strings) |
| `src/client/contexts.rs` | Context CRUD, storage/identities/client-keys, sync/resync |
| `src/client/group.rs` | Groups, subgroups, membership, capabilities, metadata, migrations/upgrades, join/leave (largest submodule, ~35 methods) |
| `src/client/namespace.rs` | Namespace CRUD, invitations, group listing within a namespace |
| `src/client/applications.rs` | Install/uninstall/list applications and versions |
| `src/client/blobs.rs` | Blob upload/download/list/delete/info (binary paths, not JSON) |
| `src/client/packages.rs` | Package/version listing |
| `src/client/system.rs` | Identity generation, peer count, network status, specialized-node invite, TEE fleet-join |
| `src/client/jsonrpc.rs` | `execute_jsonrpc` - the one generic JSON-RPC passthrough |
| `src/auth.rs` | `ClientAuthenticator` implementations (`CliAuthenticator`, `HeadlessAuthenticator`, `ApiKeyAuthenticator`) plus `OutputHandler` |
| `src/storage.rs` | `JwtToken`, unsigned `exp` decoding, `merged_with` (preserve-on-refresh merge), `TokenValidation` |
| `src/traits.rs` | `ClientStorage`, `ClientAuthenticator`, `ClientConfig`, `ClientSettings`, `HttpClientConfig` |
| `src/errors.rs` | `ClientError` and its `From<reqwest::Error>` / `From<serde_json::Error>` / `From<std::io::Error>` / `From<url::ParseError>` conversions |
| `src/tests.rs` | `wiremock`-backed integration tests covering nearly every endpoint plus the 401-retry/idempotency/traversal/proxy-base-path edge cases |

## Invariants and Gotchas

- **POST/PATCH are never replayed after a 401.** The session is re-authenticated so the *next* call works, but the failed write is surfaced as an error rather than resent - resending could duplicate a create/governance op the origin already applied. GET/PUT/DELETE/DELETE-with-body are replayed since they're idempotent. Adding a new mutating endpoint under `RequestType::Post`/`Patch` gets this protection for free; putting a genuinely non-idempotent operation behind `RequestType::Put` would silently break that guarantee.
- **`update_tokens`'s default load-merge-save is not atomic.** `ConnectionInfo` serializes its own refresh path through `auth_lock`, but any caller updating tokens from outside that path (or from multiple tasks against the same `ClientStorage`) must serialize itself, or add a real compare-and-swap by overriding `update_tokens`.
- **Body size caps are load-bearing, not arbitrary.** `MAX_JSON_BODY_BYTES` (16MiB), `MAX_ERROR_BODY_BYTES` (64KiB), `MAX_TOKEN_BODY_BYTES` (64KiB), `MAX_BINARY_BODY_BYTES` (512MiB) all guard against OOM from a hostile/misbehaving node; `read_body_capped` enforces the cap by per-chunk tally even when `Content-Length` is absent or lies.
- **`resolve_path` is the only sanctioned way to build a request URL.** A raw `Url::join`/`set_path` would drop a reverse-proxy base path and doesn't reject `../` traversal; `ws_url` and `try_refresh_token`'s `auth/refresh` call go through the same helper for the same reason.
- **`admin-api/health` is the only path treated as not requiring auth** (`path_requires_auth`); every other path - including `detect_auth_mode`'s probe target `admin-api/contexts` - is auth-gated.
- **An empty `access_token` means "logged out," never "authenticated as empty string."** `JwtToken::is_usable()` encodes this; `ClientStorage::remove_tokens`'s default fallback (no delete primitive on the trait) persists exactly this empty-token sentinel.
- **JWT signatures are never verified client-side** - `decode_jwt_exp` only reads the unsigned `exp` claim to schedule proactive refresh; a malformed/exp-less token just falls back to reactive (401-driven) refresh, and any real validation is the server's job.
- **`Client::connection()` is `#[cfg(test)]`-only** - production code (e.g. `meroctl`) cannot reach into `ConnectionInfo` directly and must go through the typed endpoint methods or `execute_jsonrpc`.
- **`CliAuthenticator::authenticate` is a stdin-prompt placeholder**, not a real OAuth flow (see the "For now, this is a placeholder implementation" comments in `auth.rs`) - don't assume it drives an actual browser round-trip beyond opening the URL.

## Consumers

`calimero-client` is a workspace dependency of `crates/meroctl` only (the CLI). `meroctl` builds its own `Client`/`ConnectionInfo`/authenticator/storage wiring on top of these traits (`crates/meroctl/src/{client,connection,auth,storage}.rs`) rather than embedding node-specific logic in this crate - keep new abstractions here generic across authenticator/storage backends rather than CLI-specific.

Part of [crates/](../AGENTS.md).
