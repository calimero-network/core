# Context Group Management — Complete Implementation Overview

**Status:** Implemented (state propagation, join-via-group, cascade removal, migration-aware upgrades, group permission system, and JSON invitation format complete)
**Branch:** `feat/context-management-proposal` (both `core/` and `contracts/`)
**Date:** March 2026

---

## 1. Why This Exists

Calimero's original architecture treats every context as a fully independent
entity — its own app, state, members, sync topic. This works for single
contexts but breaks at scale:

- A chat app with 500 DMs + 30 channels = 531 independent contexts
- Upgrading the app version requires 531 separate upgrade operations
- No organizational boundary — no concept of "these contexts belong together"

**Context Groups** solve this by introducing a **workspace entity** that:
- Owns a set of **users** (group members) authorized to create contexts
- Owns a set of **contexts** that share a common application
- Enables **single-trigger version propagation** across all contexts
- Is governed by an **admin identity** who controls upgrades and membership

See `PROPOSAL-hierarchical-context-management.md` for the full design rationale.

---

## 2. Architecture

```
┌─────────────────────────────────────────────────────┐
│  Context Group (Workspace)                           │
│                                                      │
│  group_id:        ContextGroupId (random 32 bytes)   │
│  app_key:         32-byte key identifying the app    │
│  target_app:      ApplicationId (current version)    │
│  upgrade_policy:  Automatic | LazyOnAccess | ...     │
│                                                      │
│  ┌───────────────────────────────────┐               │
│  │ Members (user identities)         │               │
│  │  Admin: ed25519 pubkey            │               │
│  │  Member: ed25519 pubkey           │               │
│  │  Member: ed25519 pubkey           │               │
│  └───────────────────────────────────┘               │
│                                                      │
│  ┌───────────────────────────────────┐               │
│  │ Contexts (version-synced)         │               │
│  │  Context A  (app: target_app)     │               │
│  │  Context B  (app: target_app)     │               │
│  └───────────────────────────────────┘               │
└─────────────────────────────────────────────────────┘
```

**Key design decision:** Group membership ≠ context membership. Being a group
member lets you *create* contexts (if you have `CAN_CREATE_CONTEXT` capability).
Each context has its own member set — a DM has 2 participants, not the entire
group. This preserves privacy. Context visibility (Open vs Restricted with
allowlists) further controls who can join each context.

---

## 3. What's Implemented — Contract Side (`contracts/`)

### 3.1 On-Chain Data Model

File: `contracts/contracts/near/context-config/src/lib.rs`

```rust
struct OnChainGroupMeta {
    app_key: AppKey,
    target_application: Application<'static>,
    admins: IterableSet<SignerId>,
    admin_nonces: IterableMap<SignerId, u64>,
    members: IterableSet<SignerId>,
    approved_registrations: IterableSet<ContextId>,
    context_ids: IterableSet<ContextId>,
    context_count: u64,
    invitation_commitments: IterableMap<CryptoHash, BlockHeight>,
    used_invitations: IterableSet<CryptoHash>,
    member_contexts: IterableMap<(SignerId, ContextId), ContextIdentity>,
    // Permission system (migration 06)
    member_capabilities: IterableMap<SignerId, u32>,
    context_visibility: IterableMap<ContextId, VisibilityInfo>,
    context_allowlists: IterableMap<(ContextId, SignerId), ()>,
    default_member_capabilities: u32,   // default: CAN_JOIN_OPEN_CONTEXTS
    default_context_visibility: VisibilityMode, // default: Open
}
```

Storage maps on `ContextConfigs`:
- `groups: IterableMap<ContextGroupId, OnChainGroupMeta>`
- `context_group_refs: IterableMap<ContextId, ContextGroupId>` (reverse index)

### 3.2 Mutation Methods

File: `contracts/contracts/near/context-config/src/mutate.rs`

| Method | Auth | What it does |
|--------|------|-------------|
| `create_group` | Creator (becomes admin) | Creates group with app_key + target application |
| `delete_group` | Group admin | Deletes group (requires context_count == 0) |
| `add_group_members` | Group admin | Adds identities to `members` set |
| `remove_group_members` | Group admin | Removes from `members` set + cascade-removes from all group contexts via `member_contexts` mapping |
| `register_context_in_group` | Admin or member with `CAN_CREATE_CONTEXT` | Links a context to the group; accepts optional `visibility_mode` (defaults to `default_context_visibility`); auto-adds creator to allowlist when Restricted |
| `unregister_context_from_group` | Group admin | Unlinks a context |
| `set_group_target` | Group admin | Updates `target_application` (triggers upgrade) |
| `approve_context_registration` | Group admin | Pre-approves a context for proxy-based registration |
| `proxy_register_in_group` | Context proxy | Registers context (consumes approval) |
| `proxy_unregister_from_group` | Context proxy | Unregisters context |
| `join_context_via_group` | Group member (capability + visibility gated) | Adds caller to a context within their group; Open contexts require `CAN_JOIN_OPEN_CONTEXTS`; Restricted contexts require allowlist membership; admins bypass all checks (with event logging); stores `(signer, ctx) → identity` mapping for cascade removal |
| `set_member_capabilities` | Group admin | Sets capability bits for a specific member |
| `set_context_visibility` | Context creator or group admin | Sets visibility mode (Open/Restricted) for a context; auto-adds creator to allowlist when switching to Restricted |
| `manage_context_allowlist` | Context creator or group admin | Adds/removes members from a context's allowlist; works on both Open and Restricted contexts (pre-population allowed) |
| `set_default_capabilities` | Group admin | Sets the default capability bits for new members |
| `set_default_visibility` | Group admin | Sets the default visibility mode for new contexts |

All admin mutations use `check_and_increment_group_nonce` for replay protection.
`join_context_via_group`, `commit_group_invitation`, and `reveal_group_invitation` skip the nonce check (caller may be a regular member without an admin nonce).

### 3.3 Invitation Commit/Reveal Flow

File: `contracts/contracts/near/context-config/src/group_invitation.rs`

**Purpose:** MEV-protected invitation flow. Prevents front-running of group join
operations on-chain.

**Step 1 — Commit** (`commit_group_invitation`):
- Joiner submits `SHA256(borsh(GroupRevealPayloadData))` as a commitment
- Stored in `invitation_commitments` with expiration block height
- No nonce required (anyone can call)

