---
name: Core Context Groups
overview: "Implement the Context Groups feature in `core/` across 6 phases: storage keys, group CRUD, context-group integration, upgrade propagation, advanced policies, and group invitations/join flow. Contracts are complete; all work is in `core/crates/`. A living context doc (CONTEXT-core-groups.md) is maintained after each phase for agent handoffs."
todos:
  - id: ctx-doc-create
    content: "Create /Users/beast/Developer/Calimero/CONTEXT-core-groups.md — initial state: what contracts completed, what shared types already exist in core, full file-change table with all statuses set to ⬜, and a Phase 1 section agent can follow"
    status: completed
  - id: p1-upgrade-policy
    content: "P1.1 — Add UpgradePolicy enum (Automatic | LazyOnAccess | Coordinated { deadline: Option<Duration> }) to core/crates/primitives/src/context.rs with Serialize/Deserialize/Clone/Debug derives"
    status: completed
  - id: p1-member-role
    content: P1.2 — Add GroupMemberRole enum (Admin | Member) to core/crates/primitives/src/context.rs with Serialize/Deserialize/Clone/Debug/PartialEq derives
    status: completed
  - id: p1-key-group-meta
    content: "P1.3 — Create core/crates/store/src/key/group.rs: add GroupMeta key struct (prefix 0x20, 32-byte group_id) implementing AsKeyParts (Column::Config) and FromKeyParts, following the exact pattern in key/context.rs"
    status: completed
  - id: p1-key-group-member
    content: "P1.4 — In key/group.rs: add GroupMember key struct (prefix 0x21, 32-byte group_id + 32-byte identity) implementing AsKeyParts + FromKeyParts"
    status: completed
  - id: p1-key-group-ctx-index
    content: "P1.5 — In key/group.rs: add GroupContextIndex key struct (prefix 0x22, 32-byte group_id + 32-byte context_id) implementing AsKeyParts + FromKeyParts"
    status: completed
  - id: p1-key-ctx-group-ref
    content: "P1.6 — In key/group.rs: add ContextGroupRef key struct (prefix 0x23, 32-byte context_id) implementing AsKeyParts + FromKeyParts"
    status: completed
  - id: p1-key-group-upgrade
    content: "P1.7 — In key/group.rs: add GroupUpgradeKey struct (prefix 0x24, 32-byte group_id) implementing AsKeyParts + FromKeyParts"
    status: completed
  - id: p1-value-types
    content: "P1.8 — In key/group.rs: add value types GroupMetaValue { app_key, target_application_id, upgrade_policy, created_at, admin_identity }, GroupMemberRole (re-export from primitives), GroupUpgradeValue { from_revision, to_revision, migration, initiated_at, initiated_by, status }, GroupUpgradeStatus { InProgress { total, completed, failed }, Completed { completed_at }, RolledBack { reason } } — all with BorshSerialize/Deserialize + unit tests for roundtrip serialization of each. NOTE: ApplicationId is stable across versions (hash(package, signer_id)), so upgrades track revision numbers, not ApplicationIds."
    status: completed
  - id: p1-key-mod
    content: "P1.9 — In core/crates/store/src/key.rs: add 'mod group;' and pub use group::{GroupMeta, GroupMember, GroupContextIndex, ContextGroupRef, GroupUpgradeKey}"
    status: completed
  - id: p1-ctx-doc-update
    content: "P1.10 — Update CONTEXT-core-groups.md: mark Phase 1 tasks complete (✅), add notes on exact prefix byte values used, confirm no prefix collisions with existing context keys"
    status: completed
  - id: p2-prim-group-create-delete
    content: "P2.1 — Create core/crates/context/primitives/src/group.rs: add CreateGroupRequest { group_id: Option<ContextGroupId>, app_key, application_id, upgrade_policy, admin_identity } + CreateGroupResponse { group_id } + DeleteGroupRequest { group_id, requester } — each implements actix::Message"
    status: completed
  - id: p2-prim-group-members
    content: "P2.2 — In group.rs: add AddGroupMembersRequest { group_id, members: Vec<(PublicKey, GroupMemberRole)>, requester } + RemoveGroupMembersRequest { group_id, members: Vec<PublicKey>, requester } — both implement actix::Message with eyre::Result<()> result type"
    status: completed
  - id: p2-prim-group-queries
    content: "P2.3 — In group.rs: add GetGroupInfoRequest { group_id } + ListGroupMembersRequest { group_id, offset, limit } + response types GroupInfoResponse { group_id, app_key, target_application_id, upgrade_policy, member_count, context_count, active_upgrade: Option<GroupUpgradeValue> } + GroupMemberEntry { identity, role } — all implement actix::Message"
    status: completed
  - id: p2-prim-lib
    content: "P2.4 — In core/crates/context/primitives/src/lib.rs: add 'pub mod group;'"
    status: completed
  - id: p2-msg-create-context
    content: "P2.5 — In messages.rs: add group_id: Option<ContextGroupId> field to CreateContextRequest (import ContextGroupId from calimero_context_config::types)"
    status: completed
  - id: p2-msg-enum-crud
    content: "P2.6 — In messages.rs ContextMessage enum: add CreateGroup { request, outcome }, DeleteGroup { request, outcome }, AddGroupMembers { request, outcome }, RemoveGroupMembers { request, outcome } variants following the existing oneshot::Sender pattern"
    status: completed
  - id: p2-msg-enum-queries
    content: "P2.7 — In messages.rs ContextMessage enum: add GetGroupInfo { request, outcome } and ListGroupMembers { request, outcome } variants"
    status: completed
  - id: p2-store-meta
    content: "P2.8 — Create core/crates/context/src/group_store.rs: implement load_group_meta, save_group_meta, delete_group_meta using the GroupMeta key + borsh value codec pattern from existing context handlers"
    status: completed
  - id: p2-store-members
    content: "P2.9 — In group_store.rs: implement add_group_member, remove_group_member, get_group_member_role, check_group_membership, is_group_admin, list_group_members (with offset/limit) using the GroupMember key"
    status: completed
  - id: p2-store-ctx-index
    content: "P2.10 — In group_store.rs: implement register_context_in_group (writes GroupContextIndex + ContextGroupRef), unregister_context_from_group (deletes both), get_group_for_context (reads ContextGroupRef), enumerate_group_contexts, count_group_contexts"
    status: completed
  - id: p2-store-upgrade
    content: "P2.11 — In group_store.rs: implement save_group_upgrade, load_group_upgrade, delete_group_upgrade using the GroupUpgradeKey"
    status: completed
  - id: p2-handler-create-group
    content: "P2.12 — Create core/crates/context/src/handlers/create_group.rs: implement Handler<CreateGroupRequest> — generate ContextGroupId, save GroupMetaValue + admin GroupMember to store, sign and submit GroupRequest::Create on-chain via existing external client pattern"
    status: completed
  - id: p2-handler-delete-group
    content: "P2.13 — Create core/crates/context/src/handlers/delete_group.rs: load group, verify requester is admin (is_group_admin), verify count_group_contexts == 0 (error otherwise), delete GroupMeta + all GroupMember entries, submit GroupRequest::Delete on-chain"
    status: completed
  - id: p2-handler-add-members
    content: "P2.14 — Create core/crates/context/src/handlers/add_group_members.rs: load group, is_group_admin check, batch add_group_member calls, submit GroupRequest::AddMembers on-chain"
    status: completed
  - id: p2-handler-remove-members
    content: "P2.15 — Create core/crates/context/src/handlers/remove_group_members.rs: load group, is_group_admin check, batch remove_group_member calls, submit GroupRequest::RemoveMembers on-chain"
    status: completed
  - id: p2-handlers-rs
    content: "P2.16 — In core/crates/context/src/handlers.rs: add pub mod create_group, delete_group, add_group_members, remove_group_members; extend Handler<ContextMessage> match arms for all 4 CRUD + 2 query variants; also add get_group_info inline (reads from store, no on-chain call)"
    status: completed
  - id: p2-server-handlers-create-get
    content: "P2.17 — Create core/crates/server/src/admin/handlers/groups.rs: implement create_group handler (POST /groups) and get_group handler (GET /groups/:group_id) following the axum + AdminState pattern from context/create_context.rs"
    status: completed
  - id: p2-server-handlers-rest
    content: "P2.18 — In groups.rs: implement delete_group (DELETE /groups/:group_id), add_members (POST /groups/:group_id/members), remove_members (POST /groups/:group_id/members/remove), list_members (GET /groups/:group_id/members) handlers"
    status: completed
  - id: p2-server-handlers-mod
    content: "P2.19 — In core/crates/server/src/admin/handlers.rs: add 'pub mod groups;'"
    status: completed
  - id: p2-server-service
    content: "P2.20 — In service.rs protected_routes: import group handlers and register 6 routes (POST /groups, GET/DELETE /groups/:id, POST/GET /groups/:id/members, POST /groups/:id/members/remove)"
    status: completed
  - id: p2-ctx-doc-update
    content: "P2.21 — Update CONTEXT-core-groups.md: mark Phase 2 complete (✅), document the ContextMessage enum additions, list all new files created, note the on-chain call pattern used for GroupRequest signing"
    status: completed
  - id: p3-create-ctx-validation
    content: "P3.1 — Modify create_context.rs Prepared::new(): after protocol validation, if request.group_id is Some, load GroupMetaValue (error if not found), call check_group_membership (error if not member), override application_id with group target if different"
    status: completed
  - id: p3-create-ctx-version-override
    content: "P3.2 — In create_context.rs: after group validation, if application_id != group.target_application_id, log a warning and override application_id with group.target_application_id"
    status: completed
  - id: p3-create-ctx-post-hook
    content: "P3.3 — In create_context.rs: after successful context creation, if group_id is Some, call register_context_in_group (local store)"
    status: completed
  - id: p3-delete-ctx-hook
    content: "P3.4 — Modify delete_context.rs: after successful deletion, call get_group_for_context; if Some, call unregister_context_from_group (local store)"
    status: completed
  - id: p3-list-contexts-endpoint
    content: "P3.5 — Full-stack ListGroupContexts: message type in group.rs, ContextMessage variant, handler, ContextClient method, server handler (GET /groups/:group_id/contexts), route in service.rs"
    status: completed
  - id: p3-ctx-doc-update
    content: "P3.6 — Update CONTEXT_GROUPS_IMPL_PLAN.md: mark Phase 3 complete, update file table"
    status: completed
  - id: p4-msg-upgrade-variants
    content: "P4.1 — In messages.rs ContextMessage enum: add UpgradeGroup { request: UpgradeGroupRequest, outcome }, GetGroupUpgradeStatus { request: GetGroupUpgradeStatusRequest, outcome }, RetryGroupUpgrade { request: RetryGroupUpgradeRequest, outcome } — add corresponding request/response types to group.rs in primitives"
    status: completed
  - id: p4-handler-upgrade-validation
    content: "P4.2 — Create core/crates/context/src/handlers/upgrade_group.rs: implement Handler<UpgradeGroupRequest> preamble — load group, verify requester is admin, verify AppKey continuity between current target and new application_id, check no active InProgress upgrade (return 409-equivalent error if so), install new application via node_client if not present"
    status: completed
  - id: p4-handler-upgrade-canary
    content: "P4.3 — In upgrade_group.rs: select canary (first ContextId from enumerate_group_contexts sorted deterministically), run canary upgrade via existing update_application_with_migration or update_application_id; on canary failure return error immediately with no state change; on canary success: update GroupMetaValue.target_application_id in store, submit GroupRequest::SetTargetApplication on-chain"
    status: completed
  - id: p4-handler-upgrade-spawn
    content: "P4.4 — In upgrade_group.rs: persist GroupUpgradeValue { status: InProgress { total, completed: 1, failed: [] } } to store, spawn UpgradePropagator as actix ctx.spawn future, return Ok with InProgress status"
    status: completed
  - id: p4-propagator-struct
    content: "P4.5 — Propagator implemented as inline async fn propagate_upgrade() in upgrade_group.rs (consolidated with P4.6/P4.7 — no separate file needed)"
    status: completed
  - id: p4-propagator-run
    content: "P4.6 — In upgrade_group.rs: propagate_upgrade() async fn — enumerates contexts, skips canary, calls context_client.update_application() per context, persists progress after each step, writes final Completed or InProgress status"
    status: completed
  - id: p4-propagator-single
    content: "P4.7 — Consolidated into propagate_upgrade() — uses context_client.update_application() with optional migrate_method directly"
    status: completed
  - id: p4-handler-status
    content: "P4.8 — Create core/crates/context/src/handlers/get_group_upgrade_status.rs: implement Handler<GetGroupUpgradeStatusRequest> — load_group_upgrade from store, return Option<GroupUpgradeValue>"
    status: completed
  - id: p4-handler-retry
    content: "P4.9 — Create core/crates/context/src/handlers/retry_group_upgrade.rs: load current GroupUpgradeValue, validate failed > 0, extract migration params, re-spawn propagate_upgrade with skip_context = zero sentinel"
    status: completed
  - id: p4-handlers-rs
    content: "P4.10 — In handlers.rs: add pub mod upgrade_group, get_group_upgrade_status, retry_group_upgrade; extend ContextMessage match for UpgradeGroup, GetGroupUpgradeStatus, RetryGroupUpgrade"
    status: completed
  - id: p4-server-upgrade-routes
    content: "P4.11 — In groups.rs: add upgrade_group, get_group_upgrade_status, retry_group_upgrade handlers; register routes POST /groups/:id/upgrade, GET /groups/:id/upgrade/status, POST /groups/:id/upgrade/retry in service.rs"
    status: completed
  - id: p4-ctx-doc-update
    content: "P4.12 — Update CONTEXT_GROUPS_IMPL_PLAN.md: mark Phase 4 complete"
    status: completed
  - id: p5-lazy-helper
    content: "P5.1 — In core/crates/context/src/handlers/execute.rs: implement private maybe_lazy_upgrade() helper — reads ContextGroupRef, loads GroupMeta, checks UpgradePolicy::LazyOnAccess, compares context's current application_id with group.target_application_id, returns upgrade params if stale else None"
    status: completed
  - id: p5-lazy-wire
    content: "P5.2 — In execute.rs execute path: call maybe_lazy_upgrade() before the main execute logic; if Some, perform the upgrade inline using update_application_id (await completion before proceeding), then continue with the user method call"
    status: completed
  - id: p5-crash-recovery
    content: "P5.3 — In core/crates/context/src/lib.rs: add recover_in_progress_upgrades() method to ContextManager — iterate store for all GroupUpgradeKey entries with InProgress status, re-spawn an UpgradePropagator for each with skip_context = ContextId::zero() (no canary skip on resume, idempotency handles duplicates); call this from ContextManager::started()"
    status: completed
  - id: p5-auto-retry
    content: "P5.4 — In upgrade_group.rs: add automatic retry logic to propagate_upgrade() — after first pass, if any contexts failed, retry up to 3 times with exponential backoff (5s, 10s, 20s). Removed rollback feature (RolledBack variant, rollback handler, route, API types) — replaced with auto-retry."
    status: completed
  - id: p5-ctx-doc-final
    content: "P5.5 — Update CONTEXT_GROUPS_IMPL_PLAN.md: mark Phase 5 complete, update status table"
    status: completed
  - id: p6-invitation-payload-type
    content: "P6.1 — In core/crates/primitives/src/context.rs: add GroupInvitationPayload newtype (Vec<u8>, borsh-serialized + base58-encoded string), following the exact pattern of ContextInvitationPayload — implement Display (base58), FromStr, From<String>, TryFrom<&str>"
    status: completed
  - id: p6-invitation-payload-impl
    content: "P6.2 — In context.rs: implement GroupInvitationPayload::new(group_id, inviter_identity, invitee_identity: Option<PublicKey>, expiration: Option<u64>) + parts() -> (ContextGroupId, PublicKey, Option<PublicKey>, Option<u64>) — using borsh for inner serialization, same as ContextInvitationPayload"
    status: completed
  - id: p6-prim-invite-requests
    content: "P6.3 — In core/crates/context/primitives/src/group.rs: add CreateGroupInvitationRequest { group_id, invitee_identity: Option<PublicKey>, expiration: Option<u64>, requester: PublicKey } + CreateGroupInvitationResponse { payload: GroupInvitationPayload } — implements actix::Message"
    status: completed
  - id: p6-prim-join-request
    content: "P6.4 — In group.rs: add JoinGroupRequest { invitation_payload: GroupInvitationPayload, joiner_identity: PublicKey } + JoinGroupResponse { group_id, member_identity: PublicKey } — implements actix::Message"
    status: completed
  - id: p6-msg-enum
    content: "P6.5 — In messages.rs ContextMessage enum: add CreateGroupInvitation { request, outcome } and JoinGroup { request, outcome } variants"
    status: completed
  - id: p6-handler-create-invitation
    content: "P6.6 — Create core/crates/context/src/handlers/create_group_invitation.rs: verify requester is group admin (is_group_admin), build inner borsh payload { group_id, inviter_identity, invitee_identity, expiration }, wrap in GroupInvitationPayload, return encoded string — no on-chain call needed (invitation is a local signed artifact)"
    status: completed
  - id: p6-handler-join-group
    content: "P6.7 — Create core/crates/context/src/handlers/join_group.rs: decode GroupInvitationPayload, extract (group_id, inviter_identity, invitee_identity, expiration), verify inviter is a group admin via is_group_admin, verify invitee_identity matches joiner or invitation is open (None), check expiration if set, call add_group_member locally + submit GroupRequest::AddMembers on-chain for the joiner's key, return JoinGroupResponse"
    status: completed
  - id: p6-handlers-rs
    content: "P6.8 — In core/crates/context/src/handlers.rs: add pub mod create_group_invitation, join_group; extend ContextMessage match for CreateGroupInvitation and JoinGroup"
    status: completed
  - id: p6-server-invite-endpoint
    content: "P6.9 — In server/src/admin/handlers/groups.rs: add create_invitation handler (POST /groups/:group_id/invite) — admin-only, returns { payload: '<base58 string>' }; add join_group handler (POST /groups/join) — takes { invitation_payload: string, identity_secret? } in body, resolves or generates joiner identity, returns { group_id, member_identity }"
    status: completed
  - id: p6-server-service
    content: "P6.10 — In service.rs: register POST /groups/:group_id/invite and POST /groups/join routes in protected_routes"
    status: completed
  - id: p6-ctx-doc-final
    content: "P6.11 — Final update to CONTEXT-core-groups.md: mark Phase 6 complete, document the invitation payload format and the open vs targeted invitation distinction, update full status table to all ✅"
    status: completed
