# Improvement: Group API Should Auto-Resolve Node Identity & Signing Keys

**Status: RESOLVED**

> **Resolution:** Group operations now use a **dedicated group identity keypair**
> (`[identity.group]` in `config.toml`, generated at `merod init`), completely
> decoupled from the NEAR signer key. `node_near_identity()` was removed and
> replaced by `node_group_identity()`. All identity and signing key parameters
> (`--admin-identity`, `--joiner-identity`, `--requester-secret`) are
> auto-resolved server-side — the CLI no longer requires these flags. Dead code
> related to the old NEAR-coupled identity path was cleaned up as part of this
> change. Use `meroctl node identity` (or `meroctl node id`) to view a node's
> group public key.

## Original Problem (for historical context)

The `context create` API auto-generates identities and resolves signing keys
from the node's config internally — callers don't pass any keys.

The `group` API commands previously required callers to manually extract and pass:

- `--admin-identity` / `--joiner-identity` (public key from `config.toml`)
- `--requester-secret` (secret key hex, derived from base58 `secret_key` in `config.toml`)
- `--requester` (public key for invite/members commands)
- `--app-key` (could be auto-generated as a random 32-byte key)

This was inconsistent and created a poor developer experience. The manual
workaround (reading keys from `config.toml`, base58-decoding, hex-converting)
is no longer needed.

## Improvement: Group Membership State Not Propagated to Inviter

**Status: RESOLVED**

> **Resolution:** Group membership state is now propagated via `group sync`.
> The contract exposes group member lists and the node-side sync fetches and
> persists them. After Node B joins a group, running `group sync` on Node A
> updates its local member list. P2P gossip for real-time propagation remains
> a future enhancement.

## Improvement: Group Context List Not Propagated to Other Members

**Status: RESOLVED**

> **Resolution:** `sync_group_state_from_contract` now calls the contract's
> `group_contexts()` view method and persists the context list to the local
> store. Running `group sync` on Node B after Node A creates a context makes
> the context visible on Node B.

## Improvement: Group Members Should Be Able to Join Group Contexts Without Full Invitation Flow

**Status: RESOLVED**

> **Resolution:** The `group join-group-context` command allows a verified
> group member to join any context in their group directly — no invitation
> ceremony needed. The handler verifies group membership, auto-generates a
> context identity, adds the member on-chain, and syncs state. The full
> invitation flow still works for non-group contexts and cross-group invites.

## Improvement: Group Member Removal Should Cascade to Context Membership

**Status: RESOLVED**

> **Resolution:** `remove_group_members` now cascades to all contexts in the
> group. The handler calls `group_client.remove_group_members()` on-chain
> (the contract cascades removal to all context memberships), then enumerates
> all group contexts and runs `sync_context_config` for each to propagate
> the removal locally. Verified during testing: after removing Node B from
> the group, Node B received `Unauthorized` when accessing any group context,
> and `context identity list` confirmed Node B's context identity was removed.

---

## Improvement: API Error Messages Not Surfaced to CLI

**Status: RESOLVED**

> **Resolution:** Server handlers now extract and forward the actual error
> message from contract panics and internal failures to the HTTP response body.
> The CLI displays the error message instead of just the HTTP status code.
> Permission-related rejections (missing capability, not on allowlist, not a
> member, not an admin) are now visible directly in `meroctl` output.

---

## Bug: Group Upgrade Blocks Same-ApplicationId Version Upgrades

**Status: RESOLVED**

For signed bundles, `ApplicationId = hash(package, signer_id)` — it stays the
same across version upgrades as long as the package name and signer are
unchanged (see `core/docs/app-lifecycle/README.md`, "AppKey Continuity").

The `group upgrade trigger` command fails for same-app version upgrades with:

```
"group is already targeting this application"
```

---

### How the existing context update system handles this (reference implementation)

The non-group context update system **already supports** same-ApplicationId
upgrades. Two complementary mechanisms detect and apply bytecode changes:

#### Mechanism 1: Revision-based sync (`sync_context_config`)

The on-chain contract (`context-config`) wraps each context's `Application` in a
`Guard<T>` whose `Drop` impl auto-increments a revision counter on every
mutable access:

```rust
// contracts/contracts/near/context-config/src/guard.rs:126
impl<T> Drop for GuardMut<'_, T> {
    fn drop(&mut self) {
        self.inner.revision = self.inner.revision.wrapping_add(1);
    }
}
```

