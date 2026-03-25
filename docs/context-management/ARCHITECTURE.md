# Local Group Governance Architecture

## Overview

Every piece of group and context management that used to live on a blockchain
now lives in a **signed operation DAG** replicated via P2P gossip. There is no
central server, no chain, no relayer. Nodes agree on state by applying the
same signed operations in the same causal order.

## Data Model

```
Group (the top-level authority)
  ├── GroupMeta: admin, target_application, upgrade_policy
  ├── Members: {PublicKey → Role (Admin/Member)}
  ├── MemberCapabilities: {PublicKey → u32 bitfield}
  ├── Contexts: {ContextId → visibility, creator, alias}
  │     ├── ContextIdentity: {ContextId + PublicKey → keys}
  │     ├── ContextMemberCap: {ContextId + PublicKey → u8 bitfield}
  │     └── Allowlist: {ContextId + PublicKey → ()}
  ├── MemberContextJoins: {PublicKey + ContextId → context_identity}
  ├── GovernanceDAG: {sequence → SignedGroupOp borsh}
  └── DAG Head: {dag_heads: Vec<[u8;32]>, sequence: u64}
```

Every context belongs to a group. If you create a context without specifying
a group, one is auto-created with you as the sole admin.

All data is stored in RocksDB under the `Group` column with distinct prefix
bytes for each key type (0x20–0x33).

## The Signed Operation

Every mutation to group/context state is a `SignedGroupOp` (schema v3):

```
SignedGroupOp:
  version: 3
  group_id: [u8; 32]
  parent_op_hashes: Vec<[u8; 32]>   — DAG parents (causal ordering)
  state_hash: [u8; 32]              — SHA-256 of group state at sign time
  signer: PublicKey                  — who signed this
  nonce: u64                         — monotonic per signer (replay protection)
  op: GroupOp                        — the actual mutation
  signature: [u8; 64]               — Ed25519 over all the above
```

The signature covers a domain-separated message:
`b"calimero.group.v1" || borsh(SignableGroupOp)`

### GroupOp Variants

| Category | Variants |
|----------|----------|
| **Membership** | `MemberAdded`, `MemberRemoved`, `MemberRoleSet` |
| **Group capabilities** | `MemberCapabilitySet`, `DefaultCapabilitiesSet` |
| **Context lifecycle** | `ContextRegistered`, `ContextDetached` |
| **Context membership** | `MemberJoinedContext`, `MemberLeftContext` |
| **Context capabilities** | `ContextCapabilityGranted`, `ContextCapabilityRevoked` |
| **Visibility** | `DefaultVisibilitySet`, `ContextVisibilitySet`, `ContextAllowlistReplaced` |
| **Aliases** | `GroupAliasSet`, `MemberAliasSet`, `ContextAliasSet` |
| **Upgrades** | `UpgradePolicySet`, `TargetApplicationSet`, `GroupMigrationSet` |
| **Joining** | `JoinWithInvitationClaim` |
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

## Cascade Removal

When a member is kicked from a group, they are atomically removed from all
contexts they joined through that group:

```
Admin signs: MemberRemoved { member: M }

apply_local_signed_group_op:
  1. Look up GroupMemberContext tracking records for (group, M)
       → [(context_1, identity_1), (context_2, identity_2), ...]
  2. For each: delete ContextIdentity record (removes context access)
  3. Delete all tracking records
  4. Remove M from group member list
```

This happens identically on every node because:
- The `MemberJoinedContext` ops that created the tracking records propagated
  via the governance DAG (so every node has the same tracking data)
- The `MemberRemoved` op also propagates via the DAG
- The cascade is deterministic given the same input

This matches the on-chain contract's `remove_group_members` behavior where
cascade removal was atomic within a single NEAR transaction.

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
governance state the delta was created against. If the author was removed in
a governance op that happened *after* the delta's epoch, the node knows the
delta was produced during the propagation window — it was legitimately
authorized when created.

## State Hash (Convergence)

Each `SignedGroupOp` includes a `state_hash` — a deterministic SHA-256 of the
group's authorization-relevant state:

- Sorted members and their roles
- Admin identity
- Target application ID

This serves as an optimistic lock:

- If two admins sign concurrent ops against the same state, whichever applies
  first succeeds; the second is rejected (state changed)
- Single-admin groups are fully convergent (linear chain, no concurrent ops)
- Multi-admin concurrent ops are detected and the losing admin must re-sign

### What state_hash covers

```
SHA-256(
    group_id ||
    admin_identity ||
    target_application_id ||
    for each member sorted by public key:
        public_key || borsh(role)
)
```

Cosmetic state (aliases, capabilities) is NOT included — these changes don't
affect authorization and shouldn't force re-signing.

## Context Membership Model

### Principle

The group governance DAG is the single authority for who can access which
contexts. All joins and removals propagate as governance ops.

### Flows

| Flow | Governance Op | Replicated? |
|------|---------------|-------------|
| Create context | `ContextRegistered` + `MemberJoinedContext` | Yes |
| Join via group | `MemberJoinedContext` | Yes |
| Join via invitation | `MemberJoinedContext` (signed by admin) | Yes |
| WASM AddMember | `MemberJoinedContext` | Yes |
| WASM RemoveMember | `MemberLeftContext` | Yes |
| Kick from group | `MemberRemoved` (cascades to contexts) | Yes |
| Grant capability | `ContextCapabilityGranted` | Yes |
| Revoke capability | `ContextCapabilityRevoked` | Yes |

### Context-Level Capabilities

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

## Wire Protocol

### Gossip Messages (on `group/<hex>` topic)

| Message | Purpose |
|---------|---------|
| `GroupGovernanceDelta` | Carries a signed op with DAG metadata (`delta_id`, `parent_ids`) |
| `GroupStateHeartbeat` | Periodic broadcast of DAG heads for divergence detection |
| `GroupMutationNotification` | Legacy notification of group state change |

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
| `crates/context/src/handlers/apply_signed_group_op.rs` | Actor handler routing through DagStore |
| `crates/store/src/key/group.rs` | All group store key types (0x20–0x33) |
| `crates/node/primitives/src/sync/snapshot.rs` | `BroadcastMessage` variants |
| `crates/node/primitives/src/sync/wire.rs` | `GroupDeltaRequest`/`Response` stream types |
| `crates/node/primitives/src/client.rs` | `publish_signed_group_op`, `publish_group_heartbeat` |
| `crates/node/src/handlers/network_event.rs` | Gossip ingress handlers |
| `crates/node/src/sync/manager.rs` | `GroupDeltaRequest` stream responder |

## Known Limitations

**Multi-admin convergence**: Two admins signing concurrent ops against the
same state can cause nodes to accept different ops depending on gossip
delivery order. The state hash detects this but doesn't resolve it
deterministically. Single-admin groups have no issue. Future options:
deterministic tiebreaker, CRDTs, or consensus protocol.

**Governance epoch validation**: The governance epoch is embedded in state
deltas and logged on receive, but not yet used to reject deltas from
revoked members. Future work: compare delta's epoch against current
governance state to detect stale authorization.

**Context-level capability enforcement**: Capabilities are stored but not
yet checked during WASM execution. Future work: `ContextHost` checks
`MANAGE_APPLICATION` before app updates and `MANAGE_MEMBERS` before
member mutations.
