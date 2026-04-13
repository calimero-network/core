# Reliable Event Handler Execution via Sidecar Event Store

**Issue:** [#2139 — Event handlers not executed when deltas arrive via sync instead of broadcast](https://github.com/calimero-network/core/issues/2139)
**Date:** 2026-04-13
**Status:** Proposed

## Problem

Events emitted with handler tags (`app::emit!((event, "handler_name"))`) are only transmitted in-band with gossipsub broadcasts. When a node misses the broadcast and catches up via periodic DAG sync, the delta's state is applied correctly via CRDT merge, but the associated events and handler tags are lost — they were never included in the sync protocol.

This makes `app::emit!` with handlers **unreliable** for any cross-node side-effect that must execute on the receiving node.

### Root Cause

Calimero has two delivery paths for deltas, but only one carries events:

| Path | Carries Events | Handler Execution |
|------|---------------|-------------------|
| Gossipsub broadcast (`BroadcastMessage::StateDelta`) | Yes — `events: Option<Cow<'a, [u8]>>` | Yes — `execute_event_handlers_parsed()` |
| DAG sync (`DeltaResponse`) | No — only `delta: Cow<'a, [u8]>` | No |

The `CausalDelta` struct (the DAG/storage format) contains `id`, `parents`, `actions`, `hlc`, and `expected_root_hash` — no events field. Events are ephemeral metadata attached only to the gossipsub delivery mechanism.

### Impact

In the battleships demo, `propose_shot` emits `ShotProposed` with handler `acknowledge_shot`. If gossipsub delivers the broadcast, the handler fires and the shot resolves. If the broadcast is missed and the delta arrives via sync, the handler never fires — the shot is stuck as pending forever. Reproduction rate: 30-50% of shots in local e2e testing.

This affects any app that uses handler-tagged events for cross-node coordination.

## Design Decisions

### Delivery Semantics: At-Least-Once on Every Peer

Every node in a context that applies a delta executes the associated handlers. This is the right default because:

1. **CRDTs guarantee convergence** — even if all 10 nodes in a context execute the same handler, the resulting CRDT operations merge idempotently.
2. **No single point of failure** — no node is "special." If only one node executed a handler and that node went offline, the side-effect would be lost.
3. **Simple mental model** — app developers know "your handler runs on every peer." Handlers that should only run on a specific node can self-filter via `env::executor_id()`.

### Storage Model: Sidecar Event Store

Events are stored **separately** from `CausalDelta`, keyed by `(context_id, delta_id)`.

```
DAG Store (UNCHANGED)
  CausalDelta { id, parents, actions, hlc, expected_root_hash }

Event Sidecar Store (NEW)
  Key:   (context_id: [u8; 32], delta_id: [u8; 32])
  Value: borsh-serialized Vec<ExecutionEvent>
```

**Why sidecar, not embedded in CausalDelta:**
- Deltas stay lean and deterministic — delta ID computation is unchanged
- Events have a different lifecycle (GC'd after full propagation, deltas live forever in the DAG)
- Additive change — no migration of existing delta storage
- Wire protocol change is backward compatible (optional field)

### Core Invariant: Handler Execution Is Atomic with Delta Application

There is exactly one moment a delta transitions from "not applied" to "applied" on a given node. Events must be available at that moment. If they are, handlers fire. If not, the node fetches them before applying.

Delta application is already deduplicated by the DAG (content-addressed delta IDs in the `applied` set). By tying handler execution to delta application, handler dedup is free — no additional tracking flags needed.

### Availability Guarantee

The author node always persists events (it produced them). Every node that receives events (via gossipsub or sync) also persists them to the sidecar. When a sync responder doesn't have events for a delta, the requester falls back to asking other context peers. As long as the author or any gossipsub receiver is reachable, events are available.

## Architecture

### Delivery Paths

**Path 1 — Gossipsub (existing, enriched with sidecar persistence)**

```
Author executes method
  -> WASM produces delta + events
  -> Author persists events to sidecar (keyed by delta_id)
  -> Author broadcasts BroadcastMessage::StateDelta { delta, events }
  -> Receiver gets delta + events
  -> Receiver persists events to sidecar
  -> Receiver applies delta + executes handlers
```

**Path 2 — Sync with events available (common case)**

```
Node B requests missing deltas from Node A
  -> A responds with DeltaResponse { delta, events? }
  -> A checks sidecar: has events for this delta? Include them.
  -> B receives delta + events
  -> B persists events to sidecar
  -> B applies delta + executes handlers
```

**Path 3 — Sync without events (fallback)**

```
Node B requests missing deltas from Node C
  -> C responds with DeltaResponse { delta, events: None }
     (C also got this delta via sync and never had events)
  -> B receives delta, no events
  -> B sends EventRequest { delta_ids: [...] } to other context peers
  -> Some peer responds with EventResponse { events_by_delta }
  -> B persists events, applies delta + executes handlers

  If NO peer has events (all offline):
  -> B applies delta WITHOUT handler execution
  -> Logs warning: "Events unavailable for delta {id}, handlers skipped"
```

### Wire Protocol Changes

**Modified message (backward compatible):**

```rust
// crates/node/primitives/src/sync/wire.rs
MessagePayload::DeltaResponse {
    delta: Cow<'a, [u8]>,
    events: Option<Cow<'a, [u8]>>,  // NEW — optional, old nodes send None
}
```

**New messages:**

```rust
// crates/node/primitives/src/sync/wire.rs
MessagePayload::EventRequest {
    delta_ids: Cow<'a, [[u8; 32]]>,
}

MessagePayload::EventResponse {
    // Only includes entries where responder has events
    entries: Cow<'a, [(/* delta_id */ [u8; 32], /* serialized events */ Vec<u8>)]>,
}
```

**Modified broadcast message:**

```rust
// crates/node/primitives/src/sync/snapshot.rs
BroadcastMessage::StateDelta {
    // ... existing fields ...
    events: Option<Cow<'a, [u8]>>,
    handler_depth: u8,  // NEW — tracks handler chain depth
}
```

### Handler Depth Limit

With N nodes, a handler chain of depth D produces O(N^D) executions across the network (10 nodes, depth 2 = ~90 executions; depth 3 = ~900). A configurable depth limit prevents runaway cascades.

**Mechanism:**

- User-initiated method executions broadcast with `handler_depth: 0`
- When a handler fires and produces a new delta with events, `handler_depth` = parent's depth + 1
- If `handler_depth >= MAX_HANDLER_DEPTH` (default: 2), handler tags are stripped from events before broadcast
- Events still emit to WebSocket/SSE clients (frontend notifications unaffected)
- Enforced at the broadcast layer — WASM apps don't need to know about it

### Garbage Collection

**Trigger:** Events can be GC'd once all nodes in the context have applied the delta.

**Detection:** The sync protocol already exchanges DAG heads via heartbeats. A delta is fully propagated when every peer's heads are descendants of it.

```
For each delta_id in event sidecar:
  if ALL known context members' last-seen heads descend from delta_id:
    -> delete events for delta_id from sidecar
```

**Safeguards:**
- **Offline node timeout:** If a node hasn't been seen in 24 hours (configurable), don't let it block GC
- **Maximum retention:** Hard cap at 7 days — prevents unbounded growth if a node drops permanently
- **GC runs on heartbeat cycle** — no separate timer, piggybacks on existing sync heartbeat

### Storage Schema

**New RocksDB column family:**

```
Column Family: "context_delta_events"
Key:   (context_id: [u8; 32], delta_id: [u8; 32])
Value: Vec<u8>  // borsh-serialized Vec<ExecutionEvent>
```

**Write points:**
- Author node at execution time, before broadcast
- Gossipsub receiver when processing `BroadcastMessage::StateDelta`
- Sync receiver when `DeltaResponse` includes events or `EventResponse` arrives

**Read points:**
- Delta application path — to retrieve events for handler execution
- Sync responder — to populate `DeltaResponse.events` or `EventResponse`

## File Changes

| File | Change |
|------|--------|
| `crates/node/primitives/src/sync/wire.rs` | Add `events` to `DeltaResponse`, add `EventRequest`/`EventResponse` variants |
| `crates/node/primitives/src/sync/snapshot.rs` | Add `handler_depth` to `BroadcastMessage::StateDelta` |
| `crates/node/primitives/src/client.rs` | Pass `handler_depth` in `broadcast()` |
| `crates/node/src/handlers/state_delta/mod.rs` | Persist events to sidecar on receive, depth check before handler execution |
| `crates/node/src/handlers/network_event.rs` | Pass events through from broadcast to state_delta handler |
| `crates/node/src/sync/delta_request.rs` | Enrich `DeltaResponse` with events from sidecar, handle `EventRequest`/`EventResponse` |
| `crates/node/src/sync/manager/mod.rs` | After sync apply, check sidecar for events, fallback `EventRequest` to peers |
| `crates/node/src/delta_store.rs` | New sidecar storage methods: `store_events()`, `get_events()`, `gc_events()` |
| `crates/context/src/handlers/execute/mod.rs` | Author persists events to sidecar before broadcast, pass handler depth |

## What Does NOT Change

- `CausalDelta` struct — untouched
- Delta ID computation — untouched
- CRDT merge logic — untouched
- `app::emit!` SDK macro — untouched (app developers change nothing)
- WASM runtime host functions — untouched
- Snapshot sync — untouched (snapshot is full state recovery, events irrelevant for historical deltas)
- WebSocket/SSE notification path — untouched (events still flow to frontends as before)

## Backward Compatibility

- `DeltaResponse.events` is an `Option` — old nodes send `None`, new nodes handle `None` gracefully (fallback to `EventRequest`)
- `EventRequest`/`EventResponse` are new message variants — old nodes ignore unknown variants
- `handler_depth` defaults to `0` — old nodes that don't send it are treated as user-initiated executions
- No storage migration — sidecar is a new column family, populated going forward
- Mixed-version contexts degrade to current behavior (events only via gossipsub) until all nodes upgrade

## Open Questions

1. **EventRequest routing:** Should `EventRequest` be broadcast to all context peers, or targeted at specific peers (e.g., the delta author if known)? Broadcasting is simpler but noisier.
2. **Batch size for EventRequest:** Should there be a cap on `delta_ids` per request to prevent oversized messages?
3. **Handler depth configurability:** Should `MAX_HANDLER_DEPTH` be per-context or global? Per-context gives app developers control but adds config complexity.
