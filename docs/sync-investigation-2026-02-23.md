# Sync Protocol Investigation — Feb 23, 2026

Investigation triggered by a failed 3-person Curb chat session on Feb 20 (Fran, Matea, Sandi).

## Issues Found

### Issue 1: Gossipsub mesh not forming fast enough for new joiners (FIXED)

**Severity**: Critical — prevents initial state sync entirely  
**Status**: Fixed in this branch  
**Files**: `crates/node/src/sync/manager.rs`

**Problem**: When a node joins a context, it subscribes to the gossipsub topic. The sync manager immediately tries `mesh_peers()` which returns empty because the gossipsub mesh takes 5-10 heartbeats (~5-10s) to add the new subscriber. The sync manager only retried 3× with 500ms delay (1.5s total) — not enough. The node stays permanently uninitialized (`root_hash = [0;32]`).

**Fix**: Uninitialized nodes now retry 10× with 1s delay (10s total), giving the mesh time to form.

**Reproduced**: Locally with merobox workflow (3 and 6 nodes), and manually with `run-nodes.sh` kill/restart scenario.

---

### Issue 2: Protocol negotiation missing Rule 2b — remote uninitialized (FIXED)

**Severity**: Medium — wastes time on useless HashComparison  
**Status**: Fixed in this branch  
**Files**: `crates/node/primitives/src/sync/protocol.rs`

**Problem**: `select_protocol()` had rules for "local is fresh → Snapshot" but no rule for "remote is fresh". When an initialized peer synced with an uninitialized peer, it ran HashComparison against an empty Merkle tree — completely useless.

**Fix**: Added Rule 2b: if remote has no state, return `SyncProtocol::None`. The uninitialized peer must initiate its own snapshot pull. Test added: `test_select_protocol_rule2b_remote_fresh_returns_none`.

---

### Issue 3: Key share runs unconditionally every sync cycle (NOT FIXED)

**Severity**: Medium — wastes ~200ms per sync cycle per peer  
**Status**: Open  
**Files**: `crates/node/src/sync/manager.rs:1310`, `crates/node/src/sync/key.rs`

**Problem**: `initiate_sync_inner` calls `initiate_key_share_process` every sync cycle (every 10s), even though `sender_key` is persisted to DB after the first successful exchange. The key share involves a full challenge-response protocol (Init → ack → challenge → response → key exchange).

**Impact**: ~200ms per sync cycle × N peers × M contexts = significant overhead in steady state.

**Suggested fix**: Check if we already have the peer's `sender_key` in the local DB before initiating key share. Skip if cached.

---

### Issue 4: Relay peers appear in gossipsub mesh but can't complete stream protocols (NOT FIXED)

**Severity**: High — wastes 10s per sync cycle on unreachable peers  
**Status**: Open  
**Files**: `crates/node/src/sync/manager.rs`

**Problem**: `mesh_peers()` returns ALL gossipsub mesh peers, including peers connected via relay. These peers can exchange gossipsub pub/sub messages (small payloads routed through the relay) but cannot complete the key share stream protocol (bidirectional challenge-response with 10s timeout). The sync manager picks these peers randomly and wastes 10s waiting for a response that never comes.

**Observed**: Peer `12D3KooWK1jm7PdtDmfdE8bUFvQXumDsEfARSwe241yCLED9AwkW` consistently fails key share (100% failure rate, always 10s timeout) while peer `12D3KooWScAqGKyhka8QLt8FQSAVsjormkTj6nH7wakisAS9aBth` works fine (~200ms). Both claim the same identity (Fran). The failing peer is likely behind a relay.

**Impact**: When the failing peer is picked first, the entire sync cycle takes ~11s instead of ~700ms. With 2 mesh peers, this happens ~50% of the time.

**Suggested fix**: Track peers that fail key share and temporarily blacklist them (5-minute TTL). Or: verify stream capability before adding a peer to the sync candidate list.

---

### Issue 5: `load_persisted_deltas` warns about already-tracked deltas (NOT FIXED)

**Severity**: Low — cosmetic/log noise  
**Status**: Open  
**Files**: `crates/node/src/delta_store.rs:469-600`

