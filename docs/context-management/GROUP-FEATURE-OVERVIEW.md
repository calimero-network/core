# Context Group Management

Context Groups let you organize multiple contexts under a single workspace, manage
shared membership, and propagate application upgrades across all contexts at once.

**If you're looking for:**
- [What groups are and why they exist](#what-are-context-groups)
- [Creating and managing groups (CLI)](#cli-reference)
- [HTTP API reference](#http-api)
- [Permission system](#permissions)
- [How invitations work](#invitations)
- [Upgrade propagation](#upgrade-propagation)
- [Group aliases](#group-aliases)
- [Architecture internals](#architecture-internals)

---

## What Are Context Groups?

Calimero's base unit is a **context** -- an application instance with its own state,
members, and sync topic. This works fine individually, but at scale you run into
problems:

- A chat app with 500 DMs + 30 channels = 531 independent contexts
- Upgrading the app version requires 531 separate operations
- There's no concept of "these contexts belong together"

A **Context Group** (or just "group") solves this:

```
Group "TeamChat" (app: chat-v2.3)
 |-- #general       (chat-v2.3)
 |-- #engineering   (chat-v2.3)
 |-- DM-alice-bob   (chat-v2.3)
 +-- #random        (chat-v2.3)
```

All contexts in a group run the **same application**. Upgrading the group to v2.4
propagates to every context automatically.

### Key Concepts

| Concept | Description |
|---------|-------------|
| **Group** | A workspace that owns users and contexts sharing one application |
| **Admin** | Identity that controls the group (membership, upgrades, settings) |
| **Member** | Identity authorized to create/join contexts within the group |
| **Capabilities** | Per-member permission bits (what a member can do) |
| **Visibility** | Per-context access mode (Open or Restricted with allowlists) |
| **Alias** | Optional human-friendly name for a group (local-only, not on-chain) |

### Group Membership vs Context Membership

These are separate. Being a group member lets you *create* and *join* contexts
(subject to permissions), but each context has its own member set. A DM context
has 2 participants, not the entire group. This preserves privacy.

---

## CLI Reference

All group operations use `meroctl --node <NODE> group ...`.

Most flags like `--requester` and `--admin-identity` are optional -- they default
to the node's dedicated group identity (`[identity.group]` in `config.toml`),
generated at `merod init` time.

### Group Lifecycle

```bash
# List all groups
meroctl --node <N> group list

# Create a group
meroctl --node <N> group create --application-id <APP_ID>
  # [optional] --app-key <HEX>         (auto-generated if omitted)
  # [optional] --admin-identity <PK>   (defaults to node group identity)

# Get group info
meroctl --node <N> group get <GROUP_ID>

# Update group settings (upgrade policy)
meroctl --node <N> group update <GROUP_ID> --upgrade-policy <POLICY>

# Delete group (must have no registered contexts)
meroctl --node <N> group delete <GROUP_ID>
```

### Members

```bash
# List members (shows role + capabilities)
meroctl --node <N> group members list <GROUP_ID>

# Add a member
meroctl --node <N> group members add <GROUP_ID> --identity <PK>

# Remove a member (cascades to all group contexts)
meroctl --node <N> group members remove <GROUP_ID> --identities <PK>

# Change role (Admin <-> Member)
meroctl --node <N> group members set-role <GROUP_ID> --identity <PK> --role <ROLE>

# Set capabilities
meroctl --node <N> group members set-capabilities <GROUP_ID> <MEMBER_PK> \
  [--can-create-context] [--can-invite-members] [--can-join-open-contexts]

# View capabilities
meroctl --node <N> group members get-capabilities <GROUP_ID> <MEMBER_PK>
```

### Invitations

Invitations use a two-phase commit/reveal protocol for MEV protection on-chain.
The CLI handles this transparently.

```bash
# Admin creates an invitation (outputs JSON)
meroctl --node <N> group invite <GROUP_ID>
  # [optional] --expiration-block-height N   (defaults to 999_999_999)

# Joiner uses the invitation JSON to join
meroctl --node <N> group join '<INVITATION_JSON>'
```

Invitations are `SignedGroupOpenInvitation` JSON -- transparent and inspectable.
Anyone with the JSON can join (no pre-assigned invitee).

### Contexts in a Group

```bash
# Create a context inside a group (app version auto-set to group target)
meroctl --node <N> context create --protocol near --application-id <APP> \
  --group-id <GROUP_ID>

# List contexts in a group
meroctl --node <N> group contexts list <GROUP_ID>

# Detach a context from a group
meroctl --node <N> group contexts detach <GROUP_ID> <CONTEXT_ID>

# Join an existing group context (as a group member)
meroctl --node <N> group join-group-context <GROUP_ID> --context-id <CTX_ID>
```

### Context Visibility & Allowlists

```bash
# Set visibility (open or restricted)
meroctl --node <N> group contexts set-visibility <GROUP_ID> <CTX_ID> --mode <open|restricted>

# View visibility
meroctl --node <N> group contexts get-visibility <GROUP_ID> <CTX_ID>

# Manage allowlist for restricted contexts
meroctl --node <N> group contexts allowlist list <GROUP_ID> <CTX_ID>
meroctl --node <N> group contexts allowlist add <GROUP_ID> <CTX_ID> <MEMBER_PK>...
meroctl --node <N> group contexts allowlist remove <GROUP_ID> <CTX_ID> <MEMBER_PK>...
```

### Group Defaults

```bash
# Set default capabilities for new members
meroctl --node <N> group settings set-default-capabilities <GROUP_ID> \
  [--can-create-context] [--can-invite-members] [--can-join-open-contexts]

# Set default visibility for new contexts
meroctl --node <N> group settings set-default-visibility <GROUP_ID> --mode <open|restricted>
```

### Upgrades

```bash
# Trigger version upgrade (canary -> propagate)
meroctl --node <N> group upgrade trigger <GROUP_ID> --target-application-id <APP>

# Check upgrade status
meroctl --node <N> group upgrade status <GROUP_ID>

# Retry failed upgrades
meroctl --node <N> group upgrade retry <GROUP_ID>
```

### Sync & Misc

```bash
# Sync group state from on-chain contract
meroctl --node <N> group sync <GROUP_ID>

# Register a signing key for a group
meroctl --node <N> group signing-key register <GROUP_ID> <HEX_KEY>

# Delete a group-owned context (must be group admin)
meroctl --node <N> context delete <CONTEXT_ID>
```

---

## HTTP API

Base path: `/admin-api`

### Group CRUD

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/groups` | List all groups |
| POST | `/groups` | Create group (accepts optional `alias`) |
| GET | `/groups/:id` | Get group info (includes optional `alias`) |
| PATCH | `/groups/:id` | Update group settings |
| DELETE | `/groups/:id` | Delete group |

### Members

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/groups/:id/members` | List members (role + capabilities) |
| POST | `/groups/:id/members` | Add members |
| POST | `/groups/:id/members/remove` | Remove members (cascades) |
| PUT | `/groups/:id/members/:identity/role` | Set member role |
| GET | `/groups/:id/members/:identity/capabilities` | Get capabilities |
| PUT | `/groups/:id/members/:identity/capabilities` | Set capabilities |

### Contexts

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/groups/:id/contexts` | List group contexts |
| POST | `/groups/:id/contexts/:ctx/remove` | Detach context |
| GET | `/groups/:id/contexts/:ctx/visibility` | Get visibility mode |
| PUT | `/groups/:id/contexts/:ctx/visibility` | Set visibility mode |
| GET | `/groups/:id/contexts/:ctx/allowlist` | Get allowlist |
| POST | `/groups/:id/contexts/:ctx/allowlist` | Manage allowlist (add/remove) |
| GET | `/contexts/:ctx/group` | Get context's parent group |

### Invitations

| Method | Path | Purpose |
|--------|------|---------|
| POST | `/groups/:id/invite` | Create invitation (response includes `groupAlias`) |
| POST | `/groups/join` | Join via invitation (accepts `groupAlias` hint) |
| POST | `/groups/:id/join-context` | Join a context via group membership |

### Settings & Upgrades

| Method | Path | Purpose |
|--------|------|---------|
| PUT | `/groups/:id/settings/default-capabilities` | Default capabilities for new members |
| PUT | `/groups/:id/settings/default-visibility` | Default visibility for new contexts |
| POST | `/groups/:id/upgrade` | Trigger version upgrade |
| GET | `/groups/:id/upgrade/status` | Upgrade progress |
| POST | `/groups/:id/upgrade/retry` | Retry failed upgrades |
| POST | `/groups/:id/sync` | Sync from on-chain contract |
| POST | `/groups/:id/signing-key` | Register signing key |

### Aliases

| Method | Path | Purpose |
|--------|------|---------|
| PUT | `/groups/:id/alias` | Set/update group alias (local-only) |

---

## Permissions

### Member Capabilities

Each group member has a capability bitfield (`u32`) controlling what they can do:

| Capability | What it allows |
|------------|---------------|
| `CAN_CREATE_CONTEXT` | Register new contexts in the group |
| `CAN_INVITE_MEMBERS` | Create group invitations for others |
| `CAN_JOIN_OPEN_CONTEXTS` | Join contexts with Open visibility |

**Admins bypass all capability checks.**

When a member is added (via `add_group_members` or invitation), they receive the
group's `default_member_capabilities` (default: `CAN_JOIN_OPEN_CONTEXTS`). Admins
can change this default or set capabilities per-member.

### Context Visibility

Each context in a group has a visibility mode:

| Mode | Who can join |
|------|-------------|
| **Open** (default) | Any group member with `CAN_JOIN_OPEN_CONTEXTS` |
| **Restricted** | Only members on the context's allowlist |

When a context is registered as Restricted, the creator is automatically added to the
allowlist (prevents accidental lockout). Allowlists can be pre-populated on Open
contexts before switching to Restricted.

### Authorization Matrix

| Operation | Admin | Member (with cap) | Member (no cap) |
|-----------|:-----:|:-----------------:|:---------------:|
| Create context | Yes | Yes (`CAN_CREATE_CONTEXT`) | No |
| Join Open context | Yes | Yes (`CAN_JOIN_OPEN_CONTEXTS`) | No |
| Join Restricted context | Yes (logged) | Yes (if on allowlist) | No |
| Create invitation | Yes | Yes (`CAN_INVITE_MEMBERS`) | No |
| Set capabilities | Yes | No | No |
| Set visibility | Yes or creator | No | No |
| Manage allowlist | Yes or creator | No | No |
| Set defaults | Yes | No | No |

When an admin force-joins a Restricted context they're not on the allowlist for,
a structured NEP-297 event (`AdminContextJoinEvent`) is emitted for auditability.

---

## Invitations

Group invitations use a **commit-reveal protocol** to prevent front-running on-chain.

### How It Works

**Step 1 -- Commit:** The joiner submits `SHA256(payload)` as a commitment to the
contract, stored with an expiration block height.

**Step 2 -- Reveal:** The joiner submits the full `SignedGroupRevealPayload`:
- `GroupInvitationFromAdmin`: group_id, inviter identity, expiration, secret salt
- `inviter_signature`: Ed25519 signature from the inviter
- `invitee_signature`: Ed25519 signature from the joiner

The contract verifies:
1. Commitment hash matches the revealed payload
2. Block height is within expiration
3. Both signatures are valid
4. Inviter is admin or has `CAN_INVITE_MEMBERS`
5. Joiner is not already a member
6. Invitation hasn't been used before (replay protection)

On success, the joiner is added to the group with `default_member_capabilities`.

### Flow Diagram

```
Admin (Node A)              Joiner (Node B)           On-Chain Contract
     |                           |                       |
     |  SignedGroupOpenInvitation |                       |
     |  (JSON) ----------------->|                       |
     |                           |                       |
     |                           |  commit_group_invitation
     |                           |---------------------->|  store commitment
     |                           |<----------------------|  ok
     |                           |                       |
     |                           |  reveal_group_invitation
     |                           |---------------------->|  verify signatures
     |                           |<----------------------|  ok, member added
     |                           |                       |
     |  group sync               |                       |
     |  -> sees new member       |  sees both members   |
```

---

## Upgrade Propagation

When an admin triggers a group upgrade:

1. **Canary**: The first context is upgraded as validation
2. **Contract update**: Group's `target_application` updated on-chain + locally
3. **Background propagation**: Remaining contexts upgraded sequentially
4. **Crash recovery**: Progress persisted in `GroupUpgradeValue`; resumes on restart

### Upgrade Policies

| Policy | Behavior |
|--------|----------|
| `Automatic` | All contexts upgraded immediately |
| `LazyOnAccess` | Contexts upgraded on next interaction |
| `Coordinated { deadline }` | Opt-in window with forced deadline |

### Peer Propagation

The `migration_method` field is stored on-chain. During group sync, peer nodes:
1. Discover the new target application version
2. Fetch the application binary via P2P blob sharing
3. Install the binary locally
4. Apply migration on next context access (`maybe_lazy_upgrade`)

---

## Group Aliases

Aliases provide human-friendly names for groups (e.g., "TeamChat" instead of a 32-byte hex ID).

**Aliases are local-only** -- they are never stored on-chain. This keeps on-chain
storage costs down and allows different nodes to use different names for the same group.

### Setting an Alias

```bash
# Via CLI (during group creation -- not yet exposed, use API)
# Via API:
curl -X PUT http://localhost:2428/admin-api/groups/<GROUP_ID>/alias \
  -H "Content-Type: application/json" \
  -d '{"alias": "TeamChat"}'
```

Aliases propagate to peer nodes via gossip (`GroupMutationKind::GroupAliasSet`).

### Where Aliases Appear

- `GET /admin-api/groups` -- each group summary includes its alias
- `GET /admin-api/groups/:id` -- response includes alias
- `POST /admin-api/groups` -- accepts optional `alias` on creation
- `POST /admin-api/groups/:id/invite` -- invitation response includes `groupAlias`
- `POST /admin-api/groups/join` -- join request accepts `groupAlias` hint

---

## Architecture Internals

This section covers implementation details for contributors and advanced users.

### Identity Model

Calimero uses two distinct identity types:

| Identity | Scope | Created when | Used for |
|----------|-------|-------------|----------|
| `SignerId` (ed25519) | Group-level | `merod init` | Admin actions, group membership |
| `ContextIdentity` (ed25519) | Per-context | Each `join_context_via_group` | Context state mutations, execution |

A group member has **one SignerId** but potentially **many ContextIdentity keys** --
one per context they've joined. These per-context keys are random keypairs unrelated
to the group identity. This separation means compromising one context key doesn't
affect the group or other contexts.

### The `member_contexts` Mapping

```
member_contexts: Map<(SignerId, ContextId), ContextIdentity>
```

This mapping is the backbone of cascade operations:
- **Populated** when a member joins a context via the group
- **Consumed** when removing a member from the group (cascade-removes from all contexts)
- **Cleaned up** when unregistering a context, deleting a group, or erasing the contract

Only tracks group-authorized joins. Members who joined via direct context invitation
are unaffected by group removal.

### Cascade Removal

When an admin removes a member from a group:

```
Admin calls remove_group_members(member_pk)
  |
  |-- Phase 1: Group removal
  |   |-- Remove from group.members
  |   +-- Collect all (member, ctx) -> identity mappings
  |
  +-- Phase 2: Context cascade (for each context)
      |-- Remove ContextIdentity from context.members
      |-- Remove nonce
      |-- Revoke member privileges
      +-- Revoke app privileges
```

The removed member's past contributions remain in the DAG -- removal only prevents
future access.

### On-Chain Data Model

The `OnChainGroupMeta` struct (NEAR contract):

```
OnChainGroupMeta
 |-- app_key: AppKey
 |-- target_application: Application
 |-- admins: IterableSet<SignerId>
 |-- admin_nonces: IterableMap<SignerId, u64>         (replay protection)
 |-- members: IterableSet<SignerId>
 |-- approved_registrations: IterableSet<ContextId>
 |-- context_ids: IterableSet<ContextId>              (O(1) count via .len())
 |-- invitation_commitments: IterableMap<CryptoHash, BlockHeight>
 |-- used_invitations: IterableSet<CryptoHash>        (replay protection)
 |-- member_contexts: IterableMap<(SignerId, ContextId), ContextIdentity>
 |-- migration_method: Option<String>
 |-- member_capabilities: IterableMap<SignerId, u32>
 |-- context_visibility: IterableMap<ContextId, VisibilityInfo>
 |-- context_allowlists: IterableMap<(ContextId, SignerId), ()>
 |-- default_member_capabilities: u32
 +-- default_context_visibility: VisibilityMode
```

### Local Storage Keys (Node)

| Key Type | Prefix | Content |
|----------|--------|---------|
| `GroupMeta` | `0x20` | App key, target app, upgrade policy, admin, migration |
| `GroupMember` | `0x21` | Role (Admin / Member) |
| `GroupContextIndex` | `0x22` | Context belongs to group (presence index) |
| `ContextGroupRef` | `0x23` | Context -> group (reverse index) |
| `GroupUpgradeKey` | `0x24` | Upgrade state tracking |
| `GroupSigningKey` | `0x25` | Private signing key for group operations |
| `GroupMemberCapability` | `0x26` | Capability bitfield per member |
| `GroupContextVisibility` | `0x27` | Visibility mode + creator per context |
| `GroupContextAllowlist` | `0x28` | Allowlist entries (presence index) |
| `GroupDefaultCapabilities` | `0x29` | Default capabilities for new members |
| `GroupDefaultVisibility` | `0x2A` | Default visibility for new contexts |
| `GroupAlias` | `0x2E` | Human-friendly group name |

### Guard<T> Access Control

The `Guard<T>` wrapper on context data (member lists, application settings) controls
mutable access via a set of **privileged signers** and a **revision counter** that
auto-increments on mutation (used for sync detection).

Group operations that need to modify context data (e.g., adding a member via
`join_context_via_group`) use `authorized_get_mut()` -- a method that provides
direct mutable access for contract-internal operations where authorization has
already been verified at a higher level (group membership check).

### Storage Cleanup

Every NEAR storage `insert` must have a corresponding `remove` path, or storage
leaks permanently (costing NEAR staking tokens). The cleanup matrix:

| Collection | `delete_group` | `unregister_context` | `proxy_unregister` | `erase` | `remove_members` |
|------------|:-:|:-:|:-:|:-:|:-:|
| `admins` | clear | -- | -- | clear | -- |
| `admin_nonces` | clear | -- | -- | clear | -- |
| `members` | clear | -- | -- | clear | remove |
| `approved_registrations` | clear | -- | -- | clear | -- |
| `context_ids` | clear | remove | remove | clear | -- |
| `invitation_commitments` | clear | -- | -- | clear | -- |
| `used_invitations` | clear | -- | -- | clear | -- |
| `member_contexts` | clear | per-context | per-context | clear | cascade |
| `member_capabilities` | clear | -- | -- | clear | remove |
| `context_visibility` | clear | remove | remove | clear | -- |
| `context_allowlists` | clear | per-context | per-context | clear | -- |

### Migration History

| Migration | What it adds |
|-----------|-------------|
| `03_context_groups` | `groups` and `context_group_refs` maps, `group_id` on Context |
| `04_group_invitations` | `invitation_commitments`, `used_invitations`, `member_contexts` |
| `05_group_migration_method` | `migration_method: Option<String>` |
| `06_group_permissions` | Capabilities, visibility, allowlists, defaults; existing members get `CAN_JOIN_OPEN_CONTEXTS` |

### One Group = One Application

A group is tied to a single `target_application`. All contexts run the same app.
This is intentional -- upgrade propagation requires all contexts to use the same
binary.

For multi-app scenarios, use multiple groups:

```
Workspace "Acme Corp"
 |-- Group "AcmeChat"   (app: chat-v2.3)        -> chat contexts
 |-- Group "AcmeFiles"  (app: filestorage-v1.0)  -> file contexts
 +-- Group "AcmeTasks"  (app: taskboard-v3.1)    -> task contexts
```

### Sync Model

Group state synchronization is currently **pull-based** -- nodes run `group sync`
to fetch the latest state from the on-chain contract. Group aliases are the exception:
they propagate via P2P gossip (`GroupMutationKind::GroupAliasSet`).

Full push-based propagation for all group state changes is planned but not yet implemented.

---

## Code Map

### Contract (`contracts/`)

| File | Purpose |
|------|---------|
| `near/context-config/src/lib.rs` | Data structures, types, storage prefixes |
| `near/context-config/src/mutate.rs` | All group mutations |
| `near/context-config/src/query.rs` | Read-only queries |
| `near/context-config/src/group_invitation.rs` | Commit/reveal invitation protocol |
| `near/context-config/src/guard.rs` | Guard<T> access control |
| `near/context-config/src/sys.rs` | System operations (erase, migrations) |
| `near/context-config/src/sys/migrations/` | Schema migrations (03-06 for groups) |
| `near/context-config/tests/groups.rs` | Integration tests |

### Core (`core/`)

| File | Purpose |
|------|---------|
| `crates/context/src/group_store.rs` | Local storage CRUD |
| `crates/store/src/key/group.rs` | Storage key types (prefixes 0x20-0x2E) |
| `crates/context/src/handlers/` | All group operation handlers |
| `crates/context/primitives/src/client/external/group.rs` | Contract client wrapper |
| `crates/context/primitives/src/group.rs` | Message/request types |
| `crates/server/src/admin/handlers/groups/` | HTTP endpoint handlers |
| `crates/server/src/admin/service.rs` | Route registration |
| `crates/server/primitives/src/admin.rs` | API request/response types |
| `crates/meroctl/src/cli/group/` | CLI commands |
| `crates/node/primitives/src/sync/snapshot.rs` | Gossip sync variants |
