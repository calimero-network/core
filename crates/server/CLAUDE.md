# calimero-server - HTTP/WS/SSE Server

HTTP, WebSocket, and Server-Sent Events server for Admin API, JSON-RPC, and real-time subscriptions.

- **Crate**: `calimero-server`
- **Entry**: `src/lib.rs`
- **Frameworks**: axum (HTTP), tokio (async)

## Build & Test

```bash
cargo build -p calimero-server
cargo test -p calimero-server
```

## File Layout

```
src/
├── lib.rs
├── config.rs
├── admin/
│   ├── service.rs                     # Router setup
│   ├── handlers/
│   │   ├── context/
│   │   │   ├── create_context.rs
│   │   │   ├── delete_context.rs
│   │   │   ├── get_context.rs
│   │   │   ├── invite_to_context.rs
│   │   │   ├── join_context.rs
│   │   │   └── ...
│   │   ├── applications/
│   │   │   ├── install_application.rs
│   │   │   ├── list_applications.rs
│   │   │   └── ...
│   │   ├── identity/
│   │   ├── alias/
│   │   ├── blob.rs
│   │   ├── peers.rs
│   │   ├── proposals.rs
│   │   └── tee/
│   └── storage/
├── jsonrpc/execute.rs
├── ws/subscribe.rs
├── ws/unsubscribe.rs
├── sse/
│   ├── events.rs
│   └── handlers.rs
├── auth.rs                            # JWT & request signing middleware
└── metrics.rs                         # Prometheus metrics
primitives/src/
├── jsonrpc.rs                         # JSON-RPC 2.0 types
└── admin.rs                           # Admin API types
```

## API Endpoints

```
# Admin
GET    /admin-api/contexts
POST   /admin-api/contexts
GET    /admin-api/contexts/:id
DELETE /admin-api/contexts/:id
POST   /admin-api/contexts/:id/invite
POST   /admin-api/contexts/:id/join
GET    /admin-api/applications
POST   /admin-api/applications

# JSON-RPC 2.0
POST   /jsonrpc

# Real-time
WS     /ws
GET    /events          # SSE
```

## Patterns

### Admin Handler

```rust
use axum::extract::{Path, State};
use axum::Json;

pub async fn get_context(
    Path(context_id): Path<ContextId>,
    State(state): State<AppState>,
) -> Result<Json<ContextResponse>, ApiError> {
    // ...
}
```

### Router Setup

```rust
// src/admin/service.rs
use axum::routing::{delete, get, post};

pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/contexts", get(list_contexts).post(create_context))
        .route("/contexts/:id", get(get_context).delete(delete_context))
}
```

## Key Files

| File | Purpose |
|---|---|
| `src/lib.rs` | Server initialization |
| `src/admin/service.rs` | Admin router |
| `src/admin/handlers/context/create_context.rs` | Context creation |
| `src/jsonrpc/execute.rs` | JSON-RPC dispatch |
| `src/ws/subscribe.rs` | WS subscriptions |
| `src/auth.rs` | Auth middleware |
| `primitives/src/jsonrpc.rs` | JSON-RPC types |

## Quick Search

```bash
rg -n "pub async fn" src/admin/handlers/
rg -n "\.route\(" src/
rg -n "pub struct.*Request" primitives/src/
rg -n "pub async fn" src/auth.rs
```

## Gotchas

- Admin API requires authentication (JWT or signed request)
- JSON-RPC follows JSON-RPC 2.0 spec exactly
- WebSocket requires context subscription before receiving events
- SSE streams are scoped per context
