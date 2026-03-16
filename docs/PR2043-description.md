# [core] Implement hierarchical context group management

## Description

Implements the full Context Groups feature across `core/crates/`, enabling administrators to organize contexts into groups with shared application targets, coordinated upgrades, and membership management. Built across 6 phases on top of the `context-config` contract types (`ContextGroupId`, `GroupRequest`, `AppKey`).

### Phase 1 — Storage Foundation
- **5 new key structs** in `store/src/key/group.rs`: `GroupMeta` (0x20), `GroupMember` (0x21), `GroupContextIndex` (0x22), `ContextGroupRef` (0x23), `GroupUpgradeKey` (0x24) — all with `AsKeyParts`/`FromKeyParts` impls and roundtrip tests
- **Value types**: `GroupMetaValue`, `GroupUpgradeValue`, `GroupUpgradeStatus` with Borsh serialization
- **Primitive types** in `primitives/src/context.rs`: `UpgradePolicy` (Automatic / LazyOnAccess / Coordinated), `GroupMemberRole` (Admin / Member), `GroupInvitationPayload`

### Phase 2 — Group CRUD + Membership
- **`group_store.rs`** (~390 LOC): Complete storage helper layer — CRUD for group metadata, members, context indices, and upgrade records with efficient key-only iteration for counting
- **6 actor handlers**: `create_group`, `delete_group`, `add_group_members`, `remove_group_members`, `get_group_info`, `list_group_members`
- **6 admin API routes** with corresponding server handlers

### Phase 3 — Context-Group Integration
- **`create_context.rs`**: Pre-validates group membership + app override; post-creation registers context in group index
- **`delete_context.rs`**: Unregisters context from group on deletion
- **`list_group_contexts`**: New paginated endpoint for listing a group's contexts

### Phase 4 — Upgrade Propagation
- **Canary-first upgrade strategy**: Upgrades the first context as a canary; on success, spawns an async propagator for the remaining contexts
- **`propagate_upgrade()`**: Async fn that iterates group contexts, calls `update_application()` per context, persists progress after each step
- **Status tracking**: `GroupUpgradeStatus::InProgress { total, completed, failed }` → `Completed { completed_at }`
- **3 API endpoints**: `POST /upgrade`, `GET /upgrade/status`, `POST /upgrade/retry`

### Phase 5 — Advanced Policies + Crash Recovery
- **Lazy-on-access upgrade**: `execute.rs` transparently upgrades stale contexts before method execution when `UpgradePolicy::LazyOnAccess` is set
- **Crash recovery**: `ContextManager::started()` scans for `InProgress` upgrades and re-spawns propagators
- **Auto-retry**: Failed context upgrades are retried up to 3× with exponential backoff (5s, 10s, 20s)

### Phase 6 — Group Invitations + Join Flow
- **`GroupInvitationPayload`**: Borsh-serialized + base58-encoded invitation token (mirrors `ContextInvitationPayload` pattern)
- **Targeted** (`invitee_identity: Some`) and **open** (`invitee_identity: None`) invitation modes
- **Join validation**: Verifies inviter is still admin, checks invitee identity match, validates expiration
- **2 API endpoints**: `POST /groups/:id/invite`, `POST /groups/join`

### Post-implementation fixes (PR review)
- **Phase 1**: Fixed off-by-one in `propagate_upgrade` completed counter on retry/recovery paths
- **Phase 2**: Fixed swallowed store errors, stale `GroupContextIndex` on re-registration, orphaned upgrade records on group deletion, `Duration::new` panic on malformed Borsh, switched to `ValidatedJson` for upgrade input
- **Phase 3**: Rewrote `count_group_admins`/`count_group_contexts` without Vec allocation, added offset/limit to `enumerate_group_contexts`, batched member cleanup in delete_group
- **Phase 4**: Decoupled `GroupUpgradeValue`/`GroupUpgradeStatus` store types from `calimero-context-primitives` API boundary — introduced `GroupUpgradeInfo` and primitives-local `GroupUpgradeStatus` with `From` conversion impls
- **Phase 5**: Added `count_group_members()` for efficient member counting, inlined value read in `enumerate_in_progress_upgrades` to reuse store handle

### New API routes

```
POST   /admin-api/groups                              → CreateGroup
GET    /admin-api/groups/:group_id                    → GetGroupInfo
DELETE /admin-api/groups/:group_id                    → DeleteGroup
POST   /admin-api/groups/:group_id/members            → AddGroupMembers
POST   /admin-api/groups/:group_id/members/remove     → RemoveGroupMembers
GET    /admin-api/groups/:group_id/members            → ListGroupMembers
GET    /admin-api/groups/:group_id/contexts           → ListGroupContexts
POST   /admin-api/groups/:group_id/upgrade            → UpgradeGroup
GET    /admin-api/groups/:group_id/upgrade/status     → GetGroupUpgradeStatus
POST   /admin-api/groups/:group_id/upgrade/retry      → RetryGroupUpgrade
POST   /admin-api/groups/:group_id/invite             → CreateGroupInvitation
POST   /admin-api/groups/join                         → JoinGroup
```

## Test plan

- [x] `cargo check --workspace` — compiles cleanly
- [x] `cargo fmt --check` — no formatting issues
- [x] `cargo clippy -- -A warnings` — no errors
- [x] `cargo test -p calimero-context` — 16 tests pass
- [x] `cargo test -p calimero-context-primitives` — 3 tests pass
- [x] `cargo test -p calimero-store` — key roundtrip + value serialization tests pass
- [ ] End-to-end group lifecycle tests via `meroctl` against a running `merod` node
- [ ] Contract integration tests in `contracts/` repo

## Stats

**47 files changed** — 16 new files, 31 modified — **+4,197 / −20 lines**