isProject: false
---

# Core/ Context Groups Implementation Plan

## Current State — What Is Already Done

### Shared Types (`core/crates/context/config/`) — All Complete ✅


| Symbol                                                | File       | Status |
| ----------------------------------------------------- | ---------- | ------ |
| `ContextGroupId`                                      | `types.rs` | ✅      |
| `AppKey`                                              | `types.rs` | ✅      |
| `GroupRequest`, `GroupRequestKind`                    | `lib.rs`   | ✅      |
| `RequestKind::Group`                                  | `lib.rs`   | ✅      |
| `ProposalAction::RegisterInGroup/UnregisterFromGroup` | `lib.rs`   | ✅      |


### Contracts (`contracts/`) — All 5 Phases Complete ✅

See `CONTEXT-contracts-groups.md` for full detail.

### `core/` Runtime — All Pending ⬜

Everything in this plan targets `core/crates/` only.

---

## Living Context Document

A file `**CONTEXT-core-groups.md**` lives at the repo root alongside `CONTEXT-contracts-groups.md`. It is the **primary handoff artifact** between agents/sessions. It contains:

- The full status table of every file change (⬜ / 🔄 / ✅)
- Notes on patterns, prefix bytes, and on-chain call conventions used
- A "What's Next" section pointing to the active phase
- Any open questions resolved during implementation

