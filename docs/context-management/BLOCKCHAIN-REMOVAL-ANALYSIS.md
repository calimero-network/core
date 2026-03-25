# Blockchain Removal: Deep Analysis & Next Steps

> Status: Work-in-progress analysis of the `feat/local-group-governance-ops` branch.

## 1. What Has Been Done

### Phase 0 — Design (Complete)

- `LOCAL-GROUP-GOVERNANCE.md`: Full design for governance modes (`external` vs `local`),
  `SignedGroupOp` wire model, ordering/replay rules, privacy constraints.
- Phased removal roadmap (R1–R4) documented in §11.

### Phase 1 — Types (Complete)

- `crates/context/primitives/src/local_governance.rs`:
  - `SignedGroupOp` with Ed25519 signatures over `SignableGroupOp`
  - `GroupOp` enum covering: member CRUD, capabilities, visibility, context registration,
    aliases, invitations, group delete, upgrade policy, target app, migration
  - `content_hash()` / `op_content_hash()` via SHA-256 of domain-prefixed borsh
  - `parent_op_hash: Option<[u8; 32]>` field for causal chaining (unused)

### Phase 2 — Wire (Complete)

- `BroadcastMessage::SignedGroupOpV1` gossip variant in `node-primitives`
- `NodeClient::publish_signed_group_op()` publishes borsh bytes to `group/<hex>` topic
- `network_event.rs` ingress: size check → borsh decode → topic match → verify sig → apply

### Phase 3 — Apply Path (Complete)

- `group_store::apply_local_signed_group_op()`: verify sig → nonce check → dispatch → bump nonce
- `group_store::sign_apply_local_group_op_borsh()`: auto-increment nonce, sign, apply, return bytes
- All handlers (`create_context`, `create_group`, `join_group`, `upgrade_group`, etc.) now
  call `sign_apply_local_group_op_borsh` + `publish_signed_group_op` instead of chain RPCs

### Removed Code (~6700 lines deleted)

- `crates/relayer/` — entire relayer crate
- `crates/context/config/src/client.rs` + all sub-modules (NEAR config client, env, proxy, transport)
- `crates/context/primitives/src/client/external/` — external config/group/proxy clients
- `crates/runtime/src/logic/host_functions/governance.rs` — WASM governance host functions
- `crates/server/src/admin/handlers/proposals.rs` — proposal endpoints
- `crates/meroctl/src/cli/context/proposals.rs` — proposal CLI
- `apps/demo-blockchain-integrations/` — blockchain demo app
- `crates/sdk/libs/near/` — NEAR SDK lib
- Relayer Dockerfile, scripts, CI release job

### New Tests

- `crates/context/tests/local_group_governance_convergence.rs` — two logical nodes converge
  on same op sequence (member add, target app, join with invitation, visibility, aliases)
- `crates/network/tests/gossipsub_group_topic.rs` — group topic subscription
- `crates/merod/tests/init_local_governance_config.rs` — merod init produces correct config
- `crates/node/src/local_governance_node_e2e.rs` — node-level e2e sketch

---

## 2. Critical Gaps (Ordered by Severity)

### GAP-1: No Offline Catch-Up for Group Ops (CRITICAL)

**Problem:** Group ops propagate exclusively via gossipsub. If a node is offline when an op
is published, that op is permanently lost for that node.

**Current behavior:**
- `publish_signed_group_op` silently returns `Ok(())` when zero peers are on the topic
- `sync_group` handler only re-reads local store + syncs context configs — it does NOT
  fetch missing group ops from peers
- No op log is persisted, so even online peers cannot serve historical ops to latecomers
- Context data has a full sync protocol (heartbeats, delta catch-up, snapshots);
  group governance has NONE

**Impact:** A node that was offline during member additions, capability changes, or context
registrations will have a divergent group view with no way to converge. This breaks the
fundamental consistency guarantee.

**Fix required:**
1. Persist applied ops in an ordered log (per group)
2. Track the latest op sequence/hash per group
3. Implement a group-op-log exchange protocol (request-response over libp2p streams)
4. On group subscription / rejoin, fetch missing ops from peers
5. Optionally: periodic group state hash heartbeats (like context heartbeats)

### GAP-2: Non-Idempotent Op Application (HIGH)

**Problem:** `apply_local_signed_group_op` bails with an error when `op.nonce <= last`.
Gossipsub can redeliver the same message, and the op log replay protocol (once built) will
also re-send ops the node already has.

**Current behavior:**
```rust
if op.nonce <= last {
    bail!("signed group op nonce must be strictly increasing (got {}, last {})");
}
```

This returns `Err` and logs a warning in `network_event.rs`, which is noisy and masks
actual failures.

**Fix required:** Return `Ok(())` (with a trace log) for ops where `nonce <= last_applied_nonce`
for the same signer. Keep `bail!` only for ops that fail validation (wrong signer, missing
permissions, etc.).

### GAP-3: `parent_op_hash` Never Used (HIGH)

**Problem:** Every call to `SignedGroupOp::sign` passes `None` for `parent_op_hash`. The
field exists in the signed payload but is never set or checked.

**Impact:** Without causal linking, there is no way to:
- Detect gaps in the op stream
- Determine correct ordering when ops arrive out of sequence
- Build a verifiable audit trail

**Fix required:**
1. Track the last applied op's content hash in the store (per group)
2. Pass it as `parent_op_hash` when signing new ops
3. Use it in the log to verify chain integrity on replay

### GAP-4: Stale Blockchain Types in Core (MEDIUM)

**Problem:** Chain-related fields remain throughout core types, creating confusion and
unnecessary complexity:

