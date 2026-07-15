# mero-auth - Forward Authentication Service

JWT-issuing authentication and authorization service for the Calimero network: verifies credentials, mints/refreshes access and refresh tokens, and gates every admin/JSON-RPC/WS/SSE route behind a permission model.

## Package Identity

- **Crate**: `mero-auth`
- **Binary**: `mero-auth` (built from `src/main.rs`; `Cargo.toml` has no `publish = false`, it ships as both a binary and a library)
- **Entry**: `src/main.rs` (standalone process) and `src/embedded.rs::build_app` (mounted inside another Axum app)
- **Key deps**: `axum`/`axum-extra`/`tower-http` (HTTP, cookies, CORS, body limits), `jsonwebtoken` (HS256 JWT), `rocksdb` (persistent key/secret storage), `config` + `clap` (TOML/env config, CLI flags), `ctor` (self-registering provider/storage-backend macros)

## Commands

```bash
# Build
cargo build -p mero-auth

# Test (all)
cargo test -p mero-auth

# Run the standalone service
cargo run -p mero-auth -- --config crates/auth/config/config.toml --bind 0.0.0.0:3001
```

Building the binary requires `CALIMERO_AUTH_FRONTEND_PATH` to point at a built auth-frontend static bundle (`src/api/handlers/mod.rs` embeds it via `rust_embed`); `build.rs` fetches the `calimero-network/auth-frontend` release archive into `OUT_DIR` if the env var isn't already set to a local checkout.

## What it does

`mero-auth` is a self-contained auth microservice, not a library the node calls into for crypto. It:

1. Authenticates a caller via a pluggable **provider** (currently only `user_password`), producing an `AuthResponse { is_valid, key_id, permissions }`.
2. Issues a paired **access/refresh JWT** (`TokenManager`, HS256, secret rotated by `SecretManager`) bound to a `key_id` and optionally a `node_url`.
3. Validates bearer tokens on every subsequent request (`auth_middleware`) and enforces **path-to-permission** mapping (`PermissionValidator`) so a scoped client token can't reach node-operator routes.
4. Persists root keys (user identities), client keys (per-app derived keys), and system secrets in a pluggable `Storage` backend (RocksDB by default, in-memory for tests).

It has two deployment shapes, both compiled from the same code:

- **Proxy mode** (`AuthMode::Proxy` in `calimero-server`, the default): `mero-auth` runs as its own process (`src/main.rs`), and `calimero-server`/a reverse proxy calls `GET /auth/validate` (forward-auth pattern) to authorize each request before forwarding it.
- **Embedded mode** (`AuthMode::Embedded`): `calimero-server` calls `mero_auth::embedded::build_app(config)` directly (see `crates/server/src/auth.rs`) and mounts the returned `Router` at `/auth` and `/admin` inside its own Axum app - no second process, no HTTP hop for the check.

## Module inventory