**Every phase ends with a context doc update task** (`ctx-doc-update`). An agent picking up mid-work reads this doc first before touching any code.

---

## Phase 1 — Foundation: Storage Types

**Goal**: New types and storage keys only. Zero behavior changes. Fully backward-compatible.

### Dependency graph

```
primitives/src/context.rs   (UpgradePolicy, GroupMemberRole)
        ↓
store/src/key/group.rs      (5 key structs + value types, uses UpgradePolicy)
        ↓
store/src/key.rs            (mod group; pub use)
```

### Files


| Task      | File                                    | Change                                 |
| --------- | --------------------------------------- | -------------------------------------- |
| P1.1–P1.2 | `core/crates/primitives/src/context.rs` | Add `UpgradePolicy`, `GroupMemberRole` |
| P1.3–P1.8 | `core/crates/store/src/key/group.rs`    | **New** — 5 keys + value types + tests |
| P1.9      | `core/crates/store/src/key.rs`          | `mod group; pub use group::...`        |


### Key struct pattern (from `key/context.rs`)

```rust
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub struct GroupMeta(Key<(GroupId,)>);

impl GroupMeta {
    pub fn new(group_id: ContextGroupId) -> Self {
        let mut key = [0u8; 33];
        key[0] = 0x20;
        key[1..].copy_from_slice(&group_id.to_bytes());
        Self(Key(key.into()))
    }
}
impl AsKeyParts for GroupMeta {
    type Components = (GroupId,);
    fn column() -> Column { Column::Config }
    fn as_key(&self) -> &Key<Self::Components> { (&self.0).into() }
}
```