During `sync_context_config` (`context/primitives/src/client/sync.rs:276-335`),
the node compares its local `ContextConfig.application_revision` against the
contract's `application_revision()` view. If they differ → the application was
updated on-chain → fetch the new `Application` → install bytecode → update
`ContextMeta`. This is the passive/background mechanism.

#### Mechanism 2: Migration-aware direct update (`UpdateApplicationRequest`)

The `UpdateApplicationRequest` handler
(`context/src/handlers/update_application.rs:57-66`) uses the `migration`
parameter as the **signal** that bytecode changed under the same ApplicationId:

```rust
// Skip ONLY when ApplicationId is unchanged AND no migration is requested.
// When migration IS requested, the WASM binary may have been replaced
// under the same application ID (same signing key), so we must proceed.
if migration.is_none() {
    if let Some(ref context) = context_meta {
        if application_id == context.application_id {
            return ActorResponse::reply(Ok(()));
        }
    }
}
```

When migration IS present (same-app version upgrade):

1. **Invalidates the in-memory module cache** (line 84) — the WASM binary was
   replaced under the same ApplicationId by `app install` of v2, so the cached
   module is stale:
   ```rust
   if self.applications.remove(&application_id).is_some() {
       debug!("Invalidated stale cached application before migration module load");
   }
   ```
2. **Re-fetches bytecode** from the node's blob storage (now pointing to v2).
3. **Runs the migration function** against the new WASM module.
4. **Calls `finalize_application_update`** → contract's `update_application`
   → bumps `application_revision` on-chain (prevents re-sync).
5. **Updates `ContextMeta`** in the local store.

**Key insight:** The `migration` parameter is the discriminator. Without
migration, same-ApplicationId is a no-op. With migration, the system knows
bytecode changed and proceeds with the full update pipeline.

---

### Three bugs in the group upgrade flow

All three share the same root cause: the group flow compares `ApplicationId`
without considering the migration signal.

#### Bug 1: `validate_upgrade` rejects same ApplicationId

`validate_upgrade()` in `context/src/handlers/upgrade_group.rs` (line 398):

```rust
if meta.target_application_id == *target_application_id {
    bail!("group is already targeting this application");
}
```

No migration awareness. Always rejects same-app version upgrades.

#### Bug 2: Propagator skips all same-ApplicationId contexts

`propagate_upgrade()` in `upgrade_group.rs` (line 490):

```rust
Ok(Some(ctx)) if ctx.application_id == target_application_id => {
    completed += 1;
    // ... skips this context
}
```

Even if Bug 1 were bypassed, every context would be skipped because
`Context.application_id` (from `ContextMeta`) is the same `ApplicationId`. The
new bytecode would never be loaded.

Note: the *downstream* call that the propagator makes —
`context_client.update_application(context_id, &target_app, &signer, migrate_method)`
— correctly passes the `migration` parameter through to the
`UpdateApplicationRequest` handler, which **already handles same-ApplicationId
with migration correctly** (see Mechanism 2 above). The bug is only in the
propagator's skip guard before that call is reached.

#### Bug 3: `maybe_lazy_upgrade` never triggers for same ApplicationId

`maybe_lazy_upgrade()` in `context/src/handlers/execute.rs` (line 1238):

```rust
if *current_application_id == meta.target_application_id {
    return None; // already at target
}
```

For `LazyOnAccess` groups, this function is called before every context
execution to check if an upgrade is pending. With same ApplicationId, it
always returns `None` — the lazy upgrade never fires.

---

### Proposed fix: replicate the migration-aware pattern

The existing context update system shows that **migration is the signal** for
same-ApplicationId upgrades. The group flow already passes migration through
to the downstream handler. The fixes are to lift the three ApplicationId-only
gates:

#### Fix 1: `validate_upgrade` — allow same ApplicationId when migration is present

```rust
// Current (broken):
if meta.target_application_id == *target_application_id {
    bail!("group is already targeting this application");
}

// Fixed:
if meta.target_application_id == *target_application_id && migration.is_none() {
    bail!("group is already targeting this application and no migration was requested");
}
```

This mirrors the `UpdateApplicationRequest` handler's `if migration.is_none()`
guard. The `migration` parameter is already available in the
`UpgradeGroupRequest` and passed to `validate_upgrade`'s caller.

#### Fix 2: Propagator — don't skip when migration is present

