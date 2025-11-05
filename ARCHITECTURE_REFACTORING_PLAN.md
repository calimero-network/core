# Architecture Refactoring Plan

## Overview

This document outlines a phased approach to refactoring the Calimero context management architecture to address the "God Object" pattern, tight coupling, and unclear boundaries.

**Timeline**: 4-6 weeks  
**Risk**: High (touches core functionality)  
**Strategy**: Incremental refactoring with backward compatibility

---

## Current Architecture Problems

### 1. The God Object: `ContextManager`

**Current State** (`crates/context/src/lib.rs`):
```rust
pub struct ContextManager {
    // Data storage
    datastore: Store,
    contexts: LruCache<ContextId, ContextMeta>,    // Context cache
    applications: BTreeMap<ApplicationId, Application>,  // App cache
    
    // External dependencies
    node_client: NodeClient,
    context_client: ContextClient,
    external_config: ExternalClientConfig,
    
    // Metrics
    metrics: Option<Metrics>,
}
```

**What it does**:
1. Context CRUD (create, delete, list)
2. Application management (load modules)
3. Context execution (call WASM methods)
4. Identity management (join, update)
5. External blockchain sync
6. Caching
7. Metrics

**11 Message Handlers**:
- `CreateContextRequest`
- `DeleteContextRequest`
- `ListContextsRequest`
- `GetContextRequest`
- `UpdateContextApplicationRequest`
- `ExecuteRequest`
- `GetContextIdentitiesRequest`
- `GetContextStorageRequest`
- `GetContextUsersRequest`
- `MutateContextStorage`
- `ExportContextStorage`

---

### 2. Tight Coupling

**Dependency Graph**:
```
ContextManager
    ├─> NodeClient (sync, events)
    ├─> ContextClient (database)
    ├─> ExternalClientConfig (blockchain)
    ├─> Store (RocksDB)
    └─> Metrics (prometheus)

NodeClient
    └─> ContextClient (circular!)
```

**Problems**:
- Can't test `ContextManager` without full node infrastructure
- Changes to `NodeClient` API break `ContextManager`
- Circular dependency context ↔ node makes reasoning hard

---

### 3. Unclear Boundaries

**Current Module Structure**:
```
crates/
├── context/
│   ├── src/
│   │   ├── lib.rs (ContextManager - 240 lines)
│   │   └── handlers/
│   │       ├── create_context.rs (400 lines)
│   │       ├── execute.rs (922 lines)
│   │       ├── join_context.rs (253 lines)
│   │       └── ...
│   └── primitives/  <- What's the difference?
│       └── src/
│           └── client.rs (ContextClient)
└── node/
    ├── src/
    │   └── sync/
    │       ├── manager.rs (1094 lines!)
    │       └── delta_request.rs
    └── primitives/  <- Same pattern, why?
        └── src/
            └── client.rs (NodeClient)
```

**Questions**:
- Why are clients in `primitives`?
- What's the layering supposed to be?
- Where should new features go?

---

## Proposed Architecture

### Phase 1: Extract Domain Services (Week 1-2)

**Goal**: Separate concerns without changing external APIs.

#### New Structure:
```
crates/context/src/
├── lib.rs                    # ContextManager (facade/coordinator)
├── repository.rs             # ContextRepository (DB CRUD)
├── application_manager.rs    # ApplicationManager (module loading)
├── executor.rs               # ContextExecutor (WASM runtime)
├── identity_manager.rs       # IdentityManager (member management)
└── handlers/                 # Message handlers (thin wrappers)
```