**Step 2 — Reveal** (`reveal_group_invitation`):
- Joiner submits the full `SignedGroupRevealPayload` containing:
  - `GroupInvitationFromAdmin`: group_id, inviter_identity, expiration, secret_salt
  - `inviter_signature`: Admin's ed25519 signature over the invitation
  - `invitee_signature`: Joiner's ed25519 signature over the payload
  - `new_member_identity`: The joining identity

- Contract verifies:
  1. Commitment hash matches
  2. Block height ≤ expiration
  3. Both signatures are valid ed25519
  4. Inviter is a group admin **or** has `CAN_INVITE_MEMBERS` capability
  5. Joiner is not already a member
  6. Invitation not previously used (replay protection)

- On success: adds `new_member_identity` to `group.members` with `default_member_capabilities`

### 3.4 Query Methods

File: `contracts/contracts/near/context-config/src/query.rs`

| Method | Returns |
|--------|---------|
| `group(group_id)` | `GroupInfoResponse` — app_key, target_application, member_count, context_count, default_member_capabilities, default_context_visibility |
| `is_group_admin(group_id, identity)` | `bool` |
| `group_contexts(group_id, offset, length)` | `Vec<ContextId>` — paginated |
| `context_group(context_id)` | `Option<ContextGroupId>` |
| `fetch_group_nonce(group_id, admin_id)` | `Option<u64>` |
| `group_members(group_id, offset, length)` | `Vec<GroupMemberEntry>` — paginated, role-tagged (Admin/Member), includes capabilities per member |
| `context_visibility(group_id, context_id)` | `Option<ContextVisibilityResponse>` — mode, creator, allowlist_count |
| `context_allowlist(group_id, context_id, offset, length)` | `Vec<SignerId>` — paginated allowlist |

### 3.5 Migrations

- `03_context_groups.rs` — Adds `groups` and `context_group_refs` maps
- `04_group_invitations.rs` — Adds `invitation_commitments` and `used_invitations`
- `05_group_migration_method.rs` — Adds `migration_method` to `OnChainGroupMeta`
- `06_group_permissions.rs` — Adds `member_capabilities`, `context_visibility`, `context_allowlists`, `default_member_capabilities`, `default_context_visibility`. Existing members get `CAN_JOIN_OPEN_CONTEXTS` (backward compat). Existing groups default to `Open` visibility.

---

## 4. What's Implemented — Node Side (`core/`)

### 4.1 Local Storage

File: `core/crates/context/src/group_store.rs`

| Key Type | Prefix | Content |
|----------|--------|---------|
| `GroupMeta` | 0x20 | app_key, target_application_id, upgrade_policy, admin_identity, migration |
| `GroupMember` | 0x21 | GroupMemberRole (Admin / Member) |
| `GroupContextIndex` | 0x22 | Presence index — context belongs to group |
| `ContextGroupRef` | 0x23 | Reverse index — context → group |
| `GroupUpgradeKey` | 0x24 | GroupUpgradeValue (upgrade state tracking) |
| `GroupSigningKey` | 0x25 | Private signing key for group operations |
| `GroupMemberCapability` | 0x26 | u32 capability bitfield per member |
| `GroupContextVisibility` | 0x27 | (VisibilityMode, creator_pk) per context |
| `GroupContextAllowlist` | 0x28 | Presence index — member allowed for context |
| `GroupDefaultCapabilities` | 0x29 | u32 default capabilities for new members |
| `GroupDefaultVisibility` | 0x2A | VisibilityMode default for new contexts |

Key storage functions:
- CRUD: `save_group_meta`, `load_group_meta`, `delete_group_meta`
- Members: `add_group_member`, `remove_group_member`, `list_group_members`, `check_group_membership`, `is_group_admin`
- Contexts: `register_context_in_group`, `enumerate_group_contexts`
- Sync: `sync_group_state_from_contract` (metadata + contexts + members)
- Upgrades: `save_group_upgrade`, `enumerate_in_progress_upgrades`

### 4.2 ContextManager Handlers

Files: `core/crates/context/src/handlers/`

| Handler | What it does |
|---------|-------------|
| `create_group` | Validates input → calls contract `create_group` → stores locally |
| `delete_group` | Validates admin → calls contract `delete_group` → removes local data |
| `add_group_members` | Admin adds members → contract + local store |
| `remove_group_members` | Admin removes members → contract + local store |
| `create_group_invitation` | Admin creates and signs a `SignedGroupOpenInvitation` (transparent JSON) |
| `join_group` | Takes `SignedGroupOpenInvitation` directly → commit on-chain → reveal on-chain → stores membership locally |
| `list_group_members` | Reads from local store |
| `list_group_contexts` | Reads from local store |
| `get_group_info` | Reads from local store |
| `upgrade_group` | Canary upgrade → background propagation to all contexts |
| `get_group_upgrade_status` | Returns in-progress upgrade state |
| `retry_group_upgrade` | Retries failed context upgrades |
| `sync_group` | Pulls metadata, members, and contexts from contract into local store |
| `join_group_context` | Joins a context via group membership (no invitation needed) |
| `update_group_settings` | Updates upgrade_policy locally |
| `update_member_role` | Changes Admin ↔ Member role |
| `detach_context_from_group` | Unlinks a context from the group |
| `set_member_capabilities` | Admin sets capability bits for a member → contract + local store |
| `get_member_capabilities` | Reads capability bits from local store |
| `set_context_visibility` | Creator/admin sets visibility mode → contract + local store; auto-adds creator to allowlist when Restricted |
| `get_context_visibility` | Reads visibility from local store |
| `manage_context_allowlist` | Creator/admin adds/removes allowlist entries → contract + local store |
| `get_context_allowlist` | Lists allowlist members from local store |
| `set_default_capabilities` | Admin sets default capabilities for new members → contract |
| `set_default_visibility` | Admin sets default visibility for new contexts → contract |

### 4.3 Context Creation with `--group-id`

File: `core/crates/context/src/handlers/create_context.rs`

When `group_id` is provided:

1. Loads group metadata from local store
2. Verifies creator is a group member (`check_group_membership`)
3. Overrides `application_id` with group's `target_application_id` if they differ
4. Creates context normally (state, DAG, identity)
5. Calls `register_context_in_group` on the contract
6. Stores `GroupContextIndex` + `ContextGroupRef` locally

This ensures new contexts are always at the group's current target version.

### 4.4 External Client (Contract Communication)

File: `core/crates/context/primitives/src/client/external/group.rs`

`ExternalGroupClient` wraps contract calls with:
- Automatic nonce management (`fetch_group_nonce` + increment)
- Ed25519 signing of `GroupRequest` payloads
- Borsh serialization