```rust
// Current (broken):
Ok(Some(ctx)) if ctx.application_id == target_application_id => {
    completed += 1;
    continue; // skips
}

// Fixed:
Ok(Some(ctx))
    if ctx.application_id == target_application_id
       && migration.is_none() =>
{
    completed += 1;
    continue; // skip only when no migration
}
```

When migration IS present, the propagator proceeds to call
`context_client.update_application()` which passes migration through to the
handler that already correctly invalidates the cache and reloads bytecode.

#### Fix 3: `maybe_lazy_upgrade` — check migration availability

```rust
// Current (broken):
if *current_application_id == meta.target_application_id {
    return None;
}

// Fixed:
if *current_application_id == meta.target_application_id && meta.migration.is_none() {
    return None;
}
```

When the group has a stored migration method (set during `upgrade trigger`),
proceed even for same ApplicationId. The `migration` bytes are already stored
in `GroupMetaValue.migration` and extracted at line 1243.

**Idempotency guard for LazyOnAccess:** After a context successfully runs the
migration, we need to prevent re-running it on next access. Two approaches:

- **Approach A (recommended): Track `bytecode_version` in `ContextMeta`.**
  Add a `bytecode_version: Box<str>` field to `ContextMeta`. After migration,
  store `ApplicationMeta.version`. In `maybe_lazy_upgrade`, compare
  `context.bytecode_version` vs `ApplicationMeta.version`. If equal → already
  upgraded, skip. This is clean and mirrors how `application_revision` works
  on-chain.

- **Approach B (simpler): Per-context upgrade generation.**
  Add `upgrade_generation: u64` to `GroupMetaValue` (bumped on each upgrade
  trigger). Store `last_applied_generation: u64` per group-context entry.
  `maybe_lazy_upgrade` compares the two. If equal → already upgraded.

**The Automatic (non-lazy) propagator doesn't need an idempotency guard** because
it runs once per upgrade trigger, tracks progress in `GroupUpgradeValue`, and
won't re-run after completion.

---

### Files to change

| File | What changes |
|------|-------------|
| `context/src/handlers/upgrade_group.rs` | Fix 1: pass `migration` to `validate_upgrade`, relax ApplicationId check. Fix 2: add migration awareness to propagator skip. |
| `context/src/handlers/execute.rs` | Fix 3: add migration awareness to `maybe_lazy_upgrade`. Add idempotency guard (Approach A or B). |
| `store/src/types/context.rs` | (Approach A) Add `bytecode_version: Box<str>` to `ContextMeta`. |
| `context/src/handlers/update_application.rs` | (Approach A) After migration, write `ApplicationMeta.version` to `ContextMeta.bytecode_version`. |

### Call chain (working downstream, broken upstream)

```
group upgrade trigger (CLI)
  → UpgradeGroupRequest (server handler)
    → validate_upgrade()              ← BUG 1: rejects same ApplicationId
    → propagate_upgrade()             ← BUG 2: skips same ApplicationId
      → context_client.update_application(ctx, app, signer, migration)
        → UpdateApplicationRequest handler     ← ALREADY WORKS (migration-aware)
          → invalidates cache, reloads module
          → runs migration function
          → finalize_application_update → contract update_application
            → Guard<T>.Drop → revision++ ← ALREADY WORKS (revision bumped)

maybe_lazy_upgrade (LazyOnAccess)    ← BUG 3: never triggers for same ApplicationId
  → context_client.update_application(ctx, app, signer, migration)
    → UpdateApplicationRequest handler     ← ALREADY WORKS
```

The downstream pipeline (from `update_application` onward) already handles
same-ApplicationId correctly. The only blockers are the three upstream gates.

**Priority:** High — this makes the entire group upgrade propagation feature
non-functional for signed bundles, which is the recommended app packaging
format.

---

## Bug: Group Members Cannot Create Contexts Within Their Group

**Status: RESOLVED**

> **Resolution:** The capability-based permission system (migration 06) replaced
> the admin-only gate. `register_context_in_group` now checks:
>
> ```rust
> require!(
>     group.admins.contains(signer_id)
>         || (group.members.contains(signer_id) && has_capability(caps, CAN_CREATE_CONTEXT)),
>     "caller must be admin or member with CAN_CREATE_CONTEXT capability"
> );
> ```
>
> Admins bypass all capability checks. Regular members need `CAN_CREATE_CONTEXT`
> (bit 0). The `default_member_capabilities` group setting controls what new
> members receive. Context visibility (Open/Restricted) and allowlists provide
> further control over who can join each context.

