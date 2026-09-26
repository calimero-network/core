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
│   │   │   └── info.rs
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
├── sealed.rs                 # Sealed transport: /sealed/v2 envelope, wraps the router
├── sealed/
│   └── session.rs            # Noise NK handshake and the sessions it opens
└── metrics.rs                # Prometheus metrics
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
the blob ids of every other one.

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

When adding a new `.route(...)`, regenerate `crates/server/endpoints.json` via `UPDATE_MANIFEST=1 cargo test -p calimero-server --test route_manifest`, and cover the endpoint with a mero-js e2e hit or a reasoned entry in coverage-baseline.json.

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
| `403` + `X-Auth-Error: invalid_proof` | `NotServed` — sound chain, but this node serves no delegated access and the caller is not its own account |
| `401`, no header | no credential at all |

Checks run cheapest-first (covers → freshness → node binding → one signature →
the certificate chain's *n*), so a stale or misaddressed proof costs almost
nothing to refuse.

**Only the session link carries a node**, so a two-link (device-signed) proof has
no node binding and is replayable at any node serving delegated access until it
expires. A deployment relying on that binding must require the session link. See
`CallerProof::verify`'s own docs, which state the asymmetry.

`delegated-proof.yml` drives all of this against real nodes; `delegated-session.yml`
covers the token path.

## Sealed transport

`src/sealed.rs` lets a client encrypt its traffic end to end to the TD, so TLS
that ends outside the TD cannot read it. The client opens a session with a Noise
NK handshake (`sealed/session.rs`, via `snow`) to the node's X25519 transport
key, which `/tee/attest` binds into the quote on `bindTransportKey`. Requests are
sealed under the session and responses stream back in sealed frames. Four rules:

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
- **The wire format is shared with mero-js** (`src/sealed/sealed.ts`,
  `src/sealed/noise.ts`). The vectors in `sealed/tests.rs` are repeated there
  verbatim, and mero-js runs the handshake itself, so change both or neither.

## Subscription authority

Subscribing is authorized once, at subscribe time, by the gates in
`src/ws/subscribe.rs` (`caller_may_observe_context`,
`authorize_group_subscriptions`). Keeping that decision true afterwards is
`src/subscription_grants.rs`, and there are three rules worth knowing before
touching either.

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

- Admin API requires authentication
- JSON-RPC follows JSON-RPC 2.0 spec
- WebSocket requires context subscription
- SSE streams are per-context
- A membership event re-authorizes only connections whose grants it can reach;
  if a subscription stops being revoked when it should be, suspect what `vouch`
  recorded, not the gate
- All responses use consistent error format
- Every request body is `deny_unknown_fields`; add a new request type to the list in `primitives/tests/deny_unknown_fields.rs`
