# Engineering Specification: Context Groups — Implementation Plan

**Status:** In Progress — contracts/ complete, core/ pending
**Date:** 2026-02-18 (updated 2026-02-19)
**Based on:** `PROPOSAL-hierarchical-context-management.md` (RFC v2)
**Scope:** `contracts/` (NEAR smart contracts) + `core/` (runtime, store, context manager, server)

### Implementation Status Summary

| Layer | Status |
|-------|--------|
| Shared types (`calimero-context-config` crate) | ✅ Complete |
| `contracts/` — all 5 phases | ✅ Complete |
| `core/` — runtime, store, context manager, API server | ⬜ Pending |

---

## Table of Contents

1. [Problem Statement](#1-problem-statement)
2. [Goals & Non-Goals](#2-goals--non-goals)
3. [System Architecture Overview](#3-system-architecture-overview)
4. [Data Model Definitions](#4-data-model-definitions)
5. [On-Chain vs Off-Chain Responsibilities](#5-on-chain-vs-off-chain-responsibilities)
6. [Detailed Component-Level Changes](#6-detailed-component-level-changes)
7. [State Machine for Upgrades](#7-state-machine-for-upgrades)
8. [Sequence Diagrams](#8-sequence-diagrams)
9. [Failure Scenarios & Mitigation](#9-failure-scenarios--mitigation)
10. [Security Analysis](#10-security-analysis)
11. [Performance Considerations](#11-performance-considerations)
12. [Backward Compatibility Guarantees](#12-backward-compatibility-guarantees)
13. [Phased Implementation Plan](#13-phased-implementation-plan)
14. [Testing Strategy](#14-testing-strategy)
15. [Open Questions & Tradeoffs](#15-open-questions--tradeoffs)
16. [Rollout Strategy](#16-rollout-strategy)

---

## 1. Problem Statement

The current Calimero architecture treats every context as a fully independent entity. A `Context` (defined in `core/crates/primitives/src/context.rs`) holds a single `ApplicationId`, its own Merkle state tree, its own DAG delta history, and its own on-chain registration in the `context-config` contract. There is no concept of relationships between contexts.

When an application creates dozens or thousands of contexts (DMs, channels, sub-workspaces), upgrading the application version requires an independent `UpdateApplicationRequest` on **every single context** — each involving WASM module loading, optional migration execution, on-chain `update_application` transaction, and sync propagation.

For a 1000-member organization with ~5100 contexts, this means 5100 independent upgrade operations. This does not scale.

Additionally, multiple independent organizations can deploy the same application (`AppKey`). Any grouping mechanism must disambiguate between organizations using the same application.

---

## 2. Goals & Non-Goals

### Goals

1. **Single-trigger version propagation**: Admin upgrades once, all contexts in the group converge to the new version.
2. **Workspace-level user management**: Group owns a set of user identities authorized to create contexts within it.
3. **Multi-tenant isolation**: Same `AppKey` can be used by many independent groups without interference.
4. **Context isolation preservation**: State, sync, membership, and privacy of individual contexts are unaffected.
5. **Backward compatibility**: Existing ungrouped contexts continue to work identically.
6. **Reuse of existing migration machinery**: `update_application_with_migration` and `update_application_id` code paths are reused without modification.
7. **Crash recovery**: In-progress upgrades survive node restarts.

### Non-Goals

1. Cross-context state sharing or inheritance.
2. Cross-group context migration (moving a context between groups).
3. Publisher-driven automatic upgrades (all orgs upgrade simultaneously).
4. Shared sync topics across contexts in a group.
5. Group-level billing or resource quotas (future work).
6. SDK/application-level group awareness (Phase 6, out of initial scope).

---

## 3. System Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                          Node Runtime                               │
│                                                                     │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                    ContextManager (Actor)                     │   │
│  │                                                              │   │
│  │  contexts: BTreeMap<ContextId, ContextMeta>  [existing]      │   │
│  │  groups:   BTreeMap<ContextGroupId, GroupMeta> [NEW]         │   │
│  │                                                              │   │
│  │  Handlers:                                                    │   │
│  │   ├── CreateContextRequest      [MODIFIED - group_id param]  │   │
│  │   ├── UpdateApplicationRequest  [EXISTING - unchanged]       │   │
│  │   ├── CreateGroupRequest        [NEW]                        │   │
│  │   ├── AddGroupMembersRequest    [NEW]                        │   │
│  │   ├── RemoveGroupMembersRequest [NEW]                        │   │
│  │   ├── UpgradeGroupRequest       [NEW]                        │   │
│  │   ├── GetGroupUpgradeStatus     [NEW]                        │   │
│  │   └── RetryGroupUpgradeRequest  [NEW]                        │   │
│  │                                                              │   │
│  │  Background:                                                  │   │
│  │   └── UpgradePropagator         [NEW]                        │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                          │                                          │
│  ┌───────────────────────▼──────────────────────────────────────┐   │
│  │              Datastore (calimero-store)                       │   │
│  │                                                              │   │
│  │  Existing keys:                    New keys:                 │   │
│  │   ContextMeta(ContextId)            GroupMeta(GroupId)        │   │
│  │   ContextConfig(ContextId)          GroupMember(GroupId,PK)   │   │
│  │   ContextState(ContextId,...)       GroupCtxIdx(GroupId,CId)  │   │
│  │   ContextIdentity(ContextId,PK)     CtxGroupRef(ContextId)   │   │
│  │                                     GroupUpgrade(GroupId)     │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                          │                                          │
│  ┌───────────────────────▼──────────────────────────────────────┐   │
│  │                   API Server (axum)                           │   │
│  │                                                              │   │
│  │  Existing:              New:                                 │   │
│  │   POST /contexts         POST   /admin/groups                │   │
│  │   GET  /contexts/:id     GET    /admin/groups/:id            │   │
│  │   ...                    DELETE /admin/groups/:id            │   │
│  │                          POST   /admin/groups/:id/members   │   │
│  │                          DELETE /admin/groups/:id/members   │   │
│  │                          GET    /admin/groups/:id/members   │   │
│  │                          GET    /admin/groups/:id/contexts  │   │
│  │                          POST   /admin/groups/:id/upgrade   │   │
│  │                          GET    /admin/groups/:id/upgrade/  │   │
│  │                                 status                      │   │
│  │                          POST   /admin/groups/:id/upgrade/  │   │
│  │                                 retry                       │   │
│  │                          POST   /admin/groups/:id/rollback  │   │
│  └──────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
                              │
                       On-chain (NEAR)
             ┌────────────────┼────────────────┐
             ▼                ▼                ▼
      context-config    context-proxy    registry
      [MODIFIED]        [MINIMAL]        [NO CHANGE]
```

---

## 4. Data Model Definitions

### 4.1 Core Types (New — `core/crates/primitives/src/context.rs`)

```rust
/// Unique identifier for a context group.
/// 32-byte hash, generated independently — NOT derived from any context.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ContextGroupId(Hash);

impl ContextGroupId {
    pub fn generate() -> Self {
        Self(Hash::random())
    }
}
```

### 4.2 Off-Chain Group Metadata (New — `core/crates/store/src/key/group.rs`)

```rust
/// Stored in Column::Config
/// Key: GroupMeta(ContextGroupId) → GroupMetaValue
pub struct GroupMeta {
    group_id: ContextGroupId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupMetaValue {
    pub app_key: AppKey,
    pub target_application_id: ApplicationId,
    pub upgrade_policy: UpgradePolicy,
    pub created_at: u64,
    pub admin_identity: PublicKey,  // primary admin (creator)
}

/// Stored in Column::Config
/// Key: GroupMember(ContextGroupId, PublicKey) → GroupMemberRole
pub struct GroupMember {
    group_id: ContextGroupId,
    identity: PublicKey,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum GroupMemberRole {
    Admin,
    Member,
}

/// Stored in Column::Config
/// Key: GroupContextIndex(ContextGroupId, ContextId) → ()
/// Presence-only index: enumerates all contexts belonging to a group
pub struct GroupContextIndex {
    group_id: ContextGroupId,
    context_id: ContextId,
}

/// Stored in Column::Config
/// Key: ContextGroupRef(ContextId) → ContextGroupId
/// Reverse index: given a context, find its group
pub struct ContextGroupRef {
    context_id: ContextId,
}

/// Stored in Column::Config
/// Key: GroupUpgrade(ContextGroupId) → GroupUpgradeValue
/// Persists in-progress upgrade state for crash recovery
pub struct GroupUpgradeKey {
    group_id: ContextGroupId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupUpgradeValue {
    pub from_application_id: ApplicationId,
    pub to_application_id: ApplicationId,
    pub migration: Option<MigrationParams>,
    pub initiated_at: u64,
    pub initiated_by: PublicKey,
    pub status: GroupUpgradeStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum GroupUpgradeStatus {
    InProgress {
        total: usize,
        completed: usize,
        failed: Vec<(ContextId, String)>,
    },
    Completed { completed_at: u64 },
    RolledBack { reason: String },
}

/// Upgrade propagation policy
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum UpgradePolicy {
    Automatic,
    LazyOnAccess,
    Coordinated { deadline: Option<Duration> },
}
```

### 4.3 On-Chain Group Metadata (New — `contracts/contracts/near/context-config/src/lib.rs`)

```rust
/// Stored on-chain in ContextConfigs.groups
#[near(serializers = [borsh])]
pub struct OnChainGroupMeta {
    pub app_key: AppKey,
    pub target_application: Application<'static>,
    pub admins: IterableSet<SignerId>,
    pub member_count: u64,
    pub context_count: u64,
}
```

### 4.4 Storage Key Column Mapping

All new storage keys use **`Column::Config`**, following the pattern of existing `ContextConfig` keys.

| Key Type | Column | Key Bytes | Value |
|---|---|---|---|
| `GroupMeta` | Config | `[0x20][group_id: 32]` | `GroupMetaValue` (borsh) |
| `GroupMember` | Config | `[0x21][group_id: 32][identity: 32]` | `GroupMemberRole` (borsh) |
| `GroupContextIndex` | Config | `[0x22][group_id: 32][context_id: 32]` | `()` (empty) |
| `ContextGroupRef` | Config | `[0x23][context_id: 32]` | `ContextGroupId` (borsh) |
| `GroupUpgrade` | Config | `[0x24][group_id: 32]` | `GroupUpgradeValue` (borsh) |

Key prefix bytes `0x20`-`0x24` are chosen to avoid collision with existing key prefixes. The exact values should be verified against the existing `AsKeyParts` implementations in `core/crates/store/src/key/` to ensure no overlap.

### 4.5 Modified Existing Types

**`CreateContextRequest`** (`core/crates/context/primitives/src/lib.rs`):
```rust
pub struct CreateContextRequest {
    pub application_id: ApplicationId,
    pub init_params: Vec<u8>,
    pub context_seed: Option<[u8; 32]>,
    pub group_id: Option<ContextGroupId>,  // NEW
}
```

**On-chain `Context` struct** (`contracts/contracts/near/context-config/src/lib.rs`):
```rust
struct Context {
    pub application: Guard<Application<'static>>,
    pub members: Guard<IterableSet<ContextIdentity>>,
    pub member_nonces: IterableMap<ContextIdentity, u64>,
    pub proxy: Guard<AccountId>,
    pub used_open_invitations: Guard<IterableSet<CryptoHash>>,
    pub commitments_open_invitations: IterableMap<CryptoHash, BlockHeight>,
    pub group_id: Option<ContextGroupId>,  // NEW
}
```

---

## 5. On-Chain vs Off-Chain Responsibilities

| Responsibility | On-Chain (context-config) | Off-Chain (node runtime) |
|---|---|---|
| **Group existence** | Authoritative (source of truth) | Cached for fast access |
| **Group admin set** | Authoritative (`admins: IterableSet`) | Read from chain |
| **Group member count** | Counter only (no identity storage) | Full membership with roles |
| **Group member identities** | NOT stored (privacy) | Stored locally with roles |
| **Context→Group mapping** | Authoritative (`context.group_id`) | Reverse index for enumeration |
| **Target application version** | Authoritative (auditable) | Cached, drives propagation |
| **Upgrade execution** | NOT involved | Full orchestration |
| **Upgrade status tracking** | NOT involved | Persisted locally |
| **Migration execution** | NOT involved | Existing WASM runtime |

**Design rationale**: On-chain storage is expensive and public. Group member identities are privacy-sensitive. Only governance-critical data (admin set, target version, context affiliation) goes on-chain. Operational data (member list, upgrade status, propagation) stays off-chain.

---

## 6. Detailed Component-Level Changes

### 6.1 contracts/contracts/near/context-config/

#### 6.1.1 `src/lib.rs` — Storage Schema

**Existing** `ContextConfigs` struct gains two new fields:

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

**Existing** `Context` struct gains an optional group reference:

```rust
struct Context {
    // ... existing fields unchanged ...
    pub group_id: Option<ContextGroupId>,  // NEW, defaults to None
}
```

**New** `Prefix` variants for storage isolation:

```rust
enum Prefix {
    // ... existing variants 1-8 ...
    Groups = 9,
    GroupAdmins(ContextGroupId) = 10,
    ContextGroupRefs = 11,
}
```

**Files changed:**
- `contracts/contracts/near/context-config/src/lib.rs` — Add structs, Prefix variants

#### 6.1.2 `src/mutate.rs` — New Request Handling

**Extend `RequestKind`:**

```rust
pub enum RequestKind<'a> {
    Context(ContextRequest<'a>),
    Group(GroupRequest<'a>),  // NEW
}
```

**New `GroupRequest` and dispatch:**

```rust
pub struct GroupRequest<'a> {
    pub group_id: Repr<ContextGroupId>,
    pub kind: GroupRequestKind<'a>,
}

pub enum GroupRequestKind<'a> {
    Create {
        group_id: Repr<ContextGroupId>,
        app_key: AppKey,
        application: Application<'a>,
    },
    AddMembers { members: Vec<Repr<ContextIdentity>> },
    RemoveMembers { members: Vec<Repr<ContextIdentity>> },
    SetTargetApplication { application: Application<'a> },
    RegisterContext { context_id: Repr<ContextId> },
    UnregisterContext { context_id: Repr<ContextId> },
    Delete,
}
```

**New entry point** `mutate_group()` following the same signed-request pattern as `mutate()`:

1. Parse `Signed<GroupRequest>` — uses ed25519 signature verification (same as context requests)
2. Extract `signer_id` from the signed envelope
3. **No nonce check for group operations** initially (groups don't have per-member nonces yet — see Open Questions)
4. Dispatch based on `GroupRequestKind`
5. Admin authorization: check `group.admins.contains(signer_id)` for admin-only operations

**Implementation methods** (on `ContextConfigs`):
- `create_group()` — Validates group doesn't exist, creates `OnChainGroupMeta` with caller as first admin
- `add_group_members()` — Admin-only, increments `member_count`
- `remove_group_members()` — Admin-only, decrements `member_count`
- `set_group_target()` — Admin-only, updates `target_application`
- `register_context_in_group()` — Validates context exists, has no group, sets `context.group_id`
- `unregister_context_from_group()` — Admin-only, clears `context.group_id`
- `delete_group()` — Admin-only, requires `context_count == 0`

**Files changed:**
- `contracts/contracts/near/context-config/src/mutate.rs` — Add `mutate_group()` entry point and all implementation methods

#### 6.1.3 `src/query.rs` — New Query Methods

```rust
pub fn group(&self, group_id: Repr<ContextGroupId>) -> Option<GroupInfoResponse>
pub fn group_contexts(&self, group_id: Repr<ContextGroupId>, offset: usize, limit: usize) -> Vec<Repr<ContextId>>
pub fn context_group(&self, context_id: Repr<ContextId>) -> Option<Repr<ContextGroupId>>
pub fn is_group_admin(&self, group_id: Repr<ContextGroupId>, identity: Repr<SignerId>) -> bool
```

**Files changed:**
- `contracts/contracts/near/context-config/src/query.rs` — Add query methods

#### 6.1.4 `src/sys/migrations/` — Contract Migration

New migration `03_context_groups.rs`:

```rust
/// Migration: Initialize groups storage and add group_id field to existing contexts.
///
/// Previous state format: Context without group_id field.
/// New state format: Context with group_id: Option<ContextGroupId> (defaults to None).
///
/// This migration:
/// 1. Initializes the groups IterableMap
/// 2. Initializes the context_group_refs IterableMap
/// 3. Existing contexts get group_id = None via Option::default()
pub fn migrate_03_context_groups(state: &mut ContextConfigs) {
    state.groups = IterableMap::new(Prefix::Groups);
    state.context_group_refs = IterableMap::new(Prefix::ContextGroupRefs);
    // Context.group_id = None via Option default during deserialization
}
```

The migration is gated behind a cargo feature `migration_03_context_groups`, following the pattern in `sys/migrations.rs`:

```rust
migrations! {
    "01_guard_revisions" => "migrations/01_guard_revisions.rs",
    "02_nonces"          => "migrations/02_nonces.rs",
    "03_context_groups"  => "migrations/03_context_groups.rs",  // NEW
}
```

**Files changed:**
- `contracts/contracts/near/context-config/src/sys/migrations.rs` — Register new migration
- `contracts/contracts/near/context-config/src/sys/migrations/03_context_groups.rs` — New file
- `contracts/contracts/near/context-config/Cargo.toml` — Add feature gate

#### 6.1.5 Tests

**Files changed/added:**
- `contracts/contracts/near/context-config/tests/groups.rs` — New integration test file covering:
  - Group creation, deletion
  - Member add/remove
  - Context registration/unregistration in groups
  - Target application update
  - Permission enforcement (non-admin rejection)
  - Migration test: old state format → new state format

### 6.2 contracts/contracts/near/context-proxy/

#### 6.2.1 `src/lib.rs` — New Proposal Actions

Add two new variants to `ProposalAction`:

```rust
pub enum ProposalAction {
    // ... existing variants ...
    RegisterInGroup { group_id: Repr<ContextGroupId> },
    UnregisterFromGroup { group_id: Repr<ContextGroupId> },
}
```

These actions, when a proposal reaches quorum, trigger a cross-contract call to `context-config.mutate_group()` with `RegisterContext` / `UnregisterContext`.

**Files changed:**
- Context-proxy types crate (wherever `ProposalAction` is defined)
- `contracts/contracts/near/context-proxy/src/mutate.rs` — Handle new action execution

### 6.3 core/crates/primitives/

#### 6.3.1 New Type: `ContextGroupId`

Add to `core/crates/primitives/src/context.rs`:

```rust
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ContextGroupId([u8; 32]);

impl ContextGroupId {
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        Self(bytes)
    }
}

impl fmt::Display for ContextGroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", bs58::encode(&self.0).into_string())
    }
}
```

Also add `UpgradePolicy`, `GroupMemberRole` to primitives.

**Files changed:**
- `core/crates/primitives/src/context.rs` — Add `ContextGroupId`, `UpgradePolicy`, `GroupMemberRole`

### 6.4 core/crates/store/

#### 6.4.1 New Storage Keys

New file: `core/crates/store/src/key/group.rs`

Implements 5 new key types following the existing pattern in `key/context.rs`:
- `GroupMeta` — implements `Entry` with `Column::Config`
- `GroupMember` — implements `Entry` with `Column::Config`
- `GroupContextIndex` — implements `Entry` with `Column::Config`
- `ContextGroupRef` — implements `Entry` with `Column::Config`
- `GroupUpgrade` — implements `Entry` with `Column::Config`

Each key type implements:
- `AsKeyParts` — serializes the key to bytes (prefix byte + field bytes)
- `FromKeyParts` — deserializes key from bytes
- `Entry` — associates with a `Column` and a `DataType` + `Codec`

**Files changed:**
- `core/crates/store/src/key/group.rs` — New file with 5 key types
- `core/crates/store/src/key/mod.rs` — Add `pub mod group;`

### 6.5 core/crates/context/primitives/

#### 6.5.1 New Message Types

New file: `core/crates/context/primitives/src/group.rs`

```rust
// Request types — sent to ContextManager actor

pub struct CreateGroupRequest {
    pub group_id: ContextGroupId,
    pub app_key: AppKey,
    pub application_id: ApplicationId,
    pub upgrade_policy: UpgradePolicy,
    pub admin_identity: PublicKey,
}

pub struct CreateGroupResponse {
    pub group_id: ContextGroupId,
}

pub struct DeleteGroupRequest {
    pub group_id: ContextGroupId,
    pub requester: PublicKey,
}

pub struct AddGroupMembersRequest {
    pub group_id: ContextGroupId,
    pub members: Vec<(PublicKey, GroupMemberRole)>,
    pub requester: PublicKey,
}

pub struct RemoveGroupMembersRequest {
    pub group_id: ContextGroupId,
    pub members: Vec<PublicKey>,
    pub requester: PublicKey,
}

pub struct UpgradeGroupRequest {
    pub group_id: ContextGroupId,
    pub application_id: ApplicationId,
    pub requester: PublicKey,
    pub migration: Option<MigrationParams>,
}

pub struct GetGroupUpgradeStatusRequest {
    pub group_id: ContextGroupId,
}

pub struct RetryGroupUpgradeRequest {
    pub group_id: ContextGroupId,
    pub context_ids: Option<Vec<ContextId>>,  // None = retry all failed
    pub requester: PublicKey,
}

pub struct GetGroupInfoRequest {
    pub group_id: ContextGroupId,
}

pub struct ListGroupContextsRequest {
    pub group_id: ContextGroupId,
    pub offset: usize,
    pub limit: usize,
}

pub struct ListGroupMembersRequest {
    pub group_id: ContextGroupId,
    pub offset: usize,
    pub limit: usize,
}

// Response types

pub struct GroupInfoResponse {
    pub group_id: ContextGroupId,
    pub app_key: AppKey,
    pub target_application_id: ApplicationId,
    pub upgrade_policy: UpgradePolicy,
    pub member_count: usize,
    pub context_count: usize,
    pub active_upgrade: Option<GroupUpgradeValue>,
}

pub struct GroupUpgradeStatusResponse {
    pub group_id: ContextGroupId,
    pub from_version: ApplicationId,
    pub to_version: ApplicationId,
    pub policy: UpgradePolicy,
    pub status: GroupUpgradeStatus,
    pub started_at: u64,
    pub completed_at: Option<u64>,
}
```

**Files changed:**
- `core/crates/context/primitives/src/group.rs` — New file
- `core/crates/context/primitives/src/lib.rs` — Add `pub mod group;`, extend `CreateContextRequest`

#### 6.5.2 Modified: `CreateContextRequest`

```rust
pub struct CreateContextRequest {
    // ... existing fields ...
    pub group_id: Option<ContextGroupId>,  // NEW
}
```

### 6.6 core/crates/context/ — ContextManager Actor

This is the largest area of change. The `ContextManager` actor (Actix) gains new handler implementations.

#### 6.6.1 Group Storage Helper Methods

New file: `core/crates/context/src/group_store.rs`

Helper functions for reading/writing group data from the datastore:

```rust
pub fn load_group_meta(store: &Store, group_id: &ContextGroupId) -> eyre::Result<Option<GroupMetaValue>>
pub fn save_group_meta(store: &Store, group_id: &ContextGroupId, meta: &GroupMetaValue) -> eyre::Result<()>
pub fn delete_group_meta(store: &Store, group_id: &ContextGroupId) -> eyre::Result<()>

pub fn check_group_membership(store: &Store, group_id: &ContextGroupId, identity: &PublicKey) -> eyre::Result<bool>
pub fn get_group_member_role(store: &Store, group_id: &ContextGroupId, identity: &PublicKey) -> eyre::Result<Option<GroupMemberRole>>
pub fn is_group_admin(store: &Store, group_id: &ContextGroupId, identity: &PublicKey) -> eyre::Result<bool>

pub fn add_group_member(store: &Store, group_id: &ContextGroupId, identity: &PublicKey, role: GroupMemberRole) -> eyre::Result<()>
pub fn remove_group_member(store: &Store, group_id: &ContextGroupId, identity: &PublicKey) -> eyre::Result<()>

pub fn register_context_in_group(store: &Store, group_id: &ContextGroupId, context_id: &ContextId) -> eyre::Result<()>
pub fn unregister_context_from_group(store: &Store, group_id: &ContextGroupId, context_id: &ContextId) -> eyre::Result<()>

pub fn get_group_for_context(store: &Store, context_id: &ContextId) -> eyre::Result<Option<ContextGroupId>>
pub fn enumerate_group_contexts(store: &Store, group_id: &ContextGroupId) -> eyre::Result<Vec<ContextId>>
pub fn count_group_contexts(store: &Store, group_id: &ContextGroupId) -> eyre::Result<usize>

pub fn save_group_upgrade(store: &Store, group_id: &ContextGroupId, upgrade: &GroupUpgradeValue) -> eyre::Result<()>
pub fn load_group_upgrade(store: &Store, group_id: &ContextGroupId) -> eyre::Result<Option<GroupUpgradeValue>>
pub fn delete_group_upgrade(store: &Store, group_id: &ContextGroupId) -> eyre::Result<()>
```

#### 6.6.2 Handler: `create_group.rs` (New)

```
ContextManager::handle(CreateGroupRequest) → ActorResponse<CreateGroupResponse>
```

Flow:
1. Generate `ContextGroupId` (or use provided one)
2. Validate no group with this ID exists
3. Store `GroupMetaValue` locally
4. Store `GroupMember` for the admin
5. Submit signed `GroupRequest::Create` to on-chain `context-config.mutate_group()`
6. Return `CreateGroupResponse { group_id }`

**Files:** `core/crates/context/src/handlers/create_group.rs`

#### 6.6.3 Handler: `add_group_members.rs` (New)

```
ContextManager::handle(AddGroupMembersRequest) → ActorResponse<()>
```

Flow:
1. Load group, verify requester is admin
2. Store each `GroupMember` locally
3. Submit signed `GroupRequest::AddMembers` to chain

**Files:** `core/crates/context/src/handlers/add_group_members.rs`

#### 6.6.4 Handler: `remove_group_members.rs` (New)

Similar to add, with admin check and removal.

**Files:** `core/crates/context/src/handlers/remove_group_members.rs`

#### 6.6.5 Handler: `create_context.rs` (Modified)

The existing `Prepared::new()` method is extended with a group-aware wrapper. Changes occur **before** the existing context creation flow:

```rust
// In Prepared::new(), after protocol validation and before context key generation:

if let Some(group_id) = request.group_id {
    // 1. Load group metadata from local store
    let group = load_group_meta(&datastore, &group_id)?
        .ok_or_else(|| eyre::eyre!("group '{}' not found", group_id))?;

    // 2. Verify caller is a group member
    if !check_group_membership(&datastore, &group_id, &caller_identity)? {
        bail!("caller is not a member of group '{}'", group_id);
    }

    // 3. Verify AppKey match
    let app = node_client.get_application(application_id)?;
    if app.app_key() != group.app_key {
        bail!(
            "application AppKey '{}' does not match group AppKey '{}'",
            app.app_key(), group.app_key
        );
    }

    // 4. Version override: use group's target version
    if *application_id != group.target_application_id {
        info!(
            requested = %application_id,
            target = %group.target_application_id,
            "Overriding requested version with group target"
        );
        *application_id = group.target_application_id;
    }
}
```

**Post-creation hook** (after successful context creation):

```rust
if let Some(group_id) = request.group_id {
    // Register context in group index (local)
    register_context_in_group(&datastore, &group_id, &context_id)?;

    // Submit RegisterContext to chain
    submit_register_context_on_chain(&node_client, &group_id, &context_id, &sender_key)?;
}
```

**Files changed:** `core/crates/context/src/handlers/create_context.rs`

#### 6.6.6 Handler: `delete_context.rs` (Modified)

Post-deletion, if the context belonged to a group:

```rust
if let Some(group_id) = get_group_for_context(&datastore, &context_id)? {
    unregister_context_from_group(&datastore, &group_id, &context_id)?;
    // Submit UnregisterContext to chain
    submit_unregister_context_on_chain(&node_client, &group_id, &context_id, &sender_key)?;
}
```

**Files changed:** `core/crates/context/src/handlers/delete_context.rs`

#### 6.6.7 Handler: `upgrade_group.rs` (New)

This is the core upgrade orchestration handler.

```
ContextManager::handle(UpgradeGroupRequest) → ActorResponse<GroupUpgradeStatus>
```

**Flow:**

1. Load group meta, verify requester is admin
2. Verify `AppKey` continuity between current target and new application
3. Install new application on local node (if not present)
4. Select canary context (first `ContextId` in deterministic order)
5. Execute canary upgrade using **existing** `update_application_with_migration` / `update_application_id`:
   - If canary fails → abort, return error, do NOT update target version
   - If canary succeeds → proceed
6. Update group `target_application_id` locally
7. Submit `SetTargetApplication` to chain
8. Create `GroupUpgradeValue` with `InProgress` status, persist to store
9. Spawn `UpgradePropagator` as background task
10. Return `GroupUpgradeStatus::InProgress { total, completed: 1, failed: [] }`

**Files:** `core/crates/context/src/handlers/upgrade_group.rs`

#### 6.6.8 Background Task: `upgrade_propagator.rs` (New)

The `UpgradePropagator` runs as a spawned async task on the ContextManager actor context.

```rust
pub struct UpgradePropagator {
    group_id: ContextGroupId,
    target_application_id: ApplicationId,
    migration: Option<MigrationParams>,
    admin_key: PublicKey,
    skip_context: ContextId,  // canary, already upgraded
    datastore: Store,
    node_client: NodeClient,
    context_client: ContextClient,
}

impl UpgradePropagator {
    pub async fn run(self) -> GroupUpgradeStatus {
        let contexts = enumerate_group_contexts(&self.datastore, &self.group_id)
            .unwrap_or_default();
        let total = contexts.len();
        let mut completed = 1; // canary
        let mut failed = Vec::new();

        for context_id in contexts {
            if context_id == self.skip_context {
                continue;
            }

            // Check if already at target version (idempotency for crash recovery)
            if self.is_at_target_version(&context_id) {
                completed += 1;
                continue;
            }

            match self.upgrade_single_context(context_id).await {
                Ok(()) => {
                    completed += 1;
                    // Persist progress for crash recovery
                    let status = GroupUpgradeStatus::InProgress {
                        total,
                        completed,
                        failed: failed.clone(),
                    };
                    let _ = save_group_upgrade(
                        &self.datastore,
                        &self.group_id,
                        &GroupUpgradeValue {
                            status,
                            // ... other fields ...
                        },
                    );
                }
                Err(e) => {
                    warn!(%context_id, error = %e, "Context upgrade failed");
                    failed.push((context_id, e.to_string()));
                }
            }
        }

        let final_status = if failed.is_empty() {
            GroupUpgradeStatus::Completed {
                completed_at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            }
        } else {
            GroupUpgradeStatus::InProgress { total, completed, failed }
        };

        let _ = save_group_upgrade(&self.datastore, &self.group_id, &/* ... */);
        final_status
    }

    async fn upgrade_single_context(&self, context_id: ContextId) -> eyre::Result<()> {
        // Reuses EXISTING code paths:
        if let Some(ref migration) = self.migration {
            update_application_with_migration(
                self.datastore.clone(),
                self.node_client.clone(),
                self.context_client.clone(),
                context_id,
                None,  // old_application_id (auto-detected)
                self.target_application_id,
                None,  // old_root_hash (auto-detected)
                self.admin_key,
                Some(migration.clone()),
                self.load_module().await?,
            ).await
        } else {
            update_application_id(
                self.datastore.clone(),
                self.node_client.clone(),
                self.context_client.clone(),
                context_id,
                None,
                self.target_application_id,
                None,
                self.admin_key,
            ).await
        }
    }
}
```

**Key property**: This reuses the exact same `update_application_with_migration` and `update_application_id` functions from `core/crates/context/src/handlers/update_application.rs`. No new migration logic is introduced.

**Files:** `core/crates/context/src/upgrade_propagator.rs`

#### 6.6.9 Lazy-on-Access Interceptor (Phase 5)

For `UpgradePolicy::LazyOnAccess`, the execute handler (`core/crates/context/src/handlers/execute.rs`) gains a pre-check:

```rust
// Before executing the user's method:
fn maybe_lazy_upgrade(&self, context_id: &ContextId) -> Option<UpgradeGroupRequest> {
    let group_id = get_group_for_context(&self.datastore, context_id).ok()??;
    let group = load_group_meta(&self.datastore, &group_id).ok()??;

    if !matches!(group.upgrade_policy, UpgradePolicy::LazyOnAccess) {
        return None;
    }

    let context_meta = self.datastore.handle().get(&key::ContextMeta::new(*context_id)).ok()??;
    if context_meta.application.application_id == group.target_application_id {
        return None; // Already at target
    }

    // Load upgrade params if active
    let upgrade = load_group_upgrade(&self.datastore, &group_id).ok()??;

    Some(/* upgrade params */)
}
```

This intercepts the execute path, performs the upgrade transparently, then continues with the user's method call.

**Files changed:** `core/crates/context/src/handlers/execute.rs` (Phase 5 only)

#### 6.6.10 Crash Recovery (Phase 5)

On `ContextManager` startup:

```rust
// In ContextManager::started() or initialization:
fn recover_in_progress_upgrades(&mut self, ctx: &mut actix::Context<Self>) {
    // Iterate all GroupUpgrade keys in store
    let handle = self.datastore.handle();
    for (key, upgrade) in handle.iter::<GroupUpgrade>().unwrap() {
        if matches!(upgrade.status, GroupUpgradeStatus::InProgress { .. }) {
            info!(
                group_id = %key.group_id,
                "Resuming in-progress group upgrade after restart"
            );

            let propagator = UpgradePropagator::new(
                key.group_id,
                upgrade.to_application_id,
                upgrade.migration.clone(),
                upgrade.initiated_by,
                ContextId::default(), // no canary skip on resume
                self.datastore.clone(),
                self.node_client.clone(),
                self.context_client.clone(),
            );
            ctx.spawn(propagator.run().into_actor(self));
        }
    }
}
```

**Files changed:** `core/crates/context/src/lib.rs` or wherever `ContextManager::started()` is implemented

### 6.7 core/crates/server/ — API Layer

#### 6.7.1 New Route Module

New file: `core/crates/server/src/admin/groups.rs`

Endpoints (all under `/admin/groups`):

| Method | Path | Handler | Actor Message |
|---|---|---|---|
| POST | `/admin/groups` | `create_group` | `CreateGroupRequest` |
| GET | `/admin/groups/:group_id` | `get_group` | `GetGroupInfoRequest` |
| DELETE | `/admin/groups/:group_id` | `delete_group` | `DeleteGroupRequest` |
| POST | `/admin/groups/:group_id/members` | `add_members` | `AddGroupMembersRequest` |
| DELETE | `/admin/groups/:group_id/members` | `remove_members` | `RemoveGroupMembersRequest` |
| GET | `/admin/groups/:group_id/members` | `list_members` | `ListGroupMembersRequest` |
| GET | `/admin/groups/:group_id/contexts` | `list_contexts` | `ListGroupContextsRequest` |
| POST | `/admin/groups/:group_id/upgrade` | `upgrade_group` | `UpgradeGroupRequest` |
| GET | `/admin/groups/:group_id/upgrade/status` | `upgrade_status` | `GetGroupUpgradeStatusRequest` |
| POST | `/admin/groups/:group_id/upgrade/retry` | `retry_upgrade` | `RetryGroupUpgradeRequest` |
| POST | `/admin/groups/:group_id/rollback` | `rollback_group` | (same as upgrade with reversed target) |

#### 6.7.2 Modified: Context Creation Endpoint

The existing `POST /contexts` endpoint gains an optional `group_id` field in the JSON body:

```rust
#[derive(Deserialize)]
pub struct CreateContextBody {
    pub application_id: ApplicationId,
    pub init_params: Option<Vec<u8>>,
    pub context_seed: Option<String>,
    pub group_id: Option<ContextGroupId>,  // NEW
}
```

**Files changed:**
- `core/crates/server/src/admin/groups.rs` — New file
- `core/crates/server/src/admin/mod.rs` — Register group routes
- `core/crates/server/src/admin/context.rs` (or wherever context creation is) — Add `group_id` param

### 6.8 Sync Layer — No Changes

The sync layer (`core/crates/network/`) requires **no changes**. Each context syncs independently via its own gossipsub topic. The upgrade propagation produces per-context state changes that sync via existing protocols.

---

## 7. State Machine for Upgrades

```
                                    ┌─────────┐
                                    │  IDLE    │
                                    │ (no      │
                                    │ active   │
                                    │ upgrade) │
                                    └────┬─────┘
                                         │
                              Admin triggers upgrade
                              POST /admin/groups/:id/upgrade
                                         │
                                         ▼
                                ┌────────────────┐
                                │  CANARY_TEST   │
                                │                │
                                │  Upgrade first │
                                │  context as    │
                                │  validation    │
                                └───┬────────┬───┘
                                    │        │
                              success      failure
                                    │        │
                                    ▼        ▼
                          ┌──────────┐   ┌──────────┐
                          │PROPAGATE │   │ ABORTED  │
                          │          │   │          │
                          │ Upgrade  │   │ No state │
                          │ remaining│   │ changed  │
                          │ contexts │   │ (except  │
                          │ in       │   │  canary) │
                          │ background│  └──────────┘
                          └───┬──┬───┘
                              │  │
                     all      │  │  some
                     succeed  │  │  fail
                              │  │
                              ▼  ▼
                    ┌──────────┐  ┌──────────────┐
                    │COMPLETED │  │PARTIAL_FAIL  │
                    │          │  │              │
                    │ All ctxs │  │ Some failed  │
                    │ upgraded │  │ Admin can    │
                    └──────────┘  │ retry        │
                                  └──────┬───────┘
                                         │
                              Admin retries failed
                              POST .../upgrade/retry
                                         │
                                         ▼
                                  ┌──────────────┐
                                  │  RETRY       │
                                  │              │
                                  │  Re-attempt  │
                                  │  failed ctxs │
                                  └──────┬───────┘
                                         │
                                    ┌────┴────┐
                                    │         │
                                    ▼         ▼
                              COMPLETED  PARTIAL_FAIL


    At any point during PROPAGATE or PARTIAL_FAIL:

                              Admin triggers rollback
                              POST .../rollback
                                         │
                                         ▼
                                  ┌──────────────┐
                                  │  ROLLBACK    │
                                  │              │
                                  │  Same flow   │
                                  │  as upgrade  │
                                  │  with target │
                                  │  = previous  │
                                  │  version     │
                                  └──────────────┘
```

### Upgrade State Transitions

| From | Event | To | Side Effects |
|---|---|---|---|
| IDLE | Admin triggers upgrade | CANARY_TEST | Load new app, select canary |
| CANARY_TEST | Canary succeeds | PROPAGATE | Update target on-chain, persist GroupUpgrade |
| CANARY_TEST | Canary fails | IDLE | Return error, no state changed |
| PROPAGATE | Context N succeeds | PROPAGATE | Increment completed, persist progress |
| PROPAGATE | Context N fails | PROPAGATE | Record failure, continue |
| PROPAGATE | All done, 0 failures | COMPLETED | Persist final status |
| PROPAGATE | All done, >0 failures | PARTIAL_FAIL | Persist final status |
| PARTIAL_FAIL | Admin retries | PROPAGATE | Re-attempt failed contexts |
| PROPAGATE | Node crashes | PROPAGATE | On restart, resume from persisted state |
| PROPAGATE/PARTIAL_FAIL | Admin rollback | ROLLBACK→PROPAGATE | New upgrade with reversed target |

---

## 8. Sequence Diagrams

### 8.1 Group Creation

```
Admin                API Server            ContextManager         Store           Chain
  │                     │                       │                   │               │
  │ POST /admin/groups  │                       │                   │               │
  │ { app_key, app_id,  │                       │                   │               │
  │   upgrade_policy }  │                       │                   │               │
  │────────────────────►│                       │                   │               │
  │                     │ CreateGroupRequest     │                   │               │
  │                     │──────────────────────►│                   │               │
  │                     │                       │ generate GroupId   │               │
  │                     │                       │──┐                │               │
  │                     │                       │◄─┘                │               │
  │                     │                       │                   │               │
  │                     │                       │ put GroupMeta      │               │
  │                     │                       │──────────────────►│               │
  │                     │                       │                   │               │
  │                     │                       │ put GroupMember    │               │
  │                     │                       │ (admin)           │               │
  │                     │                       │──────────────────►│               │
  │                     │                       │                   │               │
  │                     │                       │ sign & submit     │               │
  │                     │                       │ GroupRequest::     │               │
  │                     │                       │ Create             │               │
  │                     │                       │──────────────────────────────────►│
  │                     │                       │◄─────────────────────────────────│
  │                     │                       │                   │               │
  │                     │ CreateGroupResponse    │                   │               │
  │                     │◄──────────────────────│                   │               │
  │ 201 { group_id }    │                       │                   │               │
  │◄────────────────────│                       │                   │               │
```

### 8.2 Context Creation Within a Group

```
User                API Server            ContextManager         Store           Chain
  │                     │                       │                   │               │
  │ POST /contexts      │                       │                   │               │
  │ { app_id, group_id, │                       │                   │               │
  │   init_params }     │                       │                   │               │
  │────────────────────►│                       │                   │               │
  │                     │ CreateContextRequest   │                   │               │
  │                     │ (with group_id)        │                   │               │
  │                     │──────────────────────►│                   │               │
  │                     │                       │                   │               │
  │                     │                       │ get GroupMeta      │               │
  │                     │                       │──────────────────►│               │
  │                     │                       │◄─────── group     │               │
  │                     │                       │                   │               │
  │                     │                       │ get GroupMember    │               │
  │                     │                       │ (caller)          │               │
  │                     │                       │──────────────────►│               │
  │                     │                       │◄─────── role      │               │
  │                     │                       │                   │               │
  │                     │                       │ verify AppKey     │               │
  │                     │                       │ match             │               │
  │                     │                       │──┐                │               │
  │                     │                       │◄─┘ ok             │               │
  │                     │                       │                   │               │
  │                     │                       │ override app_id   │               │
  │                     │                       │ with group target │               │
  │                     │                       │──┐                │               │
  │                     │                       │◄─┘                │               │
  │                     │                       │                   │               │
  │                     │                       │ ╔═══════════════╗ │               │
  │                     │                       │ ║ EXISTING      ║ │               │
  │                     │                       │ ║ create_context║ │               │
  │                     │                       │ ║ flow          ║ │               │
  │                     │                       │ ║ (unchanged)   ║ │               │
  │                     │                       │ ╚═══════════════╝ │               │
  │                     │                       │                   │               │
  │                     │                       │ put GroupCtxIdx   │               │
  │                     │                       │──────────────────►│               │
  │                     │                       │ put CtxGroupRef   │               │
  │                     │                       │──────────────────►│               │
  │                     │                       │                   │               │
  │                     │                       │ RegisterContext   │               │
  │                     │                       │──────────────────────────────────►│
  │                     │                       │◄─────────────────────────────────│
  │                     │                       │                   │               │
  │                     │ CreateContextResponse  │                   │               │
  │                     │◄──────────────────────│                   │               │
  │ 201 { context_id }  │                       │                   │               │
  │◄────────────────────│                       │                   │               │
```

### 8.3 Group Upgrade — Full Flow

```
Admin               API Server        ContextManager       Propagator          Store         Chain
  │                    │                    │                   │                 │              │
  │ POST .../upgrade   │                    │                   │                 │              │
  │ { app_id, migrate }│                    │                   │                 │              │
  │───────────────────►│                    │                   │                 │              │
  │                    │ UpgradeGroupReq    │                   │                 │              │
  │                    │──────────────────►│                   │                 │              │
  │                    │                    │                   │                 │              │
  │                    │                    │ verify admin      │                 │              │
  │                    │                    │ verify AppKey     │                 │              │
  │                    │                    │ install app       │                 │              │
  │                    │                    │                   │                 │              │
  │                    │                    │ ┌─────────────┐   │                 │              │
  │                    │                    │ │ CANARY      │   │                 │              │
  │                    │                    │ │ upgrade     │   │                 │              │
  │                    │                    │ │ context[0]  │   │                 │              │
  │                    │                    │ │ using       │   │                 │              │
  │                    │                    │ │ EXISTING    │   │                 │              │
  │                    │                    │ │ update_app  │   │                 │              │
  │                    │                    │ │ _with_      │   │                 │              │
  │                    │                    │ │ migration() │   │                 │              │
  │                    │                    │ └──────┬──────┘   │                 │              │
  │                    │                    │        │ success   │                 │              │
  │                    │                    │        ▼          │                 │              │
  │                    │                    │ update target      │                 │              │
  │                    │                    │────────────────────────────────────►│              │
  │                    │                    │                   │                 │              │
  │                    │                    │ SetTargetApp      │                 │              │
  │                    │                    │────────────────────────────────────────────────── ►│
  │                    │                    │◄────────────────────────────────────────────────── │
  │                    │                    │                   │                 │              │
  │                    │                    │ persist upgrade   │                 │              │
  │                    │                    │ status            │                 │              │
  │                    │                    │────────────────────────────────────►│              │
  │                    │                    │                   │                 │              │
  │                    │                    │ spawn propagator  │                 │              │
  │                    │                    │─────────────────►│                 │              │
  │                    │                    │                   │                 │              │
  │ 202 Accepted       │                    │                   │                 │              │
  │ { status: InProg } │                    │                   │                 │              │
  │◄───────────────────│                    │                   │                 │              │
  │                    │                    │                   │                 │              │
  │                    │                    │                   │ foreach ctx:     │              │
  │                    │                    │                   │  check version   │              │
  │                    │                    │                   │─────────────────►│              │
  │                    │                    │                   │◄────────────────│              │
  │                    │                    │                   │                 │              │
  │                    │                    │                   │  if !at_target:  │              │
  │                    │                    │                   │  upgrade_single  │              │
  │                    │                    │                   │  _context()      │              │
  │                    │                    │                   │  [EXISTING       │              │
  │                    │                    │                   │   code path]     │              │
  │                    │                    │                   │─────────────────►│              │
  │                    │                    │                   │◄────────────────│              │
  │                    │                    │                   │                 │              │
  │                    │                    │                   │  persist         │              │
  │                    │                    │                   │  progress        │              │
  │                    │                    │                   │─────────────────►│              │
  │                    │                    │                   │                 │              │
  │ GET .../status     │                    │                   │                 │              │
  │───────────────────►│                    │                   │                 │              │
  │ { completed: N }   │                    │                   │                 │              │
  │◄───────────────────│                    │                   │                 │              │
  │                    │                    │                   │                 │              │
  │                    │                    │                   │ done             │              │
  │                    │                    │                   │─────────────────►│ final status │
```

### 8.4 Lazy-on-Access Upgrade (Phase 5)

```
Client              API Server        ContextManager         Store
  │                    │                    │                   │
  │ POST /execute      │                    │                   │
  │ { context_id,      │                    │                   │
  │   method, args }   │                    │                   │
  │───────────────────►│                    │                   │
  │                    │ ExecuteRequest     │                   │
  │                    │──────────────────►│                   │
  │                    │                    │                   │
  │                    │                    │ get CtxGroupRef   │
  │                    │                    │──────────────────►│
  │                    │                    │◄──── group_id     │
  │                    │                    │                   │
  │                    │                    │ get GroupMeta      │
  │                    │                    │──────────────────►│
  │                    │                    │◄──── target_ver   │
  │                    │                    │                   │
  │                    │                    │ compare ctx.app_id │
  │                    │                    │ vs group.target    │
  │                    │                    │──┐                │
  │                    │                    │◄─┘ MISMATCH       │
  │                    │                    │                   │
  │                    │                    │ ╔═══════════════╗ │
  │                    │                    │ ║ EXISTING      ║ │
  │                    │                    │ ║ update_app    ║ │
  │                    │                    │ ║ flow          ║ │
  │                    │                    │ ╚═══════════════╝ │
  │                    │                    │                   │
  │                    │                    │ ╔═══════════════╗ │
  │                    │                    │ ║ EXISTING      ║ │
  │                    │                    │ ║ execute flow  ║ │
  │                    │                    │ ║ (user method) ║ │
  │                    │                    │ ╚═══════════════╝ │
  │                    │                    │                   │
  │ response           │                    │                   │
  │◄───────────────────│                    │                   │
```

### 8.5 Rollback

Rollback follows the **exact same flow** as upgrade (Section 8.3), with:
- `target_application_id` = the previous version
- `migration` = optional reverse migration function (`rollback_v2_to_v1`)

The `UpgradePropagator` is reused identically.

---

## 9. Failure Scenarios & Mitigation

### 9.1 Canary Upgrade Failure

**Scenario**: The migration function fails on the canary context (bad WASM, incompatible state schema, runtime error).

**Impact**: Entire group upgrade is aborted.

**Mitigation**:
- Group `target_application_id` is NOT updated
- No other contexts are modified
- Error returned to admin with canary context ID and error details
- Admin can fix the migration function and retry

**Recovery**: No recovery needed — no state was changed (except the canary context, which may need manual attention if migration was partially applied). The existing migration system uses atomic writes within a single context, so partial canary failure should leave state consistent.

### 9.2 Non-Canary Context Upgrade Failure

**Scenario**: Context N fails to upgrade (e.g., corrupted local state, disk full, migration logic error on edge-case state).

**Impact**: That context remains at old version. Other contexts continue upgrading.

**Mitigation**:
- Failure recorded in `GroupUpgradeStatus.failed`
- Propagation continues to remaining contexts
- Admin notified via status endpoint
- Admin can retry failed contexts: `POST .../upgrade/retry`

**Recovery**: `RetryGroupUpgradeRequest` re-attempts the upgrade on specified (or all) failed contexts.

### 9.3 Node Crash During Propagation

**Scenario**: Node crashes after upgrading 500 of 5000 contexts.

**Impact**: Upgrade paused mid-propagation.

**Mitigation**:
- `GroupUpgradeValue` is persisted to store after every successful context upgrade
- On node restart, `recover_in_progress_upgrades()` scans for `InProgress` upgrades
- Propagation resumes from where it left off
- Already-upgraded contexts are detected by comparing `context.application_id` to `target_application_id` (idempotency check)

**Recovery**: Automatic on restart. No manual intervention needed.

### 9.4 On-Chain Transaction Failure

**Scenario**: `SetTargetApplication` or `RegisterContext` on-chain transaction fails (gas, network error).

**Impact**: On-chain state and off-chain state diverge.

**Mitigation**:
- Off-chain state is the operational source of truth for upgrade propagation
- On-chain state is for auditability, not for upgrade execution
- Failed on-chain transactions should be retried (background retry queue)
- Status endpoint should report on-chain sync status

**Recovery**: Background retry of failed on-chain transactions. Admin can also manually trigger on-chain sync.

### 9.5 Concurrent Upgrade Attempts

**Scenario**: Admin triggers upgrade to v3 while v2 upgrade is still propagating.

**Impact**: Must prevent conflicting upgrades.

**Mitigation**:
- Check for `active_upgrade` with `InProgress` status before accepting new upgrade
- Reject with `409 Conflict` if an upgrade is already in progress
- Admin must wait for completion or explicitly cancel the current upgrade

### 9.6 Mixed-Version Window

**Scenario**: During propagation, some contexts are at v1 and some at v2.

**Impact**: Application must handle version coexistence.

**Mitigation**:
- This is standard rolling-deployment behavior
- Application developers MUST ensure v(N) and v(N+1) can coexist during the upgrade window
- Each context's `application_id` is individually accurate — application logic can check its own version
- Contexts are upgraded in deterministic order (by `ContextId`) for predictability

---

## 10. Security Analysis

### 10.1 Threat Model

| Threat | Vector | Mitigation |
|---|---|---|
| **Unauthorized upgrade** | Non-admin triggers upgrade | Admin role check in handler + on-chain admin set |
| **Malicious application version** | Compromised admin pushes malicious WASM | AppKey continuity check (same publisher signer), on-chain audit trail |
| **Context state leakage via group** | Group admin reads DM contents | Group has NO access to context state — only `ContextId` and `application_id` visible |
| **Group membership enumeration** | Attacker enumerates group members | Group member identities stored off-chain only, not on-chain |
| **Replay attack on group operations** | Replay old signed group request | Nonce tracking for group operations (or timestamp-based validity window matching existing pattern) |
| **Silent downgrade** | Admin downgrades from signed to unsigned app | Existing `verify_appkey_continuity` check prevents this |
| **Concurrent modification** | Two admins trigger conflicting operations | Single active upgrade per group, `409 Conflict` on concurrent attempts |

### 10.2 AppKey Continuity Enforcement

Before any upgrade, the system verifies:
1. The new application has the same `AppKey` (package name + publisher signer) as the current target
2. The new version is >= the current version (no silent downgrades; explicit rollback required)
3. The application binary is available locally

This uses the existing `verify_appkey_continuity` function from the update_application handler.

### 10.3 Privacy Preservation

| Data | Visible to Group Admin | Visible On-Chain |
|---|---|---|
| Group exists | Yes | Yes |
| Group `AppKey` | Yes | Yes |
| Target application version | Yes | Yes |
| Context IDs in group | Yes | Yes (via `context.group_id`) |
| Context state (messages, data) | **NO** | **NO** |
| Context members (who is in a DM) | **NO** | **NO** |
| Context activity/timestamps | **NO** | **NO** |
| Group member identities | Yes (off-chain) | **NO** (only count) |

---

## 11. Performance Considerations

### 11.1 Large Groups (10K+ Contexts)

**Concern**: Upgrading 10,000 contexts sequentially could take significant time.

**Mitigation strategy**:
1. **Sequential with progress**: Initial implementation is sequential (one context at a time). Each context upgrade takes ~50-200ms (WASM load, migration, state write).
   - 10K contexts × 100ms avg = ~17 minutes total
   - Acceptable for most deployments
2. **Configurable concurrency** (future): Add `max_concurrent_upgrades` to `UpgradePolicy` to parallelize. Requires careful datastore locking.
3. **Lazy-on-Access**: For very large groups, `LazyOnAccess` avoids upgrading dormant contexts entirely.

### 11.2 Storage Overhead

Per group:
- `GroupMeta`: ~200 bytes
- `GroupMember` × N members: ~65 bytes each
- `GroupContextIndex` × M contexts: ~65 bytes each
- `ContextGroupRef` × M contexts: ~65 bytes each
- `GroupUpgrade`: ~300 bytes (during upgrade only)

For a 1000-member group with 5000 contexts: ~650KB total. Negligible.

### 11.3 On-Chain Gas Costs

- `create_group`: ~5-10 TGas (storage write + log)
- `add_group_members`: ~5 TGas per batch (counter increment only, no identity storage)
- `register_context`: ~10 TGas (storage write for `context.group_id` + counter)
- `set_group_target`: ~5-10 TGas (storage update)

Total for creating a group with 100 members and 500 contexts: ~5000 TGas (~0.0005 NEAR). Acceptable.

### 11.4 Context Creation Overhead

Adding `group_id` processing to context creation adds:
- 2 store reads (`GroupMeta`, `GroupMember`): ~0.1ms each
- 1 comparison (AppKey match): negligible
- 2 store writes (`GroupContextIndex`, `ContextGroupRef`): ~0.1ms each
- 1 on-chain transaction (RegisterContext): async, not on hot path

Total added latency: <1ms (excluding async on-chain call). Negligible.

---

## 12. Backward Compatibility Guarantees

### 12.1 Zero Breaking Changes for Ungrouped Contexts

1. **All existing API endpoints** remain unchanged in behavior
2. **`POST /contexts`** without `group_id` creates an ungrouped context (identical to current behavior)
3. **`UpdateApplicationRequest`** on individual contexts continues to work for ungrouped contexts
4. **Context sync**, state, membership, proxy — all unchanged for ungrouped contexts
5. **On-chain `Context` struct** — `group_id: Option<ContextGroupId>` defaults to `None` for existing contexts

### 12.2 Contract Migration Safety

The contract migration (`03_context_groups`) is **additive only**:
- New fields (`groups`, `context_group_refs`) are initialized empty
- Existing `Context` structs gain `group_id: Option<...>` which borsh-deserializes as `None` for old data (borsh Option is prefix-byte-based, and old data without the field will need the migration to add it)
- No existing storage keys are modified or deleted

### 12.3 Core Runtime Migration

- New storage key types (`GroupMeta`, etc.) are simply new key prefixes
- No existing data needs modification
- New code paths are only triggered when `group_id` is present in requests
- All existing handlers remain functionally identical

### 12.4 API Versioning

New endpoints are under `/admin/groups/` — no collision with existing routes. The `POST /contexts` body gains an optional field, which is backward compatible for JSON deserialization (missing fields default to `None`).

---

## 13. Phased Implementation Plan

### Phase 1: Foundation (contracts + core storage)

**Goal**: Add all storage schema changes and types without behavior changes.

**Estimated scope**: ~15-20 files, ~1500 LOC

#### contracts/ ✅ Complete
| File | Change | Description | Status |
|---|---|---|---|
| `context-config/src/lib.rs` | MODIFY | Add `groups`, `context_group_refs` to `ContextConfigs`; add `group_id` to `Context`; add `OnChainGroupMeta` struct; add `Prefix` variants | ✅ |
| `context-config/Cargo.toml` | MODIFY | Add `03_context_groups` feature gate | ✅ |
| `context-config/src/sys/migrations.rs` | MODIFY | Register migration 03 | ✅ |
| `context-config/src/sys/migrations/03_context_groups.rs` | NEW | Migration implementation | ✅ |
| `context-config/tests/migrations.rs` | MODIFY | Migration roundtrip test | ✅ |

#### core/ ⬜ Pending
| File | Change | Description | Status |
|---|---|---|---|
| `crates/primitives/src/context.rs` | MODIFY | Add `ContextGroupId`, `UpgradePolicy`, `GroupMemberRole` types | ⬜ |
| `crates/store/src/key/group.rs` | NEW | All 5 storage key types | ⬜ |
| `crates/store/src/key/mod.rs` | MODIFY | Add `pub mod group;` | ⬜ |
| `crates/context/primitives/src/lib.rs` | MODIFY | Add `group_id: Option<ContextGroupId>` to `CreateContextRequest` | ⬜ |

> **Note**: `ContextGroupId`, `AppKey`, `GroupRequest`, `GroupRequestKind`, `RequestKind::Group`, `ProposalAction::RegisterInGroup/UnregisterFromGroup` added to shared types crate (`core/crates/context/config/`) ✅

**Testing**:
- Contract migration test (old state → new state roundtrip) ✅
- Storage key serialization/deserialization tests ⬜
- Type serialization tests ⬜

**Backward compatibility**: 100% — no behavior changes, just new types and storage slots.

---

### Phase 2: Group CRUD + Membership

**Goal**: Implement group lifecycle management (create, delete, add/remove members).

**Estimated scope**: ~10-15 files, ~2000 LOC

#### contracts/ ✅ Complete
| File | Change | Description | Status |
|---|---|---|---|
| `context-config/src/mutate.rs` | MODIFY | `handle_group_request()` dispatcher, `create_group()`, `delete_group()`, `add_group_members()`, `remove_group_members()` | ✅ |
| `context-config/src/query.rs` | MODIFY | Add `group()`, `is_group_admin()` queries | ✅ |
| `context-config/tests/groups.rs` | NEW | Integration tests for group CRUD | ✅ |

#### core/ ⬜ Pending
| File | Change | Description | Status |
|---|---|---|---|
| `crates/context/primitives/src/group.rs` | NEW | All group message types | ⬜ |
| `crates/context/primitives/src/lib.rs` | MODIFY | Add `pub mod group;` | ⬜ |
| `crates/context/src/group_store.rs` | NEW | Storage helper functions | ⬜ |
| `crates/context/src/handlers/create_group.rs` | NEW | CreateGroupRequest handler | ⬜ |
| `crates/context/src/handlers/delete_group.rs` | NEW | DeleteGroupRequest handler | ⬜ |
| `crates/context/src/handlers/add_group_members.rs` | NEW | AddGroupMembersRequest handler | ⬜ |
| `crates/context/src/handlers/remove_group_members.rs` | NEW | RemoveGroupMembersRequest handler | ⬜ |
| `crates/context/src/lib.rs` | MODIFY | Register new handlers with actor | ⬜ |
| `crates/server/src/admin/groups.rs` | NEW | API endpoints for group CRUD | ⬜ |
| `crates/server/src/admin/mod.rs` | MODIFY | Register group routes | ⬜ |

**Testing**:
- Unit tests for group_store helper functions ⬜
- Integration tests for group CRUD via API ⬜
- Contract sandbox tests for group operations ✅ (17 tests in `tests/groups.rs`)
- Permission enforcement tests (non-admin rejection) ✅

**Backward compatibility**: 100% — all new functionality, no existing behavior modified.

---

### Phase 3: Context-Group Integration

**Goal**: Connect context lifecycle to groups — group-aware context creation and deletion.

**Estimated scope**: ~5-8 files, ~500 LOC

#### contracts/ ✅ Complete
| File | Change | Description | Status |
|---|---|---|---|
| `context-config/src/mutate.rs` | MODIFY | Add `register_context_in_group()`, `unregister_context_from_group()` | ✅ |
| `context-config/src/query.rs` | MODIFY | Add `group_contexts()`, `context_group()` queries | ✅ |

#### core/ ⬜ Pending
| File | Change | Description | Status |
|---|---|---|---|
| `crates/context/src/handlers/create_context.rs` | MODIFY | Add group validation, version override, post-creation registration | ⬜ |
| `crates/context/src/handlers/delete_context.rs` | MODIFY | Add post-deletion group deregistration | ⬜ |
| `crates/server/src/admin/groups.rs` | MODIFY | Add `list_contexts` endpoint | ⬜ |

**Testing**:
- Contract sandbox: register/unregister context in group ✅
- Contract sandbox: double-register rejected ✅
- Contract sandbox: `group_contexts()` pagination ✅
- Contract sandbox: `context_group()` reverse lookup ✅
- Integration test: create context with group_id → verify group index updated ⬜
- Integration test: version override on creation ⬜
- Integration test: AppKey mismatch rejection ⬜
- Integration test: non-member rejection ⬜
- Integration test: delete context → verify group index cleaned up ⬜
- Regression test: create context without group_id → unchanged behavior ⬜

**Backward compatibility**: 100% — existing context creation without `group_id` is unchanged.

---

### Phase 4: Upgrade Propagation

**Goal**: Implement the core upgrade orchestration — canary testing, background propagation, status tracking.

**Estimated scope**: ~8-12 files, ~1500 LOC

#### contracts/ ✅ Complete
| File | Change | Description | Status |
|---|---|---|---|
| `context-config/src/mutate.rs` | MODIFY | Add `set_group_target()` | ✅ |

#### core/ ⬜ Pending
| File | Change | Description | Status |
|---|---|---|---|
| `crates/context/src/handlers/upgrade_group.rs` | NEW | UpgradeGroupRequest handler (canary + spawn propagator) | ⬜ |
| `crates/context/src/upgrade_propagator.rs` | NEW | Background upgrade propagator | ⬜ |
| `crates/context/src/handlers/get_group_status.rs` | NEW | GetGroupUpgradeStatusRequest handler | ⬜ |
| `crates/context/src/handlers/retry_group_upgrade.rs` | NEW | RetryGroupUpgradeRequest handler | ⬜ |
| `crates/context/src/lib.rs` | MODIFY | Register new handlers | ⬜ |
| `crates/server/src/admin/groups.rs` | MODIFY | Add upgrade, status, retry endpoints | ⬜ |

**Testing**:
- Contract sandbox: `set_group_target()` + admin rejection + nonexistent group ✅
- Integration test: full upgrade flow ⬜
- Integration test: canary failure → abort ⬜
- Integration test: partial failure → retry ⬜
- Integration test: upgrade status polling ⬜
- Integration test: concurrent upgrade rejection ⬜
- Unit test: propagator logic (mock store) ⬜

**Backward compatibility**: 100% — existing `UpdateApplicationRequest` on individual contexts is unchanged.

---

### Phase 5: Advanced Policies + Crash Recovery

**Goal**: Implement lazy-on-access, coordinated upgrades, rollback, and crash recovery.

**Estimated scope**: ~5-8 files, ~800 LOC

> **Note**: The contracts plan's "Phase 5 — Proxy Proposal Actions" (`ProposalAction::RegisterInGroup` / `UnregisterFromGroup`) has been completed ✅. That is a separate concern from this phase.

#### contracts/ ✅ Complete (proxy governance actions)
| File | Change | Description | Status |
|---|---|---|---|
| `context-proxy/src/mutate.rs` | MODIFY | Handle `RegisterInGroup` and `UnregisterFromGroup` in `execute_proposal()` | ✅ |
| `context-proxy/src/ext_config.rs` | MODIFY | Add `proxy_register_in_group`, `proxy_unregister_from_group` to ext interface | ✅ |
| `context-config/src/mutate.rs` | MODIFY | Add `proxy_register_in_group()` and `proxy_unregister_from_group()` callable by proxy | ✅ |

#### core/ ⬜ Pending
| File | Change | Description | Status |
|---|---|---|---|
| `crates/context/src/handlers/execute.rs` | MODIFY | Add lazy-on-access pre-check | ⬜ |
| `crates/context/src/lib.rs` | MODIFY | Add `recover_in_progress_upgrades()` on startup | ⬜ |
| `crates/server/src/admin/groups.rs` | MODIFY | Add rollback endpoint | ⬜ |

**Testing**:
- Integration test: lazy-on-access ⬜
- Integration test: crash recovery ⬜
- Integration test: rollback flow ⬜
- Integration test: coordinated policy with deadline ⬜

**Backward compatibility**: 100%

---

### Phase 6: Application Integration + SDK (Future)

**Goal**: SDK helpers, application templates, documentation. Out of scope for initial implementation.

---

## 14. Testing Strategy

### 14.1 Unit Tests

| Area | What to Test | Location |
|---|---|---|
| Storage keys | Serialization roundtrip for all 5 key types | `core/crates/store/src/key/group.rs` |
| Group store helpers | CRUD operations on group data | `core/crates/context/src/group_store.rs` |
| Upgrade propagator | Logic with mocked store (version check, skip, failure recording) | `core/crates/context/src/upgrade_propagator.rs` |
| Type serialization | Borsh roundtrip for `ContextGroupId`, `GroupMetaValue`, etc. | `core/crates/primitives/src/context.rs` |

### 14.2 Integration Tests

| Test | Phase | Description |
|---|---|---|
| Contract migration | P1 | Deploy old contract, migrate, verify new fields exist |
| Group CRUD | P2 | Create/delete group, add/remove members via API |
| Permission enforcement | P2 | Non-admin cannot add members, trigger upgrades |
| Context creation with group | P3 | Create context in group, verify index, verify version override |
| Context deletion cleanup | P3 | Delete grouped context, verify group index updated |
| Ungrouped context regression | P3 | Create context without group_id, verify identical behavior |
| Full upgrade flow | P4 | Create group → add contexts → upgrade → verify all at new version |
| Canary failure abort | P4 | Upgrade with broken migration → verify abort, no contexts changed |
| Partial failure + retry | P4 | Upgrade with one bad context → verify partial, retry → verify complete |
| Lazy-on-access | P5 | Set LazyOnAccess policy → execute on stale context → verify transparent upgrade |
| Crash recovery | P5 | Simulate crash during propagation → restart → verify resume |
| Multi-tenant isolation | P4 | Two groups with same AppKey → upgrade one → verify other unchanged |

### 14.3 Contract Sandbox Tests

Follow the existing pattern in `contracts/contracts/near/context-config/tests/sandbox.rs`:

| Test | Description |
|---|---|
| `test_create_group` | Create group, verify storage, query |
| `test_group_admin_only` | Non-admin operations rejected |
| `test_register_context_in_group` | Context + group affiliation |
| `test_set_target_application` | Admin updates target version on-chain |
| `test_unregister_context` | Remove context from group |
| `test_delete_group_with_contexts` | Should fail if contexts remain |
| `test_migration_03` | Old state format deserializes correctly after migration |

### 14.4 Performance Tests

| Test | Description | Threshold |
|---|---|---|
| Group creation overhead | Time to create context with vs without group | <2ms additional |
| Upgrade throughput | Time to upgrade 1000 contexts sequentially | <2 minutes |
| Storage scan | Time to enumerate 10K contexts in a group | <100ms |

---

## 15. Open Questions & Tradeoffs

### 15.1 Resolved in This Spec

| Question | Resolution |
|---|---|
| Group ID derivation | Independent (random), not derived from any context |
| Group member storage | Off-chain (local store) for privacy, on-chain counter only |
| Upgrade orchestration location | Node-local (ContextManager actor), not on-chain |
| Nonce handling for group operations | Follow existing pattern — `validity_threshold_ms` timestamp check rather than per-member nonce (groups don't need per-member nonce tracking since operations are admin-only and idempotent) |

### 15.2 Open — To Be Resolved Before Implementation

| # | Question | Impact | Recommendation |
|---|---|---|---|
| 1 | **Cross-node group awareness**: Should group metadata be gossiped to peer nodes? | If not gossiped, only admin's node can orchestrate upgrades. Peers rely on per-context sync to receive version changes. | **Start without gossip**. Peer nodes get version changes via existing per-context sync. Add gossip later if needed for status reporting. |
| 2 | **Group discovery**: How does a user discover which groups they belong to? Should we add a per-identity index (`PublicKey → Vec<ContextGroupId>`)? | UX for multi-group users. | **Add index in Phase 2**. New key: `IdentityGroupIndex(PublicKey, ContextGroupId) → ()` |
| 3 | **Admin transfer/rotation**: How is admin authority transferred? | Admin key compromise recovery. | **Phase 2**: Add `PromoteToAdmin` / `DemoteAdmin` group request kinds. Require existing admin signature. |
| 4 | **Maximum group size**: Should there be a limit on contexts per group? | Performance for very large groups. | **No hard limit initially**. Monitor performance. Add configurable limit if needed. |
| 5 | **Upgrade concurrency**: Should multiple upgrades be queued? | Admin tries v1→v2 while v2→v3 is pending. | **One active upgrade per group**. Return `409 Conflict` for concurrent attempts. |
| 6 | **Member removal + context cleanup**: What happens to contexts created by a removed member? | Orphaned contexts. | **Contexts remain in group** even if creator is removed. Context membership is independent of group membership. |
| 7 | **On-chain gas batching**: For large groups, batch operations? | Gas costs for 10K `RegisterContext` calls. | **Support batch operations** in contract: `RegisterContexts { context_ids: Vec<...> }`. Implement in Phase 3. |
| 8 | **Canary failure + recovery**: If canary context state is corrupted by partial migration, how to recover? | Data loss risk. | **Existing migration system is atomic** within a single context (write_migration_state is all-or-nothing). If the migration function panics, state is rolled back. Document this guarantee. |
| 9 | **Group metadata sync**: Should group metadata be shared across nodes for the same group? | Multi-node admin operations. | **Defer**. Initially, group operations are admin-node-local. Cross-node group sync is a separate feature. |

### 15.3 Tradeoffs Made

| Decision | Tradeoff | Rationale |
|---|---|---|
| Sequential upgrade propagation | Slower for large groups | Simpler, no concurrency bugs, predictable resource usage |
| Off-chain member storage | Not auditable on-chain | Privacy: member identities shouldn't be public |
| Single admin model (initially) | No multi-sig governance | Simplicity; multi-admin can be added in Phase 2 |
| No upgrade queueing | Admin must wait for current upgrade | Avoids complex state machine for queued/conflicting upgrades |
| Canary is first context by ID | Not user-selectable | Deterministic, no decision paralysis, can be made configurable later |

---

## 16. Rollout Strategy

### 16.1 Prerequisites

1. Contract migration tested on testnet
2. Core runtime changes reviewed and tested
3. Existing test suite passes with zero regressions
4. Performance benchmarks within thresholds

### 16.2 Deployment Order

1. **Deploy contract migration** (Phase 1):
   - Deploy updated `context-config` contract to testnet
   - Run migration `03_context_groups`
   - Verify all existing contexts still function
   - Repeat on mainnet

2. **Deploy core runtime** (Phases 2-4):
   - Release new node version with group support
   - Nodes without groups behave identically to before
   - Early adopters can create groups and test

3. **Deploy advanced policies** (Phase 5):
   - Lazy-on-access, crash recovery
   - Requires node restart to activate

### 16.3 Feature Flags

No runtime feature flags needed. The feature is entirely opt-in:
- Contexts without `group_id` are unchanged
- New API endpoints are simply available
- Groups are only created when admin explicitly creates one

### 16.4 Monitoring

During rollout, monitor:
- Upgrade propagation duration (P50, P95, P99)
- Failed context upgrades per group
- Storage growth from group indices
- On-chain gas costs for group operations
- API latency for context creation with group (vs without)

---

## Appendix A: File Change Summary

### New Files

| File | Phase | LOC (est.) | Status |
|---|---|---|---|
| `contracts/.../migrations/03_context_groups.rs` | P1 | ~70 | ✅ |
| `contracts/.../tests/groups.rs` | P2 | ~1513 | ✅ |
| `core/crates/store/src/key/group.rs` | P1 | ~250 | ⬜ |
| `core/crates/context/primitives/src/group.rs` | P2 | ~200 | ⬜ |
| `core/crates/context/src/group_store.rs` | P2 | ~300 | ⬜ |
| `core/crates/context/src/handlers/create_group.rs` | P2 | ~100 | ⬜ |
| `core/crates/context/src/handlers/delete_group.rs` | P2 | ~80 | ⬜ |
| `core/crates/context/src/handlers/add_group_members.rs` | P2 | ~80 | ⬜ |
| `core/crates/context/src/handlers/remove_group_members.rs` | P2 | ~80 | ⬜ |
| `core/crates/server/src/admin/groups.rs` | P2 | ~300 | ⬜ |
| `core/crates/context/src/handlers/upgrade_group.rs` | P4 | ~200 | ⬜ |
| `core/crates/context/src/upgrade_propagator.rs` | P4 | ~250 | ⬜ |
| `core/crates/context/src/handlers/get_group_status.rs` | P4 | ~50 | ⬜ |
| `core/crates/context/src/handlers/retry_group_upgrade.rs` | P4 | ~100 | ⬜ |

### Modified Files

| File | Phase | Nature of Change | Status |
|---|---|---|---|
| `contracts/.../context-config/src/lib.rs` | P1 | Add structs, Prefix variants | ✅ |
| `contracts/.../context-config/src/mutate.rs` | P2-P5 | Group mutation methods, proxy-callable methods | ✅ |
| `contracts/.../context-config/src/query.rs` | P2-P3 | Add group query methods | ✅ |
| `contracts/.../context-config/src/sys.rs` | P1 | Group/ref cleanup in `erase()` | ✅ |
| `contracts/.../context-config/src/sys/migrations.rs` | P1 | Register migration 03 | ✅ |
| `contracts/.../context-config/Cargo.toml` | P1 | Feature gate | ✅ |
| `contracts/.../context-proxy/src/mutate.rs` | P5 | Handle `RegisterInGroup`, `UnregisterFromGroup` | ✅ |
| `contracts/.../context-proxy/src/ext_config.rs` | P5 | Add proxy group methods to ext interface | ✅ |
| `core/crates/context/config/src/lib.rs` (shared types) | P1 | `GroupRequest`, `GroupRequestKind`, `RequestKind::Group`, `ProposalAction` variants | ✅ |
| `core/crates/context/config/src/types.rs` (shared types) | P1 | `ContextGroupId`, `AppKey` | ✅ |
| `core/crates/primitives/src/context.rs` | P1 | Add `UpgradePolicy`, `GroupMemberRole` (runtime types) | ⬜ |
| `core/crates/store/src/key/mod.rs` | P1 | Add `pub mod group;` | ⬜ |
| `core/crates/context/primitives/src/lib.rs` | P1-P2 | Add `group_id` to `CreateContextRequest`, add `pub mod group;` | ⬜ |
| `core/crates/context/src/handlers/create_context.rs` | P3 | Group validation + version override + post-creation registration | ⬜ |
| `core/crates/context/src/handlers/delete_context.rs` | P3 | Post-deletion group deregistration | ⬜ |
| `core/crates/context/src/lib.rs` | P2-P5 | Register handlers, crash recovery | ⬜ |
| `core/crates/server/src/admin/mod.rs` | P2 | Register group routes | ⬜ |
| `core/crates/context/src/handlers/execute.rs` | P5 | Lazy-on-access pre-check | ⬜ |

### Unchanged Files (Explicitly)

| Component | Why Unchanged |
|---|---|
| Sync layer (`core/crates/network/`) | Per-context sync is independent; no group-level sync needed |
| Registry contract | Groups reference `ApplicationId` directly; no registry changes |
| WASM runtime | Migration execution is per-context; existing runtime sufficient |
| Context state/Merkle tree | Group is metadata-only; state isolation preserved |
| DAG/delta handling | Per-context; unaffected by group membership |

---

*End of specification.*