## Original Problem (for historical context)

The [proposal](../../PROPOSAL-hierarchical-context-management.md) explicitly
states that **any group member** should be able to create contexts within a
group without admin approval:

> "Users create and destroy contexts freely within their group." (§1)
>
> "Who can create contexts — any group member" (§3)
>
> "The user never needs admin approval to create a context. They just need to
> be a group member." (§5.2)

The motivating use case is DMs and channels: in a messaging workspace with
hundreds of members, every member must be able to create DM contexts (2-person
private conversations) on their own. Requiring admin approval for every DM
defeats the purpose of the group model.

The original contract code (`register_context_to_group`) enforced admin-only
context creation, which broke this use case. The permission system now allows
admins to grant `CAN_CREATE_CONTEXT` to members selectively or via defaults.

---

## Bug: Lazy Upgrade Not Propagated to Peer Nodes

**Status: RESOLVED**

> **Resolution:** All three sub-bugs fixed:
>
> **Fix C:** `join_group_context.rs` now clones the datastore and calls
> `group_store::register_context_in_group()` after the contract call
> succeeds, ensuring `get_group_for_context()` works immediately.
>
> **Fix A:** Migration method is now stored on-chain in `OnChainGroupMeta.migration_method`.
> `set_group_target` accepts and stores `Option<String>`. The query response
> (`GroupInfoResponse`) returns it. `sync_group_state_from_contract` reads it
> from the contract first, falling back to local. Contract storage migration
> `05_group_migration_method` handles existing state. SDK types
> (`GroupRequestKind::SetTargetApplication`, `GroupInfoQueryResponse`) and
> external client updated to pass the field through.
>
> **Fix B:** `sync_group.rs` now checks if the target application blob is
> available locally after syncing group state. If not, it fetches the blob
> via P2P (DHT discovery + `get_blob_bytes`) and installs the bundle using
> `install_application_from_bundle_blob`. Non-fatal — logs warnings on
> failure so sync doesn't break if the blob isn't immediately available.
> `sync_group_state_from_contract` returns `(GroupMetaValue, GroupInfoQueryResponse)`
> so callers have access to the full target application info (blob ID, source URL).

---

## Bug: `group sync` Prematurely Fetches Blobs for Unjoined Contexts

**Status: OPEN**

When a node runs `group sync`, the sync handler fetches the context list from
the contract and then attempts to fetch the application blob for the group's
target application via P2P (DHT). This blob fetch fails with a timeout when
the node hasn't joined any of the group's contexts, because the node isn't
part of the P2P mesh for those contexts and can't discover blob providers.

**Observed behavior:**

```
INFO  Blob not found locally, attempting network discovery
      blob_id=5GXpVb... context_id=9XQgmx...
WARN  Failed to query DHT for blob blob_id=5GXpVb...
      error=Blob query failed: Timeout { key: Key(b"...") }
```

This causes `group sync` to take ~15-30 seconds (DHT timeout) even though the
sync of metadata, members, and contexts succeeds. It also means the recommended
testing order must be:

1. `join-group-context` (join the context first, establishing P2P connectivity)
2. `group sync` (now blob discovery works because the node is on the mesh)

**Expected behavior:** `group sync` should sync metadata, members, and contexts
**without** attempting to fetch application blobs. Blob fetching should only
happen when the node actually joins a context (which already works — the
`join_group_context` handler installs the app). The sync handler's blob fetch
is a premature optimization that causes failures for the common case of syncing
before joining.

**Workaround:** Run `join-group-context` before `group sync`, or ignore the
DHT timeout warning (sync still succeeds for metadata).

**Proposed fix:** Remove (or make optional/lazy) the blob fetch in
`sync_group.rs`. The blob will be fetched when the node actually joins a
context via `join_group_context`, which already handles app installation.

**Priority:** Medium — causes confusing timeout errors and forces a non-obvious
command ordering. Does not block functionality.

---

## Bug: `get-visibility` Returns 500 for Newly Created Contexts

**Status: OPEN**

Running `group contexts get-visibility` immediately after creating a context
in a group returns a 500 error:

```
./target/release/meroctl --node node-a group contexts get-visibility "$GROUP_ID" "$CONTEXT_A"
# → 500 Internal Server Error
```

