# Local Group Governance Architecture

> **Interactive version:** See the [Architecture Site](../../architecture/local-governance.html) for interactive diagrams and visual deep-dives of this system.

## Overview

Every piece of group and context management that used to live on a blockchain
now lives in a **signed operation DAG** replicated via P2P gossip. There is no
central server, no chain, no relayer. Nodes agree on state by applying the
same signed operations in the same causal order.

## Data Model

```
Group (the top-level authority)
  ├── GroupMeta: admin, target_application, upgrade_policy, auto_join
  ├── Members: {PublicKey → GroupMemberValue {role, private_key?, sender_key?}}
  ├── MemberCapabilities: {PublicKey → u32 bitfield}
  ├── Contexts: {ContextId → visibility, creator, alias}
  │     ├── ContextIdentity: {ContextId + PublicKey → keys} (derived from GroupMember)
  │     ├── ContextMemberCap: {ContextId + PublicKey → u8 bitfield}
  │     └── Allowlist: {ContextId + PublicKey → ()}
  ├── GovernanceDAG: {sequence → SignedGroupOp borsh}
  └── DAG Head: {dag_heads: Vec<[u8;32]>, sequence: u64}
```

Every context belongs to a group. If you create a context without specifying
a group, one is auto-created with you as the sole admin.

All data is stored in RocksDB under the `Group` column with distinct prefix
bytes for each key type (0x20-0x33).

## The Signed Operation

Every mutation to group/context state is a `SignedGroupOp` (schema v3):

```
SignedGroupOp:
  version: 3
  group_id: [u8; 32]
  parent_op_hashes: Vec<[u8; 32]>   -- DAG parents (causal ordering)
  state_hash: [u8; 32]              -- SHA-256 of group state at sign time
  signer: PublicKey                  -- who signed this
  nonce: u64                         -- monotonic per signer (replay protection)
  op: GroupOp                        -- the actual mutation
  signature: [u8; 64]               -- Ed25519 over all the above
```

The signature covers a domain-separated message:
`b"calimero.group.v1" || borsh(SignableGroupOp)`

### GroupOp Variants

| Category | Variants |
|----------|----------|
| **Membership** | `MemberAdded`, `MemberRemoved`, `MemberRoleSet` |
| **Group capabilities** | `MemberCapabilitySet`, `DefaultCapabilitiesSet` |
| **Context lifecycle** | `ContextRegistered`, `ContextDetached` |
| **Context capabilities** | `ContextCapabilityGranted`, `ContextCapabilityRevoked` |
| **Visibility** | `DefaultVisibilitySet`, `ContextVisibilitySet`, `ContextAllowlistReplaced` |
| **Aliases** | `GroupAliasSet`, `MemberAliasSet`, `ContextAliasSet` |
| **Upgrades** | `UpgradePolicySet`, `TargetApplicationSet`, `GroupMigrationSet` |
| **Joining** | `JoinWithInvitationClaim`, `MemberJoinedViaContextInvitation` |
| **Deletion** | `GroupDelete` |
| **Placeholder** | `Noop` |

## Operation Flow

### 1. Sign and Publish

When a handler (e.g. `add_group_members`) needs to mutate group state, it
calls `sign_apply_and_publish()`:

1. Read current DAG heads from `GroupOpHead`
2. Compute `state_hash = SHA-256(members + roles + admin + target_app)`
3. Increment nonce for this signer
4. Build `SignableGroupOp`, sign with Ed25519
5. Apply locally (step 2 below)
6. Publish `BroadcastMessage::GroupGovernanceDelta` to gossipsub topic
   `group/<hex(group_id)>`

### 2. Apply (on every node)

`apply_local_signed_group_op()` runs identically on every node:

1. Verify Ed25519 signature
2. Check nonce > last seen (idempotent: `Ok(())` for duplicates)
3. Bound check: `parent_op_hashes.len() <= 256`
4. Validate `state_hash` against current group state (skip if zero)
5. Dispatch on `GroupOp` variant (admin/capability checks per op)
6. Write to RocksDB (group store, ContextIdentity, tracking records)
7. Append to persistent op log (`GroupOpLog`)
8. Update DAG heads (remove parents, add new hash, cap at 64)
9. Bump nonce for this signer

