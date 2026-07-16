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
│   │   │   ├── attest.rs
│   │   │   ├── info.rs
│   │   │   └── verify_quote.rs
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
└── metrics.rs                # Prometheus metrics
primitives/                   # calimero-server-primitives
└── src/
    ├── lib.rs                # Shared types
    ├── jsonrpc.rs            # JSON-RPC types
    └── admin/mod.rs          # Admin API types
```

## API Endpoints

### Admin API

```
GET  /admin-api/contexts              # List contexts
POST /admin-api/contexts              # Create context
GET  /admin-api/contexts/:id          # Get context
DELETE /admin-api/contexts/:id        # Delete context

GET  /admin-api/applications          # List apps
POST /admin-api/install-application   # Install app
GET  /admin-api/applications/:id      # Get app

POST /admin-api/contexts/:id/join     # Join context
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
        .route("/contexts/:id", get(get_context).delete(delete_context))
}
```

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

Authentication handled via middleware in `src/auth.rs`:

- JWT token validation
- Node authorization
- Request signing verification

## Common Gotchas

- Admin API requires authentication
- JSON-RPC follows JSON-RPC 2.0 spec
- WebSocket requires context subscription
- SSE streams are per-context
- All responses use consistent error format