**Root cause:** The `get_context_visibility` handler reads visibility from the
local store (`GroupContextVisibility` key, prefix 0x27). However, the
`create_context` handler calls `register_context_in_group` on-chain (which
stores visibility with the group's `default_context_visibility`) but may not
write the `GroupContextVisibility` entry to the local store. The local store
only gets populated after a `group sync` or when `set-visibility` is called
explicitly.

**Workaround:** Run `group sync` after context creation, then query visibility.

**Proposed fix:** After `register_context_in_group` succeeds in the context
creation handler, also write the `GroupContextVisibility` entry to the local
store using the group's `default_context_visibility`. Alternatively, the
`get_context_visibility` handler could fall back to querying the contract when
the local entry is missing.

**Priority:** Medium — DX issue. The data exists on-chain but the local query
fails until sync.

---

## Bug: `list-groups` Shows Synced Groups on Non-Member Nodes

**Status: OPEN**

When two local nodes are connected over P2P, a group created on Node A can
appear in Node B's `group list` / `GET /admin-api/groups` output even when
Node B has **not** joined that group. Follow-up operations then fail because
Node B is not actually a member.

**Observed behavior:**

- Node A creates a group.
- Node B, which is connected to Node A but has not joined the group, starts
  showing that group in `/admin-api/groups`.
- Attempts to use the group from Node B fail later because membership-dependent
  operations still reject the node.

**Root cause:** `list_all_groups` enumerates all locally stored group metadata
without checking whether the current node is a member of each group. Group
metadata can be pulled into the local store by the automatic `sync_group`
path:

- `NetworkEvent::Subscribed` on a `group/<group_id>` topic triggers
  `sync_group()`
- `GroupMutationNotification` also triggers `sync_group()`
- `sync_group_state_from_contract()` persists `GroupMetaValue` locally via
  `save_group_meta()`

Once that metadata exists in the local store, `list_all_groups` includes it
because it simply calls `enumerate_all_groups(...)`.

This means the API currently answers the question:

> "Which groups do I know about locally?"

instead of the more useful user-facing question:

> "Which groups is this node actually a member of?"

**Workaround:** Ignore groups that appear on a node until membership is
verified, or explicitly join the group before expecting it to be usable.

**Proposed fix:** Filter `list_all_groups` so it only returns groups where the
current node's group identity is present in the local member list (or otherwise
mark synced-but-not-joined groups separately in the API/UI so they are not
presented as usable workspaces).

**Priority:** Medium — confusing in multi-node local testing and misleading in
the UI, but does not corrupt data.

---

## Improvement: Group State Changes Should Auto-Propagate via P2P

**Status: OPEN — Design approved, see `2026-03-11-group-p2p-topic-design.md`**

Currently group mutation broadcasts piggyback on **context P2P topics**. Each
mutation iterates all group contexts and publishes on each context's gossipsub
topic. This has two problems:

1. **New members who haven't joined a context yet never receive notifications**
   — they're not subscribed to any context topic in the group.
2. **O(N) messages** — a group with 100 contexts sends 100 copies of every
   notification.

The existing infrastructure is mostly in place:
- `GroupMutationNotification` message type — already defined
- `GroupMutationKind` enum — already has all variants (MembersAdded,
  MembersRemoved, Upgraded, Deleted, VisibilityUpdated, etc.)
- Handler in `network_event.rs` — already auto-triggers `sync_group()` on
  receive
- Every mutation handler — already calls `broadcast_group_mutation()`

**Approved fix: Dedicated group P2P topic.** Each group gets its own gossipsub
topic (`/calimero/group/<group_id_hex>`). Mutations broadcast on this topic
(replaces context-topic piggybacking). All group members subscribe on join,
unsubscribe on removal. See `2026-03-11-group-p2p-topic-design.md` for the
full design.

**Key changes:**
- `broadcast_group_mutation` publishes on group topic (remove context list param)
- `subscribe_group` / `unsubscribe_group` on `NodeClient`
- Subscribe in `create_group`, `join_group` handlers
- Unsubscribe in `remove_group_members`, `delete_group` handlers
- Group subscription loop in `NodeManager::started` (startup)
- All 12+ mutation handlers simplified (remove context enumeration boilerplate)

**Deprecation note:** `group members add` (admin-push) will be deprecated in
favor of the invitation flow, which naturally handles group topic subscription
on the joining node.

**Priority:** High for production readiness — the current model requires N-1
manual syncs per mutation.