Methods: `create_group`, `delete_group`, `add_group_members`,
`remove_group_members`, `register_context_in_group`,
`unregister_context_from_group`, `set_group_target`,
`commit_group_invitation`, `reveal_group_invitation`,
`join_context_via_group`, `set_member_capabilities`,
`set_context_visibility`, `manage_context_allowlist`,
`set_default_capabilities`, `set_default_visibility`

Read-only queries on `ContextClient`: `query_group_info`,
`query_group_contexts`, `query_group_members`,
`query_context_visibility`, `query_context_allowlist`

### 4.5 Upgrade Propagation

File: `core/crates/context/src/handlers/upgrade_group.rs`

Flow:
1. Admin triggers upgrade with target `ApplicationId` + optional `--migrate-method`
2. **Canary upgrade**: First context is upgraded as validation
3. If canary succeeds: update group's `target_application` on-chain + locally
4. **Background propagation**: Remaining contexts upgraded sequentially
5. Progress tracked in `GroupUpgradeValue` (persisted for crash recovery)
6. On restart: `enumerate_in_progress_upgrades` resumes incomplete upgrades

Upgrade policies:
- `Automatic` — all contexts upgraded immediately
- `LazyOnAccess` — contexts upgraded on next interaction
- `Coordinated { deadline }` — opt-in window with forced deadline

> **Peer propagation:** Upgrade propagation now works across nodes.
> Migration method is stored on-chain (`OnChainGroupMeta.migration_method`).
> During `group sync`, peer nodes fetch the target app blob via P2P and
> install it locally. `maybe_lazy_upgrade` then triggers on next context
> access. `join_group_context` stores the `ContextGroupRef` immediately.

---

## 5. HTTP API Endpoints

File: `core/crates/server/src/admin/service.rs`

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/admin-api/groups` | List all groups |
| POST | `/admin-api/groups` | Create group |
| GET | `/admin-api/groups/:id` | Get group info |
| PATCH | `/admin-api/groups/:id` | Update group settings |
| DELETE | `/admin-api/groups/:id` | Delete group |
| GET | `/admin-api/groups/:id/members` | List members |
| POST | `/admin-api/groups/:id/members` | Add members |
| POST | `/admin-api/groups/:id/members/remove` | Remove members |
| PUT | `/admin-api/groups/:id/members/:identity/role` | Set member role |
| GET | `/admin-api/groups/:id/contexts` | List group contexts |
| POST | `/admin-api/groups/:id/contexts/:ctx/remove` | Detach context |
| POST | `/admin-api/groups/:id/upgrade` | Trigger upgrade |
| GET | `/admin-api/groups/:id/upgrade/status` | Upgrade status |
| POST | `/admin-api/groups/:id/upgrade/retry` | Retry failed upgrades |
| POST | `/admin-api/groups/:id/signing-key` | Register signing key |
| POST | `/admin-api/groups/:id/sync` | Sync from contract |
| POST | `/admin-api/groups/:id/invite` | Create invitation |
| POST | `/admin-api/groups/join` | Join via invitation |
| POST | `/admin-api/groups/:id/join-context` | Join a context via group membership |
| GET | `/admin-api/contexts/:ctx/group` | Get context's group |
| GET | `/admin-api/groups/:id/members/:identity/capabilities` | Get member capabilities |
| PUT | `/admin-api/groups/:id/members/:identity/capabilities` | Set member capabilities |
| PUT | `/admin-api/groups/:id/settings/default-capabilities` | Set default capabilities for new members |
| PUT | `/admin-api/groups/:id/settings/default-visibility` | Set default visibility for new contexts |
| GET | `/admin-api/groups/:id/contexts/:ctx/visibility` | Get context visibility mode |
| PUT | `/admin-api/groups/:id/contexts/:ctx/visibility` | Set context visibility mode |
| GET | `/admin-api/groups/:id/contexts/:ctx/allowlist` | Get context allowlist |
| POST | `/admin-api/groups/:id/contexts/:ctx/allowlist` | Manage context allowlist (add/remove) |

---

## 6. CLI Commands (`meroctl`)

Files: `core/crates/meroctl/src/cli/group/`

Identity/key flags marked `[optional]` are auto-resolved server-side from
the node's **dedicated group identity** (`[identity.group]` in `config.toml`)
— a keypair generated at `merod init` time, completely decoupled from the
NEAR signer key. Use `meroctl node identity` (or `meroctl node id`) to view
a node's group public key.

> **Identity change:** `node_near_identity()` was removed and replaced by
> `node_group_identity()`, which reads from `[identity.group]` instead of
> the NEAR signer config.

### Group CRUD

```bash
meroctl --node <N> group list
meroctl --node <N> group create --application-id <APP>
  # [optional] --app-key <HEX>         (auto-generated if omitted)
  # [optional] --admin-identity <PK>   (defaults to node group identity)
meroctl --node <N> group get <GROUP_ID>
meroctl --node <N> group update <GROUP_ID> --upgrade-policy <POLICY>
  # [optional] --requester <PK>        (defaults to node group identity)
meroctl --node <N> group delete <GROUP_ID>
  # [optional] --requester <PK>        (defaults to node group identity)
```

### Membership

```bash
meroctl --node <N> group members list <GROUP_ID>
meroctl --node <N> group members add <GROUP_ID> --identity <PK>
  # [optional] --requester <PK>        (defaults to node group identity)
meroctl --node <N> group members remove <GROUP_ID> --identities <PK>
  # [optional] --requester <PK>        (defaults to node group identity)
meroctl --node <N> group members set-role <GROUP_ID> --identity <PK> --role <ROLE>
  # [optional] --requester <PK>        (defaults to node group identity)
meroctl --node <N> group members set-capabilities <GROUP_ID> <MEMBER_PK>
  [--can-create-context] [--can-invite-members] [--can-join-open-contexts]
  # [optional] --requester <PK>
meroctl --node <N> group members get-capabilities <GROUP_ID> <MEMBER_PK>
```

### Invitation Flow

```bash
# Admin creates invitation (outputs SignedGroupOpenInvitation as JSON)
meroctl --node <N> group invite <GROUP_ID>
  # [optional] --requester <PK>              (defaults to node group identity)
  # [optional] --expiration-block-height N   (defaults to 999_999_999)
# → prints pretty JSON + ready-to-use join command

