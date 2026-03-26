# Sync Fix — Agent Prompts

Three independent tasks to fix the sync protocol issues discovered on Feb 23, 2026. Each can be worked in parallel on a separate branch. All share the same test infrastructure in `apps/sync-test/`.

Read `docs/sync-investigation-2026-02-23.md` for full context before starting any task.

---

## Agent 1: Fix gossipsub mesh wait for uninitialized nodes

**Branch**: `fix/sync-mesh-wait-uninitialized`  
**Issue doc**: `docs/issues/sync-issue-1-mesh-wait.md`

### Prompt

You are fixing a critical sync bug in the Calimero node. When a node joins a context, it needs to find gossipsub mesh peers to initiate snapshot sync. The current code retries only 3 times with 500ms delay (1.5s total). On real networks, gossipsub mesh formation takes 5-30+ seconds depending on relay/NAT conditions. Uninitialized nodes that can't find mesh peers stay permanently broken.

**Your task:**

1. Read `docs/issues/sync-issue-1-mesh-wait.md` and `docs/sync-investigation-2026-02-23.md` for full context.

2. In `crates/node/src/sync/manager.rs`, function `perform_interval_sync`:
   - Move the `get_context` + `is_uninitialized` check BEFORE the mesh peer retry loop
   - For uninitialized nodes: retry 10× with 1s delay (10s total)
   - For initialized nodes: keep existing 3× with 500ms (1.5s)
   - Log the `is_uninitialized` flag and `max_retries` in the retry debug message

3. Consider a further improvement: when the node is uninitialized and mesh_peers is still empty after all retries, try `open_stream` to any connected peer (from the general peer list, not context-specific mesh). This handles relay-connected networks where gossipsub mesh never forms for the context topic.

4. Run the existing tests:
   ```bash
   cargo test -p calimero-node-primitives
   cargo check -p calimero-node
   ```

5. Build merod and run the merobox workflow:
   ```bash
   cargo build -p merod
   cd apps/sync-test
   ./build.sh  # or: cargo build -p sync-test --target wasm32-unknown-unknown --profile app-release && cp target/wasm32-unknown-unknown/app-release/sync_test.wasm res/
   merobox bootstrap run --no-docker --binary-path ../../target/debug/merod --e2e-mode -v workflows/three-node-sync.yml
   ```

6. Manual verification with the kill/restart scenario:
   ```bash
   cd apps/sync-test && ./run-nodes.sh
   # In another terminal: install app, create context, write data, kill node 3,
   # restart node 3, invite+join — verify it snapshot syncs
   ```

**Acceptance criteria:**
- Uninitialized nodes find mesh peers and snapshot sync within 15s on localhost
- Initialized nodes are not affected (same 1.5s retry)
- All existing tests pass
- Both merobox workflows (3-node and 6-node) pass

---

## Agent 2: Move NEAR RPC out of key share stream handler

**Branch**: `fix/sync-key-share-no-rpc-block`  
**Issue doc**: `docs/issues/sync-issue-2-key-share-blocks-on-near-rpc.md`

### Prompt

You are fixing the primary cause of the Feb 20 production sync failure. When a node receives a key share request from a new member, `internal_handle_opened_stream` calls `sync_context_config` which does 3+ NEAR view calls. These RPCs can take >10s through a relayer, causing the initiator's key share to timeout. The new member never gets synced.

**Your task:**

1. Read `docs/issues/sync-issue-2-key-share-blocks-on-near-rpc.md` and `docs/sync-investigation-2026-02-23.md` for full context.

2. Add two new methods to `ContextClient` in `crates/context/primitives/src/client.rs`:

   ```rust
   /// Single NEAR view call to check membership on-chain (~200ms vs 10s+ for full sync)
   pub async fn check_member_on_chain(&self, context_id: &ContextId, public_key: &PublicKey) -> eyre::Result<bool>
   
   /// Add a member to local DB cache without full sync
   pub fn add_member_to_local_cache(&self, context_id: &ContextId, public_key: &PublicKey) -> eyre::Result<()>
   ```

   `check_member_on_chain` should use the existing `external_client.config().has_member()` path (single `has_member` NEAR view call). `add_member_to_local_cache` should insert a `ContextIdentity { private_key: None, sender_key: None }` into the datastore.

3. In `crates/node/src/sync/manager.rs`, function `internal_handle_opened_stream` (line ~1925):
   - When `has_member` returns false, call `check_member_on_chain` instead of `sync_context_config`
   - If confirmed on-chain, call `add_member_to_local_cache` and proceed
   - If the single RPC fails, fall back to `sync_context_config` with a WARN log
   - If still not a member after fallback, bail as before