---

## Bug: `CAN_INVITE_MEMBERS` Does Not Actually Allow Invitation Creation

**Status: OPEN**

A non-admin group member can have capabilities `7` (all currently defined
permission bits enabled, including `CAN_INVITE_MEMBERS`) and still fail to
create a group invitation through:

- `POST /admin-api/groups/:group_id/invite`

with the runtime error:

> `requester is not an admin of group 'ContextGroupId(...)'`

This is a mismatch between the documented permission model and the current
implementation.

### Expected Behavior

Per the permission system design, invitation creation should be allowed for:

- Group admins, or
- Group members with `CAN_INVITE_MEMBERS`

The docs already describe this model:

- `CAN_INVITE_MEMBERS` = "Can create group invitations"
- authorization matrix row: `Create invitation | ✓ | ✓ (CAN_INVITE_MEMBERS) | ✗`

### Actual Behavior

`create_group_invitation` still enforces an **admin-only** role check in the
context layer:

- `core/crates/context/src/handlers/create_group_invitation.rs`
- `core/crates/context/src/group_store.rs`

Specifically, the handler currently calls `require_group_admin(...)` instead of
allowing either admin role or invite capability.

### Why This Happens

The invitation flow currently splits responsibilities:

- `create_group_invitation` checks **admin role**
- `reveal_group_invitation` is the place where capability-based invitation
  permissions were intended to matter

That leaves a broken UX/API contract: the UI can correctly show a member has
invite permission (`capabilities = 7`), but the invitation creation endpoint
still rejects the request before the capability bit has any effect.

### Proposed Fix

Update `create_group_invitation` authorization to allow:

- admins, or
- non-admin members with `CAN_INVITE_MEMBERS`

while preserving the existing requirement that the node must hold the
requester's registered signing key for the group.

### Priority

**Medium-High** — this breaks a documented and surfaced permission flow in the
admin UI and makes capability assignment misleading.

---

## Improvement: Group Members Should Have Propagated Per-Group Aliases

**Status: OPEN**

Today, group member identity is still surfaced to peers as a raw public key in
many places, especially for DM discovery/member listing flows. There is no
group-level concept of:

> "For this group, member `<public_key>` wants to be displayed as `<alias>`."

That is a gap in the current core model. We already have a working precedent for
**context aliases** inside groups, but not for **member aliases** inside groups.

### Current Implementation

The current group-member path only stores and returns membership/role data:

- `group_store::list_group_members(...)` returns `Vec<(PublicKey, GroupMemberRole)>`
- `GroupMemberEntry` in `core/crates/context/primitives/src/group.rs` contains only:
  - `identity`
  - `role`
- `GET /admin-api/groups/:group_id/members` maps that directly to
  `GroupMemberApiEntry { identity, role }`
- `sync_group_state_from_contract()` syncs member presence, role, and
  capabilities from the contract, but no alias/display-name field

So even after group membership propagates correctly, the API contract still only
answers:

> "Who are the members and what are their roles?"

instead of:

> "Who are the members, and how should each member be displayed inside this
> group?"

### Why Existing Alias Support Does Not Solve This

Core already has alias functionality, but it is not the right layer for this
problem:

- The generic alias API (`/admin-api/alias/create/...`) stores aliases in the
  node's local alias store
- Context identity aliases are typically scoped to a context and are useful for
  local/operator convenience
- Group context aliases propagate because there is dedicated group plumbing:
  `GroupContextEntry.alias`, local group-store persistence, and
  `GroupMutationKind::ContextAliasSet`

There is **no equivalent propagated group-member alias state** today:

- no `alias` field on `GroupMemberEntry`
- no group-store key/value dedicated to member aliases
- no on-chain group member alias metadata
- no `MemberAliasSet` group mutation
- no member-alias sync path in `sync_group_state_from_contract()`

That means a user cannot set an alias for their **group identity** and have it
show up on other nodes in that same group.

### Desired Behavior

Each group member should be able to set an alias for **their own identity within
that group**.

That alias should:

- be scoped to a specific group
- be visible to all group members
- propagate to peer nodes through the normal group sync / group mutation path
- appear in group member listing responses
- be usable by clients/UI for DM/member display without replacing the canonical
  identity key

Important: this is **display metadata only**. Authorization, signatures,
membership checks, and allowlists must continue to use the underlying public
key.

### Proposed Core-Level Fix

Add a first-class **group member alias** concept to the group data model.