# Joiner pastes the JSON
meroctl --node <N> group join '<INVITATION_JSON>'
```

> **Format change:** Invitations are now `SignedGroupOpenInvitation` JSON
> (transparent, inspectable) instead of an opaque Base58 blob. The JSON is
> what the on-chain contract already expects — no encode/decode roundtrip.
> `--invitee-identity` and `--expiration` flags have been removed; invitations
> are always open (anyone with the JSON can join).

### Contexts in Group

```bash
meroctl --node <N> group contexts list <GROUP_ID>
meroctl --node <N> group contexts detach <GROUP_ID> <CONTEXT_ID>
  # [optional] --requester <PK>        (defaults to node group identity)
meroctl --node <N> group contexts set-visibility <GROUP_ID> <CONTEXT_ID> --mode <open|restricted>
  # [optional] --requester <PK>
meroctl --node <N> group contexts get-visibility <GROUP_ID> <CONTEXT_ID>
meroctl --node <N> group contexts allowlist list <GROUP_ID> <CONTEXT_ID>
meroctl --node <N> group contexts allowlist add <GROUP_ID> <CONTEXT_ID> <MEMBER_PK>...
  # [optional] --requester <PK>
meroctl --node <N> group contexts allowlist remove <GROUP_ID> <CONTEXT_ID> <MEMBER_PK>...
  # [optional] --requester <PK>
```

### Group Settings

```bash
meroctl --node <N> group settings set-default-capabilities <GROUP_ID>
  [--can-create-context] [--can-invite-members] [--can-join-open-contexts]
  # [optional] --requester <PK>
meroctl --node <N> group settings set-default-visibility <GROUP_ID> --mode <open|restricted>
  # [optional] --requester <PK>
```

### Upgrades

```bash
meroctl --node <N> group upgrade trigger <GROUP_ID> --target-application-id <APP>
  # [optional] --requester <PK>        (defaults to node group identity)
meroctl --node <N> group upgrade status <GROUP_ID>
meroctl --node <N> group upgrade retry <GROUP_ID>
  # [optional] --requester <PK>        (defaults to node group identity)
```

### Join Context via Group

```bash
meroctl --node <N> group join-group-context <GROUP_ID> --context-id <CTX>
  # [optional] --joiner-identity <PK>  (defaults to node group identity)
```

### Sync & Signing Keys

```bash
meroctl --node <N> group sync <GROUP_ID>
  # [optional] --requester <PK>        (defaults to node group identity)
meroctl --node <N> group signing-key register <GROUP_ID> <HEX_KEY>
```

### Context Deletion (Group-Aware)

```bash
meroctl --node <N> context delete <CONTEXT>
  # [optional] --requester <PK>        (required for group contexts; must be group admin)
```

### Context Creation with Group

```bash
meroctl --node <N> context create --protocol near --application-id <APP> \
  --group-id <GROUP_ID>
```

---

## 7. Complete Flow — Tested End-to-End

Tested with two local `merod` nodes (node-a, node-b) against
`ctx-groups-ronit.testnet` on NEAR testnet.

### Flow 1: Group Lifecycle

```
Admin (Node A)                          Contract (NEAR testnet)
     │                                       │
     │  group create                         │
     │  (app_key, application_id, admin_pk)  │
     │──────────────────────────────────────►│  create_group()
     │◄──────────────────────────────────────│  ✓ group_id
     │                                       │
     │  group invite                         │
     │  (group_id, admin_pk)                 │
     │──► signs GroupInvitationFromAdmin     │
     │    returns SignedGroupOpenInvitation  │
     │    (JSON, shareable)                  │
     │                                       │
```

### Flow 2: Member Joins via Invitation

```
Admin (Node A)              Joiner (Node B)           Contract
     │                           │                       │
     │  SignedGroupOpenInvitation│                       │
     │  (JSON) ─────────────────►│                       │
     │                           │                       │
     │                           │  commit_group_invitation
     │                           │──────────────────────►│  store commitment
     │                           │◄──────────────────────│  ✓
     │                           │                       │
     │                           │  reveal_group_invitation
     │                           │──────────────────────►│  verify sigs
     │                           │◄──────────────────────│  ✓ add member
     │                           │                       │
     │  ✓ Node A runs group sync │  ✓ Node B sees both  │
     │    → sees the new member  │    members locally    │
```

### Flow 3: Context Creation Inside Group

```
Admin (Node A)                          Contract
     │                                       │
     │  context create                       │
     │  (app_id, group_id, identity_secret)  │
     │                                       │
     │  1. check_group_membership ──────┐    │
     │  2. override app with target ────┘    │
     │  3. create context normally           │
     │  4. register_context_in_group ──────►│
     │◄────────────────────────────────────│  ✓
     │                                       │
     │  ✓ Node B runs group sync → sees      │
     │    the context; can join via group    │
```

### Flow 4: Getting Node B Into a Group Context

**Option A — Join via group membership (new, preferred):**

```
Node B (group member)                      Contract
  │                                            │
  │  group join-group-context                  │
  │  (group_id, context_id)                    │
  │────────────────────────────────────────►   │  join_context_via_group
  │                                            │  verify group membership
  │                                            │  add member to context
  │◄────────────────────────────────────────   │  ✓
  │                                            │
  │  ✓ context identity generated locally      │
  │  ✓ context config synced                   │
  │  ✓ subscribed + state synced via P2P       │
  │  ✓ app auto-installed                      │
```

**Option B — Full context invitation (still works, required for non-group-members):**

```
Node B                      Node A                   Contract
  │                           │                        │
  │  context identity generate│                        │
  │──► new pubkey             │                        │
  │  (send pubkey to admin)  ─►│                       │
  │                           │  context invite        │
  │                           │───────────────────────►│  add_members
  │◄── invitation payload ────│                        │
  │  context join (payload)   │                        │
  │───────────────────────────┼───────────────────────►│  verify
  │  ✓ state synced via P2P ◄─┤                        │
```

### Flow 5: Admin-Push Member Add/Remove

```
Admin (Node A)                          Contract
     │                                       │
     │  group members add                    │
     │  (group_id, joiner_pk, admin_pk)      │
     │──────────────────────────────────────►│  add_group_members
     │◄──────────────────────────────────────│  ✓
     │                                       │
     │  ✓ Node A sees both members           │
     │  ✓ Node B runs group sync            │
     │    → sees the change                  │