### 3. Receive via Gossip

When a node receives a `GroupGovernanceDelta` from gossip:

1. Decode borsh payload
2. Check `group_id` matches the gossip topic
3. Verify Ed25519 signature
4. Send to `ContextManager` actor
5. `ContextManager` routes through the per-group `DagStore`

### 4. DAG Ordering

Each group has an in-memory `DagStore<SignedGroupOp>` in the `ContextManager`.
The DagStore provides:

- **Topological ordering**: ops apply only after all parents are applied
- **Pending queue**: out-of-order ops wait for their parents
- **Cascading apply**: when a parent arrives, all children that were waiting
  automatically apply in order
- **Dedup**: duplicate deltas return `Ok(false)` without re-applying

The `GroupGovernanceApplier` bridges the DagStore to the group store: it
implements `DeltaApplier<SignedGroupOp>` and delegates to
`apply_local_signed_group_op()`.

## Context Membership Model

### Principle

Group membership is the single source of truth. Context access is **derived**
from group membership + context visibility rules.

A `GroupMemberValue` stores the member's `role`, `private_key`, and `sender_key`.
These keys are reused across all contexts in the group. When a member accesses
a context, a `ContextIdentity` entry is written from the group member's keys.

### Invitation and Join Flow

```
Node-1 (admin)                          Node-2 (joiner)
     |                                       |
     |  1. POST /contexts/invite             |
     |  -> invite_member() creates           |
     |     SignedOpenInvitation with:         |
     |     - invitation (signed)             |
     |     - application_id, blob_id         |
     |     - source URL, group_id            |
     |                                       |
     |  -------- invitation token -------->  |
     |                                       |
     |                          2. POST /contexts/join
     |                          -> join_context():
     |                             a. Write ContextIdentity (own keys)
     |                             b. Write ContextGroupRef + GroupMember
     |                             c. Write stub ApplicationMeta
     |                             d. Subscribe to context + group topics
     |                             e. Publish MemberJoinedViaContext-
     |                                Invitation on group topic
     |                                       |
     |  <-- governance op via gossip ---     |
     |  apply: verify inviter sig,           |
     |  add node-2 as GroupMember            |
     |                                       |
     |                             f. Trigger sync
     |  <-- sync stream (key share) ---      |
     |  has_member(node-2)? YES              |
     |  (GroupMember was just added)         |
     |                                       |
     |  --- blob share (WASM binary) --->    |
     |  --- DAG sync (state deltas) ---->    |
     |                                       |
     |  Root hashes converge                 |
```

### SignedOpenInvitation

The invitation carries everything the joiner needs to bootstrap without
an external source of truth (replacing the on-chain contract):

| Field | Purpose |
|-------|---------|
| `invitation` | Signed `InvitationFromMember` (context_id, inviter, expiry, salt) |
| `inviter_signature` | Ed25519 over borsh(invitation) |
| `application_id` | Which app to install |
| `blob_id` | Which WASM blob to request via blob sharing |
| `source` | Original source URL (enables registry re-download) |
| `group_id` | Which group to publish membership to |

Fields after `inviter_signature` are NOT covered by the signature.
They are populated by the inviter from local state.

### MemberJoinedViaContextInvitation

When a node joins via a context invitation, it publishes this governance op
on the group gossip topic. Receiving nodes verify:

1. The inviter is an admin of the group
2. The inviter's signature matches the invitation payload
3. The signer (joiner) is not already a member

No admin role is required from the joiner -- authorization comes from
the admin-signed invitation. This is the mechanism that lets node-1 learn
about node-2's membership before sync starts.

### Membership Flows