Minimum shape:

1. **Persist alias as group state**
   - Add optional alias metadata per group member, keyed by `(group_id, member_identity)`
   - This should not live only in the local alias store

2. **Make alias part of sync/propagation**
   - `sync_group_state_from_contract()` must fetch and persist member aliases
   - group mutation broadcasting should include a dedicated member-alias update
     signal (similar to `ContextAliasSet`) so peers can update without waiting
     for a full manual sync

3. **Expose alias through API types**
   - Extend `GroupMemberEntry` and `GroupMemberApiEntry` with:
     - `alias?: String`
   - `GET /admin-api/groups/:group_id/members` should return `{ identity, role, alias? }`

4. **Add write path**
   - Add a core handler / server endpoint that lets a member set or update the
     alias for their own group identity
   - By default, a member should only be allowed to update their own alias
     unless we explicitly decide to add admin override semantics later

### Suggested API / Data Model Direction

At a high level, the group stack should gain the equivalent of:

- `set_group_member_alias(group_id, member_identity, alias)`
- `list_group_members(...) -> [{ identity, role, alias? }]`
- `GroupMutationKind::MemberAliasSet { member_identity, alias }`

If aliases are stored on-chain, `group sync` becomes the recovery path and
gossip becomes the low-latency path. If aliases are stored only locally, the
feature will not satisfy the requirement that aliases be visible to other nodes,
so local-only storage is insufficient.

### Files Likely Affected

| File | What changes |
|------|-------------|
| `core/crates/context/primitives/src/group.rs` | Extend `GroupMemberEntry` and add request/response types for member-alias mutation if needed |
| `core/crates/server/primitives/src/admin.rs` | Extend `GroupMemberApiEntry` with optional `alias` |
| `core/crates/context/src/group_store.rs` | Add local persistence/read helpers for group member aliases |
| `core/crates/context/src/handlers/list_group_members.rs` | Return alias alongside identity/role |
| `core/crates/server/src/admin/handlers/groups/list_group_members.rs` | Surface alias in REST response |
| `core/crates/context/src/handlers/sync_group.rs` / group sync helpers | Sync member aliases from authoritative group state |
| `core/crates/node/primitives/src/sync/snapshot.rs` | Add a member-alias group mutation kind |
| `core/crates/node/src/handlers/network_event.rs` | Apply propagated member-alias updates locally |

If the authoritative source of truth is on-chain, the corresponding group
contract / external client query path will also need to be extended to store and
query alias metadata per member.

### Priority

**Medium-High** — not a security blocker, but it is a major UX limitation for
DM/member surfaces because the system propagates identities but not human-readable
names for those identities.

---

## Improvement: Group IDs Should Support Human-Friendly Group Aliases

**Status: OPEN**

When a group is created, users often want to refer to it by a human-friendly
name instead of the raw group ID. Today there is no first-class group-level
alias/display-name concept in the core group model, so peers that join the same
group still end up identifying it by the raw ID unless some node or frontend
maintains its own local mapping.

### Current Implementation

The current group flow propagates operational metadata, but not a display alias
for the group itself:

- `CreateGroupResponse` returns only `group_id`
- `GroupInfoResponse` / `GroupSummary` do not include a group alias/display name
- `SignedGroupOpenInvitation` carries the join artifact (`group_id`,
  `inviter_identity`, contract coordinates, expiry, etc.) but no group alias
- `join_group` bootstraps the group from chain + invitation data, but does not
  persist any display alias for the joined group

So even if the creator wants to label the group as "Acme Workspace" or
"Team Alpha", that label is not part of the shared group experience today.

### Requested Improvement

Two related capabilities are useful:

1. **Local alias on create**
   - When a node creates a group, it should be able to store a local alias for
     that group ID immediately

2. **Alias bootstrap through invitation**
   - When an invitation is created, it should optionally include the human-friendly
     group alias
   - When another node joins via that invitation, it should save that alias
     locally for the group it just joined

### Invitation-Based Approach

Using the invitation as the bootstrap carrier is a sensible improvement because
the invitation is already the artifact used to teach a new member:

- which group they are joining
- who invited them
- which protocol/network/contract coordinates to use

Adding an optional `group_alias` field to the invitation would let the joiner
seed a local alias at join time without an extra request.

That would improve first-join UX:

- creator creates group + sets alias locally
- invitation includes alias
- joiner stores alias locally on successful join

