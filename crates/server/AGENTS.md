# calimero-server - HTTP/WS/SSE Server

HTTP, WebSocket, and Server-Sent Events server for Admin API, JSON-RPC, and real-time subscriptions.

## Package Identity

- **Crate**: `calimero-server`
- **Entry**: `src/lib.rs`
- **Framework**: axum (HTTP), tokio (async)

## Commands

```bash
# Build
cargo build -p calimero-server

# Test
cargo test -p calimero-server
```

## File Organization

```
src/
├── lib.rs                    # Server initialization
├── config.rs                 # Server configuration
├── admin.rs                  # Admin API module parent
├── admin/
│   ├── handlers.rs           # Handlers module parent
│   ├── handlers/
│   │   ├── applications.rs   # Applications handlers parent
│   │   ├── applications/
│   │   │   ├── get_application.rs
│   │   │   ├── install_application.rs
│   │   │   ├── install_dev_application.rs
│   │   │   ├── list_applications.rs
│   │   │   └── uninstall_application.rs
│   │   ├── context.rs        # Context handlers parent
│   │   ├── context/
│   │   │   ├── create_context.rs
│   │   │   ├── delete_context.rs
│   │   │   ├── get_context.rs
│   │   │   ├── get_context_ids.rs
│   │   │   ├── get_context_identities.rs
│   │   │   ├── get_context_storage.rs
│   │   │   ├── get_contexts_for_application.rs
│   │   │   ├── get_contexts_with_executors_for_application.rs
│   │   │   ├── join_context.rs
│   │   │   ├── sync.rs
│   │   │   └── update_context_application.rs
│   │   ├── identity.rs       # Identity handlers parent
│   │   ├── identity/
│   │   │   └── generate_context_identity.rs
│   │   ├── alias.rs          # Alias handlers parent
│   │   ├── alias/
│   │   │   ├── create_alias.rs
│   │   │   ├── delete_alias.rs
│   │   │   ├── list_aliases.rs
│   │   │   └── lookup_alias.rs
│   │   ├── blob.rs           # Blob handlers
│   │   ├── peers.rs           # Peer handlers
│   │   ├── groups/            # Group management handlers
│   │   ├── namespaces/        # Namespace handlers
│   │   ├── network/           # Network status handlers
│   │   ├── tee.rs             # TEE handlers parent
│   │   ├── tee/
│   │   │   ├── announce.rs       # The TeeAttestationAnnounce a replica publishes
│   │   │   ├── attest.rs
│   │   │   ├── evidence_retry.rs # Re-announces a TEE whose authority evidence is missing
│   │   │   ├── fleet_join.rs
│   │   │   ├── info.rs
│   │   │   └── registration_attest.rs # Quote with the registration binding (protected)
│   │   ├── packages.rs        # Package handlers
│   │   ├── list_packages.rs   # List packages
│   │   ├── list_versions.rs   # List versions
│   │   └── get_latest_version.rs  # Get latest version
│   ├── service.rs            # Admin service setup
│   ├── storage.rs            # Admin storage
│   └── storage/
│       └── ssl.rs            # SSL storage
├── jsonrpc.rs                # JSON-RPC module parent
├── jsonrpc/
│   └── execute.rs            # JSON-RPC execution
├── ws.rs                     # WebSocket module parent
├── ws/
│   ├── subscribe.rs          # WS subscription
│   └── unsubscribe.rs        # WS unsubscription
├── sse.rs                    # SSE module parent
├── sse/
│   ├── config.rs             # SSE config
│   ├── events.rs             # SSE events
│   ├── handlers.rs           # SSE handlers
│   └── ...
├── auth.rs                   # Authentication middleware
├── proxy_permissions.rs      # Proxy mode: X-Auth-Permissions → GrantedPermissions
├── sealed.rs                 # Sealed transport: /sealed/v2 envelope, wraps the router
├── sealed/
│   └── session.rs            # Noise NK handshake and the sessions it opens
├── metrics/
│   └── tests.rs              # Metrics served on their own listener only
└── metrics.rs                # Prometheus metrics and their listener
primitives/                   # calimero-server-primitives
└── src/
    ├── lib.rs                # Shared types
    ├── jsonrpc.rs            # JSON-RPC types
    └── admin/mod.rs          # Admin API types
```

## API Endpoints

### Admin API