| Flow | What Happens | Governance Op |
|------|-------------|---------------|
| Create context | Auto-create group, subscribe to both topics, `ContextRegistered` op | `ContextRegistered` |
| Join via invitation | Write local membership, publish `MemberJoinedViaContextInvitation`, sync | `MemberJoinedViaContextInvitation` |
| Join group context | Visibility check + `ContextIdentity` from group keys | None (local) |
| Kick from group | Cascade: all `ContextIdentity` entries removed | `MemberRemoved` |
| Grant capability | Per-context per-member bitfield | `ContextCapabilityGranted` |
| Revoke capability | Per-context per-member bitfield | `ContextCapabilityRevoked` |

### Sync Protocol (3 phases)

After membership is established, the sync protocol runs:

1. **Key share**: Bidirectional Diffie-Hellman exchange for encrypted communication.
   Requires both nodes to have `ContextIdentity` records (or `GroupMember` for
   the `has_member` check).

2. **Blob share**: Transfer the application WASM binary. The joiner has a stub
   `ApplicationMeta` with the correct `blob_id` from the invitation. After
   receiving the blob, a proper `ApplicationMeta` is written.

3. **DAG sync**: Synchronize state deltas. The WASM module is loaded from the
   blob to process incoming deltas. Root hashes converge when both nodes have
   applied the same set of deltas.

## Cascade Removal

When a member is kicked from a group, they are atomically removed from all
contexts in that group:

```
Admin signs: MemberRemoved { member: M }

apply_local_signed_group_op:
  1. Enumerate all contexts registered in the group
  2. For each context: delete ContextIdentity(context_id, M)
  3. Remove M from group member list
```

This happens identically on every node because:
- The set of contexts in the group is replicated via `ContextRegistered` ops
- The `MemberRemoved` op also propagates via the DAG
- The cascade is deterministic (sorted context enumeration, same deletion)

## Offline Catch-Up

### Heartbeat

Every 30 seconds, the `ContextManager` broadcasts a `GroupStateHeartbeat`
for each group:

```
GroupStateHeartbeat {
    group_id: [u8; 32],
    dag_heads: Vec<[u8; 32]>,
    member_count: u32,
}
```

### Active Catch-Up

When a node receives a heartbeat with DAG heads it doesn't have locally:

1. Compare peer's heads against local heads
2. For each missing head, open a libp2p stream to the peer
3. Send `GroupDeltaRequest { group_id, delta_id }`
4. Peer searches its persistent op log and responds with
   `GroupDeltaResponse { delta_id, parent_ids, payload }` or
   `GroupDeltaNotFound`
5. Apply received ops (DagStore handles ordering and cascading)

### Startup Reload

On node restart, `ContextManager::started()` calls `reload_group_dags()`:

1. Enumerate all groups from the store
2. For each group, read the persistent op log
3. Restore each op into the in-memory DagStore via `restore_applied_delta`
4. This rebuilds the pending queue, head tracking, and applied set

## Governance Epoch in State Deltas

When a WASM app executes and produces a state delta (CRDT operations), the
delta carries the governance DAG heads at execution time:

```
BroadcastMessage::StateDelta {
    ...
    governance_epoch: Vec<[u8; 32]>
}
```

This allows receiving nodes to determine if the author was authorized at the
governance state the delta was created against.

## State Hash (Convergence)

Each `SignedGroupOp` includes a `state_hash` -- a deterministic SHA-256 of the
group's authorization-relevant state:

- Sorted members and their roles
- Admin identity
- Target application ID

This serves as an optimistic lock:

- If two admins sign concurrent ops against the same state, whichever applies
  first succeeds; the second is rejected (state changed)
- Single-admin groups are fully convergent (linear chain, no concurrent ops)
- Multi-admin concurrent ops are detected and the losing admin must re-sign

## Gossip Topics

Two separate gossipsub topics per group/context:

| Topic | Format | Content | Encryption |
|-------|--------|---------|------------|
| **Context** | `<context_id>` | State deltas (CRDT ops from WASM), sync streams | Encrypted (shared key) |
| **Group** | `group/<hex(group_id)>` | Governance ops (membership, visibility, etc.) | Plaintext (signed) |