| Module | Purpose |
| --- | --- |
| `main.rs` | CLI entry (`clap`), loads config, builds storage/secrets/token manager/providers, starts the server, awaits `shutdown_signal` |
| `lib.rs` | Crate root: `AuthResponse`, `AuthError` (the shared error enum every layer maps to an HTTP status) |
| `embedded.rs` | `build_app`/`EmbeddedAuthApp`/`default_config` - the in-process mounting path used by `calimero-server` |
| `server.rs` | `AppState` (shared handler state), `start_server` (standalone binary path), `shutdown_signal` |
| `config.rs` | `AuthConfig` and all sub-configs (`JwtConfig`, `StorageConfig`, `CorsConfig`, `SecurityConfig`, `DevelopmentConfig`); `load_config` layers a TOML file under an `AUTH__`-prefixed env override |
| `secrets.rs` | `SecretManager` / `VersionedSecret`: generates, persists, and (hourly) rotates the JWT-signing and CSRF secrets, with a backup-key fallback |
| `auth/service.rs` | `AuthService` - routes a `TokenRequest` to the matching provider, authenticates, and re-exposes token-manager operations |
| `auth/token/jwt.rs` | `TokenManager` - HS256 issue/verify/refresh, challenge tokens, node-URL host binding, client-key rotation on refresh |
| `auth/middleware.rs` | `auth_middleware` - the Axum middleware every `/admin/*` route runs through: verifies the bearer token, checks permissions, injects `CallerPermissions` |
| `auth/permissions/` | `Permission` enum + `FromStr`/`Display` (the `"context:execute[ctx,user,method]"` string encoding) and `PermissionValidator` (request-path → required-`Permission` mapping, admin default-deny for unmapped `/admin-api/*`) |
| `auth/rate_limit.rs` | `LoginRateLimiter` - in-memory sliding-window brute-force throttle keyed by `(auth_method, public_key)` |
| `auth/security.rs` | Builds `tower-http` security-header layers (HSTS, CSP, frame options) and the request-body-size limiter from `SecurityConfig` |
| `auth/validation.rs` | `ValidatedJson` extractor (JSON + `validator::Validate` in one step), string/identifier/HTML sanitizers |
| `providers/` | `AuthProvider` trait, `ProviderContext`/`ProviderFactory`, the `ctor`-based self-registration macros, and `impls/user_password.rs` (the one shipped provider) |
| `storage/` | `Storage` trait, `KeyManager` (root/client key CRUD + indices), `models::Key`/`KeyType`, RocksDB and in-memory backends, self-registering `StorageProvider`s |
| `api/routes.rs` | `create_router` - assembles public (`/auth/*`) and protected (`/admin/*`) route trees, CORS, security headers, body limit, panic-catch |
| `api/handlers/` | `auth.rs` (token/challenge/refresh/validate/callback/mock-token), `root_keys.rs`, `client_keys.rs`, `permissions.rs`, plus health/metrics/identity/providers/asset handlers in `mod.rs` |
| `utils.rs` | `AuthMetrics` (atomic counters + timer), `sanitize_for_log` (CR/LF and ANSI-escape stripping for log injection) |

## Mental model: the auth flow

**Token issuance** (`POST /auth/token`, handled in `api/handlers/auth.rs::token_handler`): the raw `auth_method`/`public_key` pair is captured for rate-limiting *before* sanitization (so distinct identities can't be collapsed into one bucket), the request is checked against `LoginRateLimiter`, then `AuthService::authenticate_token_request` looks up the `AuthProvider` whose `supports_method` matches, asks it to `prepare_auth_data` from the request, parses that JSON through the `provider_data_registry` back into a typed struct, and calls the provider's `AuthRequestVerifier`. For `user_password`, verification means: hash `sha256("user_password:{username}:{password}")` into a deterministic `key_id`, and either find an existing valid root key with that ID or - only if **no root keys exist yet** - bootstrap the very first one with `admin` permissions. On success, `TokenManager::generate_token_pair` mints an access + refresh JWT scoped to the key's permissions and (if `client_name` looks like a node URL) bound to that node.

**Token verification** happens on every protected request via `auth_middleware`: it skips `/public/*`, otherwise extracts the `Bearer` token, calls `TokenManager::verify_token_from_headers`, which decodes the HS256 JWT, checks the `node_url` claim against the request's `Host`/`X-Forwarded-Host` (fail-closed if a node-bound token's request carries no host header at all - otherwise a client could strip the header to bypass node binding), and confirms the `key_id` still resolves to a non-revoked key in storage. `AuthError::TokenExpired`/`TokenRevoked` are distinct enum variants (not string-matched), so the middleware maps them to `401`/`403` respectively without any risk of a renamed error message silently downgrading a revocation to an expiry.

**Authorization** is a second, independent check after authentication succeeds: `PermissionValidator::determine_required_permissions` maps the request's method+path to zero or more `Permission` values (exact-match table first, then a battery of pre-compiled regexes for parameterized routes like `/admin-api/contexts/:id`), and any unmapped `/admin-api/*` route defaults to requiring `Permission::Admin` - a deliberate default-deny so a new route added to `calimero-server` without a corresponding permission mapping here fails closed instead of being silently open to any valid token. `Permission::satisfies` implements the actual hierarchy (global scope satisfies specific, `admin` satisfies everything, umbrella verbs like `namespace` cover every namespace sub-verb).