### Prefix byte allocation


| Key                 | Prefix | Total bytes      |
| ------------------- | ------ | ---------------- |
| `GroupMeta`         | `0x20` | 33 (1 + 32)      |
| `GroupMember`       | `0x21` | 65 (1 + 32 + 32) |
| `GroupContextIndex` | `0x22` | 65 (1 + 32 + 32) |
| `ContextGroupRef`   | `0x23` | 33 (1 + 32)      |
| `GroupUpgradeKey`   | `0x24` | 33 (1 + 32)      |


Verify no collision with existing context key prefixes before committing.

---

## Phase 2 — Group CRUD + Membership

**Goal**: Full group lifecycle — create, delete, add/remove members, query — reachable via the admin API.

### Dependency order within Phase 2

```
primitives/src/group.rs      (message types)
messages.rs                  (ContextMessage variants)
        ↓
group_store.rs               (store helpers)
        ↓
handlers/create_group.rs     (uses store helpers + on-chain client)
handlers/delete_group.rs
handlers/add_group_members.rs
handlers/remove_group_members.rs
        ↓
handlers.rs                  (wires ContextMessage → handlers)
        ↓
server/handlers/groups.rs    (axum handlers)
server/service.rs            (route registration)
```

### New files

- `core/crates/context/primitives/src/group.rs` (~200 LOC)
- `core/crates/context/src/group_store.rs` (~300 LOC)
- `core/crates/context/src/handlers/create_group.rs` (~100 LOC)
- `core/crates/context/src/handlers/delete_group.rs` (~80 LOC)
- `core/crates/context/src/handlers/add_group_members.rs` (~80 LOC)
- `core/crates/context/src/handlers/remove_group_members.rs` (~80 LOC)
- `core/crates/server/src/admin/handlers/groups.rs` (~300 LOC)