Both nodes must subscribe to both topics. Context creation subscribes to both;
joining subscribes to both before publishing the membership governance op.

## Context-Level Capabilities

Per-context per-member capability bitfield stored in `GroupContextMemberCap`:

| Bit | Constant | Meaning |
|-----|----------|---------|
| 0 | `MANAGE_APPLICATION` | Can update the context's application |
| 1 | `MANAGE_MEMBERS` | Can add/remove context members |

Granted/revoked via governance ops with admin authorization.

## Security Bounds

| Bound | Value | Purpose |
|-------|-------|---------|
| `MAX_PARENT_OP_HASHES` | 256 | Prevents DoS from ops with huge parent lists |
| `MAX_DAG_HEADS` | 64 | Caps head growth from concurrent ops |
| `MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES` | 64 KB | Prevents oversized gossip payloads |
| Nonce monotonicity | per signer | Replay protection |
| State hash validation | per op | Stale-state rejection |
| Ed25519 signature | per op | Authentication + tamper protection |
| Invitation expiry | timestamp | Limits window for leaked tokens |

## Wire Protocol

### Gossip Messages (on `group/<hex>` topic)

| Message | Purpose |
|---------|---------|
| `GroupGovernanceDelta` | Carries a signed op with DAG metadata (`delta_id`, `parent_ids`) |
| `GroupStateHeartbeat` | Periodic broadcast of DAG heads for divergence detection |

### Stream Protocol (libp2p request-response)

| Request | Response | Purpose |
|---------|----------|---------|
| `GroupDeltaRequest { group_id, delta_id }` | `GroupDeltaResponse { delta_id, parent_ids, payload }` | Fetch a specific op by content hash |
| | `GroupDeltaNotFound` | Requested op not in this node's log |

## File Map

| File | Purpose |
|------|---------|
| `crates/context/primitives/src/local_governance.rs` | `SignedGroupOp`, `GroupOp`, signing/verification |
| `crates/context/src/group_store.rs` | Apply logic, store helpers, `sign_apply_and_publish` |
| `crates/context/src/governance_dag.rs` | `GroupGovernanceApplier`, `signed_op_to_delta` |
| `crates/context/src/lib.rs` | `ContextManager` with per-group `DagStore`, heartbeat, reload |
| `crates/context/src/handlers/create_context.rs` | Context + auto-group creation, subscribes to both topics |
| `crates/context/src/handlers/join_context.rs` | Join via invitation, publish `MemberJoinedViaContextInvitation` |
| `crates/context/src/handlers/apply_signed_group_op.rs` | Actor handler routing through DagStore |
| `crates/context/config/src/types.rs` | `SignedOpenInvitation`, `InvitationFromMember` |
| `crates/store/src/key/group.rs` | All group store key types (0x20-0x33) |
| `crates/node/primitives/src/sync/snapshot.rs` | `BroadcastMessage` variants |
| `crates/node/primitives/src/sync/wire.rs` | `GroupDeltaRequest`/`Response` stream types |
| `crates/node/primitives/src/client.rs` | `publish_signed_group_op`, `publish_group_heartbeat` |
| `crates/node/src/handlers/network_event.rs` | Gossip ingress handlers |
| `crates/node/src/sync/manager.rs` | Sync protocol, `has_member` retry, blob/DAG sync |

## Known Limitations

**Multi-admin convergence**: Two admins signing concurrent ops against the
same state can cause nodes to accept different ops depending on gossip
delivery order. The state hash detects this but doesn't resolve it
deterministically. Single-admin groups have no issue.

**Gossip propagation latency**: The sync responder retries `has_member` with
a 2-second delay to handle the window between when a joiner publishes their
membership op and when the responder receives it via gossip. In high-latency
networks this window may need tuning.

**Context-level capability enforcement**: Capabilities are stored but not
yet checked during WASM execution. Future work: `ContextHost` checks
`MANAGE_APPLICATION` before app updates and `MANAGE_MEMBERS` before
member mutations.