#### 1.1: `ContextRepository`
```rust
/// Low-level context storage operations.
/// 
/// Responsibilities:
/// - CRUD operations on contexts
/// - Cache management (LRU)
/// - Database persistence
pub struct ContextRepository {
    datastore: Store,
    cache: LruCache<ContextId, ContextMeta>,
    metrics: Option<RepositoryMetrics>,
}

impl ContextRepository {
    pub fn get(&mut self, id: &ContextId) -> Result<Option<&ContextMeta>> {
        // get_or_fetch_context logic
    }
    
    pub fn create(&mut self, context: Context) -> Result<ContextId> {
        // Create and cache
    }
    
    pub fn delete(&mut self, id: &ContextId) -> Result<()> {
        // Remove from cache and DB
    }
    
    pub fn list(&self) -> Vec<ContextId> {
        // List all contexts
    }
    
    pub fn update_dag_heads(&mut self, id: &ContextId, heads: Vec<Hash>) -> Result<()> {
        // Update DAG heads in cache and DB
    }
}
```

**Benefits**:
- Single responsibility: storage
- Easy to test (mock Store)
- Clear contract

#### 1.2: `ApplicationManager`
```rust
/// Application and module management.
///
/// Responsibilities:
/// - Load/compile WASM modules
/// - Cache compiled modules
/// - Application metadata
pub struct ApplicationManager {
    node_client: NodeClient,  // For fetching apps
    applications: LruCache<ApplicationId, Application>,
    compiled_modules: LruCache<ApplicationId, Module>,
    metrics: Option<ApplicationMetrics>,
}

impl ApplicationManager {
    pub async fn get_module(&mut self, id: &ApplicationId) -> Result<Module> {
        // Load or compile
    }
    
    pub fn get_application(&mut self, id: &ApplicationId) -> Result<&Application> {
        // Fetch metadata
    }
}
```

#### 1.3: `ContextExecutor`
```rust
/// WASM execution engine.
///
/// Responsibilities:
/// - Execute WASM methods
/// - Manage execution environment
/// - Apply deltas
pub struct ContextExecutor {
    repository: Arc<Mutex<ContextRepository>>,
    app_manager: Arc<Mutex<ApplicationManager>>,
    node_client: NodeClient,
}

impl ContextExecutor {
    pub async fn execute(
        &self,
        context_id: &ContextId,
        method: &str,
        args: Vec<u8>,
        caller: PublicKey,
    ) -> Result<ExecutionResult> {
        // execute.rs logic
    }
    
    pub async fn init_context(
        &self,
        context_id: &ContextId,
        params: Vec<u8>,
    ) -> Result<Hash> {
        // Initialization logic from create_context.rs
    }
}
```

#### 1.4: `IdentityManager`
```rust
/// Member identity and permissions.
///
/// Responsibilities:
/// - Identity CRUD
/// - Membership verification
/// - Key management
pub struct IdentityManager {
    context_client: ContextClient,
}

impl IdentityManager {
    pub fn get_identity(
        &self,
        context_id: &ContextId,
        member: &PublicKey,
    ) -> Result<Option<ContextIdentity>> {
        // Passthrough to context_client for now
    }
    
    pub fn update_identity(
        &self,
        context_id: &ContextId,
        identity: &ContextIdentity,
    ) -> Result<()> {
        // Update identity
    }
    
    pub fn is_member(&self, context_id: &ContextId, key: &PublicKey) -> Result<bool> {
        // Check membership
    }
}
```

#### 1.5: Refactored `ContextManager`
```rust
/// Coordinator for context operations.
///
/// Delegates to specialized services but maintains backward compatibility
/// with existing message-based API.
pub struct ContextManager {
    // Specialized services
    repository: Arc<Mutex<ContextRepository>>,
    app_manager: Arc<Mutex<ApplicationManager>>,
    executor: Arc<Mutex<ContextExecutor>>,
    identity_manager: Arc<Mutex<IdentityManager>>,
    
    // External config (might move later)
    external_config: ExternalClientConfig,
}

impl Handler<CreateContextRequest> for ContextManager {
    fn handle(&mut self, req: CreateContextRequest, _ctx: &mut Self::Context) 
        -> Self::Result 
    {
        // Thin wrapper - delegates to services
        let repository = self.repository.clone();
        let app_manager = self.app_manager.clone();
        let executor = self.executor.clone();
        
        ActorResponse::r#async(async move {
            // 1. Repository creates context
            // 2. AppManager loads module
            // 3. Executor runs init()
            // 4. Repository saves final state
        }.boxed())
    }
}
```