### Why Invitation-Only Is Probably Not Enough

Invitation-carried alias is a good bootstrap path, but it is not a complete
propagated group alias system:

- different invitations could carry different aliases for the same group
- alias updates after the first join would not reach existing members
- nodes that learn about a group via sync rather than invitation would still
  miss the alias
- there would be no authoritative source of truth for the current group alias

So invitation alias should be treated as a **bootstrap hint**, not the sole
source of truth.

### Better Long-Term Design

The stronger design is:

1. **Authoritative group alias in shared group metadata**
   - extend group metadata / group info responses with optional `alias`
   - sync it through the same path as other group metadata

2. **Invitation carries alias as a convenience bootstrap**
   - add optional `group_alias` to the invitation payload
   - on successful join, persist it locally immediately

3. **Sync/gossip reconciles later changes**
   - if the group alias changes later, existing members should learn that via
     group sync / group mutation propagation rather than from stale invitations

This gives both:

- good first-join UX
- consistent cross-node naming over time

### Recommended Ownership and Propagation Model

Recommended model: **use both**, but with clear ownership boundaries.

#### Who sets the alias?

The authoritative group alias should be set by whoever is allowed to edit group
metadata.

Best default:

- **group admins** can set/update the group alias

Possible future extension:

- a dedicated capability such as `CAN_EDIT_GROUP_PROFILE`

Regular members should not independently set conflicting group aliases in shared
state, because that would turn a single group display name into contested
metadata.

#### How does it propagate if a joiner initially only knows IDs?

This is where the combo approach matters:

1. **Invitation bootstrap**
   - the invitation carries optional `group_alias`
   - the joiner stores and shows it locally immediately on successful join

2. **Authoritative sync/reconciliation**
   - after join, the node syncs group metadata from the shared source of truth
   - that metadata includes the authoritative alias
   - if the invitation alias was stale or missing, sync corrects it

3. **Later updates**
   - if an admin renames the group later, that change propagates through normal
     group sync / group mutation broadcast
   - existing members do not need a new invitation to learn the new alias

So the invitation solves:

- "I only have IDs at join time"

while shared metadata solves:

- "Who owns the current alias?"
- "How do later renames propagate?"
- "How do all members converge on one name?"

#### Recommended conclusion

The recommended design is:

- **source of truth:** shared group metadata
- **who can edit it:** admins by default
- **bootstrap path:** invitation includes `group_alias`
- **steady-state propagation:** sync/gossip from shared metadata

That is better than invitation-only, and also better than metadata-only.

### Proposed Fix

At minimum, core should support:

- optional group alias capture at group creation time
- optional group alias field in invitations
- local persistence of that alias on the joining node

Preferred end state:

- `GroupInfoResponse` / `GroupSummary` expose `alias?: String`
- group creation can persist alias into shared group metadata
- invitations carry `group_alias?: String` as a bootstrap hint
- `join_group` stores the alias locally on success
- sync / propagation reconciles alias changes across nodes

### Files Likely Affected

| File | What changes |
|------|-------------|
| `core/crates/context/primitives/src/group.rs` | Extend group create/info/summary types if alias becomes first-class metadata |
| `core/crates/server/primitives/src/admin.rs` | Extend group API request/response types with optional alias |
| `core/crates/context/src/handlers/create_group.rs` | Accept/store alias when creating a group |
| `core/crates/context/src/handlers/create_group_invitation.rs` | Include optional group alias in invitation payload if using bootstrap path |
| `core/crates/context/src/handlers/join_group.rs` | Persist alias locally on successful join |
| `core/crates/context/src/group_store.rs` | Add local persistence/read helpers for group aliases |
| group contract / external query path | Needed if alias becomes authoritative shared group metadata |

### Priority

**Medium** for invitation bootstrap alone, **Medium-High** for the full
authoritative shared-metadata version.

---

## Priority (overall)

The remaining open issues are DX/correctness issues:

| Issue | Priority | Status |
|-------|----------|--------|
| Group state changes should auto-propagate via P2P | High | Open |
| `group sync` prematurely fetches blobs for unjoined contexts | Medium | Open |
| `get-visibility` returns 500 for newly created contexts | Medium | Open |
| Signed bundle version upgrades without migration silently ignored (#2060) | Medium | Open |
| Group members should have propagated per-group aliases | Medium-High | Open |
| Group IDs should support human-friendly group aliases | Medium-High | Open |