**Problem**: After snapshot sync, incoming gossipsub deltas are applied to state via CRDT merge (`__calimero_sync_next` WASM execution). They are also persisted to DB. On subsequent sync cycles, `load_persisted_deltas` scans the DB, finds these deltas, and tries `restore_applied_delta` — which returns `false` because they're already in the in-memory DAG. This triggers a WARN log: "Some deltas could not be loaded — they will remain pending until parents arrive".

**Reality**: The deltas ARE applied. State converges (root hashes match). The warning is misleading.

**Impact**: Noisy logs, unnecessary DB scan every sync cycle. The `remaining_count` grows with every new delta (observed 1→9 in 2 minutes) even though all state is consistent.

**Suggested fix**: In `load_persisted_deltas`, skip deltas that are already in the in-memory DAG (`self.dag.read().await.deltas.contains_key(&id)`). Demote the remaining warning to DEBUG.

---

### Issue 6: Sync cycle runs full pipeline even when already in sync (NOT FIXED)

**Severity**: Medium — network overhead  
**Status**: Open  
**Files**: `crates/node/src/sync/manager.rs`

**Problem**: Every sync cycle (every 10s) runs: key share (200ms) → load_persisted_deltas (DB scan) → query_peer_dag_state (stream round-trip) → select_protocol → "None" (already in sync). Total: ~700ms of wasted work per cycle per peer.

**Context**: Gossipsub hash heartbeats already carry root hash information. The node receives `HashHeartbeat` messages and can detect "Peer has DAG heads we don't have" vs "already in sync" without opening a new stream.

**Suggested fix**: Use gossipsub heartbeat hashes to skip the full sync cycle when hashes match. Only run the full pipeline when a mismatch is detected or on a longer interval (e.g., every 60s as a safety net).

---

### Issue 7: Curb DM invitations invisible when main context deltas don't propagate (ARCHITECTURAL)

**Severity**: High — breaks DM feature  
**Status**: Root cause identified, upstream fixes needed  

**Problem**: The Curb chat app stores DM invitation payloads in the **main context state** (via `create_dm_chat`). The invitee discovers the invitation by reading `get_dms()` from the synced main context. If the main context's gossipsub deltas don't propagate to the invitee (due to any of the above sync issues), the invitee never sees the DM invitation, never joins the DM context, and the DM is permanently invisible.

**Observed**: On Feb 20, Sandi created DMs with Matea. On-chain, only `Add context` transactions were recorded — no `CommitOpenInvitation`/`RevealOpenInvitation` for Matea. The `create_dm_chat` delta from Sandi never reached Matea because of the sync failures (Issues 1, 4).

**Impact**: DM creation appears successful to the creator but the invitee never sees it. No error surfaced to either user.

---

## Verification

| Test | Before Fix | After Fix |
|------|-----------|-----------|
| 3-node merobox workflow (sandbox) | ❌ Node 2 stays uninitialized | ✅ All phases pass |
| 3-node merobox workflow (testnet relayer) | Not tested before | ✅ All phases pass |
| 6-node merobox workflow (3 late joiners) | Not tested before | ✅ All phases pass, snapshot sync works |
| Manual kill/restart/join | ❌ Stays uninitialized forever | ✅ Snapshot syncs in 8s |
| Join real Curb chat via open invitation | Not tested before | ✅ 1283 records synced, messages readable, real-time deltas work |

## Files Changed

| File | Change |
|------|--------|
| `crates/node/primitives/src/sync/protocol.rs` | Rule 2b + test |
| `crates/node/src/sync/manager.rs` | Longer mesh wait for uninitialized nodes |
| `apps/sync-test/` | New test app + workflows + run-nodes.sh |
| `Cargo.toml` | sync-test added to workspace |

## Test Assets Created

- `apps/sync-test/src/lib.rs` — Minimal WASM app with write/read/snapshot/invitation methods
- `apps/sync-test/workflows/three-node-sync.yml` — 3-node, 6-phase workflow
- `apps/sync-test/workflows/six-node-sync.yml` — 6-node stress test with late joiners
- `apps/sync-test/run-nodes.sh` — Manual 3-node launcher for meroctl-based debugging
