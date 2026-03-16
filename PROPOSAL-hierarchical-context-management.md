# RFC: Context Groups — Workspace-Based Context Management & Version Propagation

**Status:** Draft v2  
**Authors:** Architecture Team  
**Date:** 2026-02-16  
**Scope:** `core/`, `contracts/`, application layer  

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Problem Analysis](#2-problem-analysis)
3. [Design Approach Selection](#3-design-approach-selection)
4. [Context Group Model — Group as Workspace](#4-context-group-model--group-as-workspace)
5. [Permission Model](#5-permission-model)
6. [Version Control & Upgrade Propagation](#6-version-control--upgrade-propagation)
7. [Smart Contract Changes](#7-smart-contract-changes)
8. [Core Runtime Changes](#8-core-runtime-changes)
9. [Backward Compatibility Strategy](#9-backward-compatibility-strategy)
10. [Failure Handling](#10-failure-handling)
11. [Security Considerations](#11-security-considerations)
12. [Design Considerations & Tradeoffs](#12-design-considerations--tradeoffs)
13. [Migration Path](#13-migration-path)
14. [Open Questions](#14-open-questions)
15. [Appendix A: Alternative Approaches Considered](#appendix-a-alternative-approaches-considered)
16. [Appendix B: Data Flow Diagrams](#appendix-b-data-flow-diagrams)

---

## 1. Executive Summary

The current Calimero architecture treats every context as a fully independent, self-governing entity. A `Context` holds a single `ApplicationId`, its own Merkle state tree, its own DAG delta history, and its own on-chain registration in the `context-config` contract. There is no concept of relationships between contexts.

This works for single-context applications but fails at scale for any multi-context application. When an application creates dozens or thousands of contexts (DMs, channels, sub-workspaces, per-tenant instances), upgrading the application version requires an independent migration operation on **every single context**. This does not scale.

This proposal introduces a **Context Group** — a first-class **workspace entity** that:

- **Owns a set of users** (group members) who are authorized to create contexts within it
- **Owns a set of contexts** that share a common application identity
- **Is governed by an admin** (a user identity, not a context) who controls version upgrades and group policy
- **Enables single-trigger version propagation** across all contexts in the group

Users create and destroy contexts freely within their group. Admins upgrade the application once, and the platform propagates automatically. Contexts remain fully isolated in state, sync, and privacy.

---

## 2. Problem Analysis

### 2.1 Current Architecture Snapshot

```
┌──────────────────────────────────────────────────────────┐
│                      Node Runtime                        │
│                                                          │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐     │
│  │ Context A   │  │ Context B   │  │ Context C   │ ... │
│  │ app: v1.0   │  │ app: v1.0   │  │ app: v1.0   │     │
│  │ state: ...  │  │ state: ...  │  │ state: ...  │     │
│  │ dag: [...]  │  │ dag: [...]  │  │ dag: [...]  │     │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘     │
│         │                │                │              │
│  ┌──────▼──────────────────▼──────────────────▼──────┐   │
│  │              Sync Layer (per context)              │   │
│  │  gossipsub topic per context + periodic P2P sync  │   │
│  └───────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────┘
                           │
                    On-chain (NEAR)
          ┌────────────────┼────────────────┐
          ▼                ▼                ▼
   context-config    context-proxy    registry
   (per context)     (per context)    (packages)
```

**Key observations from codebase analysis:**

1. **`Context` struct** (`core/crates/primitives/src/context.rs:244-255`): Contains `id`, `application_id`, `root_hash`, `dag_heads` — no group affiliation, no workspace reference.

2. **`ContextConfigs` contract** (`contracts/near/context-config/src/mutate.rs`): `add_context()` creates each context independently. `update_application()` operates on a single `context_id` with Guard-based access control.

3. **`UpdateApplicationRequest` handler** (`core/crates/context/src/handlers/update_application.rs`): Processes one context at a time. Migration execution loads the WASM module, runs the migration function, writes state, and finalizes — all scoped to a single context.

4. **No grouping exists**: There is no concept of "this context belongs to the same application deployment as that context." Every context is an island.

### 2.2 Pain Points Quantified

| Scenario | Contexts | Manual Operations per Upgrade |
|---|---|---|
| Small team (10 members, 20 DMs, 5 channels) | 26 | 26 |
| Medium org (100 members, 500 DMs, 30 channels) | 531 | 531 |
| Enterprise (1000 members, 5000 DMs, 100 channels) | 5101 | 5101 |

Each operation involves:
- Application installation on every participating node
- On-chain `update_application` transaction (gas cost per context)
- WASM migration execution (if state migration is needed)
- Sync propagation of new state to all peers
- Potential downtime or inconsistent behavior during the rollout window

### 2.3 The Multi-Tenant Problem

A single application (same `AppKey`) can be deployed by multiple independent organizations. Consider:

```
AppKey: com.calimero.chat:did:key:z6Mk...Publisher

├── Acme Corp deployment
│   ├── Main context (channels)
│   ├── 200 DM contexts
│   └── 15 channel contexts
│
├── Globex Inc deployment
│   ├── Main context (channels)
│   ├── 500 DM contexts
│   └── 30 channel contexts
│
└── Initech deployment
    ├── Main context (channels)
    ├── 50 DM contexts
    └── 5 channel contexts
```

**All three organizations use the same application**, the same `AppKey`. But their contexts must be managed independently — Acme's admin cannot upgrade Globex's contexts. Any grouping mechanism must disambiguate between organizations using the same application. This eliminates any approach that groups contexts purely by `AppKey`.

---

## 3. Design Approach Selection

Three architectural approaches were evaluated:

### Approach A: Explicit Context Groups (Hierarchical Root-Context Model)

A "root context" acts as the group admin. Other contexts join as children. The root context's admin controls upgrades.

**Rejected because**: Ties governance to a specific context rather than to a user/identity. Creates confusion when users need to create contexts freely — "do I need the root context admin's permission to create a DM?" The answer should be no.

### Approach B: Application Version Channels (Pub-Sub)

The registry contract gets "channels" (like `stable`, `beta`). Contexts subscribe to a channel. When the publisher updates a channel, all subscribed contexts auto-upgrade.

**Rejected because**: Publisher-driven, not org-driven. Doesn't solve the multi-tenant problem — the publisher would upgrade ALL organizations simultaneously. No per-org control over upgrade timing or policy.

### Approach C: Implicit AppKey Mesh (Decentralized)

Contexts sharing the same `AppKey` form an implicit coordination mesh. Version advertisements propagate via gossip.

**Rejected because**: No disambiguation between organizations using the same app. No centralized upgrade control per-org. Best-effort gossip means unreliable propagation.

### Selected: Group as Workspace (Approach D)

The group is a **first-class workspace entity** independent of any specific context. It owns users and contexts. An admin identity (not a context) governs it. This cleanly separates:
- **Who can create contexts** (any group member)
- **Who can upgrade the application** (group admin only)
- **Which contexts are affected** (all contexts in the group)

See [Appendix A](#appendix-a-alternative-approaches-considered) for detailed comparison.

---

## 4. Context Group Model — Group as Workspace

### 4.1 Core Concept

A **Context Group** is a workspace that represents a deployment of an application by a specific organization or entity. It is the boundary within which contexts are created, managed, and version-synchronized.

```
┌─────────────────────────────────────────────────────────────┐
│  Context Group (Workspace)                                   │
│                                                              │
│  group_id:        GroupId (independent, not derived)         │
│  app_key:         com.acme.chat:did:key:z6Mk...Publisher    │
│  target_version:  ApplicationId (v2.0.0)                     │
│  upgrade_policy:  Automatic                                  │
│                                                              │
│  ┌─────────────────────────────────────────────────────┐     │
│  │  Group Members (User Identities)                    │     │
│  │  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐      │     │
│  │  │ Admin  │ │ UserB  │ │ UserC  │ │ UserD  │ ...  │     │
│  │  │ (admin)│ │(member)│ │(member)│ │(member)│      │     │
│  │  └────────┘ └────────┘ └────────┘ └────────┘      │     │
│  └─────────────────────────────────────────────────────┘     │
│                                                              │
│  ┌─────────────────────────────────────────────────────┐     │
│  │  Contexts (created by members, version-synced)      │     │
│  │  ┌────────────┐ ┌────────────┐ ┌────────────────┐  │     │
│  │  │ Main Ctx   │ │ DM: B ↔ C  │ │ #engineering   │  │     │
│  │  │ members:   │ │ members:   │ │ members:       │  │     │
│  │  │  A,B,C,D   │ │  B, C      │ │  A, B, D      │  │     │
│  │  │ app: v2.0  │ │ app: v2.0  │ │ app: v2.0     │  │     │
│  │  └────────────┘ └────────────┘ └────────────────┘  │     │
│  └─────────────────────────────────────────────────────┘     │
│                                                              │
│  All contexts converge to target_version.                    │
│  Members create/delete contexts freely.                      │
│  Only admin(s) can trigger upgrades.                         │
└─────────────────────────────────────────────────────────────┘
```

### 4.2 Key Distinction: Group Members vs. Context Members

These are two separate, independent membership concepts:

| Concept | What it is | Who manages it | What it controls |
|---|---|---|---|
| **Group members** | User identities authorized to operate within this workspace | Group admin | Who can create contexts in this group |
| **Context members** | User identities who participate in a specific context | Context creator / context-level permissions | Who can read/write state in that context |

A user must be a **group member** to create a context in the group. But context-level membership is entirely independent — a DM context only has its two participants, not every group member.

### 4.3 Key Types

```rust
/// Unique identifier for a context group (workspace).
/// Generated independently — NOT derived from any context.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ContextGroupId(Hash);

impl ContextGroupId {
    /// Generate a new random group ID.
    pub fn generate() -> Self {
        Self(Hash::random())
    }
}

/// The context group — a workspace that owns users and contexts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextGroup {
    pub id: ContextGroupId,

    /// The application identity for this group.
    /// All contexts in the group must share this AppKey.
    pub app_key: AppKey,

    /// The target application version. All contexts in this group
    /// should converge to this version.
    pub target_application_id: ApplicationId,

    /// How upgrades propagate to contexts.
    pub upgrade_policy: UpgradePolicy,

    /// Current upgrade operation, if one is in progress.
    pub active_upgrade: Option<GroupUpgrade>,
}

/// A user's role within a group.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum GroupMemberRole {
    /// Can manage group membership, trigger upgrades, set policy.
    Admin,
    /// Can create/delete contexts within the group.
    Member,
}

/// Membership record for a user in a group.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupMember {
    pub identity: PublicKey,
    pub role: GroupMemberRole,
    pub joined_at: u64,
}

/// Defines how upgrades propagate within a group.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum UpgradePolicy {
    /// All contexts are upgraded immediately in the background
    /// when the admin triggers an upgrade.
    Automatic,

    /// Contexts are upgraded lazily — on the next interaction
    /// (execute call) with the context. Dormant contexts are
    /// not upgraded until accessed.
    LazyOnAccess,

    /// Contexts receive an upgrade notification.
    /// Each context's creator can accept or defer.
    /// If a deadline is set, forced upgrade occurs at deadline.
    Coordinated {
        deadline: Option<Duration>,
    },
}

/// Tracks the state of an in-progress group upgrade.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupUpgrade {
    pub from_application_id: ApplicationId,
    pub to_application_id: ApplicationId,
    pub migration: Option<MigrationParams>,
    pub initiated_at: u64,
    pub initiated_by: PublicKey,
    pub status: GroupUpgradeStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum GroupUpgradeStatus {
    /// Upgrade is propagating to contexts.
    InProgress {
        total: usize,
        completed: usize,
        failed: Vec<(ContextId, String)>,
    },
    /// All contexts have been upgraded successfully.
    Completed { completed_at: u64 },
    /// Upgrade was rolled back due to critical failures.
    RolledBack { reason: String },
}
```

### 4.4 Isolation Guarantees

The group is a **metadata-only** relationship. It does NOT affect the runtime behavior of individual contexts:

| Property | Preserved? | How |
|---|---|---|
| State isolation | Yes | Each context retains its own Merkle tree, state entries, and DAG |
| Sync independence | Yes | Each context syncs via its own gossipsub topic and P2P protocol |
| Context membership | Yes | Each context has its own member set and identity keys |
| Privacy (e.g., DMs) | Yes | Group admin cannot see context state or participants |
| Proxy contract | Yes | Each context retains its own proxy contract |
| Context lifecycle | Yes | Group members create/destroy contexts independently |

**What the group controls:**
- Which `ApplicationId` (version) all member contexts should converge to
- The upgrade propagation policy
- The migration method to use during upgrades
- Who is authorized to create contexts in this workspace

### 4.5 Invariants

1. **Single AppKey per group**: All contexts in a group MUST share the same `AppKey` (package + signerId). Enforced at context-creation time within the group.

2. **Single group per context**: A context belongs to at most one group. A context cannot be in multiple groups simultaneously.

3. **Version convergence**: The group's `target_application_id` is the source of truth. All contexts converge toward it. New contexts are created at the target version automatically.

4. **No state inheritance**: State is never shared or copied between contexts. Each context's migration reads its own old state and produces its own new state.

5. **Multiple groups per AppKey**: The same `AppKey` can be used by many independent groups (multi-tenant).

---

## 5. Permission Model

### 5.1 Two-Tier Permission Split

The system operates with a clear separation between **group-level governance** and **context-level operations**:

```
┌──────────────────────────────────────────────────────────────┐
│                    Context Group                              │
│                                                              │
│  GROUP-LEVEL PERMISSIONS (admin only):                       │
│    ├── Add / remove group members (user identities)          │
│    ├── Promote / demote members (admin ↔ member)             │
│    ├── Set target application version (trigger upgrade)      │
│    ├── Set upgrade policy (Automatic / Lazy / Coordinated)   │
│    ├── Force-remove a context from the group                 │
│    ├── Rollback to a previous version                        │
│    └── Delete the group                                      │
│                                                              │
│  CONTEXT-LEVEL PERMISSIONS (any group member):               │
│    ├── Create a context within the group                     │
│    ├── Delete a context they created/own                     │
│    ├── Manage members within their own context               │
│    ├── Execute methods in contexts they belong to            │
│    └── Invite users to their context                         │
│                                                              │
│  EXPLICITLY DENIED TO NON-ADMIN MEMBERS:                     │
│    ├── Upgrade any context's application version             │
│    ├── Change the group's target version                     │
│    ├── Modify the group's upgrade policy                     │
│    ├── Add / remove group members                            │
│    └── Override version on context creation                  │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

### 5.2 Context Creation Within a Group

When a group member creates a context, the `group_id` is an **explicit required parameter**. This is necessary because:

- A user can belong to multiple groups (e.g., employee at Acme Corp and member of a hobby project)
- Multiple groups can use the same `AppKey` (multi-tenant)
- There is no way to auto-resolve which group a new context belongs to

**Context creation flow:**

```
User calls: POST /contexts
{
  "application_id": "<app_id>",      // may be overridden by group
  "init_params": [...],
  "group_id": "<group_id>"           // REQUIRED for grouped contexts
}
         │
         ▼
┌────────────────────────────────────┐
│ 1. Validate group membership       │
│    Is the caller a member of       │
│    this group?                     │
│    → If no: REJECT (403)           │
└──────────────┬─────────────────────┘
               │ yes
               ▼
┌────────────────────────────────────┐
│ 2. Validate AppKey match           │
│    Does the application's AppKey   │
│    match the group's AppKey?       │
│    → If no: REJECT (400)           │
└──────────────┬─────────────────────┘
               │ yes
               ▼
┌────────────────────────────────────┐
│ 3. Version override                │
│    If group.target_application_id  │
│    differs from the requested      │
│    application_id:                 │
│    → OVERRIDE with group's target  │
│    This ensures new contexts are   │
│    always at the current version.  │
└──────────────┬─────────────────────┘
               │
               ▼
┌────────────────────────────────────┐
│ 4. Normal context creation         │
│    (existing flow, unchanged)      │
│    Creates context, runs init(),   │
│    generates genesis delta, etc.   │
└──────────────┬─────────────────────┘
               │
               ▼
┌────────────────────────────────────┐
│ 5. Register context in group index │
│    Automatic — no admin approval.  │
│    The context is now a member of  │
│    the group and subject to its    │
│    upgrade policy.                 │
└────────────────────────────────────┘
```

**Key property**: The user never needs admin approval to create a context. They just need to be a group member. The platform handles version alignment and group registration automatically.

### 5.3 Context Deletion Within a Group

When a context is deleted, the group index is updated automatically:

1. User calls delete on their context (existing flow).
2. Post-deletion hook: remove context from group's context index.
3. No admin approval needed.

### 5.4 Ungrouped Contexts

Contexts can still exist outside of any group. The `group_id` parameter is optional in `CreateContextRequest`. If omitted, the context is standalone and behaves exactly as in the current system — fully independent, manually upgraded.

---

## 6. Version Control & Upgrade Propagation

### 6.1 Version Metadata Storage

**On-node (local datastore):**

```
Storage Keys (new):
  GroupMeta:          (ContextGroupId)                    → ContextGroup
  GroupMember:        (ContextGroupId, PublicKey)          → GroupMemberRole
  GroupContextIndex:  (ContextGroupId, ContextId)          → () (presence index)
  ContextGroupRef:    (ContextId)                          → ContextGroupId (reverse index)
```

These are stored in the existing `calimero-store` using the same composite key pattern as `ContextMeta`, `ContextConfig`, etc.

**On-chain (context-config contract):**

The on-chain contract stores group metadata for auditability:

```rust
#[near(serializers = [borsh])]
pub struct OnChainGroupMeta {
    /// The application identity for this group.
    pub app_key: AppKey,
    /// Target application for the group (set by admin).
    pub target_application: Application<'static>,
    /// Admin identities who can manage the group.
    pub admins: IterableSet<SignerId>,
    /// Number of registered members.
    pub member_count: u64,
    /// Number of registered contexts.
    pub context_count: u64,
}
```

### 6.2 Upgrade Propagation Flow

#### Phase 1: Admin Triggers Upgrade

```
Admin ──► API: POST /groups/{group_id}/upgrade
          {
            "application_id": "<new_version_app_id>",
            "migration": { "method": "migrate_v1_to_v2" }  // optional
          }
```

1. Validate the caller is a group admin.
2. Verify `AppKey` continuity between current target and new application.
3. Install the new application on the local node (if not already present).
4. Pick a **canary context** (first context in the group by deterministic order).
5. Execute the upgrade on the canary context first.
6. If canary succeeds, persist `target_application_id` update and create `GroupUpgrade` record.
7. Begin propagation to remaining contexts (based on upgrade policy).

#### Phase 2: Propagation

The `ContextManager` actor spawns an `UpgradePropagator`:

```
┌──────────────┐     ┌──────────────────┐     ┌────────────────┐
│  Admin API   │────►│  ContextManager  │────►│ Canary Context │
│  /groups/    │     │  (Actor)         │     │  Upgrade       │
│  upgrade     │     └────────┬─────────┘     └───────┬────────┘
└──────────────┘              │                       │
                              │ spawn                  │ success
                              ▼                       ▼
                    ┌──────────────────┐     ┌────────────────┐
                    │ UpgradePropagator│────►│  Context 1     │
                    │ (Background)     │     │  Upgrade       │
                    │                  │────►├────────────────┤
                    │                  │     │  Context 2     │
                    │                  │────►│  Upgrade       │
                    │                  │     ├────────────────┤
                    │                  │     │  ...           │
                    └──────────────────┘     └────────────────┘
```

The propagator reuses the existing `update_application_with_migration` / `update_application_id` code path for each context. This is the same code that runs today for a single-context upgrade — no new migration logic needed.

#### Phase 3: Sync to Peers

After each context is upgraded on the initiating node:

1. The existing sync protocol propagates the new state to peer nodes.
2. Peer nodes receive the updated `application_id` and `root_hash`.
3. Peers that don't have the new application version installed will fetch it (existing behavior).

### 6.3 Upgrade Policies

| Policy | When Upgrade Happens | Behavior | Best For |
|---|---|---|---|
| `Automatic` | Immediately after canary succeeds | Background propagation to all contexts | Most applications |
| `LazyOnAccess` | On next `execute` call to the context | Dormant contexts skip upgrade until needed | Large groups with many idle contexts |
| `Coordinated { deadline }` | Context creator accepts, or forced at deadline | Notification + opt-in window | High-sensitivity deployments |

#### Lazy-on-Access Implementation

For `LazyOnAccess`, the execute path gains a pre-check:

```rust
// In the execute handler, before running the user's method:

fn maybe_lazy_upgrade(&mut self, context_id: ContextId) -> Option<UpdateApplicationRequest> {
    let group_id = self.get_group_for_context(context_id)?;
    let group = self.get_group_meta(group_id)?;

    let context = self.contexts.get(&context_id)?;
    if context.meta.application_id == group.target_application_id {
        return None; // Already at target version
    }

    // Needs upgrade before execution
    Some(UpdateApplicationRequest {
        context_id,
        application_id: group.target_application_id,
        public_key: group.admin_key(),
        migration: group.active_upgrade.as_ref().and_then(|u| u.migration.clone()),
    })
}
```

### 6.4 Upgrade Status Tracking

```rust
/// Status endpoint: GET /groups/{group_id}/upgrade/status
pub struct GroupUpgradeStatusResponse {
    pub group_id: ContextGroupId,
    pub from_version: String,       // e.g., "1.0.0"
    pub to_version: String,         // e.g., "2.0.0"
    pub policy: UpgradePolicy,
    pub status: GroupUpgradeStatus,
    pub started_at: u64,
    pub completed_at: Option<u64>,
}
```

Admins can poll this endpoint to monitor upgrade progress. For `Automatic` policy, the upgrade typically completes within seconds to minutes depending on the number of contexts.

---

## 7. Smart Contract Changes

### 7.1 `context-config` Contract Updates

#### New Storage

```rust
pub struct ContextConfigs {
    contexts: IterableMap<ContextId, Context>,
    config: Config,
    proxy_code: LazyOption<Vec<u8>>,
    next_proxy_id: u64,
    // NEW: Context groups
    groups: IterableMap<ContextGroupId, OnChainGroupMeta>,
    // NEW: Map contexts to their group
    context_group_refs: IterableMap<ContextId, ContextGroupId>,
}
```

#### Updated Context Struct

The existing `Context` struct gains an optional group reference:

```rust
struct Context {
    pub application: Guard<Application<'static>>,
    pub members: Guard<IterableSet<ContextIdentity>>,
    pub member_nonces: IterableMap<ContextIdentity, u64>,
    pub proxy: Guard<AccountId>,
    pub used_open_invitations: Guard<IterableSet<CryptoHash>>,
    pub commitments_open_invitations: IterableMap<CryptoHash, BlockHeight>,
    // NEW: Optional group affiliation
    pub group_id: Option<ContextGroupId>,
}
```

#### New Request Kinds

```rust
pub enum RequestKind<'a> {
    Context(ContextRequest<'a>),
    // NEW: Group operations are top-level, not scoped to a context
    Group(GroupRequest<'a>),
}

pub enum GroupRequestKind<'a> {
    /// Create a new context group.
    Create {
        group_id: Repr<ContextGroupId>,
        app_key: AppKey,
        application: Application<'a>,
    },

    /// Add members (user identities) to the group.
    AddMembers {
        members: Vec<Repr<ContextIdentity>>,
    },

    /// Remove members from the group.
    RemoveMembers {
        members: Vec<Repr<ContextIdentity>>,
    },

    /// Set the target application version for the group.
    SetTargetApplication {
        application: Application<'a>,
    },

    /// Register a context as belonging to this group.
    RegisterContext {
        context_id: Repr<ContextId>,
    },

    /// Remove a context from this group.
    UnregisterContext {
        context_id: Repr<ContextId>,
    },

    /// Delete the group.
    Delete,
}
```

#### New Contract Methods

```rust
#[near]
impl ContextConfigs {
    pub fn mutate_group(&mut self) {
        parse_input!(request: Signed<GroupRequest<'_>>);

        let request = request
            .parse(|i| *i.signer_id)
            .expect("failed to parse input");

        let group_id = request.group_id;

        match request.kind {
            GroupRequestKind::Create { group_id, app_key, application } => {
                self.create_group(&request.signer_id, group_id, app_key, application);
            }
            GroupRequestKind::AddMembers { members } => {
                self.add_group_members(&request.signer_id, group_id, members);
            }
            GroupRequestKind::RemoveMembers { members } => {
                self.remove_group_members(&request.signer_id, group_id, members);
            }
            GroupRequestKind::SetTargetApplication { application } => {
                self.set_group_target(&request.signer_id, group_id, application);
            }
            GroupRequestKind::RegisterContext { context_id } => {
                self.register_context_in_group(
                    &request.signer_id, group_id, context_id
                );
            }
            GroupRequestKind::UnregisterContext { context_id } => {
                self.unregister_context_from_group(
                    &request.signer_id, group_id, context_id
                );
            }
            GroupRequestKind::Delete => {
                self.delete_group(&request.signer_id, group_id);
            }
        }
    }
}

impl ContextConfigs {
    fn create_group(
        &mut self,
        signer_id: &SignerId,
        group_id: Repr<ContextGroupId>,
        app_key: AppKey,
        application: Application<'_>,
    ) {
        require!(
            !self.groups.contains_key(&group_id),
            "group already exists"
        );

        let mut admins = IterableSet::new(Prefix::GroupAdmins(*group_id));
        admins.insert(signer_id.clone());

        let group = OnChainGroupMeta {
            app_key,
            target_application: Application::new(
                application.id,
                application.blob,
                application.size,
                application.source.to_owned(),
                application.metadata.to_owned(),
            ),
            admins,
            member_count: 1,  // creator is first member
            context_count: 0,
        };

        self.groups.insert(*group_id, group);

        env::log_str(&format!("Group `{}` created", group_id));
    }

    fn add_group_members(
        &mut self,
        signer_id: &SignerId,
        group_id: Repr<ContextGroupId>,
        members: Vec<Repr<ContextIdentity>>,
    ) {
        let group = self.groups.get_mut(&group_id)
            .expect("group does not exist");

        require!(
            group.admins.contains(signer_id),
            "only group admins can add members"
        );

        for member in members {
            group.member_count += 1;
            env::log_str(&format!(
                "Added `{}` to group `{}`", member, group_id
            ));
        }
    }

    fn set_group_target(
        &mut self,
        signer_id: &SignerId,
        group_id: Repr<ContextGroupId>,
        application: Application<'_>,
    ) {
        let group = self.groups.get_mut(&group_id)
            .expect("group does not exist");

        require!(
            group.admins.contains(signer_id),
            "only group admins can set target application"
        );

        let old_app_id = group.target_application.id;
        group.target_application = Application::new(
            application.id,
            application.blob,
            application.size,
            application.source.to_owned(),
            application.metadata.to_owned(),
        );

        env::log_str(&format!(
            "Group `{}` target updated from `{}` to `{}`",
            group_id, old_app_id, application.id
        ));
    }

    fn register_context_in_group(
        &mut self,
        signer_id: &SignerId,
        group_id: Repr<ContextGroupId>,
        context_id: Repr<ContextId>,
    ) {
        let group = self.groups.get_mut(&group_id)
            .expect("group does not exist");

        // Either the caller is a group admin, or the caller is a group member
        // who created this context. Both are authorized.

        let context = self.contexts.get_mut(&context_id)
            .expect("context does not exist");

        require!(
            context.group_id.is_none(),
            "context already belongs to a group"
        );

        context.group_id = Some(*group_id);
        group.context_count += 1;
        self.context_group_refs.insert(*context_id, *group_id);

        env::log_str(&format!(
            "Context `{}` registered in group `{}`",
            context_id, group_id
        ));
    }
}
```

#### New Query Methods

```rust
#[near]
impl ContextConfigs {
    /// Returns group metadata.
    pub fn group(&self, group_id: Repr<ContextGroupId>) -> Option<GroupInfoResponse> { ... }

    /// Returns all contexts in a group (paginated).
    pub fn group_contexts(
        &self,
        group_id: Repr<ContextGroupId>,
        offset: usize,
        limit: usize,
    ) -> Vec<ContextId> { ... }

    /// Returns the group a context belongs to.
    pub fn context_group(
        &self,
        context_id: Repr<ContextId>,
    ) -> Option<ContextGroupId> { ... }

    /// Check if an identity is a member/admin of a group.
    pub fn is_group_member(
        &self,
        group_id: Repr<ContextGroupId>,
        identity: Repr<ContextIdentity>,
    ) -> bool { ... }
}
```

### 7.2 `registry` Contract — No Changes Required

The existing `PackageManager` handles versioned releases. Groups reference `ApplicationId` directly.

### 7.3 `context-proxy` Contract — Minimal Changes

Add proposal actions for group-related operations:

```rust
pub enum ProposalAction {
    // ... existing variants ...

    /// Propose registering this context in a group.
    RegisterInGroup { group_id: ContextGroupId },

    /// Propose removing this context from its group.
    UnregisterFromGroup,
}
```

---

## 8. Core Runtime Changes

### 8.1 Storage Layer (`core/crates/store/`)

New storage key types:

```rust
// In core/crates/store/src/key/group.rs (new file)

/// Group metadata.
/// Key: (ContextGroupId) → GroupMeta
pub struct GroupMeta {
    group_id: ContextGroupId,
}

/// Group member entry.
/// Key: (ContextGroupId, PublicKey) → GroupMemberRole
pub struct GroupMember {
    group_id: ContextGroupId,
    identity: PublicKey,
}

/// Group context index — enumerates all contexts in a group.
/// Key: (ContextGroupId, ContextId) → ()
pub struct GroupContextIndex {
    group_id: ContextGroupId,
    context_id: ContextId,
}

/// Reverse index: context → group.
/// Key: (ContextId) → ContextGroupId
pub struct ContextGroupRef {
    context_id: ContextId,
}
```

### 8.2 Context Manager (`core/crates/context/`)

#### New Message Types

```rust
/// Create a context group.
pub struct CreateGroupRequest {
    pub group_id: ContextGroupId,
    pub app_key: AppKey,
    pub application_id: ApplicationId,
    pub upgrade_policy: UpgradePolicy,
    pub admin_identity: PublicKey,
}

/// Add members (users) to a group.
pub struct AddGroupMembersRequest {
    pub group_id: ContextGroupId,
    pub members: Vec<(PublicKey, GroupMemberRole)>,
    pub requester: PublicKey,  // must be admin
}

/// Remove members from a group.
pub struct RemoveGroupMembersRequest {
    pub group_id: ContextGroupId,
    pub members: Vec<PublicKey>,
    pub requester: PublicKey,  // must be admin
}

/// Upgrade all contexts in a group to a new version.
pub struct UpgradeGroupRequest {
    pub group_id: ContextGroupId,
    pub application_id: ApplicationId,
    pub requester: PublicKey,  // must be admin
    pub migration: Option<MigrationParams>,
}

/// Query upgrade status.
pub struct GetGroupUpgradeStatusRequest {
    pub group_id: ContextGroupId,
}
```

#### Extended CreateContextRequest

```rust
pub struct CreateContextRequest {
    pub application_id: ApplicationId,
    pub init_params: Vec<u8>,
    pub context_seed: Option<[u8; 32]>,
    // NEW: If provided, the context is created within this group.
    // The caller must be a group member.
    // The application version may be overridden by the group's target.
    pub group_id: Option<ContextGroupId>,
}
```

#### Handler: `create_context.rs` Changes

The existing context creation flow gains a group-aware wrapper:

```rust
// In the Prepared::new() phase of create_context:

if let Some(group_id) = request.group_id {
    // 1. Load group metadata
    let group = load_group_meta(&datastore, &group_id)?;

    // 2. Verify caller is a group member
    let is_member = check_group_membership(&datastore, &group_id, &caller_identity)?;
    if !is_member {
        bail!("caller is not a member of group '{}'", group_id);
    }

    // 3. Verify AppKey match
    let app = node_client.get_application(&request.application_id)?;
    if app.app_key() != group.app_key {
        bail!(
            "application AppKey '{}' does not match group AppKey '{}'",
            app.app_key(), group.app_key
        );
    }

    // 4. Version override: use group's target version
    let effective_application_id = if request.application_id != group.target_application_id {
        info!(
            requested = %request.application_id,
            target = %group.target_application_id,
            "Overriding requested version with group target"
        );
        group.target_application_id
    } else {
        request.application_id
    };

    // 5. Proceed with normal creation using effective_application_id
    // ... existing flow ...

    // 6. Post-creation: register context in group index
    register_context_in_group(&datastore, &group_id, &context_id)?;
}
```

#### Handler: `upgrade_group.rs` (New)

```rust
impl Handler<UpgradeGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, Result<GroupUpgradeStatus>>;

    fn handle(&mut self, msg: UpgradeGroupRequest, ctx: &mut Self::Context)
        -> Self::Result
    {
        let group_id = msg.group_id;

        // 1. Load group and verify admin
        let group = self.load_group_meta(&group_id)?;
        ensure!(
            is_group_admin(&self.datastore, &group_id, &msg.requester),
            "only group admins can trigger upgrades"
        );

        // 2. Verify AppKey continuity
        let canary_context = self.first_context_in_group(&group_id)?;
        verify_appkey_continuity(
            &self.datastore, &canary_context, &msg.application_id
        )?;

        // 3. Canary upgrade — upgrade one context first as validation
        let canary_result = self.upgrade_single_context(
            canary_context.id,
            msg.application_id,
            msg.requester,
            msg.migration.clone(),
        );

        match canary_result {
            Err(e) => {
                // Canary failed — abort entire group upgrade
                return ActorResponse::reply(Err(e));
            }
            Ok(()) => {
                info!(
                    %group_id,
                    canary = %canary_context.id,
                    "Canary upgrade succeeded, propagating to group"
                );
            }
        }

        // 4. Update group target version
        self.set_group_target(&group_id, msg.application_id)?;

        // 5. Spawn background propagator
        let propagator = UpgradePropagator::new(
            group_id,
            msg.application_id,
            msg.migration,
            msg.requester,
            canary_context.id,  // skip canary (already done)
            self.datastore.clone(),
            self.node_client.clone(),
            self.context_client.clone(),
        );
        ctx.spawn(propagator.run());

        // 6. Return initial status
        let total = self.count_contexts_in_group(&group_id);
        ActorResponse::reply(Ok(GroupUpgradeStatus::InProgress {
            total,
            completed: 1,  // canary
            failed: vec![],
        }))
    }
}
```

#### Background Propagator

```rust
struct UpgradePropagator {
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
    async fn run(self) -> GroupUpgradeStatus {
        let contexts = self.enumerate_group_contexts();
        let total = contexts.len();
        let mut completed = 1; // canary already done
        let mut failed = Vec::new();

        for context_id in contexts {
            if context_id == self.skip_context {
                continue;
            }

            // Check if already at target version
            if self.is_at_target_version(context_id) {
                completed += 1;
                continue;
            }

            match self.upgrade_single_context(context_id).await {
                Ok(()) => {
                    completed += 1;
                    self.persist_progress(total, completed, &failed);
                }
                Err(e) => {
                    warn!(
                        %context_id, error = %e,
                        "Context upgrade failed, continuing with others"
                    );
                    failed.push((context_id, e.to_string()));
                }
            }
        }

        let status = if failed.is_empty() {
            GroupUpgradeStatus::Completed {
                completed_at: now(),
            }
        } else {
            GroupUpgradeStatus::InProgress {
                total,
                completed,
                failed,
            }
        };

        self.persist_final_status(&status);
        status
    }

    async fn upgrade_single_context(&self, context_id: ContextId) -> eyre::Result<()> {
        // Reuses existing update_application_with_migration or
        // update_application_id — the SAME code path as today's
        // single-context upgrade.
        if let Some(ref migration) = self.migration {
            update_application_with_migration(
                self.datastore.clone(),
                self.node_client.clone(),
                self.context_client.clone(),
                context_id,
                None,
                self.target_application_id,
                None,
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

### 8.3 Sync Layer — No Protocol Changes

The sync layer requires **no changes**. Each context syncs independently. The upgrade propagation is a node-local operation that produces per-context state changes, which then sync via existing gossipsub/P2P protocols.

### 8.4 API Server (`core/crates/server/`)

New endpoints:

```
# Group CRUD
POST   /admin/groups                             → Create group
GET    /admin/groups/{group_id}                   → Get group info
DELETE /admin/groups/{group_id}                   → Delete group

# Group membership (users)
POST   /admin/groups/{group_id}/members           → Add members
DELETE /admin/groups/{group_id}/members            → Remove members
GET    /admin/groups/{group_id}/members            → List members

# Group contexts
GET    /admin/groups/{group_id}/contexts           → List contexts in group

# Upgrade operations
POST   /admin/groups/{group_id}/upgrade            → Trigger upgrade
GET    /admin/groups/{group_id}/upgrade/status      → Get upgrade status
POST   /admin/groups/{group_id}/upgrade/retry       → Retry failed contexts
POST   /admin/groups/{group_id}/rollback            → Rollback to previous version
```

Existing endpoints remain unchanged. The `POST /contexts` endpoint gains an optional `group_id` parameter.

---

## 9. Backward Compatibility Strategy

### 9.1 Principles

1. **Opt-in**: Groups are entirely optional. Existing contexts continue to work exactly as before.
2. **Additive**: All changes are additive — no existing fields are removed or renamed.
3. **Default: ungrouped**: A context with no group behaves identically to the current system.

### 9.2 Contract Migration

The `context-config` contract requires a storage migration to add the `groups` map and the optional `group_id` field to existing `Context` structs:

```rust
// In contracts/near/context-config/src/sys/migrations/

/// Migration: Add groups storage and group_id field to contexts.
pub fn migrate_03_context_groups(state: &mut ContextConfigs) {
    state.groups = IterableMap::new(Prefix::Groups);
    state.context_group_refs = IterableMap::new(Prefix::ContextGroupRefs);
    // Existing contexts get group_id = None via Option default
}
```

### 9.3 Core Runtime Migration

New key types are simply new key prefixes in the existing store. No existing data needs modification. Existing contexts have no entries in group indices.

### 9.4 Transitional Path for Existing Applications

For applications that already manage multiple independent contexts:

**Step 1**: Upgrade node to a version that supports groups.

**Step 2**: Admin creates a group: `POST /admin/groups`.

**Step 3**: Admin adds users to the group: `POST /admin/groups/{id}/members`.

**Step 4**: For each existing context, register it in the group via `POST /admin/groups/{id}/contexts/register` (or the contract `RegisterContext` method).

**Step 5**: Future contexts are created with `group_id` in the creation request.

**Step 6**: Subsequent upgrades use `POST /admin/groups/{id}/upgrade`.

This can be automated by the application: detect ungrouped contexts sharing the same AppKey and offer a "migrate to group" workflow.

---

## 10. Failure Handling

### 10.1 Canary Context Upgrade Failure

```
Canary upgrade fails → Entire group upgrade is aborted
                     → No other contexts are modified
                     → Error returned to admin
                     → Group target version is NOT updated
```

Rationale: The canary serves as a validation step. If migration fails on one context, it will likely fail on others (same WASM, same migration function). Aborting prevents wasted computation and partial state.

### 10.2 Non-Canary Context Upgrade Failure

```
Context N fails → Failure recorded in GroupUpgradeStatus
               → Propagation continues to remaining contexts
               → Admin is notified of partial completion
               → Failed contexts can be retried individually
```

**Retry mechanism:**

```
POST /admin/groups/{group_id}/upgrade/retry
{
  "context_ids": ["<failed_ctx_1>", "<failed_ctx_2>"]  // optional
}
```

If `context_ids` is omitted, retries all failed contexts.

### 10.3 Partial Upgrade / Mixed-Version State

During propagation, the group will temporarily have contexts at different versions. This is handled by:

1. **Application compatibility**: Application developers MUST ensure v(N) and v(N+1) can coexist during the upgrade window (standard rolling-deployment practice).

2. **Per-context version accuracy**: Each context's `application_id` is individually accurate. Application logic can check its own version.

3. **Deterministic ordering**: Contexts are upgraded in deterministic order (by `ContextId`).

### 10.4 Rollback Strategy

```
POST /admin/groups/{group_id}/rollback
{
  "target_application_id": "<previous_version>",
  "migration": { "method": "rollback_v2_to_v1" }  // optional
}
```

Rollback follows the same propagation flow as upgrade:
1. Set group's `target_application_id` to the previous version.
2. Canary rollback on one context.
3. Propagate to all contexts.

**Important**: Rollback is only safe if the application provides a reverse migration function. Destructive state changes may make rollback impossible. This is the developer's responsibility.

### 10.5 Node Crash During Propagation

`GroupUpgrade` status is persisted to the datastore after each context completes. On node restart:

1. The `ContextManager` checks for `active_upgrade` with `InProgress` status in all groups.
2. In-progress upgrades are resumed from where they left off.
3. Already-completed contexts are skipped (detected by checking `application_id`).

---

## 11. Security Considerations

### 11.1 Admin Abuse Prevention

**Threat**: A compromised group admin pushes a malicious application version.

**Mitigations:**

1. **AppKey continuity** (existing): The `verify_appkey_continuity` check ensures upgrades can only come from the same `signerId` (application publisher's key). An attacker needs the publisher's signing key, not just the admin key.

2. **On-chain audit trail**: `set_group_target()` logs version changes on-chain.

3. **Coordinated policy**: Organizations can use `UpgradePolicy::Coordinated` with a deadline, giving members time to review before forced upgrade.

4. **Multi-admin**: Groups support multiple admin identities, enabling checks-and-balances.

5. **Signed-to-unsigned protection** (existing): The runtime rejects downgrades from signed to unsigned applications.

### 11.2 Upgrade Validation Rules

Before any context upgrade is executed within a group:

```rust
fn validate_group_upgrade(
    context: &Context,
    group: &ContextGroup,
    target_app: &Application,
) -> Result<()> {
    // 1. Context must belong to this group
    ensure!(context_belongs_to_group(context.id, group.id));

    // 2. AppKey must match
    verify_appkey_continuity(context, target_app)?;

    // 3. Target version must be >= current (no silent downgrades)
    if let (Some(current_ver), Some(target_ver)) = (
        current_app.version(), target_app.version()
    ) {
        ensure!(
            target_ver >= current_ver,
            "downgrade requires explicit rollback"
        );
    }

    // 4. Application binary must be available locally
    ensure!(node_has_application(target_app.id));

    Ok(())
}
```

### 11.3 Context Integrity Verification

After migration, each context's state integrity is verified via existing mechanisms:

1. **Root hash**: Computed from the Merkle tree after migration (existing `write_migration_state` behavior).
2. **DAG consistency**: `dag_heads` reset to `[root_hash]` after migration (existing behavior).
3. **Cross-node verification**: Sync protocol handshake compares `root_hash`, `entity_count`, `dag_heads`.

### 11.4 Group Membership Authorization

| Operation | Required Role |
|---|---|
| Create group | Any identity (becomes admin) |
| Delete group | Group admin |
| Add group members (users) | Group admin |
| Remove group members (users) | Group admin |
| Set target version / trigger upgrade | Group admin |
| Rollback | Group admin |
| Set upgrade policy | Group admin |
| Create context in group | Group member (any role) |
| Delete own context | Group member (context owner) |
| Force-remove context from group | Group admin |

### 11.5 Privacy Preservation

Group membership reveals:
- That a `ContextId` belongs to a group
- The application version of that context

Group membership does **NOT** reveal:
- Context state (messages, data)
- Context members (who is in a DM)
- Context activity (when it was last used)

The `ContextId` is a cryptographic hash that does not reveal participants. The group admin can see that N contexts exist but cannot determine who is communicating with whom.

---

## 12. Design Considerations & Tradeoffs

### 12.1 Why `group_id` Must Be Explicit (Not Auto-Resolved)

Consider a user "Alice" who is:
- An employee at Acme Corp (uses `com.calimero.chat:did:key:z6Mk...Publisher`)
- A member of Hobby Club (uses the **same** `com.calimero.chat:did:key:z6Mk...Publisher`)

If Alice calls `createContext()` for a new DM, the system cannot determine which group the context should belong to based on `AppKey` alone. Multiple groups can share the same `AppKey`. Therefore, `group_id` is a **required explicit parameter** when creating a context within a group.

### 12.2 Why the Group Is Not a Context

Earlier iterations considered making one context the "root" of a group (group ID derived from root context ID). This was rejected because:

1. **Permission confusion**: "Do I need the root context admin's permission to create a DM?" The answer must be no.
2. **Lifecycle coupling**: If the root context is deleted, what happens to the group?
3. **Misaligned concepts**: A context represents isolated application state. A group represents organizational governance. These are different things.

The group is its own entity with its own identity, its own member list, and its own lifecycle.

### 12.3 Why Group Has Users, Not Just Contexts

The group needs to know **who** is authorized to create contexts within it. This is a user-level concept, not a context-level concept. A user can be a member of the group without being a member of any specific context (e.g., they haven't created any DMs yet but should be able to).

### 12.4 Why Version Override on Context Creation

When a group is at v2.0 and a user's client sends a `createContext` with v1.0 (outdated client), the platform overrides to v2.0. This prevents:
- Newly created contexts being immediately out-of-date
- Users accidentally creating contexts at stale versions
- The need for a separate "catch-up upgrade" after creation

### 12.5 Canary vs. Root-First Upgrade

Instead of upgrading a "root context" first (which doesn't exist in this model), the system picks a **canary context** — the first context in deterministic order. If the canary's migration succeeds, the migration function is proven correct for this group's application. If it fails, the entire upgrade is aborted before affecting other contexts.

### 12.6 Context Isolation Is Absolute

The group **never**:
- Accesses a context's state
- Modifies a context's member list
- Reads a context's sync data
- Interferes with a context's proxy contract

The only thing the group does to a context is change its `application_id` (and execute the migration function, which runs within the context's own state sandbox).

---

## 13. Migration Path

### 13.1 Phased Rollout

**Phase 1 — Foundation (contracts + core storage)**
- Add `groups` to contract storage
- Add `group_id` field to `Context` struct
- Add new storage keys to `calimero-store`
- Contract migration for existing deployments
- No behavior changes — all contexts remain ungrouped

**Phase 2 — Group CRUD + Membership**
- Implement `CreateGroup`, `DeleteGroup`
- Implement `AddGroupMembers`, `RemoveGroupMembers`
- API endpoints for group management
- Group metadata queries

**Phase 3 — Context-Group Integration**
- `group_id` parameter in `CreateContextRequest`
- Version override on creation
- Automatic group registration on context creation
- Automatic group deregistration on context deletion

**Phase 4 — Upgrade Propagation**
- `UpgradeGroupRequest` handler
- Canary upgrade validation
- Background `UpgradePropagator`
- Upgrade status tracking and reporting
- Retry mechanism

**Phase 5 — Advanced Policies**
- `Coordinated` upgrade policy with deadline
- `LazyOnAccess` interceptor in execute path
- Rollback support
- Crash recovery for in-progress upgrades

**Phase 6 — Application Integration + SDK**
- SDK helpers for group-aware application development
- Application templates with group support
- Documentation and best practices
- Example: group-aware chat application

### 13.2 Estimated Impact on Existing Code

| Component | Files Changed | Nature of Change |
|---|---|---|
| `core/crates/primitives` | 2-3 | New types: `ContextGroupId`, `ContextGroup`, `GroupMember`, etc. |
| `core/crates/store` | 4-6 | New storage key types and group indices |
| `core/crates/context` | 6-10 | Group handlers, propagator, create_context changes, lazy intercept |
| `core/crates/context/primitives` | 2-3 | New message types for group operations |
| `core/crates/server` | 4-6 | New API endpoints for groups |
| `core/crates/node` | 1-2 | Crash recovery for in-progress upgrades |
| `contracts/context-config` | 5-8 | New storage, methods, migration, query methods |
| `contracts/context-proxy` | 1-2 | New proposal action types |

---

## 14. Open Questions

1. **Cross-node group awareness**: Should group metadata be gossiped to all nodes, or is it sufficient for only the admin's node to orchestrate upgrades? Peer nodes could benefit from knowing group membership for status reporting and lazy-on-access upgrades.

2. **Group discovery**: How does a user discover which groups they belong to? Should the node maintain a per-identity group index (`PublicKey → Vec<ContextGroupId>`)?

3. **Admin transfer/rotation**: How is admin authority transferred? Should it require multi-sig from existing admins?

4. **Maximum group size**: Should there be a limit on contexts per group? Very large groups (10K+ contexts) may need parallel propagation with configurable concurrency.

5. **Upgrade concurrency**: Should multiple upgrades be queued (v1→v2 while v2→v3 is pending)? Current proposal allows only one active upgrade per group.

6. **Member auto-cleanup**: If a group member is removed, what happens to contexts they created? Should those contexts be force-removed from the group, or remain?

7. **Cross-group context migration**: Can a context be moved from one group to another? This could be useful for organizational restructuring but adds complexity.

8. **On-chain gas optimization**: Group operations (add members, register contexts) incur gas costs. For large groups, batch operations should be supported. What is the maximum batch size?

9. **Event/notification system**: How should group members be notified of pending upgrades (especially for `Coordinated` policy)? WebSocket events? On-chain events?

---

## Appendix A: Alternative Approaches Considered

### A.1 Hierarchical Root-Context Model

A "root context" acts as the group parent. Other contexts are children.

**Rejected because:**
- Conflates governance (who controls upgrades) with context (application state)
- Users need root admin permission to create contexts — bad UX
- Lifecycle coupling: deleting root context orphans children
- `GroupId` derived from root `ContextId` creates unnecessary dependency

### A.2 Application Version Channels (Pub-Sub)

Registry contract gets named channels (`stable`, `beta`). Contexts subscribe to channels.

**Rejected because:**
- Publisher-driven, not org-driven
- No multi-tenant support — publisher updates ALL orgs at once
- No per-org upgrade policy or timing control
- No concept of organizational membership

### A.3 Implicit AppKey Mesh (Decentralized)

Contexts sharing same `AppKey` form implicit coordination mesh via gossip.

**Rejected because:**
- Cannot disambiguate between organizations (same AppKey, different orgs)
- No centralized upgrade control per-org
- Best-effort gossip = unreliable propagation
- No global view of upgrade status

### A.4 Shared Context (Single Context for Everything)

All channels, DMs, etc., in one context.

**Rejected because:**
- Destroys privacy isolation
- Single point of failure
- Sync scalability issues
- Contradicts Calimero's fundamental isolation model

### A.5 Template/Fork Model

Template contexts define app + initial state. Subcontexts fork from template.

**Considered as future complement:**
- Could be layered on top of groups
- Useful for standardized initial state
- Not sufficient alone — doesn't solve version propagation

---

## Appendix B: Data Flow Diagrams

### B.1 Group Creation

```
Admin                 Node                    Contract
  │                    │                        │
  │ POST /groups       │                        │
  │ { app_key, app }   │                        │
  │───────────────────►│                        │
  │                    │ generate GroupId        │
  │                    │──────────┐              │
  │                    │◄─────────┘              │
  │                    │                        │
  │                    │ store group locally     │
  │                    │──────────┐              │
  │                    │◄─────────┘              │
  │                    │                        │
  │                    │ create_group()          │
  │                    │───────────────────────►│
  │                    │◄───────────────────────│ ok
  │                    │                        │
  │ { group_id }       │                        │
  │◄───────────────────│                        │
```

### B.2 Context Creation Within a Group

```
User                  Node                    Contract
  │                    │                        │
  │ POST /contexts     │                        │
  │ { group_id, app }  │                        │
  │───────────────────►│                        │
  │                    │ check group membership  │
  │                    │──────────┐              │
  │                    │◄─────────┘ ok           │
  │                    │                        │
  │                    │ override version        │
  │                    │ with group target       │
  │                    │──────────┐              │
  │                    │◄─────────┘              │
  │                    │                        │
  │                    │ normal context creation │
  │                    │──────────┐              │
  │                    │◄─────────┘              │
  │                    │                        │
  │                    │ register_context()      │
  │                    │───────────────────────►│
  │                    │◄───────────────────────│ ok
  │                    │                        │
  │                    │ add to group index      │
  │                    │──────────┐              │
  │                    │◄─────────┘              │
  │                    │                        │
  │ { context_id }     │                        │
  │◄───────────────────│                        │
```

### B.3 Group Upgrade — Happy Path

```
Admin                 Node                    Contract              Peers
  │                    │                        │                     │
  │ POST /upgrade      │                        │                     │
  │───────────────────►│                        │                     │
  │                    │ verify admin role       │                     │
  │                    │──────────┐              │                     │
  │                    │◄─────────┘ ok           │                     │
  │                    │                        │                     │
  │                    │ verify_appkey()         │                     │
  │                    │──────────┐              │                     │
  │                    │◄─────────┘ ok           │                     │
  │                    │                        │                     │
  │                    │ canary upgrade          │                     │
  │                    │──────────┐              │                     │
  │                    │◄─────────┘ ok           │                     │
  │                    │                        │                     │
  │                    │ set_group_target()      │                     │
  │                    │───────────────────────►│                     │
  │                    │◄───────────────────────│ ok                  │
  │                    │                        │                     │
  │  202 Accepted      │                        │                     │
  │◄───────────────────│                        │                     │
  │                    │                        │                     │
  │                    │ [background] foreach context:               │
  │                    │   upgrade context      │                     │
  │                    │──────────┐             │                     │
  │                    │◄─────────┘             │                     │
  │                    │   update_application() │                     │
  │                    │──────────────────────►│                     │
  │                    │◄─────────────────────│                      │
  │                    │                       │                      │
  │                    │   sync delta          │                      │
  │                    │─────────────────────────────────────────────►│
  │                    │                       │                      │
  │ GET /status        │                       │                      │
  │───────────────────►│                       │                      │
  │ { completed: N }   │                       │                      │
  │◄───────────────────│                       │                      │
```

### B.4 Lazy-on-Access Upgrade

```
Client              Node
  │                  │
  │ POST /execute    │
  │ (method call)    │
  │─────────────────►│
  │                  │ check group target version
  │                  │──────────┐
  │                  │◄─────────┘ version mismatch
  │                  │
  │                  │ upgrade context first
  │                  │──────────┐
  │                  │◄─────────┘ ok
  │                  │
  │                  │ execute user method
  │                  │──────────┐
  │                  │◄─────────┘
  │                  │
  │ response         │
  │◄─────────────────│
```

---

*End of proposal.*