| Location | Fields |
|----------|--------|
| `ContextConfigParams` | `protocol`, `network_id`, `contract_id`, `proxy_contract` |
| Store `ContextConfig` | `protocol`, `network`, `contract`, `proxy_contract` |
| `GroupInvitationPayload` | `protocol`, `network_id`, `contract_id`, `expiration_block_height` |
| `SyncGroupRequest` | `protocol`, `network_id`, `contract_id` (all ignored by handler) |
| `CreateContextRequest` | `protocol` (required string) |
| `GroupInvitationFromAdmin` | `protocol`, `network`, `contract_id`, `expiration_height` |

**Impact:** New developers and code paths must work around vestigial fields. Invitation
flows still encode "local"/"local"/"local" as synthetic blockchain coordinates.

**Fix required:**
- Remove `protocol`/`network_id`/`contract_id`/`proxy_contract` from context config
- Replace `expiration_block_height` with `expiration_timestamp` (unix seconds)
- Remove dead optional fields from `SyncGroupRequest`
- Provide a store migration for existing RocksDB data

### GAP-5: NEAR Dependencies Still in Workspace (MEDIUM)

**Problem:** Root `Cargo.toml` still declares: `near-account-id`, `near-crypto`,
`near-jsonrpc-client`, `near-jsonrpc-primitives`, `near-primitives`, `near-workspaces`.

These are used only by `crates/auth` (NEAR wallet authentication). If wallet auth is
kept, these deps remain necessary for `calimero-auth` but should be scoped to that crate
only, not workspace-wide.

**Fix required:**
- Move near-* deps from workspace to `crates/auth/Cargo.toml` only
- OR: feature-gate NEAR wallet auth behind `auth/near-wallet` feature

### GAP-6: Silent Publish Failure (MEDIUM)

**Problem:** `publish_signed_group_op` returns `Ok(())` when peer count is zero. The
caller (e.g. `create_context`) has no indication the op wasn't disseminated.

**Impact:** The local node applies the op but no other node receives it. Later, if op log
replay exists, it can recover. Without replay, the op is silently lost.

**Fix required:**
- Return a distinct result (e.g. `PublishResult::NoPeers`) instead of silent `Ok(())`
- Queue unpublished ops for retry when peers appear
- OR: at minimum, log at `warn` level instead of `debug`

### GAP-7: Stale Comments and Variable Names (LOW)

**Problem:** Many comments and variable names still reference blockchain concepts:
- `needs_chain_sync` variable in `join_group.rs`
- "re-sync the group state from the contract" in `GroupMutationNotification` docs
- "on-chain contract", "blockchain transactions" in various doc comments
- `TODO: get real block height from NEAR client` in `client.rs`
- "NEAR contract" serialization comment in `group_store.rs` test

**Fix required:** Systematic find-and-replace pass.

---

## 3. Remaining Blockchain Surface (to delete)

### Safe to Delete Now

| Area | Files/Items |
|------|-------------|
| Workspace deps | `near-*` entries in root `Cargo.toml` (if auth is refactored) |
| Runtime examples | `crates/runtime/examples/fetch.rs` (uses `nearkat.testnet`) |
| SDK env | `crates/sdk/src/env/ext.rs` — chain-related ext functions already gutted |
| Server admin | Dead `SyncGroupRequest` chain fields |

### Requires Auth Decision First

| Area | Notes |
|------|-------|
| `crates/auth/` | NEAR wallet provider — is this still needed? If yes, keep deps scoped to auth. If no, delete entirely. |
| `SignatureMetadataEnum::NEAR` | Used by auth only |
| `WalletType::NEAR` | Used by auth + identity primitives |

### Requires Store Migration

| Area | Notes |
|------|-------|
| `ContextConfig` stored fields | Removing `protocol`/`network`/`contract`/`proxy_contract` changes borsh layout |
| `ContextInvitationPayload` | Changing invitation format breaks in-flight invitations |

---

## 4. Recommended Implementation Order

### Step 1: Idempotent Op Application
Make `apply_local_signed_group_op` return `Ok(())` for already-seen nonces instead of
erroring. This is a one-line fix that immediately improves reliability.

### Step 2: Persistent Op Log
Add a `GroupOpLog` key type to the store. Each applied `SignedGroupOp` is persisted with
its sequence number and content hash. This is the foundation for replay.

### Step 3: Wire `parent_op_hash`
Track `last_op_content_hash` per group in the store. Pass it when signing new ops. Verify
continuity on apply (with grace for out-of-order delivery).

### Step 4: Group Op Exchange Protocol
Add a request-response protocol over libp2p streams:
- `GroupOpLogRequest { group_id, after_sequence }` → `GroupOpLogResponse { ops: Vec<SignedGroupOp> }`
- Triggered on group subscription / periodic sync / hash mismatch

### Step 5: Clean Blockchain Types
Remove chain fields from core types with a store migration path.

### Step 6: Clean Comments and Names
Systematic pass to remove stale blockchain references.

---

## 5. Architecture After Completion

```
Admin API / CLI
      │
      ▼
ContextManager (actix actor)
      │
      ├── sign_apply_local_group_op_borsh()
      │         │
      │         ├── apply_local_signed_group_op()  ──► group_store (RocksDB)
      │         │                                       ├── group meta
      │         │                                       ├── members
      │         │                                       ├── capabilities
      │         │                                       ├── nonces
      │         │                                       └── op_log ◄── NEW
      │         │
      │         └── publish_signed_group_op()  ──► gossipsub (group/<hex> topic)
      │
      └── Inbound gossip (network_event.rs)
                │
                ├── SignedGroupOpV1 ──► verify ──► apply (idempotent)
                │
                └── GroupOpLogExchange ◄── NEW: request/response for missed ops

Offline Node Recovery:
  subscribe to group topic
       │
       ▼
  request op log from peers (after last known sequence)
       │
       ▼
  replay ops in order ──► converged state
```
