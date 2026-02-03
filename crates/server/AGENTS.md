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
├── admin.rs                  # Admin API module parent
├── admin/
│   ├── handlers/             # Admin handlers
│   │   ├── application.rs    # App management
│   │   ├── context.rs        # Context management
│   │   ├── identity.rs       # Identity operations
│   │   └── ...
│   ├── service.rs            # Admin service setup
│   └── ...
├── jsonrpc.rs                # JSON-RPC module
├── jsonrpc/
│   └── request.rs            # JSON-RPC request handling
├── ws.rs                     # WebSocket module
├── ws/
│   ├── subscribe.rs          # WS subscription
│   └── unsubscribe.rs        # WS unsubscription
├── sse/
│   ├── mod.rs                # SSE module
│   └── ...
├── auth.rs                   # Authentication middleware
└── metrics.rs                # Prometheus metrics
primitives/                   # calimero-server-primitives
├── src/
│   ├── lib.rs                # Shared types
│   ├── jsonrpc.rs            # JSON-RPC types
│   └── admin.rs              # Admin API types
```

## API Endpoints

### Admin API

```
GET  /admin-api/contexts              # List contexts
POST /admin-api/contexts              # Create context
GET  /admin-api/contexts/:id          # Get context
DELETE /admin-api/contexts/:id        # Delete context

GET  /admin-api/applications          # List apps
POST /admin-api/applications          # Install app
GET  /admin-api/applications/:id      # Get app

POST /admin-api/contexts/:id/invite   # Invite member
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

| File                                | Purpose            |
| ----------------------------------- | ------------------ |
| `src/lib.rs`                        | Server setup       |
| `src/admin/service.rs`              | Admin router setup |
| `src/admin/handlers/context.rs`     | Context handlers   |
| `src/admin/handlers/application.rs` | App handlers       |
| `src/jsonrpc/request.rs`            | JSON-RPC handling  |
| `src/ws/subscribe.rs`               | WS subscriptions   |
| `primitives/src/jsonrpc.rs`         | JSON-RPC types     |

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