```

### What Was Verified Working

| Feature | Status |
|---------|--------|
| Group create with app + admin identity | ✓ |
| Group get / list | ✓ |
| Group invite (generates `SignedGroupOpenInvitation` JSON) | ✓ |
| Group join via commit/reveal (JSON invitation) | ✓ |
| Group members list (on initiating node) | ✓ |
| Admin-push members add/remove | ✓ |
| Context create inside group (version override) | ✓ |
| Context list in group (on creating node) | ✓ |
| Context invite + join (separate from group) | ✓ |
| App auto-install via P2P blob sharing | ✓ |
| Context state sync between nodes | ✓ |
| Group sync from contract (metadata) | ✓ |
| Group sync from contract (members) | ✓ |
| Group sync from contract (contexts) | ✓ |
| Join context via group membership (no invitation) | ✓ |
| Group members visible on other nodes after sync | ✓ |
| Group contexts visible on other nodes after sync | ✓ |
| Cascade removal: group member removed from all group contexts | ✓ |
| Cascade removal: removed member gets `Unauthorized` on context access | ✓ |
| Cascade removal: context identity list pruned after removal | ✓ |
| Group upgrade with same ApplicationId + migration (admin node) | ✓ |
| Migration-suite v1→v2 schema migration (`migrate_v1_to_v2`) | ✓ |
| `meroctl call schema_info` confirms v2 schema with new `notes` field | ✓ |
| Migration preserves existing data (`description`, `counter`) | ✓ |

### What Was Verified Broken (then fixed)

| Feature | Bug | Fix |
|---------|-----|-----|
| Group upgrade propagation to peer nodes | Peer stayed on v1 — migration method not propagated, app binary not fetched | Fixed: migration on-chain, blob fetch during sync |
| `join_group_context` local group mapping | `ContextGroupRef` not stored until next `group sync` | Fixed: `register_context_in_group` added to handler |

### Still Open

*All previously open issues resolved by the permission system.*

### Previously Known Gaps — Now Resolved

| Gap | Resolution |
|-----|-----------|
| Group members not visible on other nodes | ✓ Fixed — added `group_members()` paginated contract view + `sync_group_members_from_contract` with upsert/prune |
| Group contexts not visible on other nodes | ✓ Fixed — wired `group_contexts()` query into `sync_group_contexts_from_contract` with upsert/prune |
| `group sync` doesn't fix member visibility | ✓ Fixed — `sync_group_state_from_contract` now syncs metadata + contexts + members |
| Group members must do full context invitation to join group contexts | ✓ Fixed — new `join_context_via_group` contract mutation + `join-group-context` CLI command |
| Group member removal doesn't cascade to contexts | ✓ Fixed and verified — `member_contexts` mapping in contract, `authorized_get_mut()` on `Guard<T>`, cascade logic in `remove_group_members`, node-side context sync after removal. Tested: removed member gets `Unauthorized` on context access, identity list shows only remaining members. |
| Group upgrade rejects same-ApplicationId version upgrades | ✓ Fixed — migration-aware gates in `validate_upgrade`, propagator, and `maybe_lazy_upgrade` (see [#2060](https://github.com/calimero-network/core/issues/2060) for the remaining signed-bundle-without-migration gap) |
| Lazy upgrade not propagated to peer nodes | ✓ Fixed — migration method stored on-chain (`OnChainGroupMeta.migration_method`) with contract migration 05; `sync_group.rs` fetches target app blob via P2P and installs bundle; `join_group_context` now calls `register_context_in_group` |

### Remaining Open Gaps

| Gap | Impact |
|-----|--------|
| API errors show generic 500 | Medium — actual error message only in server logs, CLI shows `500 Internal Server Error` |

---

## 8. Code Map

### Contracts (`contracts/`)

| File | Role |
|------|------|
| `contracts/near/context-config/src/lib.rs` | `OnChainGroupMeta`, storage layout, prefixes |
| `contracts/near/context-config/src/mutate.rs` | All group mutations (`handle_group_request`) |
| `contracts/near/context-config/src/query.rs` | Group queries (`group`, `group_contexts`, etc.) |
| `contracts/near/context-config/src/group_invitation.rs` | Commit/reveal invitation flow |
| `contracts/near/context-config/src/guard.rs` | `Guard<T>` access control with `authorized_get_mut()` for group-authorized operations |
| `contracts/near/context-config/src/sys/migrations/06_group_permissions.rs` | Migration: capabilities, visibility, allowlists |
| `contracts/near/context-config/tests/groups.rs` | Contract integration tests (34 tests including 13 permission tests) |

### Core — Storage & Handlers (`core/`)

| File | Role |
|------|------|
| `crates/context/src/group_store.rs` | All local storage operations for groups |
| `crates/store/src/key/group.rs` | Storage key type definitions |
| `crates/context/src/handlers/create_group.rs` | Group creation handler |
| `crates/context/src/handlers/join_group.rs` | Invitation join (commit/reveal) |
| `crates/context/src/handlers/create_group_invitation.rs` | Invitation creation |
| `crates/context/src/handlers/add_group_members.rs` | Admin-push add members |
| `crates/context/src/handlers/remove_group_members.rs` | Admin-push remove members |
| `crates/context/src/handlers/delete_group.rs` | Group deletion |
| `crates/context/src/handlers/upgrade_group.rs` | Version propagation |
| `crates/context/src/handlers/sync_group.rs` | Contract sync (metadata + members + contexts) |
| `crates/context/src/handlers/join_group_context.rs` | Join context via group membership |
| `crates/context/src/handlers/create_context.rs` | Context creation (group-aware) |

### Core — API & CLI

| File | Role |
|------|------|
| `crates/server/src/admin/handlers/groups/` | HTTP endpoint handlers |
| `crates/server/src/admin/service.rs` | Route registration |
| `crates/server/primitives/src/admin.rs` | Request/response types |
| `crates/meroctl/src/cli/group/` | CLI commands |
| `crates/context/primitives/src/client/external/group.rs` | Contract client |
| `crates/context/primitives/src/group.rs` | Message types (including 8 permission-related request/response types); `CreateGroupInvitationResponse` and `JoinGroupRequest` use `SignedGroupOpenInvitation` directly |
| `crates/context/src/handlers/set_member_capabilities.rs` | Set member capability bits |
| `crates/context/src/handlers/get_member_capabilities.rs` | Get member capability bits |
| `crates/context/src/handlers/set_context_visibility.rs` | Set context visibility mode |
| `crates/context/src/handlers/get_context_visibility.rs` | Get context visibility |
| `crates/context/src/handlers/manage_context_allowlist.rs` | Add/remove allowlist entries |
| `crates/context/src/handlers/get_context_allowlist.rs` | List allowlist members |
| `crates/context/src/handlers/set_default_capabilities.rs` | Set default member capabilities |
| `crates/context/src/handlers/set_default_visibility.rs` | Set default context visibility |
| `crates/meroctl/src/cli/group/settings.rs` | CLI: group settings (default caps/visibility) |

---

## 9. Relationship to Proposal

The implementation covers **Phases 1–4** of the proposal's migration path:

| Phase | Proposal | Status |
|-------|----------|--------|
| 1 — Foundation | Contract storage, core storage keys | ✓ Complete |
| 2 — Group CRUD + Membership | Create, delete, add/remove members, invitation flow | ✓ Complete |
| 3 — Context-Group Integration | group_id in context create, version override, registration | ✓ Complete |
| 4 — Upgrade Propagation | Canary upgrade, background propagation, status tracking, retry | ✓ Complete |
| 5 — Advanced Policies | Coordinated policy, LazyOnAccess interceptor, crash recovery, permissions | Partial (crash recovery + permission system done) |
| 5a — Permission System | Member capabilities (3-bit bitfield), context visibility (Open/Restricted), allowlists, group defaults | ✓ Complete |
| 6 — SDK + Application Integration | SDK helpers, templates, documentation | Not started |

**Cross-cutting:** Group state synchronization (members, contexts) across nodes
is now implemented via `group sync`. Nodes must explicitly run sync to pull
the latest state from the contract. Automatic push-based propagation (e.g.,
P2P gossip of group state changes) is not yet implemented.

---

## 10. Identity Model and Guard Authorization

### Dual Identity System

Calimero uses two distinct identity types that serve different purposes:

| Identity | Type | Scope | Created when | Stored where |
|----------|------|-------|-------------|-------------|
| `SignerId` | ed25519 pubkey | Group-level | `merod init` (config.toml `[identity.group]`) | Contract `group.members` / `group.admins` |
| `ContextIdentity` | ed25519 pubkey | Context-level | Each `join_context_via_group` or context invite | Contract `context.members`, node local store |

A group member has **one `SignerId`** but potentially **many `ContextIdentity` keys**
-- one per context they've joined. These are random keypairs generated at join time
and are unrelated to the group identity. This separation ensures:

- Context-level operations (state mutations, execution) use per-context keys
- Group-level operations (admin actions, membership) use the group identity
- Compromising one context key doesn't compromise the group or other contexts

**The mapping problem (solved):** There was originally no way to go from a
`SignerId` to the `ContextIdentity` keys it generated. The `member_contexts`
mapping in `OnChainGroupMeta` (`(SignerId, ContextId) → ContextIdentity`)
solves this. Populated during `join_context_via_group`, consumed during
cascade removal in `remove_group_members`. See Section 11 for the full flow.

### Guard<T> -- Access Control on Context Data

File: `contracts/contracts/near/context-config/src/guard.rs`

The `Guard<T>` wrapper controls mutable access to sensitive context data (member
lists, application settings). It maintains:

- A set of **privileged signers** (`IterableSet<SignerId>`) who can unlock it
- A **revision counter** that auto-increments on every mutable access (via `GuardMut::Drop`)
- The **inner value** (`T`) accessible only through the guard

```
Guard<IterableSet<ContextIdentity>>  (context.members)
├── priviledged: {creator_identity, admin_identity, ...}
├── revision: 42
└── inner: {member_a, member_b, member_c}
```

The revision counter is critical for synchronization -- nodes compare their local
`application_revision` against the contract's to detect when application data has
changed (see `sync_context_config` in `context/primitives/src/client/sync.rs`).

### The Privileged Signer Bypass (replaced)

`join_context_via_group` (line 848-862 in `mutate.rs`) needs to modify a context's
member list. But the caller is a group member, not a context privileged signer. The
current workaround:

```rust
// Pick ANY privileged signer on the context -- doesn't matter who
let privileged_signer = context.members.priviledged().iter().next().copied()
    .expect("context has no privileged member");
