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

Building the binary requires `CALIMERO_AUTH_FRONTEND_PATH` to point at a built auth-frontend static bundle (`src/api/handlers/mod.rs` embeds it via `rust_embed`); `build.rs` fetches the `calimero-network/auth-frontend` release archive into `OUT_DIR` if the env var isn't already set to a local checkout. The fetched version is **pinned** in `build.rs` (`CALIMERO_AUTH_FRONTEND_VERSION`), so a given core commit always embeds the same frontend — bump that constant to take a new auth-frontend release, or set `CALIMERO_AUTH_FRONTEND_VERSION` (`latest` included) for a one-off build.

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
| `providers/` | `AuthProvider` trait, `ProviderContext`/`ProviderFactory`, the `ctor`-based self-registration macros, and the two shipped providers: `impls/user_password.rs` and `impls/account_proof.rs` |
| `auth/challenge.rs` | `ChallengeMinter` - stateless single-use login challenges (`expiry ‖ nonce ‖ tag`), the spent set, and its expiry sweep |
| `storage/` | `Storage` trait, `KeyManager` (root/client key CRUD + indices), `models::Key`/`KeyType`, RocksDB and in-memory backends, self-registering `StorageProvider`s |
| `api/routes.rs` | `create_router` - assembles public (`/auth/*`) and protected (`/admin/*`) route trees, CORS, security headers, body limit, panic-catch |
| `api/handlers/` | `auth.rs` (token/challenge/refresh/validate/callback/mock-token), `root_keys.rs`, `client_keys.rs`, `permissions.rs`, plus health/metrics/identity/providers/asset handlers in `mod.rs` |
| `utils.rs` | `AuthMetrics` (atomic counters + timer), `sanitize_for_log` (CR/LF and ANSI-escape stripping for log injection) |

## Mental model: the auth flow