**Client-key derivation**: a root key (an authenticated human/identity) can mint scoped client keys via `POST /admin/client-key` - each client key is tied to a `root_key_id` and can only be granted permissions the root key itself already holds (`KeyManager::set_key`/`add_permission` re-validate against the root key on every write, and `update_key_permissions_handler` separately checks the *caller's* JWT permissions before letting them grant anything, closing a privilege-escalation path where a `keys:permissions:update`-scoped key could otherwise hand itself `admin`).

## Key files

| Path | What's there |
| --- | --- |
| `src/main.rs` | Standalone-process bootstrap: config → storage → secrets → token manager → providers → router → serve |
| `src/embedded.rs` | In-process mounting path (`build_app`) used by `calimero-server`'s `AuthMode::Embedded` |
| `src/auth/token/jwt.rs` | All JWT issuance/verification/refresh/challenge logic, plus the node-host-binding guard and its regression tests |
| `src/auth/permissions/validator.rs` | The path→permission regex table and the admin-default-deny fallback; heavily test-covered against 403 regressions |
| `src/auth/permissions/types.rs` | The `Permission` enum family, its string `FromStr`/`Display` codec, and `satisfies` hierarchy logic |
| `src/auth/rate_limit.rs` | Login brute-force throttle; module doc explains its known limitations (in-memory, identity-keyed, wall-clock) |
| `src/providers/impls/user_password.rs` | The only shipped `AuthProvider`; bootstrap-first-root-key semantics live here |
| `src/storage/key_manager.rs` | Root/client key CRUD, the root→client and public-key secondary indices |
| `src/secrets.rs` | JWT signing secret generation, storage, and hourly rotation with a backup-key fallback |
| `crates/auth/config/config.toml` | Reference config showing every section (`jwt`, `storage`, `cors`, `security`, `providers`, `user_password`, `development`) |

## Invariants and gotchas

- **Only one provider ships today**: `user_password`. The crate depends on `starknet`, `ed25519-dalek`, and `ic-agent`, but `providers/impls/mod.rs` registers only `user_password` - those deps are present for a signature/wallet-based provider that isn't wired in yet. Don't assume NEAR/Starknet/ICP auth works because the dependency is in `Cargo.toml`.
- **Provider and storage-backend registration is global and macro-driven**: `register_auth_provider!`/`register_auth_data_type!`/`register_storage_provider!` use `#[ctor::ctor]` to run at program load, before `main`. A new provider module must be `pub mod`-declared in `providers/impls/mod.rs` (or it never registers) and must call these macros exactly once.
- **Revoked keys are invisible, not flagged**: `KeyManager::get_key` returns `None` for a revoked key rather than `Some(key)` with `is_valid() == false`. Downstream code (e.g. token verification) therefore reports "key not found," not "key revoked" - this is deliberate (tests pin it) but means you cannot distinguish "never existed" from "revoked" through this API.
- **`AuthError::TokenExpired`/`TokenRevoked` must stay dedicated variants**: the middleware and `validate_handler` branch on the *variant*, not a substring of the message, specifically so renaming an error string can never accidentally reclassify a 403 (revoked) as a 401 (expired/invalid). Preserve this pattern for any new error that needs its own status code.
- **Node-URL binding fails closed**: if a JWT carries a `node_url` claim and the incoming request has neither `Host` nor `X-Forwarded-Host`, verification is rejected rather than skipped - the alternative (skip validation on missing host) would let a client strip both headers to bypass node binding entirely.
- **The `/admin-api/*` default-deny is load-bearing**: any new route `calimero-server` adds under `/admin-api/` that isn't given an explicit mapping in `PermissionValidator::get_permissions_for_path_with_params`/`get_permissions_for_exact_paths` automatically requires `Permission::Admin`. This is intentional fail-closed behavior, not a bug to "fix" by adding a wildcard allow.
- **`/mock-token` only exists in debug builds**: both the handler and its route registration are behind `#[cfg(debug_assertions)]`, and the handler additionally checks `config.development.enable_mock_auth` (default `false`) plus an optional constant-time header comparison. It cannot be reached in a release build regardless of config.
- **The login rate limiter is in-memory and per-process, not per-IP**: restarting the process clears all lockouts, and the key is `(auth_method, public_key)` - rotating the public key resets the bucket. See the module doc in `auth/rate_limit.rs` for the full list of accepted limitations and their follow-up hooks.
- **CORS/security headers are per-instance config**, not hardcoded: a permissive `default_csp_connect_src` (allows `https:`/`http:` broadly) exists to support configurable registries and local dev; tightening it is a config change, not a code change.

Part of [crates/](../AGENTS.md).