// Use their authority to unlock the guard
let mut ctx_members = context.members.get(&privileged_signer)
    .expect("privileged signer lost access").get_mut();
let _ignored = ctx_members.insert(*new_member);
```

**Why this exists:** The `Guard::get(signer_id)` method checks if `signer_id` is in
the privileged set. Group operations are authorized by group membership (verified
earlier in the function), not by context-level privileges. Since there's no way to
bypass the guard without a privileged signer, the code borrows an arbitrary one.

**Why this was replaced:**

1. **Conceptually wrong** -- using someone else's credentials for authorization
2. **Fragile** -- panics if the context has no privileged signer (edge case)
3. **Misleading** -- the authorization is group membership, not the borrowed signer
4. **Audit risk** -- looks like a privilege escalation to anyone reviewing the code

### `authorized_get_mut()` on Guard<T> (implemented)

A method on `Guard<T>` that provides direct mutable access for contract-internal
operations where authorization has already been verified at a higher level:

```rust
pub fn authorized_get_mut(&mut self) -> &mut T {
    self.revision = self.revision.wrapping_add(1);
    &mut self.inner
}
```

- Increments revision (maintains sync consistency)
- No signer check (authorization is the caller's responsibility)
- No failure mode (no dependency on privileged signers existing)
- Used by `join_context_via_group` and cascade removal in `remove_group_members`

This is safe because it's only called from within the contract's own mutation methods,
where group admin/membership checks have already been performed. External callers
interact through `Guard::get(signer_id)` which enforces privilege checks.

---

## 11. Complete Context Membership Lifecycle

This section describes the full lifecycle of how members join and leave contexts
through groups, including the cascade removal.

### Joining a Context via Group

```
Node B (group member)              Contract                          Node B (local)
  │                                   │                                  │
  │  join-group-context               │                                  │
  │  (group_id, context_id)           │                                  │
  │                                   │                                  │
  │  1. generate random keypair ──────┤                                  │
  │     (new ContextIdentity)         │                                  │
  │                                   │                                  │
  │  2. call join_context_via_group ─►│                                  │
  │                                   │  verify group membership         │
  │                                   │  verify context in group         │
  │                                   │  add ContextIdentity to          │
  │                                   │    context.members               │
  │                                   │  store mapping:                  │
  │                                   │    (SignerId, ctx) → CtxIdentity │
  │                                   │  revision++ (sync signal)        │
  │                                   │                                  │
  │  3. store identity locally ───────┤──────────────────────────────────►│
  │  4. sync context config ──────────┤──────────────────────────────────►│
  │  5. subscribe to P2P topic ───────┤──────────────────────────────────►│
  │  6. auto-install application ─────┤──────────────────────────────────►│
  │                                   │                                  │
  │  ✓ member is now active           │                                  │