**Token issuance** (`POST /auth/token`, handled in `api/handlers/auth.rs::token_handler`): the raw `auth_method`/`public_key` pair is captured for rate-limiting *before* sanitization (so distinct identities can't be collapsed into one bucket), the request is checked against `LoginRateLimiter`, then `AuthService::authenticate_token_request` looks up the `AuthProvider` whose `supports_method` matches, asks it to `prepare_auth_data` from the request, parses that JSON through the `provider_data_registry` back into a typed struct, and calls the provider's `AuthRequestVerifier`. For `user_password`, verification means: derive a deterministic `key_id` from the credentials (salted PBKDF2) and find an existing valid root key with that ID — the login path **never mints keys**. The admin root key is provisioned out of band (`src/provisioning.rs`): at `merod init`, at startup from `MERO_AUTH_ADMIN_USER`/`MERO_AUTH_ADMIN_PASSWORD`, or offline via `merod auth set-admin`. On success, `TokenManager::generate_token_pair` mints an access + refresh JWT scoped to the key's permissions and (if `client_name` looks like a node URL) bound to that node.

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
| `src/providers/impls/user_password.rs` | Password `AuthProvider`; existing-user verification and the legacy key-id migration live here |
| `src/providers/impls/account_proof.rs` | Device-key `AuthProvider` for accounts that run no node; the four checks a login proves, in order, and why the challenge is spent last |
| `src/auth/challenge.rs` | Stateless single-use login challenges and the spent-set sweep |
| `src/provisioning.rs` | Out-of-band admin-root-key provisioning (init-time, startup-env, offline `set-admin`) — the only place the first root key is minted |
| `src/storage/key_manager.rs` | Root/client key CRUD, the root→client and public-key secondary indices |
| `src/secrets.rs` | JWT signing secret generation, storage, and hourly rotation with a backup-key fallback |
| `crates/auth/config/config.toml` | Reference config showing every section (`jwt`, `storage`, `cors`, `security`, `providers`, `user_password`, `account_proof`, `development`) |

## Invariants and gotchas

- **Two providers ship**: `user_password` and `account_proof`. Both are off unless named in the `providers` config map. The crate also depends on `starknet`, `ed25519-dalek` and `ic-agent`, but nothing registers a provider using them - don't assume NEAR/Starknet/ICP auth works because the dependency is in `Cargo.toml`.
- **`account_proof` refuses to start without `account_proof.node_key`.** The key is this node's own identity, and it is what tells a `LoginStatement` minted for this node from one minted for another. A provider that could not make that distinction would accept statements minted for any node, so absence is a startup failure rather than a default - the one attack the field exists for is a hostile relay fetching a real challenge here, serving it to a user as its own, and replaying the signed result.
- **An `account_proof` session is authentication, never membership.** Its `key_id` (and so the JWT `sub`, and so `X-Auth-User`) is the **account**; whether that account may touch any given context is an at-cut question this service cannot see and must be answered per request at the node. `session_permissions` therefore defaults to a set every member of which is gated a *second* time at the node: `context:intent` (the warrant + `CAN_AUTHOR_ON_BEHALF`), `context:query` and `context:subscribe` (per-call/per-subscription membership), and `context:list-own` + `namespace:list-own` (the caller's groups, resolved per request by `calimero-server`'s `admin/caller_scope.rs`). Nothing in it is authority the session confers by itself.
- **A challenge is spent only after a signature over it verifies.** `ChallengeMinter::verify` is cheap and runs first; `redeem` runs last. Burning on presentation instead would turn the replay guard into a denial-of-service primitive, because a challenge crosses the wire in the clear and anyone who sees one could invalidate it. Single-use holds across exactly as much as the storage is shared across: separate replicas with separate storage each accept the same challenge once.
- **Provider and storage-backend registration is global and macro-driven**: `register_auth_provider!`/`register_auth_data_type!`/`register_storage_provider!` use `#[ctor::ctor]` to run at program load, before `main`. A new provider module must be `pub mod`-declared in `providers/impls/mod.rs` (or it never registers) and must call these macros exactly once.
- **`get_key` hides revoked keys, `get_key_including_invalid` does not**: `KeyManager::get_key` returns `None` for a revoked or expired key, so a caller that never checks validity still cannot act on one - keep that as the default for anything making an access decision. Where "revoked" and "never existed" must be told apart, use `get_key_including_invalid` and branch on `is_revoked()` explicitly. `TokenManager::verify_token_string` does exactly that, so a revoked key reaches the client as `403` + `X-Auth-Error: token_revoked` (terminal, stop retrying) instead of a `401` shared with four unrelated failures.
- **`AuthError::TokenExpired`/`TokenRevoked` must stay dedicated variants**: the middleware and `validate_handler` branch on the *variant*, not a substring of the message, specifically so renaming an error string can never accidentally reclassify a 403 (revoked) as a 401 (expired/invalid). Preserve this pattern for any new error that needs its own status code.
- **Node-URL binding fails closed**: if a JWT carries a `node_url` claim and the incoming request has neither `Host` nor `X-Forwarded-Host`, verification is rejected rather than skipped - the alternative (skip validation on missing host) would let a client strip both headers to bypass node binding entirely.
- **`list-own` is narrower than `list`, one direction only**: `context:list-own` / `namespace:list-own` / `group:list-own` gate the caller-scoped reads — `GET /admin-api/contexts`, `/contexts/:id`, `/namespaces`, `/namespaces/:id{,/groups}`, `/groups/:id{,/contexts}`, and the four context read sub-resources `/contexts/:id/{identities,identities-owned,storage,group}` — whose handlers narrow the answer to the caller's own groups. The wide `list` **satisfies** the narrow one, so every pre-existing operator or client token still reaches those routes; the reverse must never hold, because `context:list` also gates the two `for-application` listings, which enumerate node-wide and are not caller-scoped. If you widen the hierarchy in `types.rs::satisfies`, that is the pair to leave alone.
- **A route on `list-own` is only half of the pair.** The narrow verb is a claim that the *handler* refuses a resource the caller's groups do not reach (`calimero-server`'s `admin/caller_scope.rs`). Verb without handler is a cross-tenant read; handler without verb is a check nothing can reach — both have happened here.
- **Two `list-own` arms are shared, and that is where the blast radius lives.** `CONTEXT_READ_SUBRESOURCE_REGEX` grants the narrow verb to four paths at once, so a fifth sub-resource added to that pattern inherits it silently. `GROUP_OWN_READ_REGEX` exists precisely to avoid the same trap on the group side: it is matched *before* the catch-all `GROUP_REGEX` and is anchored to `/groups/:id` and `/groups/:id/contexts` alone, so `/members`, `/capabilities`, `/settings` and `/signing-keys` keep requiring the wide `group:list`. Widening either pattern opens every route it newly matches, with no compile error and no failing test.
- **The `/admin-api/*` default-deny is load-bearing**: any new route `calimero-server` adds under `/admin-api/` that isn't given an explicit mapping in `PermissionValidator::get_permissions_for_path_with_params`/`get_permissions_for_exact_paths` automatically requires `Permission::Admin`. This is intentional fail-closed behavior, not a bug to "fix" by adding a wildcard allow.
- **`/mock-token` only exists in debug builds**: both the handler and its route registration are behind `#[cfg(debug_assertions)]`, and the handler additionally checks `config.development.enable_mock_auth` (default `false`) plus an optional constant-time header comparison. It cannot be reached in a release build regardless of config.
- **The login rate limiter is in-memory and per-process, not per-IP**: restarting the process clears all lockouts, and the key is `(auth_method, public_key)` - rotating the public key resets the bucket. See the module doc in `auth/rate_limit.rs` for the full list of accepted limitations and their follow-up hooks.
- **CORS/security headers are per-instance config**, not hardcoded: a permissive `default_csp_connect_src` (allows `https:`/`http:` broadly) exists to support configurable registries and local dev; tightening it is a config change, not a code change.

Part of [crates/](../AGENTS.md).