### Modified files

- `core/crates/context/primitives/src/lib.rs` — `pub mod group;`
- `core/crates/context/primitives/src/messages.rs` — extend `CreateContextRequest`, extend `ContextMessage`
- `core/crates/context/src/handlers.rs` — `pub mod` + match arms
- `core/crates/server/src/admin/handlers.rs` — `pub mod groups;`
- `core/crates/server/src/admin/service.rs` — 6 new routes

### New API routes (Phase 2) — ✅ Implemented

```
POST   /admin-api/groups                              → CreateGroup
GET    /admin-api/groups/:group_id                    → GetGroupInfo
DELETE /admin-api/groups/:group_id                    → DeleteGroup
POST   /admin-api/groups/:group_id/members            → AddGroupMembers
POST   /admin-api/groups/:group_id/members/remove     → RemoveGroupMembers
GET    /admin-api/groups/:group_id/members            → ListGroupMembers
```

Note: RemoveGroupMembers uses `POST .../members/remove` instead of `DELETE .../members` because DELETE with a request body is non-standard. Group IDs are hex-encoded 32-byte strings in both URL paths and JSON responses.

---

## Phase 3 — Context-Group Integration

**Goal**: Bind context creation and deletion to group index maintenance.

### Changes to existing handlers

`**create_context.rs`** — 3 additions in order:

1. Pre-validation block (before key generation):
  - Load `GroupMetaValue` or error
  - `check_group_membership` or error
  - `app.app_key() == group.app_key` or error
  - Version override if needed
2. Post-creation hook (after `CreateContextResponse` is ready, before returning):
  - `register_context_in_group` to local store
  - Fire-and-forget on-chain `GroupRequest::RegisterContext`

`**delete_context.rs`** — 1 addition after deletion:

- `get_group_for_context` → if Some, `unregister_context_from_group` + fire-and-forget on-chain

### New API route (Phase 3)

```
GET    /admin-api/groups/:group_id/contexts    → ListGroupContexts  (offset, limit)
```

---

## Phase 4 — Upgrade Propagation

**Goal**: Single admin trigger propagates application version across all group contexts.

### State machine

```mermaid
stateDiagram-v2
    [*] --> IDLE
    IDLE --> CANARY_TEST : Admin POST upgrade
    CANARY_TEST --> PROPAGATE : canary success
    CANARY_TEST --> IDLE : canary failure (no state changed)
    PROPAGATE --> COMPLETED : all contexts upgraded
    PROPAGATE --> PARTIAL_FAIL : some contexts failed
    PARTIAL_FAIL --> PROPAGATE : Admin POST retry
    PROPAGATE --> PROPAGATE : auto-retry failed contexts (up to 3x)
    PROPAGATE --> PROPAGATE : node crash + restart (auto-resume)
```



### New files — ✅ Implemented

- `core/crates/context/src/handlers/upgrade_group.rs` (~300 LOC) — Handler + inline `propagate_upgrade()` async fn (propagator consolidated here)
- `core/crates/context/src/handlers/get_group_upgrade_status.rs` (~20 LOC)
- `core/crates/context/src/handlers/retry_group_upgrade.rs` (~90 LOC)
- `core/crates/server/src/admin/handlers/groups/upgrade_group.rs` (~85 LOC)
- `core/crates/server/src/admin/handlers/groups/get_group_upgrade_status.rs` (~85 LOC)
- `core/crates/server/src/admin/handlers/groups/retry_group_upgrade.rs` (~60 LOC)

### Key design constraint

`UpgradePropagator::upgrade_single_context()` **must only call** the existing functions from `update_application.rs`. No new migration logic is introduced.

### New API routes (Phase 4)

```
POST   /admin-api/groups/:group_id/upgrade         → UpgradeGroup (202 Accepted)
GET    /admin-api/groups/:group_id/upgrade/status  → GetGroupUpgradeStatus
POST   /admin-api/groups/:group_id/upgrade/retry   → RetryGroupUpgrade
```

---

## Phase 5 — Advanced Policies + Crash Recovery

**Goal**: Lazy-on-access transparent upgrade, startup crash recovery, automatic retry for failed upgrades.

### Changes


| Task      | File                                       | What                                            |
| --------- | ------------------------------------------ | ----------------------------------------------- |
| P5.1–P5.2 | `handlers/execute.rs`                      | `maybe_lazy_upgrade()` pre-check                |
| P5.3      | `context/src/lib.rs`                       | `recover_in_progress_upgrades()` on `started()` |
| P5.4      | `handlers/upgrade_group.rs`                | Auto-retry failed contexts in propagator        |


### Auto-retry behavior (Phase 5) — ✅ Implemented

After the initial propagation pass, if any context upgrades failed, the propagator automatically retries failed contexts up to 3 times with exponential backoff (5s, 10s, 20s). If failures persist after all retries, the upgrade remains in `InProgress` status for manual retry via `POST /groups/:group_id/upgrade/retry`.

### Crash recovery flow

On `ContextManager::started()`:

1. Scan all `GroupUpgradeKey` entries in store
2. For each with `status == InProgress`, re-spawn `UpgradePropagator`
3. Propagator's idempotency check (compare `context.application_id` vs `target`) skips already-upgraded contexts automatically

---

## Architecture Overview

```mermaid
flowchart TD
    subgraph api [Admin API Layer]
        R1["POST /groups"]
        R2["GET /groups/:id"]
        R3["POST /groups/:id/members"]
        R4["POST /groups/:id/upgrade"]
        R5["GET /groups/:id/upgrade/status"]
        R6["Auto-retry (internal)"]
        R7["POST /groups/:id/upgrade/retry"]
    end

    subgraph actor [ContextManager Actor]
        H1[create_group]
        H2[delete_group]
        H3[add_group_members]
        H4[upgrade_group]
        H5[get_group_status]
        H6[retry_group_upgrade]
        H7["create_context (modified)"]
        H8["delete_context (modified)"]
        H9["execute (modified P5)"]
        PROP[UpgradePropagator]
    end

    subgraph storage [Store Layer]
        GS[group_store.rs helpers]
        K1[GroupMeta key]
        K2[GroupMember key]
        K3[GroupContextIndex key]
        K4[ContextGroupRef key]
        K5[GroupUpgradeKey]
    end

    subgraph existing [Existing - Unchanged]
        UA[update_application.rs]
        EXEC[execute.rs core logic]
    end

    R1 --> H1
    R4 --> H4
    H4 --> PROP
    PROP --> UA
    H7 --> K3
    H7 --> K4
    H9 --> UA
    GS --> K1
    GS --> K2
    GS --> K3
    GS --> K4
    GS --> K5
    actor --> GS
```



---

## Phase 6 — Group Invitations + Join Flow

**Goal**: Let admins generate invitation payloads that users can present to join a group, mirroring the existing context invitation pattern.

### Design

Two invitation modes, same payload type:

- **Targeted** — `invitee_identity: Some(PublicKey)` — only that specific key can redeem it
- **Open** — `invitee_identity: None` — any identity can redeem it (admin's discretion)

No new on-chain contract changes needed. The existing `GroupRequest::AddMembers` (already in the shared types crate and contracts) handles the on-chain registration when someone joins.

### Invitation payload structure (borsh inner, base58 outer)

```rust
// Inner struct — borsh serialized, then base58 encoded into GroupInvitationPayload
struct GroupInvitationInner {
    group_id:         [u8; 32],
    inviter_identity: [u8; 32],  // admin who created it
    invitee_identity: Option<[u8; 32]>,  // None = open invitation
    expiration:       Option<u64>,       // unix timestamp, None = no expiry
}
```

### Flow: Admin creates invitation

```
POST /admin-api/groups/:id/invite
{ invitee_identity?: "<pubkey>", expiration?: 1234567890 }
        ↓
CreateGroupInvitationRequest handler
  → verify requester is group admin
  → borsh-serialize inner payload
  → base58-encode → GroupInvitationPayload
        ↓
{ payload: "3xFg7k...base58..." }   ← admin sends this to the user out-of-band
```

### Flow: User joins via invitation

```
POST /admin-api/groups/join
{ invitation_payload: "3xFg7k...", identity_secret?: "..." }
        ↓
JoinGroupRequest handler
  → decode + deserialize GroupInvitationPayload
  → verify inviter_identity is still an admin (is_group_admin)
  → if targeted: verify joiner matches invitee_identity
  → if expiration set: verify not expired
  → add_group_member(joiner, Member) locally
  → submit GroupRequest::AddMembers on-chain
        ↓
{ group_id: "...", member_identity: "<pubkey>" }
```

### Comparison to context invitation


|                 | Context                          | Group                                  |
| --------------- | -------------------------------- | -------------------------------------- |
| Payload type    | `ContextInvitationPayload`       | `GroupInvitationPayload`               |
| Inner encoding  | borsh + base58                   | borsh + base58 (same)                  |
| Targeted invite | Yes                              | Yes                                    |
| Open invite     | Yes (commitment/reveal)          | Yes (simpler — no on-chain commitment) |
| On-chain step   | `AddMembers` in context contract | `AddMembers` in group contract         |
| Handler         | `join_context.rs`                | `join_group.rs`                        |


### New files (Phase 6)

- `core/crates/context/src/handlers/create_group_invitation.rs` (~60 LOC)
- `core/crates/context/src/handlers/join_group.rs` (~120 LOC)

### Modified files (Phase 6)


| File                                  | Change                                                                   |
| ------------------------------------- | ------------------------------------------------------------------------ |
| `primitives/src/context.rs`           | Add `GroupInvitationPayload` type + `new()` + `parts()`                  |
| `context/primitives/src/group.rs`     | Add `CreateGroupInvitationRequest/Response`, `JoinGroupRequest/Response` |
| `context/primitives/src/messages.rs`  | Add `CreateGroupInvitation`, `JoinGroup` to `ContextMessage`             |
| `context/src/handlers.rs`             | `pub mod` + match arms                                                   |
| `server/src/admin/handlers/groups.rs` | Add `create_invitation` + `join_group` handlers                          |
| `server/src/admin/service.rs`         | Register 2 new routes                                                    |


### New API routes (Phase 6)

```
POST  /admin-api/groups/:group_id/invite   → CreateGroupInvitation  (admin only)
POST  /admin-api/groups/join               → JoinGroup              (any identity)
```

---

## Full File Change Table


| Phase | File                                              | Type    | Est. LOC | Status |
| ----- | ------------------------------------------------- | ------- | -------- | ------ |
| P1    | `primitives/src/context.rs`                       | Modify  | +40      | ✅      |
| P1    | `store/src/key.rs`                                | Modify  | +5       | ✅      |
| P1    | `store/src/key/group.rs`                          | **New** | ~250     | ✅      |
| P1    | `store/src/types/group.rs`                        | **New** | ~30      | ✅      |
| P2    | `context/primitives/src/group.rs`                 | **New** | ~200     | ✅      |
| P2    | `context/primitives/src/messages.rs`              | Modify  | +80      | ✅      |
| P2    | `context/primitives/src/lib.rs`                   | Modify  | +1       | ✅      |
| P2    | `context/src/group_store.rs`                      | **New** | ~260     | ✅      |
| P2    | `context/src/lib.rs`                              | Modify  | +1       | ✅      |
| P2    | `store/src/types.rs`                              | Modify  | +1       | ✅      |
| P2    | `context/src/handlers/create_group.rs`            | **New** | ~65      | ✅      |
| P2    | `context/src/handlers/delete_group.rs`            | **New** | ~55      | ✅      |
| P2    | `context/src/handlers/add_group_members.rs`       | **New** | ~35      | ✅      |
| P2    | `context/src/handlers/remove_group_members.rs`    | **New** | ~35      | ✅      |
| P2    | `context/src/handlers/get_group_info.rs`          | **New** | ~50      | ✅      |
| P2    | `context/src/handlers/list_group_members.rs`      | **New** | ~40      | ✅      |
| P2    | `context/src/handlers.rs`                         | Modify  | +30      | ✅      |
| P2    | `server/src/admin/handlers/groups.rs`             | **New** | ~25      | ✅      |
| P2    | `server/src/admin/handlers/groups/create_group.rs`| **New** | ~70      | ✅      |
| P2    | `server/src/admin/handlers/groups/delete_group.rs`| **New** | ~60      | ✅      |
| P2    | `server/src/admin/handlers/groups/get_group_info.rs`| **New** | ~55    | ✅      |
| P2    | `server/src/admin/handlers/groups/add_group_members.rs`| **New** | ~55 | ✅      |
| P2    | `server/src/admin/handlers/groups/remove_group_members.rs`| **New** | ~50 | ✅   |
| P2    | `server/src/admin/handlers/groups/list_group_members.rs`| **New** | ~60  | ✅     |
| P2    | `server/primitives/src/admin.rs`                  | Modify  | +120     | ✅      |
| P2    | `server/src/admin/handlers.rs`                    | Modify  | +1       | ✅      |
| P2    | `server/src/admin/service.rs`                     | Modify  | +20      | ✅      |
| P3    | `context/src/handlers/create_context.rs`          | Modify  | +50      | ✅      |
| P3    | `context/src/handlers/delete_context.rs`          | Modify  | +20      | ✅      |
| P3    | `context/primitives/src/group.rs`                 | Modify  | +10      | ✅      |
| P3    | `context/primitives/src/messages.rs`               | Modify  | +5       | ✅      |
| P3    | `context/primitives/src/client.rs`                 | Modify  | +15      | ✅      |
| P3    | `context/src/handlers.rs`                          | Modify  | +5       | ✅      |
| P3    | `context/src/handlers/list_group_contexts.rs`     | **New** | ~25      | ✅      |
| P3    | `server/src/admin/handlers/groups/list_group_contexts.rs`| **New** | ~55 | ✅      |
| P3    | `server/src/admin/handlers/groups.rs`              | Modify  | +1       | ✅      |
| P3    | `server/src/admin/service.rs`                      | Modify  | +4       | ✅      |
| P3    | `server/primitives/src/admin.rs`                   | Modify  | +10      | ✅      |
| P4    | `context/src/handlers/upgrade_group.rs`           | **New** | ~300     | ✅      |
| P4    | `context/src/handlers/get_group_upgrade_status.rs`| **New** | ~20      | ✅      |
| P4    | `context/src/handlers/retry_group_upgrade.rs`     | **New** | ~90      | ✅      |
| P4    | `context/primitives/src/group.rs`                 | Modify  | +40      | ✅      |
| P4    | `context/primitives/src/messages.rs`              | Modify  | +15      | ✅      |
| P4    | `context/primitives/src/client.rs`                | Modify  | +60      | ✅      |
| P4    | `server/src/admin/handlers/groups/upgrade_group.rs`| **New** | ~85     | ✅      |
| P4    | `server/src/admin/handlers/groups/get_group_upgrade_status.rs`| **New**| ~85| ✅   |
| P4    | `server/src/admin/handlers/groups/retry_group_upgrade.rs`| **New** | ~60 | ✅    |
| P4    | `server/primitives/src/admin.rs`                  | Modify  | +80      | ✅      |
| P5    | `store/src/key/group.rs`                          | Modify  | +1       | ✅      |
| P5    | `store/src/key.rs`                                | Modify  | +1       | ✅      |
| P5    | `context/src/group_store.rs`                      | Modify  | +30      | ✅      |
| P5    | `context/src/handlers/execute.rs`                 | Modify  | +80      | ✅      |
| P5    | `context/src/lib.rs`                              | Modify  | +70      | ✅      |
| P5    | `context/src/handlers/upgrade_group.rs`           | Modify  | +60      | ✅      |
| P6    | `primitives/src/context.rs`                       | Modify  | +100     | ✅      |
| P6    | `context/primitives/src/group.rs`                 | Modify  | +35      | ✅      |
| P6    | `context/primitives/src/messages.rs`               | Modify  | +10      | ✅      |
| P6    | `context/primitives/src/client.rs`                 | Modify  | +35      | ✅      |
| P6    | `context/src/handlers/create_group_invitation.rs` | **New** | ~55      | ✅      |
| P6    | `context/src/handlers/join_group.rs`              | **New** | ~90      | ✅      |
| P6    | `context/src/handlers.rs`                          | Modify  | +10      | ✅      |
| P6    | `server/primitives/src/admin.rs`                   | Modify  | +65      | ✅      |
| P6    | `server/src/admin/handlers/groups.rs`              | Modify  | +2       | ✅      |
| P6    | `server/src/admin/handlers/groups/create_group_invitation.rs` | **New** | ~60 | ✅ |
| P6    | `server/src/admin/handlers/groups/join_group.rs`   | **New** | ~65      | ✅      |
| P6    | `server/src/admin/service.rs`                      | Modify  | +8       | ✅      |


**Totals**: 16 new files · 10 modified files · ~2700 LOC