```

### Removing a Member from Group (with Cascade)

When an admin removes a member from a group, the removal cascades to all contexts
the member joined through that group:

```
Admin (Node A)                     Contract                          Peers
  │                                   │                                │
  │  group members remove             │                                │
  │  (group_id, member_pk)            │                                │
  │                                   │                                │
  │  ──► remove_group_members ───────►│                                │
  │                                   │  Phase 1: Group removal        │
  │                                   │  ├─ verify admin               │
  │                                   │  ├─ remove from group.members  │
  │                                   │  └─ collect all mappings:      │
  │                                   │     (member, ctx) → identity   │
  │                                   │                                │
  │                                   │  Phase 2: Context cascade      │
  │                                   │  for each (ctx_id, ctx_identity):
  │                                   │  ├─ authorized_get_mut() on    │
  │                                   │  │  context.members            │
  │                                   │  ├─ remove ctx_identity        │
  │                                   │  ├─ remove nonce               │
  │                                   │  ├─ revoke member privileges   │
  │                                   │  └─ revoke app privileges      │
  │                                   │                                │
  │  ◄── OK ──────────────────────────│                                │
  │                                   │                                │
  │  sync_context_config for each ────┤                                │
  │  context (prunes local members)   │                                │
  │                                   │                                │
  │  broadcast MembersRemoved ────────┤───────────────────────────────►│
  │                                   │                                │  sync_group
  │                                   │                                │  sync_context_config
  │                                   │                                │  (prunes members)