4. Run tests:
   ```bash
   cargo test -p calimero-node-primitives
   cargo check -p calimero-node
   cargo build -p merod
   cd apps/sync-test && merobox bootstrap run --no-docker --binary-path ../../target/debug/merod --e2e-mode -v workflows/three-node-sync.yml
   ```

**Acceptance criteria:**
- `internal_handle_opened_stream` completes in <1s for new members (single RPC, not 3+)
- Unknown members are still rejected if not on-chain
- All existing tests pass
- Merobox 3-node workflow passes

**Key insight:** The receiver currently holds the stream open while doing 3+ NEAR RPCs. The initiator has a 10s timeout (`sync_config.timeout / 3`). A single `has_member` RPC (~200ms) fits easily within the budget. The full `sync_context_config` can run in the background afterward.

---

## Agent 3: Skip key share when sender_key already cached

**Branch**: `fix/sync-skip-cached-key-share`  
**Issue doc**: `docs/issues/sync-issue-3-redundant-key-share.md`

### Prompt

You are optimizing the sync protocol to skip the key share handshake when it's not needed. Currently, `initiate_sync_inner` runs the full 8-message key share protocol on every sync cycle (every 10s), even though the `sender_key` is persisted after the first successful exchange. This wastes 200ms per cycle on healthy peers and 10s on broken/relay peers.

**Your task:**

1. Read `docs/issues/sync-issue-3-redundant-key-share.md` and `docs/sync-investigation-2026-02-23.md` for full context.

2. In `crates/node/src/sync/manager.rs`, function `initiate_sync_inner` (line ~1321):
   - After getting `our_identity`, check if we already have `sender_key` for all known members of this context
   - If all sender_keys are cached, skip `initiate_key_share_process` entirely
   - If any sender_key is missing, run key share as before
   - Log when key share is skipped: `"Skipping key share — sender_keys already cached"`

   The check: iterate `get_context_members(context_id, Some(false))` (all members, not just owned). For each, check `get_identity(context_id, member_id).sender_key.is_some()`. If all have sender_keys, skip.

3. Additionally, add peer failure tracking to avoid wasting 10s on broken peers:
   - Add a `HashMap<(ContextId, PeerId), (Instant, u32)>` to `SyncManager` for tracking failed peers
   - After a key share failure, record the peer with timestamp and failure count
   - Before attempting sync with a peer, check if it's been failing recently (e.g., skip if failed in last 5 minutes and failure_count > 3)
   - Log when a peer is skipped: `"Skipping peer — recent key share failures"`

4. Run tests:
   ```bash
   cargo test -p calimero-node-primitives
   cargo check -p calimero-node
   cargo build -p merod
   cd apps/sync-test && merobox bootstrap run --no-docker --binary-path ../../target/debug/merod --e2e-mode -v workflows/three-node-sync.yml
   ```

5. Verify with the real Curb chat context (if available):
   - Join context `4BHG5RLqSPs9ewxUEKPAWSkA3xbrJ7Kz81K9fq24ST8h` 
   - After initial key share, verify subsequent sync cycles show "Skipping key share"
   - Verify peer `12D3KooWK1jm...` is blacklisted after 3 failures

**Acceptance criteria:**
- Key share only runs on first encounter with a new peer identity
- Subsequent sync cycles skip key share (log confirms)
- Broken/relay peers are skipped after 3 consecutive failures
- Sync cycle time drops from ~700ms to ~500ms in steady state (no key share overhead)
- All existing tests pass
- Merobox 3-node workflow passes

**Important:** The key share skip MUST check sender_keys for all context members, not just the peer we're about to sync with. The stream is between two libp2p PeerIDs, but the key share exchanges sender_keys for context-specific identities. A single PeerID may host multiple context identities.

---

## Shared Test Infrastructure

All three agents share:

- `apps/sync-test/` — WASM test app with write/read/snapshot/invitation methods
- `apps/sync-test/workflows/three-node-sync.yml` — 3-node, 6-phase merobox workflow
- `apps/sync-test/workflows/six-node-sync.yml` — 6-node stress test
- `apps/sync-test/run-nodes.sh` — Manual 3-node launcher for meroctl debugging
- `docs/sync-investigation-2026-02-23.md` — Full investigation with log analysis

Build the test app:
```bash
cd apps/sync-test
cargo build -p sync-test --target wasm32-unknown-unknown --profile app-release
cp ../../target/wasm32-unknown-unknown/app-release/sync_test.wasm res/
```

## Merge Order

1. **Agent 1** (mesh wait) — standalone, no dependencies
2. **Agent 2** (RPC out of stream handler) — standalone, no dependencies  
3. **Agent 3** (skip cached key share) — can merge after 1 or 2, no hard dependency

All three can be developed in parallel. Rule 2b (`protocol.rs`) is already implemented and tested on the current branch.
