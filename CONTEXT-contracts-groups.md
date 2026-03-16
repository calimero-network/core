# Context Groups: Contract Changes Context Document

**Purpose**: Self-contained context document for any agent/session to understand the current contract state, what has been done, and what remains for the Context Groups feature implementation.

**Last Updated**: 2026-02-19 (Phase 5 complete)

---

## Table of Contents

1. [Project Overview](#1-project-overview)
2. [Repository Layout](#2-repository-layout)
3. [Current Contract Architecture](#3-current-contract-architecture)
4. [Key Existing Data Structures](#4-key-existing-data-structures)
5. [New Data Structures to Add](#5-new-data-structures-to-add)
6. [Types Crate Dependency](#6-types-crate-dependency)
7. [Migration System](#7-migration-system)
8. [Testing Infrastructure](#8-testing-infrastructure)
9. [Phased Implementation Plan](#9-phased-implementation-plan)
10. [Progress Tracker](#10-progress-tracker)

---

## 1. Project Overview

### What is Calimero?

Calimero is a decentralized infrastructure for privacy-preserving, multi-party computation. It uses **contexts** as isolated execution environments, each with their own:
- Application (WASM module)
- Merkle state tree
- DAG delta history
- On-chain registration

### What are Context Groups?

Context Groups introduce a **hierarchical relationship** between contexts. A group:
- Contains multiple related contexts (e.g., DMs, channels in a workspace)
- Enables single-trigger version propagation (upgrade once, all contexts converge)
- Provides workspace-level user management
- Allows multi-tenant isolation (same app used by different organizations)

### Why Context Groups?

For a 1000-member organization with ~5100 contexts, upgrading requires 5100 independent operations without groups. Context Groups reduce this to a single admin action that propagates to all member contexts.

---

## 2. Repository Layout

### Contracts Repository (`/contracts/`)

```
contracts/
├── Cargo.toml                          # Workspace root
├── Cargo.lock
├── contracts/
│   └── near/
│       ├── context-config/             # Main context configuration contract
│       │   ├── Cargo.toml
│       │   ├── build.sh
│       │   ├── src/
│       │   │   ├── lib.rs              # Contract state, structs, Prefix enum
│       │   │   ├── mutate.rs           # Mutation methods (mutate())
│       │   │   ├── query.rs            # Query methods
│       │   │   ├── guard.rs            # Permission guard system
│       │   │   ├── invitation.rs       # Open invitation system
│       │   │   ├── sys.rs              # System utilities
│       │   │   └── sys/
│       │   │       ├── migrations.rs   # Migration macro and registry
│       │   │       └── migrations/
│       │   │           ├── 01_guard_revisions.rs
│       │   │           ├── 02_nonces.rs
│       │   │           └── 03_context_groups.rs
│       │   └── tests/
│       │       ├── sandbox.rs          # Main integration tests
│       │       ├── migrations.rs       # Migration tests
│       │       ├── open_invitations.rs # Invitation tests
│       │       └── groups.rs           # Group CRUD tests (Phase 2)
│       │
│       ├── context-proxy/              # Proxy contract for governance
│       │   ├── Cargo.toml
│       │   ├── src/
│       │   │   ├── lib.rs              # ProxyContract state
│       │   │   ├── mutate.rs           # Proposal execution
│       │   │   └── ext_config.rs       # External interface to context-config
│       │   ├── mock/                   # Mock contract for testing
│       │   └── tests/
│       │       └── sandbox.rs
│       │
│       └── registry/                   # Application registry contract
```

### Shared Types Crate (`/core/crates/context/config/`)

```
core/crates/context/config/
├── Cargo.toml
├── src/
│   ├── lib.rs                          # Request, RequestKind, ProposalAction, etc.
│   ├── types.rs                        # ContextId, SignerId, Application, etc.
│   ├── repr.rs                         # Repr<T> type for serialization
│   └── client/                         # Client implementations
│       ├── env/
│       │   ├── config/                 # Context-config client
│       │   └── proxy/                  # Context-proxy client
│       └── ...
```

---

## 3. Current Contract Architecture

### context-config Contract

The main contract that manages:
- Context registration and lifecycle
- Member management
- Application version tracking
- Proxy contract deployment

### context-proxy Contract

Per-context governance contract that handles:
- Proposal creation and approval
- Action execution (transfers, function calls, config changes)
- Multi-sig approval workflow

### Relationship

```
┌─────────────────────────────────────────────────────────────┐
│                     context-config                           │
│  (Single deployment, manages all contexts)                   │
│                                                             │
│  contexts: Map<ContextId, Context>                          │
│  proxy_code: LazyOption<Vec<u8>>                            │
│                                                             │
│  Methods: mutate(), application(), members(),               │
│           group(), is_group_admin(),                        │
│           group_contexts(), context_group(),                │
│           proxy_register_in_group(),                        │
│           proxy_unregister_from_group(), ...                │
└─────────────────────────────────────────────────────────────┘
                              │
                              │ deploys per-context
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                   context-proxy (per context)                │
│                                                             │
│  context_id: ContextId                                       │
│  proposals: Map<ProposalId, Proposal>                        │
│  approvals: Map<ProposalId, Set<SignerId>>                   │
│                                                             │
│  Methods: mutate(), proposals(), get_confirmations_count()   │
└─────────────────────────────────────────────────────────────┘
```

---

## 4. Key Existing Data Structures

### ContextConfigs (Contract State)

**File**: `contracts/contracts/near/context-config/src/lib.rs`

```rust
pub struct ContextConfigs {
    contexts: IterableMap<ContextId, Context>,
    config: Config,
    proxy_code: LazyOption<Vec<u8>>,
    proxy_code_hash: LazyOption<CryptoHash>,
    next_proxy_id: u64,
    groups: IterableMap<ContextGroupId, OnChainGroupMeta>,          // Added Phase 1
    context_group_refs: IterableMap<ContextId, ContextGroupId>,     // Added Phase 1
}
```

### Context

**File**: `contracts/contracts/near/context-config/src/lib.rs`

```rust
struct Context {
    pub application: Guard<Application<'static>>,
    pub members: Guard<IterableSet<ContextIdentity>>,
    pub member_nonces: IterableMap<ContextIdentity, u64>,
    pub proxy: Guard<AccountId>,
    pub used_open_invitations: Guard<IterableSet<CryptoHash>>,
    pub commitments_open_invitations: IterableMap<CryptoHash, BlockHeight>,
    pub group_id: Option<ContextGroupId>,                           // Added Phase 1
}
```

### OnChainGroupMeta (Added Phase 1)

**File**: `contracts/contracts/near/context-config/src/lib.rs`

```rust
pub struct OnChainGroupMeta {
    pub app_key: AppKey,
    pub target_application: Application<'static>,
    pub admins: IterableSet<SignerId>,
    pub member_count: u64,
    pub context_count: u64,
}
```

### Prefix Enum (Storage Keys)

**File**: `contracts/contracts/near/context-config/src/lib.rs`

```rust
enum Prefix {
    Contexts = 1,
    Members(ContextId) = 2,
    Privileges(PrivilegeScope) = 3,
    ProxyCode = 4,
    ProxyCodeHash = 5,
    MemberNonces(ContextId) = 6,
    UsedOpenInvitations(ContextId) = 7,
    CommitmentsOpenInvitations(ContextId) = 8,
    Groups = 9,                              // Added Phase 1
    GroupAdmins(ContextGroupId) = 10,        // Added Phase 1
    ContextGroupRefs = 11,                   // Added Phase 1
}
```

### RequestKind (from shared types crate)

**File**: `core/crates/context/config/src/lib.rs`

```rust
pub enum RequestKind<'a> {
    Context(ContextRequest<'a>),
    Group(GroupRequest<'a>),    // Added in Prerequisite phase
}
```

### ContextRequestKind

**File**: `core/crates/context/config/src/lib.rs:70-104`

```rust
pub enum ContextRequestKind<'a> {
    Add { author_id, application },
    UpdateApplication { application },
    AddMembers { members },
    RemoveMembers { members },
    CommitOpenInvitation { commitment_hash, expiration_block_height },
    RevealOpenInvitation { payload },
    Grant { capabilities },
    Revoke { capabilities },
    UpdateProxyContract,
}
```

### ProposalAction

**File**: `core/crates/context/config/src/lib.rs:120-159`

```rust
pub enum ProposalAction {
    ExternalFunctionCall { receiver_id, method_name, args, deposit },
    Transfer { receiver_id, amount },
    SetNumApprovals { num_approvals },
    SetActiveProposalsLimit { active_proposals_limit },
    SetContextValue { key, value },
    DeleteProposal { proposal_id },
}
```

### ProxyContract

**File**: `contracts/contracts/near/context-proxy/src/lib.rs:38-48`

```rust
pub struct ProxyContract {
    pub context_id: ContextId,
    pub context_config_account_id: AccountId,
    pub num_approvals: u32,
    pub proposals: IterableMap<Repr<ProposalId>, Proposal>,
    pub approvals: IterableMap<Repr<ProposalId>, HashSet<SignerId>>,
    pub num_proposals_pk: IterableMap<SignerId, u32>,
    pub active_proposals_limit: u32,
    pub context_storage: IterableMap<Box<[u8]>, Box<[u8]>>,
    pub code_size: (u64, Option<u64>),
}
```

---

## 5. New Data Structures to Add

### ContextGroupId (shared types crate)

```rust
#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct ContextGroupId(Identity);

impl ReprBytes for ContextGroupId {
    type EncodeBytes<'a> = [u8; 32];
    type DecodeBytes = [u8; 32];
    type Error = LengthMismatch;
    // ... standard implementation
}
```

### GroupRequest and GroupRequestKind (shared types crate)

```rust
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupRequest<'a> {
    pub group_id: Repr<ContextGroupId>,
    #[serde(borrow, flatten)]
    pub kind: GroupRequestKind<'a>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "scope", content = "params")]
pub enum GroupRequestKind<'a> {
    Create {
        app_key: Repr<AppKey>,
        #[serde(borrow)]
        target_application: Application<'a>,
    },
    Delete,
    AddMembers {
        members: Cow<'a, [Repr<SignerId>]>,
    },
    RemoveMembers {
        members: Cow<'a, [Repr<SignerId>]>,
    },
    RegisterContext {
        context_id: Repr<ContextId>,
    },
    UnregisterContext {
        context_id: Repr<ContextId>,
    },
    SetTargetApplication {
        #[serde(borrow)]
        target_application: Application<'a>,
    },
}
```

### Extended RequestKind (shared types crate)

```rust
pub enum RequestKind<'a> {
    Context(ContextRequest<'a>),
    Group(GroupRequest<'a>),  // NEW
}
```

### OnChainGroupMeta (contract)

```rust
#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct OnChainGroupMeta {
    pub app_key: AppKey,
    pub target_application: Application<'static>,
    pub admins: IterableSet<SignerId>,
    pub member_count: u64,
    pub context_count: u64,
}
```

### Extended ContextConfigs (contract)

```rust
pub struct ContextConfigs {
    contexts: IterableMap<ContextId, Context>,
    config: Config,
    proxy_code: LazyOption<Vec<u8>>,
    proxy_code_hash: LazyOption<CryptoHash>,
    next_proxy_id: u64,
    // NEW
    groups: IterableMap<ContextGroupId, OnChainGroupMeta>,
    context_group_refs: IterableMap<ContextId, ContextGroupId>,
}
```

### Extended Context (contract)

```rust
struct Context {
    // ... existing 6 fields ...
    pub group_id: Option<ContextGroupId>,  // NEW
}
```

### New Prefix Variants (contract)

```rust
enum Prefix {
    // ... existing 1-8 ...
    Groups = 9,
    GroupAdmins(ContextGroupId) = 10,
    ContextGroupRefs = 11,
}
```

### GroupInfoResponse (contract query DTO)

**File**: `contracts/contracts/near/context-config/src/query.rs`

```rust
#[derive(Debug, Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct GroupInfoResponse {
    pub app_key: Repr<AppKey>,
    pub target_application: Application<'static>,
    pub member_count: u64,
    pub context_count: u64,
}
```

### Group Mutation Flow (Phase 2 + Phase 3 + Phase 4)

Group mutations flow through the existing `mutate()` entry point via `RequestKind::Group`:

```
Client → Signed<Request> → mutate() → RequestKind::Group → handle_group_request()
                                        ├── GroupRequestKind::Create → create_group()
                                        ├── GroupRequestKind::Delete → delete_group()
                                        ├── GroupRequestKind::AddMembers → add_group_members()
                                        ├── GroupRequestKind::RemoveMembers → remove_group_members()
                                        ├── GroupRequestKind::RegisterContext → register_context_in_group()
                                        ├── GroupRequestKind::UnregisterContext → unregister_context_from_group()
                                        └── GroupRequestKind::SetTargetApplication → set_group_target()
```

Key design decisions:
- **No nonce checking** for group operations (nonces are per-context per-member; groups are context-independent)
- **Signature verification** still occurs via `Signed<Request>::parse()` (the signer must hold the private key matching `signer_id`)
- **Member identities stored off-chain** for privacy; only `member_count` is tracked on-chain
- **Admin set cleanup** on `delete_group()` -- the `IterableSet` is explicitly cleared before removal to avoid orphaned storage

### Context-Group Integration Flow (Phase 3)

Register/unregister contexts in groups:

- `register_context_in_group()`:
  - Validates group exists and signer is admin
  - Validates context exists and has no existing group (`context.group_id.is_none()`)
  - Sets `context.group_id = Some(group_id)`
  - Increments `group.context_count`
  - Inserts into `context_group_refs` map
- `unregister_context_from_group()`:
  - Validates group exists and signer is admin
  - Validates context exists and belongs to the specified group
  - Clears `context.group_id`
  - Decrements `group.context_count`
  - Removes from `context_group_refs` map

Query methods:
- `group_contexts(group_id, offset, length)` -- scans `context_group_refs` filtering by `group_id` with pagination (scan approach; optimize later if gas becomes a concern)
- `context_group(context_id)` -- O(1) lookup in `context_group_refs` returning `Option<Repr<ContextGroupId>>`

### Target Application Management Flow (Phase 4)

- `set_group_target()`:
  - Validates group exists and signer is admin
  - Replaces `group.target_application` with new `Application` value
  - Logs old and new application IDs for audit trail
  - This is the on-chain half of upgrade propagation; actual context-by-context upgrade orchestration happens off-chain in the node runtime

### Extended ProposalAction (shared types crate)

```rust
pub enum ProposalAction {
    // ... existing variants ...
    RegisterInGroup { group_id: Repr<ContextGroupId> },
    UnregisterFromGroup { group_id: Repr<ContextGroupId> },
}
```

### Proxy-Initiated Group Operations (Phase 5)

The proxy contract cannot forge `Signed<Request>` (no private key access), so context-config exposes two dedicated methods for proxy-initiated group operations:

```
Proxy governance (proposal approved) → execute_proposal()
  ├── RegisterInGroup { group_id }
  │     → cross-contract call → context-config.proxy_register_in_group(context_id, group_id)
  └── UnregisterFromGroup { group_id }
        → cross-contract call → context-config.proxy_unregister_from_group(context_id, group_id)
```

**Authorization model**: The proxy's multi-sig governance acts as authorization (proposal must reach quorum). Context-config verifies the caller is the registered proxy for the given context (`env::predecessor_account_id() == context.proxy`). No group-admin signature is required for proxy-initiated operations.

**Key methods on context-config**:
- `proxy_register_in_group(context_id, group_id)` -- verifies caller is proxy, checks group exists and context has no group, sets `context.group_id`, increments `group.context_count`, inserts into `context_group_refs`
- `proxy_unregister_from_group(context_id)` -- verifies caller is proxy, reads `context.group_id`, clears it, decrements `group.context_count`, removes from `context_group_refs`

**Key changes to context-proxy**:
- `ext_config.rs` -- added `proxy_register_in_group` and `proxy_unregister_from_group` to the `ConfigContract` ext_contract trait
- `mutate.rs` -- `RegisterInGroup` and `UnregisterFromGroup` are classified as promise actions (alongside `ExternalFunctionCall` and `Transfer`) and create cross-contract calls using `config_contract::ext()` with `with_static_gas(gas_per_call)`

---

## 6. Types Crate Dependency

### Current Setup

The contracts import types from `calimero-context-config` crate via git:

**File**: `contracts/Cargo.toml:18`

```toml
calimero-context-config = { git = "https://github.com/calimero-network/core", tag = "0.10.0-rc.1" }
```

### Required Additions to Shared Types

Before contract work can proceed, add to `core/crates/context/config/`:

1. **In `types.rs`**:
   - `ContextGroupId` type (32-byte hash newtype)
   - `AppKey` type (if not already present)

2. **In `lib.rs`**:
   - `GroupRequest<'a>` struct
   - `GroupRequestKind<'a>` enum
   - `RequestKind::Group(GroupRequest)` variant
   - `ProposalAction::RegisterInGroup` variant
   - `ProposalAction::UnregisterFromGroup` variant

### Current Setup (Active)

A `[patch]` section was added to `contracts/Cargo.toml` in Phase 1 to use the local types:

```toml
[patch."https://github.com/calimero-network/core"]
calimero-context-config = { path = "../core/crates/context/config" }
```

### Future Options

1. **Branch reference** (for CI/testing):
   ```toml
   calimero-context-config = { git = "https://github.com/calimero-network/core", branch = "feat/context-groups" }
   ```

2. **New tag** (for release):
   ```toml
   calimero-context-config = { git = "https://github.com/calimero-network/core", tag = "0.11.0" }
   ```

---

## 7. Migration System

### How Migrations Work

The contract uses a feature-gated, mutually exclusive migration system:

**File**: `contracts/contracts/near/context-config/src/sys/migrations.rs`

```rust
migrations! {
    "01_guard_revisions" => "migrations/01_guard_revisions.rs",
    "02_nonces"          => "migrations/02_nonces.rs",
    "03_context_groups"  => "migrations/03_context_groups.rs",  // Added Phase 1
}
```

Key characteristics:
- Migrations are **mutually exclusive** (only one can be active at a time)
- Requires `migrations` feature AND specific migration feature
- Each migration defines `OldState` struct and `migrate()` function

### Migration Template (from 02_nonces.rs)

```rust
// 1. Define old state structure
#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct OldContextConfigs {
    contexts: IterableMap<ContextId, OldContext>,
    config: Config,
    proxy_code: LazyOption<Vec<u8>>,
    next_proxy_id: u64,
}

#[derive(Debug)]
#[near(serializers = [borsh])]
struct OldContext {
    pub application: Guard<Application<'static>>,
    pub members: Guard<IterableSet<ContextIdentity>>,
    #[borsh(deserialize_with = "skipped")]
    pub member_nonces: IterableMap<ContextIdentity, u64>,
    pub proxy: Guard<AccountId>,
}

// 2. Implement migrate() function
pub fn migrate() {
    let mut state = env::state_read::<OldContextConfigs>().expect("failed to read state");
    
    for (context_id, context) in state.contexts.iter_mut() {
        env::log_str(&format!("Migrating context `{}`", Repr::new(*context_id)));
        // Perform migration logic...
    }
}
```

### Running Migrations

```bash
# Build with migration feature
cargo build --release --features "migrations,03_context_groups"

# Deploy and call migrate()
near call <contract> migrate --accountId <admin>
```

---

## 8. Testing Infrastructure

### Test Framework

- **Framework**: `near-workspaces` (sandbox testing)
- **Runtime**: `tokio` async runtime
- **Location**: `contracts/contracts/near/context-config/tests/`

### Test Structure (from sandbox.rs)

```rust
#[tokio::test]
async fn main() -> eyre::Result<()> {
    // 1. Setup sandbox
    let worker = near_workspaces::sandbox().await?;
    let wasm = fs::read("res/calimero_context_config_near.wasm").await?;
    let contract = worker.dev_deploy(&wasm).await?;

    // 2. Set proxy code
    let context_proxy_blob = fs::read("../context-proxy/res/calimero_context_proxy_near.wasm").await?;
    contract.call("set_proxy_code").args(context_proxy_blob).max_gas().transact().await?;

    // 3. Create test accounts
    let root_account = worker.root_account()?;
    let node1 = root_account.create_subaccount("node1")
        .initial_balance(NearToken::from_near(30))
        .transact().await?.into_result()?;

    // 4. Generate test keys
    let mut rng = rand::thread_rng();
    let alice_cx_sk = SigningKey::from_bytes(&rng.gen());
    let alice_cx_pk = alice_cx_sk.verifying_key();
    let alice_cx_id = alice_cx_pk.to_bytes().rt()?;

    // 5. Create signed requests
    let request = Request::new(
        context_id.rt()?,
        RequestKind::Context(ContextRequest::new(
            Repr::new(context_id),
            ContextRequestKind::Add {
                author_id: Repr::new(alice_cx_id),
                application,
            },
        )),
        0,  // nonce
    );
    let signed = Signed::new(&request, |b| context_secret.sign(b))?;

    // 6. Call contract
    node1.call(contract.id(), "mutate")
        .args_json(json!(signed))
        .max_gas()
        .transact().await?.into_result()?;

    // 7. Query and verify
    let members: Vec<Repr<ContextIdentity>> = contract
        .view("members")
        .args_json(json!({ "context_id": Repr::new(context_id), "offset": 0, "length": 10 }))
        .await?.json()?;
    
    assert_eq!(members.len(), 1);
    Ok(())
}
```

### Helper Patterns

```rust
// Fetch nonce helper
async fn fetch_nonce(
    contract: &Contract,
    context_id: Repr<ContextId>,
    member_pk: Repr<ContextIdentity>,
) -> eyre::Result<Option<u64>> {
    let res: Option<u64> = contract
        .view("fetch_nonce")
        .args_json(json!({ "context_id": context_id, "member_id": member_pk }))
        .await?.json()?;
    Ok(res)
}
```

### Building Test Artifacts

```bash
# Build context-config contract
cd contracts/contracts/near/context-config
./build.sh

# Build context-proxy contract
cd ../context-proxy
./build.sh

# Run tests
cd ../context-config
cargo test
```

---

## 9. Phased Implementation Plan

| Phase | Description | Files Changed | Status |
|-------|-------------|---------------|--------|
| **0** | Create context document | `CONTEXT-contracts-groups.md` | ✅ Complete |
| **Prereq** | Add shared types | `core/crates/context/config/src/{lib.rs,types.rs}` | ✅ Complete |
| **1** | Foundation: Storage schema + Migration | `lib.rs`, `03_context_groups.rs`, `migrations.rs`, `Cargo.toml`, `mutate.rs`, `sys.rs`, `tests/migrations.rs` | ✅ Complete |
| **2** | Group CRUD + Membership | `mutate.rs`, `query.rs`, `tests/groups.rs` | ✅ Complete |
| **3** | Context-Group Integration | `mutate.rs`, `query.rs`, `tests/groups.rs` | ✅ Complete |
| **4** | Target Application Management | `mutate.rs`, `tests/groups.rs` | ✅ Complete |
| **5** | Proxy Proposal Actions | `context-proxy/src/mutate.rs`, `context-proxy/src/ext_config.rs`, `context-config/src/mutate.rs` | ✅ Complete |

### Phase Dependencies

```
Prereq (shared types) ─┬─► Phase 1 ─► Phase 2 ─► Phase 3 ─► Phase 4
                       │                              │
                       └──────────────────────────────┴─► Phase 5
```

---

## 10. Progress Tracker

### Completed

- [x] **Phase 0**: Context document created
- [x] **Prerequisite**: Shared types crate updates
  - [x] Add `ContextGroupId` type to `types.rs`
  - [x] Add `AppKey` type to `types.rs`
  - [x] Add `GroupRequest<'a>` struct to `lib.rs`
  - [x] Add `GroupRequestKind<'a>` enum to `lib.rs`
  - [x] Add `RequestKind::Group` variant
  - [x] Add `ProposalAction::RegisterInGroup` variant
  - [x] Add `ProposalAction::UnregisterFromGroup` variant

- [x] **Phase 1**: Foundation
  - [x] Add `OnChainGroupMeta` struct to `lib.rs`
  - [x] Extend `ContextConfigs` with `groups` and `context_group_refs` fields
  - [x] Extend `Context` with `group_id: Option<ContextGroupId>`
  - [x] Add new `Prefix` variants: `Groups = 9`, `GroupAdmins(ContextGroupId) = 10`, `ContextGroupRefs = 11`
  - [x] Update `Default` impl for `ContextConfigs` (init empty maps)
  - [x] Create `03_context_groups.rs` migration (skipped deserializers + `env::state_write`)
  - [x] Register migration in `migrations.rs`
  - [x] Add `03_context_groups` feature gate in `Cargo.toml`
  - [x] Handle `RequestKind::Group` in `mutate.rs` (panic stub for Phase 2)
  - [x] Add `group_id: None` to Context construction in `add_context()`
  - [x] Add group/ref cleanup in `sys.rs` `erase()`
  - [x] Add `[patch]` in workspace `Cargo.toml` for local `calimero-context-config` types
  - [x] Write migration roundtrip test in `tests/migrations.rs`

- [x] **Phase 2**: Group CRUD + Membership
  - [x] Replace `RequestKind::Group` panic stub with dispatch to `handle_group_request()`
  - [x] Implement `create_group()` -- validates uniqueness, creates `OnChainGroupMeta` with caller as first admin, `member_count = 1`, `context_count = 0`
  - [x] Implement `delete_group()` -- admin-only, requires `context_count == 0`, cleans up admins `IterableSet`
  - [x] Implement `add_group_members()` -- admin-only, increments `member_count` per member (identities stored off-chain)
  - [x] Implement `remove_group_members()` -- admin-only, decrements `member_count` with underflow guard
  - [x] Add `GroupInfoResponse` struct (JSON-serializable DTO for `OnChainGroupMeta`)
  - [x] Add `group()` query -- returns `Option<GroupInfoResponse>` with app_key, target_application, member/context counts
  - [x] Add `is_group_admin()` query -- returns `bool`, safe on nonexistent groups (returns `false`)
  - [x] Write integration tests (`tests/groups.rs`): create+query, duplicate rejection, add/remove members, non-admin rejection, delete, nonexistent group queries

- [x] **Phase 3**: Context-Group Integration
  - [x] Implement `register_context_in_group()` -- admin-only, validates context exists and has no group, sets `context.group_id`, increments `context_count`, inserts into `context_group_refs`
  - [x] Implement `unregister_context_from_group()` -- admin-only, validates context belongs to specified group, clears `context.group_id`, decrements `context_count`, removes from `context_group_refs`
  - [x] Add `group_contexts(group_id, offset, length)` query -- scan `context_group_refs` with filter and pagination
  - [x] Add `context_group(context_id)` query -- O(1) reverse lookup in `context_group_refs`
  - [x] Write integration tests (`tests/groups.rs`): register+query, double-register rejected, unregister+cleanup, non-admin rejection, pagination, delete-group-with-contexts rejected, wrong-group unregister rejected, ungrouped context returns None

- [x] **Phase 4**: Target Application Management
  - [x] Replace `SetTargetApplication` panic stub with dispatch to `set_group_target()`
  - [x] Implement `set_group_target()` -- admin-only, updates `group.target_application`, logs old and new application IDs
  - [x] Write integration tests (`tests/groups.rs`): set target + verify update, non-admin rejection, nonexistent group rejection, verify original app unchanged after rejected update

- [x] **Phase 5**: Proxy Proposal Actions
  - [x] Add `proxy_register_in_group()` method to context-config -- callable by proxy contracts, verifies `predecessor_account_id` matches the context's proxy, performs registration without admin signature (proxy governance acts as authorization)
  - [x] Add `proxy_unregister_from_group()` method to context-config -- callable by proxy contracts, verifies caller is the proxy, clears group membership
  - [x] Add `proxy_register_in_group` and `proxy_unregister_from_group` to proxy's `ext_config.rs` external interface
  - [x] Handle `RegisterInGroup { group_id }` in `execute_proposal()` -- classified as promise action, creates cross-contract call to `context-config.proxy_register_in_group(context_id, group_id)`
  - [x] Handle `UnregisterFromGroup { group_id }` in `execute_proposal()` -- classified as promise action, creates cross-contract call to `context-config.proxy_unregister_from_group(context_id, group_id)`

### All Phases Complete

All 5 phases of the Context Groups contract changes have been implemented. Integration tests for the proxy proposal actions should be added to `contracts/contracts/near/context-proxy/tests/sandbox.rs` to exercise the full flow (create group → create context → propose RegisterInGroup → approve → verify registration).

---

## Quick Reference

### Key File Paths

```
# Contract state and structs
contracts/contracts/near/context-config/src/lib.rs

# Mutation methods
contracts/contracts/near/context-config/src/mutate.rs

# Query methods
contracts/contracts/near/context-config/src/query.rs

# Migration system
contracts/contracts/near/context-config/src/sys/migrations.rs
contracts/contracts/near/context-config/src/sys/migrations/

# Integration tests
contracts/contracts/near/context-config/tests/sandbox.rs

# Proxy contract
contracts/contracts/near/context-proxy/src/

# Shared types
core/crates/context/config/src/lib.rs
core/crates/context/config/src/types.rs
```

### Building Commands

```bash
# Build context-config
cd contracts/contracts/near/context-config && ./build.sh

# Build context-proxy
cd contracts/contracts/near/context-proxy && ./build.sh

# Run tests
cd contracts/contracts/near/context-config && cargo test

# Build with migration
cargo build --release --features "migrations,03_context_groups"
```

### Common Patterns

```rust
// Create signed context request
let request = Request::new(signer_id.rt()?, RequestKind::Context(...), nonce);
let signed = Signed::new(&request, |b| signing_key.sign(b))?;

// Create signed group request (nonce=0, not checked for groups)
let signer_id: SignerId = signing_key.verifying_key().to_bytes().rt()?;
let request = Request::new(
    signer_id,
    RequestKind::Group(GroupRequest::new(group_id, GroupRequestKind::Create { ... })),
    0,
);
let signed = Signed::new(&request, |b| signing_key.sign(b))?;

// Call contract (same entry point for both context and group mutations)
account.call(contract.id(), "mutate")
    .args_json(json!(signed))
    .max_gas()
    .transact().await?;

// Query group info
let group_info: Option<GroupInfoResponse> = contract
    .view("group")
    .args_json(json!({ "group_id": group_id }))
    .await?.json()?;

// Check admin status
let is_admin: bool = contract
    .view("is_group_admin")
    .args_json(json!({ "group_id": group_id, "identity": signer_repr }))
    .await?.json()?;

// Register context in group
account.call(contract.id(), "mutate")
    .args_json(make_group_request(&admin_sk, group_id,
        GroupRequestKind::RegisterContext { context_id }))
    .max_gas().transact().await?;

// Query contexts in a group (paginated)
let contexts: Vec<Repr<ContextId>> = contract
    .view("group_contexts")
    .args_json(json!({ "group_id": group_id, "offset": 0, "length": 10 }))
    .await?.json()?;

// Reverse lookup: which group does a context belong to?
let group: Option<Repr<ContextGroupId>> = contract
    .view("context_group")
    .args_json(json!({ "context_id": context_id }))
    .await?.json()?;

// Set target application for a group (admin-only)
account.call(contract.id(), "mutate")
    .args_json(make_group_request(&admin_sk, group_id,
        GroupRequestKind::SetTargetApplication {
            target_application: Application::new(
                new_app_id, new_blob_id, size, source, metadata,
            ),
        }))
    .max_gas().transact().await?;

// Create proxy proposal to register context in a group (Phase 5)
let actions = vec![ProposalAction::RegisterInGroup {
    group_id: Repr::new(group_id),
}];
let proposal = proxy_helper.create_proposal_request(&proposal_id, &alice_sk, &actions)?;
// After quorum approval, proxy makes cross-contract call to context-config

// Create proxy proposal to unregister context from its group
let actions = vec![ProposalAction::UnregisterFromGroup { group_id }];
let proposal = proxy_helper.create_proposal_request(&proposal_id, &alice_sk, &actions)?;
```