```

### The `member_contexts` Mapping

The cascade is enabled by a new mapping stored in `OnChainGroupMeta`:

```rust
pub member_contexts: IterableMap<(SignerId, ContextId), ContextIdentity>,
```

- **Populated** during `join_context_via_group` -- after adding the member to the
  context, the mapping `(signer_id, context_id) → context_identity` is stored
- **Consumed** during `remove_group_members` -- for each removed member, look up
  all their context identities and remove them from the respective contexts
- **Cleaned up** during `delete_group` (clear all) and `unregister_context_from_group`
  (remove entries for that context)
- **Scoped** -- only tracks group-authorized joins. Members who joined a context
  via regular invitation (not through the group) are unaffected by group removal

### What Removal Does NOT Do

- Does not remove members who joined the context through a separate invitation flow
- Does not delete context state or history -- the removed member's past contributions
  remain in the DAG
- Does not affect the member's other group memberships or contexts outside this group

---

## 12. Group Permission System

### Capabilities (v1, 3 bits)

Member capabilities are stored as a `u32` bitfield on-chain per member:

| Bit | Constant | Meaning |
|-----|----------|---------|
| 0 | `CAN_CREATE_CONTEXT` | Can call `register_context_in_group` |
| 1 | `CAN_INVITE_MEMBERS` | Can create group invitations (checked at reveal) |
| 2 | `CAN_JOIN_OPEN_CONTEXTS` | Can join Open-visibility contexts via group |
| 3–31 | Reserved | For future use |

Admins bypass all capability checks. The `default_member_capabilities` field (default: `CAN_JOIN_OPEN_CONTEXTS`) controls what new members receive when added via `add_group_members` or `reveal_group_invitation`. Admins can set this to `0` for lockdown mode.

### Context Visibility

Each context registered in a group has a visibility mode:

| Mode | Behavior |
|------|----------|
| `Open` | Any group member with `CAN_JOIN_OPEN_CONTEXTS` can join |
| `Restricted` | Only members on the context's allowlist can join |

- **Default**: `Open` (configurable via `set_default_visibility`)
- **Creator auto-add**: When registering a Restricted context, the creator is automatically added to the allowlist (prevents lockout)
- **Allowlist pre-population**: Allowlists can be managed on both Open and Restricted contexts (pre-populate before switching to Restricted)

### Admin Override Logging

When an admin force-joins a Restricted context they're not on the allowlist for, a structured NEAR event is emitted:

```rust
#[event(standard = "calimero_groups", version = "1.0.0")]
pub struct AdminContextJoinEvent {
    pub group_id: String,
    pub context_id: String,
    pub admin: String,
}
```

### Authorization Matrix

| Operation | Admin | Member with capability | Member without capability |
|-----------|-------|----------------------|--------------------------|
| Create context | ✓ | ✓ (`CAN_CREATE_CONTEXT`) | ✗ |
| Join Open context | ✓ | ✓ (`CAN_JOIN_OPEN_CONTEXTS`) | ✗ |
| Join Restricted context | ✓ (with event) | ✓ (if on allowlist) | ✗ |
| Create invitation | ✓ | ✓ (`CAN_INVITE_MEMBERS`) | ✗ |
| Set capabilities | ✓ | ✗ | ✗ |
| Set visibility | ✓ or creator | ✗ | ✗ |
| Manage allowlist | ✓ or creator | ✗ | ✗ |
| Set defaults | ✓ | ✗ | ✗ |

### Files Changed

| Layer | Files |
|-------|-------|
| Contract migration | `contracts/.../sys/migrations/06_group_permissions.rs` |
| Contract types | `contracts/.../context-config/src/lib.rs` (VisibilityMode, VisibilityInfo, MemberCapabilities, AdminContextJoinEvent) |
| Contract mutations | `contracts/.../context-config/src/mutate.rs` (5 new methods, 3 modified methods) |
| Contract queries | `contracts/.../context-config/src/query.rs` (2 new views, 2 updated responses) |
| Contract invitation | `contracts/.../context-config/src/group_invitation.rs` (CAN_INVITE_MEMBERS check at reveal, default_member_capabilities for new members) |
| SDK types | `core/crates/context/config/src/lib.rs` (GroupRequestKind variants, VisibilityMode, MemberCapabilities) |
| SDK mutations | `core/crates/context/config/src/client/env/config/mutate.rs` (5 new builder methods) |
| External client | `core/crates/context/primitives/src/client/external/group.rs` (5 new mutation + 2 new query methods) |
| Local store | `core/crates/context/src/group_store.rs` (5 new key types, CRUD helpers, sync extensions) |
| Node handlers | `core/crates/context/src/handlers/{set,get}_member_capabilities.rs`, `{set,get}_context_visibility.rs`, `manage_context_allowlist.rs`, `get_context_allowlist.rs`, `set_default_{capabilities,visibility}.rs` |
| Actix messages | `core/crates/context/primitives/src/{group,messages,client}.rs` (8 new message types + dispatch) |
| HTTP API | `core/crates/server/src/admin/handlers/groups/{capabilities,visibility,allowlist,default_capabilities,default_visibility}.rs` |
| API types | `core/crates/server/primitives/src/admin.rs` (16 new request/response types) |
| HTTP client | `core/crates/client/src/client.rs` (8 new methods) |
| CLI | `core/crates/meroctl/src/cli/group/{members,contexts,settings}.rs` |
| CLI output | `core/crates/meroctl/src/output/groups.rs` (8 new Report impls) |
| Contract tests | `contracts/.../tests/groups.rs` (13 new tests) |
| Store tests | `core/crates/context/src/group_store.rs` (12 new tests) |

---

## 13. Design Constraints and Trade-offs

### One Group = One Application

A group is tied to a single `target_application` (`ApplicationId`). All contexts
within the group run the same application. This was a deliberate choice:

**Why:** The primary purpose of groups is **version upgrade propagation**. When an
admin triggers `group upgrade`, the new application version propagates to all
contexts in the group. If contexts ran different applications, upgrade propagation
would be meaningless -- you can't upgrade a chat app and a file storage app with
the same binary.

**What this means in practice:**

```
Group "TeamChat" (app: chat-v2.3)
├── Context: #general     (app: chat-v2.3)
├── Context: #engineering (app: chat-v2.3)
├── Context: DM-alice-bob (app: chat-v2.3)
└── Context: #random      (app: chat-v2.3)
```

All contexts run the same chat application. Upgrading to v2.4 updates all of them.

**For multi-app scenarios** (e.g., a team workspace with chat + file storage + task
management), use multiple groups:

```
Workspace "Acme Corp" (logical, not a system entity)
├── Group "AcmeChat"   (app: chat-v2.3)       → chat contexts
├── Group "AcmeFiles"  (app: filestorage-v1.0) → file contexts
└── Group "AcmeTasks"  (app: taskboard-v3.1)   → task contexts
```

Each group manages its own application lifecycle independently. A higher-level
"workspace" abstraction could be built on top if needed.

### Pull-Based Sync (Not Push)

Group state synchronization is pull-based -- nodes must run `group sync` to fetch
the latest state from the contract. Real-time push via P2P gossip is not yet
implemented. This means:

- After Node B joins a group, Node A doesn't see the new member until it syncs
- After creating a context in a group on Node A, Node B doesn't see it until it syncs
- After removing a member, the removal propagates when peers sync

This is acceptable for the current scale but will need P2P gossip for production
use with many nodes.

---

## 14. Future: Cloud TEE Node Management

### Vision

Cloud-based Trusted Execution Environment (TEE) nodes that can be provisioned to
handle workloads across multiple contexts. A TEE node is a specialized Calimero node
running in a secure enclave (e.g., AWS Nitro, Intel SGX) that provides hardware-level
guarantees about code execution integrity.

**Use case:** A platform operator deploys TEE nodes that need access to many contexts
running different applications -- chat contexts, file storage contexts, task boards.
The operator needs to:

1. Provision a TEE node with access to many contexts at once (not one-by-one)
2. Deprovision cleanly -- remove access to all contexts atomically
3. Scale by adding more TEE replicas
4. Verify node integrity via hardware attestation

### What the Current System Provides

The group architecture is the foundation for TEE node management:

| Capability | Current status | Why it matters for TEE |
|-----------|---------------|----------------------|
| **Group membership** | Done | TEE node joins a group = gets authorization scope |
| **Context-via-group join** | Done | TEE node joins contexts through group membership |
| **Cascade removal** | Done | Remove TEE node from group = revoke all context access atomically |
| **`authorized_get_mut()`** | Done | Batch context operations without per-context privilege dependencies |
| **Upgrade propagation** | Done | Push new app versions to all contexts a TEE node participates in |
| **Pull-based sync** | Done | TEE node syncs group state on startup |

The cascade removal work is particularly critical -- it's the "deprovisioning"
primitive. Without it, removing a TEE node from a group would leave stale context
memberships that the node could still use.

### What Needs to Be Built

**Batch context join** (`join_all_group_contexts`): A single contract call that
adds a node to every context in a group. Natural extension of `join_context_via_group`
looped over `group.context_ids`. Without this, provisioning a TEE node to a group
with 100 contexts requires 100 separate transactions.

**Auto-join policy**: A per-group setting that automatically adds new members to all
existing contexts:

```rust
pub enum MembershipPolicy {
    ExplicitJoin,   // Current behavior -- member must join each context
    AutoJoinAll,    // New contexts auto-include all group members
}
```

With `AutoJoinAll`, adding a TEE node to a group immediately grants access to all
contexts. New contexts created in the group automatically include the TEE node.

**Shared identity mode**: For TEE node fleets (multiple replicas of the same node),
the current model generates a unique `ContextIdentity` per node per context. A shared
identity mode would allow replicas to share context keys:

```
TEE Fleet "compute-pool"
├── Replica 1 ─┐
├── Replica 2 ──┼── shared ContextIdentity per context
└── Replica 3 ─┘
```

This is not needed for single-node deployments but becomes important when scaling
TEE processing horizontally.

**TEE attestation**: Hardware attestation as an additional authorization factor.
The contract could verify attestation reports alongside group membership:

```
Authorization check:
  1. Is the caller a group member? (existing)
  2. Does the caller have a valid TEE attestation? (future)
```

This layers on top of the current group model -- it doesn't replace it. The
`authorized_get_mut()` pattern and cascade removal work identically regardless
of whether TEE attestation is added.

### Why Current Design Decisions Enable This

| Decision | How it helps TEE management |
|----------|---------------------------|
| Separate `SignerId` / `ContextIdentity` | TEE node's group identity stays stable; per-context keys can be rotated |
| `member_contexts` mapping | Enables atomic deprovisioning (cascade removal) |
| `authorized_get_mut()` | Batch operations on many contexts don't depend on per-context privileged signers |
| One group = one app | Upgrade propagation ensures all contexts a TEE node runs are at the same version |
| Multiple groups per workspace | TEE node can join multiple groups for multi-app access |

The current work (cascade removal, `authorized_get_mut()`) builds the primitives.
TEE-specific features (batch join, auto-join, attestation) are clean extensions
that don't require rearchitecting the group model.

---