**Migration Strategy**:
1. Extract services alongside existing code
2. Update handlers to use services internally
3. Keep message-based API intact (backward compatibility)
4. Tests pass throughout

---

### Phase 2: Break Circular Dependencies (Week 3)

**Problem**: `NodeClient` ↔ `ContextClient` circular dependency.

#### Current:
```
node/primitives/client.rs:
    pub struct NodeClient {
        // Uses ContextClient internally
    }

context/primitives/client.rs:
    pub struct ContextClient {
        // Uses NodeClient for sync
    }
```

#### Solution: Dependency Inversion
```rust
// Define trait in shared location
pub trait SyncService {
    async fn sync(&self, context: Option<&ContextId>, peer: Option<&PeerId>) 
        -> Result<SyncResult>;
    
    async fn subscribe(&self, context: &ContextId) -> Result<()>;
}

// NodeClient implements it
impl SyncService for NodeClient { ... }

// ContextClient depends on trait, not concrete type
pub struct ContextClient {
    sync: Arc<dyn SyncService>,
    store: Store,
}
```

**Benefits**:
- Breaks circular dependency
- Can mock `SyncService` for testing
- Clear contract

---

### Phase 3: Clear Module Boundaries (Week 4)

**Reorganize** `crates/context/`:

```
crates/context/
├── Cargo.toml
├── src/
│   ├── lib.rs                  # Public API
│   │
│   ├── domain/                 # Core business logic
│   │   ├── mod.rs
│   │   ├── context.rs          # Context entity
│   │   ├── identity.rs         # Identity entity
│   │   └── execution.rs        # Execution logic
│   │
│   ├── services/               # Domain services
│   │   ├── mod.rs
│   │   ├── repository.rs       # ContextRepository
│   │   ├── application.rs      # ApplicationManager
│   │   ├── executor.rs         # ContextExecutor
│   │   └── identity.rs         # IdentityManager
│   │
│   ├── actor/                  # Actix integration (optional)
│   │   ├── mod.rs
│   │   ├── manager.rs          # ContextManager actor
│   │   └── handlers/           # Message handlers
│   │       ├── create.rs
│   │       ├── execute.rs
│   │       └── ...
│   │
│   └── client/                 # Client API
│       └── context_client.rs
│
└── primitives/                 # Move to `types/`?
    └── src/
        └── types.rs            # Shared types
```

**Rationale**:
- `domain/`: Pure business logic (no I/O)
- `services/`: Orchestration + infrastructure
- `actor/`: Actix-specific glue (optional layer)
- `client/`: API for other crates

---

### Phase 4: Remove Actor Pattern (Optional - Week 5-6)

**Current**: Everything is an actor with message passing.

**Proposed**: Direct async functions with Arc<Mutex<>> for shared state.

#### Before:
```rust
impl Handler<ExecuteRequest> for ContextManager {
    type Result = ActorResponse<ExecuteResponse>;
    
    fn handle(&mut self, req: ExecuteRequest, _ctx: &mut Context<Self>) 
        -> Self::Result 
    {
        ActorResponse::r#async(
            execute(...)
                .into_actor(self)
                .map_ok(...)
                .boxed()
        )
    }
}
```

#### After:
```rust
pub struct ContextService {
    repository: Arc<Mutex<ContextRepository>>,
    executor: Arc<Mutex<ContextExecutor>>,
}

impl ContextService {
    pub async fn execute(
        &self,
        request: ExecuteRequest,
    ) -> Result<ExecuteResponse> {
        let executor = self.executor.lock().await;
        executor.execute(...).await
    }
}
```

