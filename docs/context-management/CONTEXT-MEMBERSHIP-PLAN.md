# Context Membership Replication: E2E Plan

## Current State

Context membership (`ContextIdentity` records) is written locally but **not
replicated** for most flows. The blockchain contract was the single source of
truth for "who is in context C" — every node queried it. That's gone, and the
replacement (noops) doesn't propagate membership changes.

### What Works (replicated via group governance DAG)

| Flow | Replicated? | Mechanism |
|------|-------------|-----------|
| Join context via group | **YES** | `MemberJoinedContext` governance op |
| Kick from group → cascade remove | **YES** | `MemberRemoved` cascade |
| Register context in group | **YES** | `ContextRegistered` governance op |

### What Doesn't Work (local-only)

| Flow | Replicated? | Problem |
|------|-------------|---------|
| `create_context` (creator as member) | **NO** | Direct `handle.put(ContextIdentity)` |
| `join_context` (via invitation) | **NO** | `update_identity` local only |
| `invite_member` | **NO** | `noop_config_add_members` does nothing |
| WASM `AddMember` / `RemoveMember` | **NO** | `noop_config_add/remove_members` |
| `grant` / `revoke` capabilities | **NO** | `noop_config_grant/revoke` |
| `update_application` | **NO** | `noop_config_update_application` |
| `sync_context_config` (member sync) | **BROKEN** | Reads local DB, not external source |

## Design: Group-Centric Context Membership

### Principle

**Every context belongs to a group. The group governance DAG is the single
authority for who can access which contexts.**

This matches the on-chain contract model where contexts were registered in
groups, and `join_context_via_group` was the governed join path.

### Key Decisions

1. **No standalone contexts** — every context must have a group. A "personal"
   context gets a single-member group automatically.

2. **Context membership = group membership + visibility/allowlist** — you can
   access a context if you're a group member AND (the context is Open, OR
   you're on the allowlist for Restricted contexts, OR you're an admin).

3. **`MemberJoinedContext` is the only way to join** — all join paths go
   through the governance op. No direct `ContextIdentity` writes for
   membership (private keys are still written locally).

4. **WASM `AddMember`/`RemoveMember` become governance ops** — the runtime
   emits a governance op instead of a noop. The op propagates via DAG.

5. **`sync_context_config` derives from governance state** — instead of
   querying a chain, it reads group membership + context visibility from
   the local store (populated by governance ops).

## Implementation Phases

### Phase 1: Enforce group-context binding

**Goal:** Every context belongs to a group.

Changes:
- `create_context` without a `group_id` → auto-create a single-member group
  with the creator as sole admin, register the context in it
- Remove the `group_id: Option<ContextGroupId>` optionality — make it required
- Update API/CLI to reflect this

### Phase 2: Route create_context creator membership through governance

**Goal:** The context creator's membership is replicated via DAG.

Changes:
- `create_context` handler:
  1. Create the group (if auto-created) via `CreateGroup` governance op
  2. Register context via `ContextRegistered` governance op (already done)
  3. Add creator as context member via `MemberJoinedContext` governance op
  4. Store private key + sender key locally (not replicated)
- Remove direct `handle.put(ContextIdentity)` for creator membership

### Phase 3: Route invite/join through governance

**Goal:** Invitation-based joins are replicated.

Changes:
- `invite_member`: instead of `noop_config_add_members`, sign a
  `MemberJoinedContext` governance op (the admin/inviter signs it)
- `join_context`: after invitation validation, the governance op is already
  published by the inviter — the joiner just needs to store private keys
  locally and subscribe
- `join_context_by_open_invitation`: same pattern — validate invitation,
  sign `MemberJoinedContext`, store keys locally

### Phase 4: Route WASM AddMember/RemoveMember through governance

**Goal:** Runtime membership mutations are replicated.

Changes:
- `process_context_mutations` for `AddMember`:
  1. Determine the group for this context
  2. Sign `MemberJoinedContext` governance op (signer = context identity
     that executed the WASM, or the group admin)
  3. Replace `noop_config_add_members`
- `process_context_mutations` for `RemoveMember`:
  1. Add new `GroupOp::MemberLeftContext { member, context_id }` variant
  2. Apply: delete `ContextIdentity` + tracking record
  3. Replace `noop_config_remove_members`

### Phase 5: Fix sync_context_config

**Goal:** Member sync derives from governance state.

Changes:
- `sync_members` in `client/sync.rs`:
  1. Read group membership for this context's group
  2. Read context visibility/allowlist
  3. Derive expected member set
  4. Compare with local `ContextIdentity` records
  5. Add/remove differences
- Remove `members_revision` tracking (governance DAG replaces revision
  counting — the DAG head IS the revision)
- `sync_context_config` becomes "ensure my local state matches governance"

### Phase 6: Handle capabilities via governance

**Goal:** Grant/revoke are replicated.

Changes:
- Add `GroupOp::ContextCapabilityGranted { context_id, member, capability }`
- Add `GroupOp::ContextCapabilityRevoked { context_id, member, capability }`
- Replace `noop_config_grant` / `noop_config_revoke`
- Store capability state in group store (per context+member)

### Phase 7: Handle application updates via governance

**Goal:** Application changes are replicated.

Changes:
- Application updates for group contexts already go through
  `TargetApplicationSet` governance op at the group level
- For per-context application updates: add
  `GroupOp::ContextApplicationUpdated { context_id, application_id }`
- Replace `noop_config_update_application`

### Phase 8: Remove all noops

**Goal:** No dead code.

Changes:
- Delete `noop_config_add_context`
- Delete `noop_config_update_application`
- Delete `noop_join_context_commit_invitation`
- Delete `noop_join_context_reveal_invitation`
- Delete `noop_config_add_members`
- Delete `noop_config_remove_members`
- Delete `noop_config_grant`
- Delete `noop_config_revoke`

## Dependency Order

```
Phase 1 (group-context binding)     — foundation
Phase 2 (create_context)            — depends on Phase 1
Phase 3 (invite/join)               — depends on Phase 2
Phase 4 (WASM mutations)            — depends on Phase 2
Phase 5 (sync)                      — depends on Phase 2-4
Phase 6 (capabilities)              — independent
Phase 7 (application updates)       — independent
Phase 8 (noop removal)              — depends on all above
```

Phases 1-3 are the critical path. Phase 4 and 5 follow naturally.
Phases 6-7 are polish. Phase 8 is cleanup.

## What This Achieves

After full implementation:
- **Every context membership change propagates via the governance DAG**
- **Every node converges to the same member list** (DAG ordering)
- **Cascade removal works end-to-end** (group kick → removed from all contexts)
- **sync_context_config derives from governance state** (no external source needed)
- **WASM apps can add/remove members** with full replication
- **No noops remain** — every operation has a real implementation
- **Single-admin groups are fully convergent** (DAG linear chain)