Several admin reads are **caller-scoped** (#3941): `GET /admin-api/contexts`,
`GET /admin-api/namespaces`, the single-context `GET /admin-api/contexts/{id}`
and its four read sub-resources (`/identities`, `/identities-owned`, `/storage`,
`/group`), plus `/namespaces/:id{,/groups}` and `/groups/:id{,/contexts}`,
return only what the caller's groups reach, resolved per request
through `admin/caller_scope.rs`. Every one that names a context applies the same
`ListScope::admits` predicate the listing does, through the shared
`caller_scope::admits_context` — deliberately one rule and one copy of it, so a
context the listing hides cannot be reached by naming its id on any of them
(`403`; a `404` still means the node does not hold it). At the permission layer they are
gated on the narrow `context:list-own` / `namespace:list-own`, which is what a
delegated `account_proof` session carries; the wide `context:list` /
`namespace:list` still satisfies them, so operator tokens are unaffected. The
two `for-application` listings are **not** scoped — they enumerate node-wide
without ever naming a context whose group could be checked — and keep requiring
the wide `context:list`. A node-owner session, and a node
running without the auth guard at all (`AuthMode::Proxy`, the default), keep the
node-wide view — narrowing there would empty the endpoint on every
default-configured node without closing anything, since the proxy is what decides
who gets through. `GET /admin-api/blobs` is **not** scoped: `BlobMeta` carries no
owner and blobs are deduplicated by content hash with a `refs` count, so
ownership is many-to-many and needs a model rather than an index (core #4019).
It keeps requiring the node-wide `blob:list`, so a delegated session cannot
enumerate blobs — opening it alongside the scoped reads above would hand every tenant
the blob ids of every other one. `PUT /admin-api/blobs?context_id=` records the
upload for that context (`record_blob_owner`) when this node owns an identity
there, and refuses it with `400` before storing anything otherwise; a blob
uploaded without one is never served to peers.

Blob **transfer** is scoped through contexts instead (`admin/handlers/blob.rs`).
`PUT /admin-api/blobs` requires `blob:add-own` and `GET`/`HEAD
/admin-api/blobs/{id}` `blob:get-own` (`blob:add[:stream]` / `blob:get` satisfy
them, so existing tokens are unaffected). An account-scoped caller must name a
`context_id` that `admits_context` admits (`400` without one, `403` outside its
groups), is held to `MAX_ACCOUNT_BLOB_UPLOAD_BYTES`, and is served a blob only
when `Column::ContextBlob` associates it with that context — otherwise the same
`404` as a missing blob, so it is not an existence oracle. The association is
written by an upload naming the context and by a network fetch a peer of the
context served (`NodeClient::get_blob`), **never** by an announce or an app host
call, which only name an id. If you add a path that records it, it must prove the
bytes entered for that context. `DELETE` stays on the node-wide `blob:remove`.

```
GET  /admin-api/contexts              # List contexts (caller-scoped)
POST /admin-api/contexts              # Create context
GET  /admin-api/contexts/{id}          # Get context (caller-scoped)
DELETE /admin-api/contexts/{id}        # Delete context

GET  /admin-api/namespaces            # List namespaces (caller-scoped)
GET  /admin-api/applications          # List apps
POST /admin-api/install-application   # Install app by package@version
GET  /admin-api/applications/{id}      # Get app
GET  /admin-api/applications/{id}/abi  # Embedded WASM ABI manifest (optional ?service_name=)

POST /admin-api/contexts/{id}/join     # Join context
```

### JSON-RPC

```
POST /jsonrpc                         # JSON-RPC 2.0 endpoint
```

### WebSocket

```
WS   /ws                              # WebSocket connection
```

The upgrade needs `context:subscribe`; each `execute` message needs `context:execute` for its
context and method. See "Execute authority" below.

### SSE

```
GET  /events                          # Server-sent events
```

## Patterns

### Admin Handler Pattern

- ✅ DO: Follow pattern in `src/admin/handlers/context.rs`

```rust
// src/admin/handlers/context.rs
use axum::extract::{Path, State};
use axum::Json;

pub async fn get_context(
    Path(context_id): Path<ContextId>,
    State(state): State<AppState>,
) -> Result<Json<ContextResponse>, ApiError> {
    // Implementation
}

pub async fn create_context(
    State(state): State<AppState>,
    Json(request): Json<CreateContextRequest>,
) -> Result<Json<ContextResponse>, ApiError> {
    // Implementation
}
```

### Router Setup

```rust
// src/admin/service.rs
use axum::Router;
use axum::routing::{get, post, delete};

pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/contexts", get(list_contexts).post(create_context))
        .route("/contexts/{id}", get(get_context).delete(delete_context))
}
```

When adding a new `.route(...)`, regenerate `crates/server/endpoints.json` via `UPDATE_MANIFEST=1 cargo test -p calimero-server --test route_manifest`, and cover the endpoint with a mero-js e2e hit or an entry in coverage-baseline.json: `{"route": "METHOD /path", "reason": "..."}`, where the reason says why no e2e node can reach a success (the check refuses an entry without one). Once a mero-js test reaches a baselined route with a status under 400, drop its entry: the SDK e2e fails on a stale entry when paired with a mero-js branch, and warns about it against mero-js master.

## Key Files

| File                                                     | Purpose                 |
| -------------------------------------------------------- | ----------------------- |
| `src/lib.rs`                                             | Server setup            |
| `src/admin/service.rs`                                   | Admin router setup      |
| `src/admin/handlers/context.rs`                          | Context handlers parent |
| `src/admin/handlers/context/create_context.rs`           | Context creation        |
| `src/admin/handlers/applications.rs`                     | App handlers parent     |
| `src/admin/handlers/applications/install_application.rs` | App install             |
| `src/jsonrpc/execute.rs`                                 | JSON-RPC execution      |
| `src/ws/subscribe.rs`                                    | WS subscriptions        |
| `src/subscription_grants.rs`                             | Keeping a subscription's authorization true |
| `src/sse/handlers.rs`                                    | SSE handlers            |
| `primitives/src/jsonrpc.rs`                              | JSON-RPC types          |
| `primitives/src/admin/mod.rs`                            | Admin API types         |

## JIT Index

```bash
# Find all handlers
rg -n "pub async fn" src/admin/handlers/

# Find route definitions
rg -n "\.route\(" src/

# Find API types
rg -n "pub struct.*Request" primitives/src/

# Find auth middleware
rg -n "pub async fn" src/auth.rs
```

## Authentication

Authentication handled via middleware in `src/auth.rs`. Two paths reach the same
guard, and a request may use either:

- **A session.** A JWT bearer token, validated per request, optionally bound to a
  node URL. `AuthenticatedAccount` / `AuthenticatedNodeOwner` / `AuthenticatedDevice`
  are the extensions it injects.
- **A request-carried proof** (`src/proof_auth.rs`). One header,
  `X-Calimero-Proof`, holding a hex borsh `CallerProof`: the account root's
  certificate over a device key, optionally the device's statement over a session
  key, and a signature over *this* method, path and body. Nothing is issued to the
  caller, so it works on a node the caller has no relationship with.

The proof path is installed only when `AdminConfig::delegated_access` is set —
`ProofPolicy::resolve` in `service_mounts.rs` is the single decision point. Three
refusals, and they are deliberately distinct:

| answer | meaning |
| --- | --- |
| `401` + `X-Auth-Error: invalid_proof` | `Malformed` or `Unverified` — bad signature, wrong node, outside its window, not a `CallerProof` |
| `403` + `X-Auth-Error: invalid_proof` | `NotServed`: this node serves no delegated access and the proof names an account other than its own (decided before any signature check) |
| `401`, no header | no credential at all |

Checks run cheapest-first (handoff count and served account → covers →
freshness → node binding → one signature → the certificate chain's *n*), so a
stale or misaddressed proof costs almost nothing to refuse. A credential carrying
more than `calimero_account::MAX_PRESENTED_HANDOFFS` root-key handoffs is refused as `Malformed`, here
and on the delegated-intent routes, before any signature is verified.

**Only the session link carries a node**, so a two-link (device-signed) proof has
no node binding and is replayable at any node serving delegated access until it
expires. A deployment relying on that binding must require the session link. See
`CallerProof::verify`'s own docs, which state the asymmetry.

`delegated-proof.yml` drives all of this against real nodes; `delegated-session.yml`
covers the token path.

## Presence for accounts

`POST /admin-api/contexts/{id}/presence-intents` (`admin/handlers/context/presence_intent.rs`) is how an account with no node publishes ephemeral presence. It sits on the public `delegated_execution_routes()` router with the other intents routes: the device's signature over the `PresenceStatement` is the credential, and the certificate in `authorProof` ties the device to its account. The handler only rebuilds the update (the context from the path, the author from the certificate's key, so a client cannot name another) and hands it to `NodeClient::publish_delegated_ephemeral`, which makes every decision. Each `DelegatedPresenceError` has its own status (`status_for`). It is not `/intents`: presence runs nothing, spends no warrant nonce and changes no state.

## Client key bindings

A client key minted by `POST /admin/client-key` authenticates as the node owner,
so its bindings are the only thing that narrows it. `context[<ctx>,<identity>]`
and `application-binding[<app>]` in its permission list become a
`ClientKeyScope` extension, and every surface that names a context or group
checks it:

- `/jsonrpc`, WS `execute` and context subscribe (WS and SSE): the context must
  be the bound one, or run the bound application.
- Group subscribe (WS and SSE) and admin routes with a `{group_id}` or
  `{namespace_id}`: a context binding reaches only the groups on the chain from
  its context's group up to the namespace; an application binding only groups
  targeting that application.
- Admin routes with a `{context_id}`, and `/contexts/sync/{id}`, are refused with
  `403` outside the binding (`admin/client_key_scope.rs`). `POST /contexts/sync`
  with no id syncs every context and is refused outright.
- SSE: a bound key's session principal includes its binding, so it can neither
  adopt a node-owner session by `Last-Event-ID` nor be adopted by one.

Every check fails closed when the context or group cannot be resolved. A key
with no binding is unaffected.

## Sealed transport

`src/sealed.rs` lets a client encrypt its traffic end to end to the TD, so TLS
that ends outside the TD cannot read it. The client opens a session with a Noise
NK handshake (`sealed/session.rs`, via `snow`) to the node's X25519 transport
key, which `/tee/attest` binds into the quote on `bindTransportKey`. Requests are
sealed under the session and responses stream back in sealed frames. Six rules:

- **It wraps the router from outside** (`lib.rs`, `ServiceBuilder` around the
  merged router), not as a route. The opened request is handed back to the router
  and routed afresh, so auth, permissions and metrics see it as a direct request.
  CORS sits outside the envelope so the sealed response carries it.
- **The transport key authenticates; it never encrypts.** Session keys come from
  both sides' ephemeral keys, so dropping a session (`SESSION_LIFETIME`, or idle)
  is what gives forward secrecy. Never send data under the static key alone: the
  handshake refuses a message 1 with a payload for that reason.
- **The key is per process and never persisted.** A restart replaces it: a
  handshake to the old key gets `409 stale_transport_key`, so the client
  re-attests, and a request in a dropped session gets `409 unknown_session`. Never
  let a client take a new key from a response: whoever sent the response chose it.
- **Sealed-only mode is deny-by-default on exact paths.** With `[server.sealed]
  required`, `intercept` refuses any unsealed path not in `UNSEALED_ADMIN_PATHS`
  (under the admin prefix): the probes, `/tee/info` (the release a client
  verifies the quote against) and `/tee/attest`. Add to it only what a client
  needs before it can seal and what the hop already learns from the quote.
  Keep it that way: an allow-list of prefixes or patterns invites a
  normalization bypass. The inner request of an envelope is
  routed through `next` and never passes the check again.
- **In proxy auth mode, an opened request reaches only uncredentialed routes.**
  `InnerScope::Uncredentialed` (set in `lib.rs` when auth is not embedded):
  `merod` guards nothing itself there, and the proxy sees only `POST /sealed/v2`,
  so an opened request for any route outside `UNCREDENTIALED_ADMIN_PATHS` (plus
  `.../intents` with `delegated_access`) is refused inside the envelope with
  `sealed_route_unguarded`. Matched exactly, never by prefix. Keep the list in
  step with the public router in `admin/service.rs`: a route added there is not
  sealable in proxy mode until it is added here, which is the safe direction.
- **The wire format is shared with mero-js** (`src/sealed/sealed.ts`,
  `src/sealed/noise.ts`). The vectors in `sealed/tests.rs` are repeated there
  verbatim, and mero-js runs the handshake itself, so change both or neither.

## Execute authority

`/ws` and `/jsonrpc` are each several authorities behind one route. The route
check only says a token may reach the path: `/ws` is admitted on
`context:subscribe`, and `/jsonrpc` on any `context:execute`, while the context
and method a call names are in the body. So the guard hands the token's
permissions over as `GrantedPermissions`, and `execute_request` in
`src/execute.rs`, the one function both transports call, checks `may_execute`
(`context:execute[<ctx>,,<method>]`) before it reads anything.

- **Holds for the node owner too.** A client key is answered as the node
  owner, which skips the membership check, but its token was minted for some
  purpose. A token minted to watch events must not be spent on writes.
- **Proxy mode reads `X-Auth-Permissions`** (`src/proxy_permissions.rs`),
  which mero-auth's `/auth/validate` writes and the proxy forwards. It needs
  no opt-in because it can only narrow: a request naming none is answered as
  proxy mode always answered it. mero-auth comma-joins the list, and a
  permission's own parameters contain commas, so it is split only outside
  brackets.
- **Guard ran, no permissions** is refused, never read as unrestricted.
- **A method's own `Err` is mapped once**, by `execute::method_output`, into
  `ExecutionError::FunctionCallError`. JSON-RPC, WS, the delegated `/intents`
  and the account `/query` all use it; the two admin routes answer it with
  `admin::service::method_error_response` (`400`, JSON-RPC's `type`/`data` plus
  an `error` string). Never answer a method error as a success with a `null`
  return.

## Subscription authority

Subscribing is authorized once, at subscribe time, by the gates in
`src/ws/subscribe.rs` (`caller_may_observe_context`,
`authorize_group_subscriptions`). Keeping that decision true afterwards is
`src/subscription_grants.rs`, and there are three rules worth knowing before
touching either.
A caller with an identity must be a member in every auth mode, proxy included, since a proxy tenant is one caller among many.
Only the node owner and an identity-less caller on an auth-off node bypass this.

**The gate is the only authority.** A grant never *grants* anything; it only
records what a connection's subscriptions depend on, so a membership change can
decide whether to ask the gate again. Revocation re-runs the same two
predicates the subscribe path runs rather than restating the rule — a
separately-written revocation rule drifts, and the drift reads as a stream still
serving what a fresh subscribe would refuse.

**A grant watches ancestors, not just its own group.** Membership is inherited,
so a removal naming a parent revokes authority a descendant subscription holds.
Narrowing re-authorization to "connections that watch the named group" is only
sound because descendants watch their ancestors. If you change what `vouch`
records, that is the invariant to preserve: watching too many groups costs a
redundant re-derivation, watching too few is a leak.

**Un-vouched means stale, never trusted.** `Grants::default()` is stale, which
is what makes SSE session persistence safe: a resumed session's subscriptions
come back from its record but its grant does not, so `handle_node_events`
re-derives against live membership before serving anything, using the caller the
resuming request proved rather than one remembered from the record. Anything
`vouch` cannot resolve — an unreadable ancestry, a context whose owning group
will not resolve — also leaves the grant stale rather than narrowing what it
watches.

The caller identity itself (`EventCaller`) is deliberately not persisted on an
SSE session, for the same reason: a persisted identity would let a later
connection re-authorize as whoever the record remembers. Every authenticated
request re-stamps it.

## Common Gotchas

- The embedded admin dashboard is pinned in `build.rs` by version
  (`CALIMERO_WEBUI_VERSION`) and sha256 (`CALIMERO_WEBUI_SHA256`), and the
  archive is hash-checked before it is extracted. A local-development override
  (`CALIMERO_WEBUI_SRC`, or a non-default `_REPO`, `_VERSION` or `_ASSET`) skips
  the pinned hash: the build then verifies only if you pass your own
  `CALIMERO_WEBUI_SHA256`, and otherwise prints a "not hash-verified" warning. A
  local directory in `CALIMERO_WEBUI_SRC` is never hashed. Bumping the dashboard
  means updating the version and sha256 constants together
- Admin API requires authentication
- `/metrics` is never served on the `listen` addresses: it has a listener of its own
  (`server.metrics_listen`, loopback by default), outside the auth guard and the sealed transport
- JSON-RPC follows JSON-RPC 2.0 spec
- WebSocket requires context subscription
- A WebSocket upgrade from a browser (`Origin` present) is refused with `403` unless the
  origin is listed in `[server.cors] allowed_origins`, or equals a `Host` / `X-Forwarded-Host`
  that names this node (`BrowserOrigins`: loopback, the listen addresses, every address when
  it listens on an unspecified one, and the hosts of `allowed_origins`). A browser's `Host`
  is whatever name it resolved, so it proves nothing until the node recognises it. Clients
  that send no `Origin` are not browsers and are unaffected.
- SSE streams are per-context
- The `/sse/subscription` 200 is the client's readiness signal: once it lists a
  context, live events for it reach the stream. That holds only because
  `sse_handler` joins the node-event broadcast (`receive_events`) before it
  spawns `handle_node_events` - never move that call into the task, or a delta
  emitted before its first poll is lost (an ephemeral delta for good)
- A membership event re-authorizes only connections whose grants it can reach;
  if a subscription stops being revoked when it should be, suspect what `vouch`
  recorded, not the gate
- All responses use consistent error format
- `install-application` and `install-dev-application` start compiling the
  installed application's modules in the background
  (`ctx_client.precompile_application`) and answer without waiting, so the
  first context created from it finds the module compiled, or waits only for
  the rest of that compile. Never make the install await it: a compile takes
  seconds in a debug build, and a client timing out on the install only moves
  the timeout. A compile failure is logged
- Every request body is `deny_unknown_fields`; add a new request type to the list in `primitives/tests/deny_unknown_fields.rs`