**Benefits**:
- Simpler mental model
- Easier debugging (no actor boundaries)
- Better async/await ergonomics
- Still concurrent (Arc<Mutex<>>)

**Tradeoffs**:
- Lose Actix supervision
- Manual locking (but we already have per-context locks)

**Decision**: Evaluate after Phase 3. May not be worth it.

---

## Implementation Plan

### Week 1: Extract ContextRepository
- [ ] Create `repository.rs`
- [ ] Move `get_or_fetch_context()` logic
- [ ] Add `create()`, `delete()`, `list()` methods
- [ ] Update `ContextManager` to use repository
- [ ] Add repository metrics
- [ ] Tests pass

### Week 2: Extract ApplicationManager & ContextExecutor
- [ ] Create `application_manager.rs`
- [ ] Move module loading logic
- [ ] Create `executor.rs`
- [ ] Move execution logic from `execute.rs`
- [ ] Update handlers to use new services
- [ ] Tests pass

### Week 3: Break Circular Dependencies
- [ ] Define `SyncService` trait
- [ ] Implement for `NodeClient`
- [ ] Update `ContextClient` to use trait
- [ ] Remove circular imports
- [ ] Tests pass

### Week 4: Reorganize Module Structure
- [ ] Create new directory structure
- [ ] Move files incrementally
- [ ] Update imports across codebase
- [ ] Update documentation
- [ ] Tests pass

### Week 5-6: Evaluate Actor Pattern (Optional)
- [ ] Prototype direct async API
- [ ] Benchmark performance difference
- [ ] Evaluate supervision needs
- [ ] Decide: keep actors or remove
- [ ] If removing: incremental migration

---

## Risk Mitigation

### 1. Backward Compatibility
- Keep existing message-based API throughout Phases 1-3
- Only internal implementation changes
- External users (server, CLI) see no difference

### 2. Incremental Migration
- Each phase is independently valuable
- Can pause between phases
- Can roll back if needed

### 3. Testing Strategy
- Existing E2E tests must pass after each phase
- Add integration tests for new services
- Add unit tests for pure domain logic

### 4. Feature Freeze
- No new features during refactoring weeks
- Bug fixes only
- Dedicated refactoring time

---

## Success Metrics

### Code Quality
- ✅ No circular dependencies
- ✅ Single Responsibility Principle per service
- ✅ < 300 lines per file
- ✅ Clear module boundaries

### Maintainability
- ✅ Can test services in isolation
- ✅ Can mock external dependencies
- ✅ New features have obvious home

### Performance
- ✅ No performance regression
- ✅ Memory usage stable or improved
- ✅ Latency stable or improved

---

## Alternative: Keep Current Architecture

**Pros**:
- No risk
- No time investment
- Works today

**Cons**:
- Technical debt grows
- Harder to onboard new devs
- Features take longer to implement
- Testing remains difficult

**Recommendation**: Proceed with refactoring. The codebase is at a tipping point where complexity is slowing development. Invest now to accelerate later.

---

## Decision Points

### After Week 2:
**Question**: Are services providing value?  
**Metrics**: Easier to test? Clearer code?  
**Decision**: Continue or rollback?

### After Week 3:
**Question**: Is dependency inversion working?  
**Metrics**: Can we mock easily? Tests isolated?  
**Decision**: Keep or revert?

### After Week 4:
**Question**: Is new structure clearer?  
**Metrics**: Easier to find code? Obvious where new code goes?  
**Decision**: Keep or revert?

### After Week 6:
**Question**: Remove actors?  
**Metrics**: Simpler code? Performance impact?  
**Decision**: Remove, keep, or hybrid?

---

## Conclusion

This is a significant refactoring with high risk but high reward. The incremental approach allows us to capture value at each phase while maintaining the option to stop or roll back.

**Recommended Start**: Week 1 (ContextRepository extraction) as a low-risk proof of concept. Evaluate results before committing to full plan.

