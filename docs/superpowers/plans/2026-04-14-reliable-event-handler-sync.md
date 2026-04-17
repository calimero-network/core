# Reliable Event Handler Sync — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make event handlers fire reliably when deltas arrive via DAG sync (not just gossipsub), by persisting events in a sidecar store and enriching the sync protocol to carry them.

**Architecture:** Events are stored separately from `CausalDelta` in the existing `Column::Delta` column family under a new key type `ContextDeltaEvents`. The sync wire protocol's `DeltaResponse` gains an optional `events` field. New `EventRequest`/`EventResponse` messages provide a fallback when the sync peer doesn't have events. Handler execution is tied atomically to delta application — no separate dedup mechanism needed. A `handler_depth` field on broadcasts prevents exponential cascade fan-out.

**Tech Stack:** Rust, Borsh serialization, RocksDB (via `calimero-store`), libp2p streams, actix actors, tokio async

**Spec:** `docs/superpowers/specs/2026-04-13-reliable-event-handler-sync-design.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/store/src/key/context.rs` | Create key type | `ContextDeltaEvents` storage key |
| `crates/store/src/types/context.rs` | Create value type | `ContextDeltaEvents` value struct |
| `crates/node/primitives/src/sync/wire.rs` | Modify | Add `events` to `DeltaResponse`, add `EventRequest`/`EventResponse` variants |
| `crates/node/primitives/src/sync/snapshot.rs` | Modify | Add `handler_depth` to `BroadcastMessage::StateDelta` |
| `crates/node/primitives/src/client.rs` | Modify | Pass `handler_depth` through `broadcast()` |
| `crates/node/src/delta_store.rs` | Modify | `store_events()`, `get_events()`, `gc_events()` methods |
| `crates/node/src/handlers/state_delta/mod.rs` | Modify | Persist events on receive, pass depth, use sidecar for cascaded deltas |
| `crates/node/src/sync/delta_request.rs` | Modify | Enrich `DeltaResponse` with events, handle `EventRequest`/`EventResponse` |
| `crates/node/src/sync/manager/mod.rs` | Modify | After sync apply, execute handlers from sidecar events |
| `crates/context/src/handlers/execute/mod.rs` | Modify | Persist events to sidecar before broadcast, pass handler_depth |
| `workflows/fuzzy-tests/kv-store-with-handlers/fuzzy-test.yml` | Modify | Update success threshold after fix |
| `apps/e2e-kv-store/workflows/e2e.yml` | Verify | Handler count assertions should pass reliably after fix |

---

## Task 1: Add `ContextDeltaEvents` Storage Key and Value Types

**Files:**
- Modify: `crates/store/src/key/context.rs:365+` (after `ContextDagDelta`)
- Modify: `crates/store/src/types/context.rs:137+` (after `ContextDagDelta`)

This task adds the sidecar storage types. Events are stored in the same `Column::Delta` column family as deltas but under a distinct key type with a different byte prefix, so they are namespaced separately.

- [ ] **Step 1: Add `ContextDeltaEvents` key type**

In `crates/store/src/key/context.rs`, add after the `ContextDagDelta` block (after line 364):

```rust
/// Key for storing events associated with a DAG delta (sidecar)
///
/// Events are stored separately from the delta itself so they can be
/// garbage-collected independently and don't bloat the CausalDelta wire format.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct ContextDeltaEvents(Key<(ContextId, DeltaId)>);

impl ContextDeltaEvents {
    #[must_use]
    pub fn new(context_id: PrimitiveContextId, delta_id: [u8; 32]) -> Self {
        Self(Key(
            GenericArray::from(*context_id).concat(GenericArray::from(delta_id))
        ))
    }

    #[must_use]
    pub fn context_id(&self) -> PrimitiveContextId {
        let mut context_id = [0; 32];
        context_id.copy_from_slice(&AsRef::<[_; 64]>::as_ref(&self.0)[..32]);
        context_id.into()
    }

    #[must_use]
    pub fn delta_id(&self) -> [u8; 32] {
        let mut delta_id = [0; 32];
        delta_id.copy_from_slice(&AsRef::<[_; 64]>::as_ref(&self.0)[32..]);
        delta_id
    }
}

impl AsKeyParts for ContextDeltaEvents {
    type Components = (ContextId, DeltaId);

    fn column() -> Column {
        Column::Generic // Use Generic column to avoid collision with Delta column's ContextDagDelta
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for ContextDeltaEvents {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for ContextDeltaEvents {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextDeltaEvents")
            .field("context_id", &self.context_id())
            .field("delta_id", &self.delta_id())
            .finish()
    }
}
```

- [ ] **Step 2: Add `ContextDeltaEvents` value type**

In `crates/store/src/types/context.rs`, add after the `ContextDagDelta` impl block:

```rust
/// Sidecar event data for a DAG delta
///
/// Stored separately from the delta. Contains serialized `Vec<ExecutionEvent>`
/// that should be replayed when the delta is applied.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug)]
pub struct ContextDeltaEvents {
    /// Serialized events (Vec<ExecutionEvent> as JSON bytes)
    pub events: Vec<u8>,
}

impl PredefinedEntry for key::ContextDeltaEvents {
    type Codec = Borsh;
    type DataType<'a> = ContextDeltaEvents;
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p calimero-store`
Expected: Compiles with no errors

- [ ] **Step 4: Commit**

```bash
git add crates/store/src/key/context.rs crates/store/src/types/context.rs
git commit -m "feat(store): add ContextDeltaEvents sidecar storage key and value types

Part of #2139 — events stored separately from CausalDelta for sync reliability."
```

---

## Task 2: Add Event Sidecar Methods to DeltaStore

**Files:**
- Modify: `crates/node/src/delta_store.rs:634+`

This task adds methods to read/write events from the sidecar store. These methods are used by both the gossipsub receive path and the sync path.

- [ ] **Step 1: Add `store_events` method**

In `crates/node/src/delta_store.rs`, add a new impl block or add to the existing impl block (after line 634, before `add_delta_with_events`):

```rust
/// Persist events to the sidecar store for a given delta
///
/// This is idempotent — calling it twice with the same delta_id overwrites.
pub fn store_events(
    &self,
    delta_id: [u8; 32],
    events: &[u8],
) -> Result<()> {
    let mut handle = self.applier.context_client.datastore_handle();
    handle
        .put(
            &calimero_store::key::ContextDeltaEvents::new(
                self.applier.context_id,
                delta_id,
            ),
            &calimero_store::types::ContextDeltaEvents {
                events: events.to_vec(),
            },
        )
        .map_err(|e| eyre::eyre!("Failed to store events for delta: {}", e))?;

    debug!(
        context_id = %self.applier.context_id,
        delta_id = ?delta_id,
        events_len = events.len(),
        "Stored events in sidecar"
    );

    Ok(())
}

/// Retrieve events from the sidecar store for a given delta
///
/// Returns `None` if no events were stored for this delta.
pub fn get_events(
    &self,
    delta_id: [u8; 32],
) -> Result<Option<Vec<u8>>> {
    let handle = self.applier.context_client.datastore_handle();
    let key = calimero_store::key::ContextDeltaEvents::new(
        self.applier.context_id,
        delta_id,
    );

    match handle.get(&key) {
        Ok(Some(stored)) => Ok(Some(stored.events)),
        Ok(None) => Ok(None),
        Err(e) => Err(eyre::eyre!("Failed to read events from sidecar: {}", e)),
    }
}

/// Delete events from the sidecar store for a given delta (GC)
pub fn delete_events(
    &self,
    delta_id: [u8; 32],
) -> Result<()> {
    let mut handle = self.applier.context_client.datastore_handle();
    handle
        .delete(
            &calimero_store::key::ContextDeltaEvents::new(
                self.applier.context_id,
                delta_id,
            ),
        )
        .map_err(|e| eyre::eyre!("Failed to delete events from sidecar: {}", e))?;
    Ok(())
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p calimero-node`
Expected: Compiles (may have unused warnings — that's fine, consumers come in later tasks)

- [ ] **Step 3: Commit**

```bash
git add crates/node/src/delta_store.rs
git commit -m "feat(node): add event sidecar store/get/delete methods to DeltaStore

Part of #2139 — methods for persisting and retrieving events alongside deltas."
```

---

## Task 3: Enrich Wire Protocol — `DeltaResponse` + `EventRequest`/`EventResponse`

**Files:**
- Modify: `crates/node/primitives/src/sync/wire.rs:226-230`

This is the core wire protocol change. `DeltaResponse` gains an optional `events` field. Two new `MessagePayload` variants are added for the fallback event fetch.

- [ ] **Step 1: Add `events` field to `DeltaResponse`**

In `crates/node/primitives/src/sync/wire.rs`, replace the `DeltaResponse` variant (lines 226-230):

```rust
    /// Response to DeltaRequest containing the requested delta.
    DeltaResponse {
        /// The serialized delta data.
        delta: Cow<'a, [u8]>,
        /// Sidecar events for this delta (if available).
        /// None if the responder doesn't have events for this delta.
        events: Option<Cow<'a, [u8]>>,
    },
```

- [ ] **Step 2: Add `EventRequest` and `EventResponse` variants**

In the same enum (before the closing `}` of `MessagePayload`, after `NamespaceJoinRejected`):

```rust
    /// Request events for deltas that arrived without them.
    ///
    /// Sent when a sync peer's DeltaResponse had events=None.
    EventRequest {
        /// Delta IDs to fetch events for.
        delta_ids: Vec<[u8; 32]>,
    },

    /// Response containing events for requested deltas.
    EventResponse {
        /// (delta_id, serialized_events) pairs.
        /// Only includes entries where the responder has events.
        entries: Vec<([u8; 32], Vec<u8>)>,
    },
```

- [ ] **Step 3: Fix all compilation errors from the `DeltaResponse` field change**

The `events` field addition will break every pattern match on `DeltaResponse`. Find and fix all match sites:

**In `crates/node/src/sync/delta_request.rs`:**

Line ~200 (receiving DeltaResponse):
```rust
// Before:
payload: MessagePayload::DeltaResponse { delta },
// After:
payload: MessagePayload::DeltaResponse { delta, events: _events },
```

Lines ~286-288 and ~311-313 (constructing DeltaResponse):
```rust
// Before:
MessagePayload::DeltaResponse {
    delta: serialized.into(),
}
// After:
MessagePayload::DeltaResponse {
    delta: serialized.into(),
    events: None, // Will be populated in Task 6
}
```

Search for all other `DeltaResponse` pattern matches with:
```bash
rg "DeltaResponse" crates/ --type rust
```
Fix each match site to include the `events` field.

- [ ] **Step 4: Verify it compiles**

Run: `cargo check --workspace`
Expected: Compiles. All existing `DeltaResponse` sites now handle the new field.

- [ ] **Step 5: Commit**

```bash
git add crates/node/primitives/src/sync/wire.rs crates/node/src/sync/delta_request.rs
# Add any other files that needed DeltaResponse match fixes
git commit -m "feat(wire): add events to DeltaResponse, add EventRequest/EventResponse

Part of #2139 — wire protocol changes for sidecar event delivery.
DeltaResponse.events is Optional for backward compatibility.
EventRequest/EventResponse provide fallback when peer lacks events."
```

---

## Task 4: Add `handler_depth` to Broadcast Message

**Files:**
- Modify: `crates/node/primitives/src/sync/snapshot.rs:583-613`
- Modify: `crates/node/primitives/src/client.rs:233-288`
- Modify: `crates/context/src/handlers/execute/mod.rs` (broadcast call site)
- Modify: `crates/node/src/handlers/state_delta/mod.rs` (receive side)

This task adds handler chain depth tracking. Broadcasts carry `handler_depth` so receivers know whether to allow further handler cascades.

- [ ] **Step 1: Add `handler_depth` field to `BroadcastMessage::StateDelta`**

In `crates/node/primitives/src/sync/snapshot.rs`, add to the `StateDelta` variant (after `key_id`):

```rust
    /// Handler chain depth (0 = user-initiated, increments with each handler cascade).
    /// Handlers are suppressed when depth >= MAX_HANDLER_DEPTH.
    handler_depth: u8,
```

- [ ] **Step 2: Add `handler_depth` parameter to `broadcast()` function**

In `crates/node/primitives/src/client.rs`, modify the `broadcast()` signature (line 233) to add `handler_depth: u8` parameter, and include it in the `StateDelta` construction.

- [ ] **Step 3: Fix all call sites of `broadcast()`**

Search: `rg "\.broadcast\(" crates/ --type rust`

For each call site:
- In `crates/context/src/handlers/execute/mod.rs`: pass `0` for user-initiated executions
- In handler execution paths: pass `parent_depth + 1`

- [ ] **Step 4: Fix all pattern matches on `BroadcastMessage::StateDelta`**

Search: `rg "StateDelta\s*\{" crates/ --type rust`

Add `handler_depth` to each destructuring pattern. On the receive side in `crates/node/src/handlers/state_delta/mod.rs`, extract it and pass it through to the handler execution logic.

- [ ] **Step 5: Add depth check before handler execution**

In `crates/node/src/handlers/state_delta/mod.rs`, in the handler execution section (around line 388-397), add a depth check:

```rust
const MAX_HANDLER_DEPTH: u8 = 2;

if handler_depth < MAX_HANDLER_DEPTH && !is_author {
    execute_event_handlers_parsed(
        &node_clients.context,
        &context_id,
        &our_identity,
        payload,
    )
    .await?;
} else if handler_depth >= MAX_HANDLER_DEPTH {
    info!(
        %context_id,
        handler_depth,
        "Skipping handler execution (max depth {} reached)",
        MAX_HANDLER_DEPTH
    );
}
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo check --workspace`
Expected: Compiles with no errors.

- [ ] **Step 7: Commit**

```bash
git add crates/node/primitives/src/sync/snapshot.rs crates/node/primitives/src/client.rs \
       crates/context/src/handlers/execute/mod.rs crates/node/src/handlers/state_delta/mod.rs
git commit -m "feat(node): add handler_depth to broadcast for cascade depth limiting

Part of #2139 — prevents O(N^D) handler amplification in multi-node contexts.
Default MAX_HANDLER_DEPTH=2. Events still emit to WebSocket clients at any depth."
```

---

## Task 5: Persist Events to Sidecar on Gossipsub Receive

**Files:**
- Modify: `crates/node/src/handlers/state_delta/mod.rs:304-306`
- Modify: `crates/context/src/handlers/execute/mod.rs` (author-side persistence)

This task ensures events are persisted to the sidecar at every entry point: the author node before broadcast, and the gossipsub receiver upon receipt. This is what makes events available for sync later.

- [ ] **Step 1: Persist events on the author node before broadcast**

In `crates/context/src/handlers/execute/mod.rs`, where events are serialized before calling `broadcast()`, add sidecar persistence:

```rust
// After events are serialized and before broadcast() is called:
if let Some(ref events_data) = events {
    if let Some(delta_store) = node_state.delta_stores.get(&context_id) {
        if let Err(e) = delta_store.store_events(delta_id, events_data) {
            warn!(?e, %context_id, "Failed to persist events to sidecar on author node");
        }
    }
}
```

Find the exact location by searching for the `broadcast()` call in execute/mod.rs and add this before it.

- [ ] **Step 2: Persist events on the gossipsub receiver**

In `crates/node/src/handlers/state_delta/mod.rs`, right after `add_delta_with_events` is called (line 304-306), persist to the sidecar:

```rust
// After: let add_result = delta_store_ref.add_delta_with_events(delta, events.clone()).await?;
// Add:
if let Some(ref events_data) = events {
    if let Err(e) = delta_store_ref.store_events(delta_id, events_data) {
        warn!(?e, %context_id, "Failed to persist events to sidecar on receiver");
    }
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p calimero-node -p calimero-context`
Expected: Compiles.

- [ ] **Step 4: Commit**

```bash
git add crates/node/src/handlers/state_delta/mod.rs crates/context/src/handlers/execute/mod.rs
git commit -m "feat(node): persist events to sidecar on both author and receiver

Part of #2139 — ensures events are available in sidecar for sync peers to retrieve."
```

---

## Task 6: Enrich Sync DeltaResponse with Events from Sidecar

**Files:**
- Modify: `crates/node/src/sync/delta_request.rs:244-320` (`handle_delta_request`)

When a peer requests a delta, the responder now checks its sidecar and includes events if available. This is the "opportunistic" path — most of the time, the responder received the gossipsub broadcast and has events.

- [ ] **Step 1: Look up sidecar events when constructing DeltaResponse**

In `crates/node/src/sync/delta_request.rs`, in `handle_delta_request()`, modify both `DeltaResponse` construction sites.

For the RocksDB path (lines 263-288):
```rust
let response = if let Some(stored_delta) = handle.get(&db_key)? {
    let actions: Vec<calimero_storage::interface::Action> =
        borsh::from_slice(&stored_delta.actions)?;

    let causal_delta = CausalDelta {
        id: stored_delta.delta_id,
        parents: stored_delta.parents,
        actions,
        hlc: stored_delta.hlc,
        expected_root_hash: stored_delta.expected_root_hash,
    };

    let serialized = borsh::to_vec(&causal_delta)?;

    // Look up sidecar events for this delta
    let events_key = calimero_store::key::ContextDeltaEvents::new(context_id, delta_id);
    let sidecar_events = handle.get(&events_key)
        .ok()
        .flatten()
        .map(|e| Cow::from(e.events));

    debug!(
        %context_id,
        delta_id = ?delta_id,
        has_events = sidecar_events.is_some(),
        source = "RocksDB",
        "Sending requested delta to peer"
    );

    MessagePayload::DeltaResponse {
        delta: serialized.into(),
        events: sidecar_events,
    }
}
```

For the DeltaStore path (lines 289-313): similarly look up events from the sidecar store via `delta_store.get_events(delta_id)` and include them.

- [ ] **Step 2: Handle `EventRequest` messages**

Add a new handler method in `delta_request.rs`:

```rust
/// Handle incoming EventRequest from a peer
pub async fn handle_event_request(
    &self,
    context_id: ContextId,
    delta_ids: Vec<[u8; 32]>,
    stream: &mut Stream,
) -> Result<()> {
    let handle = self.context_client.datastore_handle();
    let mut entries = Vec::new();

    for delta_id in &delta_ids {
        let key = calimero_store::key::ContextDeltaEvents::new(context_id, *delta_id);
        if let Ok(Some(stored)) = handle.get(&key) {
            entries.push((*delta_id, stored.events));
        }
    }

    info!(
        %context_id,
        requested = delta_ids.len(),
        found = entries.len(),
        "Responding to EventRequest"
    );

    let response = StreamMessage::Message {
        payload: MessagePayload::EventResponse { entries },
        next_nonce: super::helpers::generate_nonce(),
    };

    super::stream::send(stream, &response, None).await
}
```

- [ ] **Step 3: Wire `EventRequest` into the stream dispatch**

Find where incoming `InitPayload` variants are dispatched (in `crates/node/src/sync/manager/mod.rs` or the stream handler). Add a match arm for `EventRequest` that calls `handle_event_request`.

If `EventRequest` needs a new `InitPayload` variant, add it to the init payload enum in `wire.rs` as well.

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p calimero-node`
Expected: Compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/node/src/sync/delta_request.rs crates/node/primitives/src/sync/wire.rs \
       crates/node/src/sync/manager/mod.rs
git commit -m "feat(sync): enrich DeltaResponse with sidecar events, handle EventRequest

Part of #2139 — sync responders include events from sidecar when available.
EventRequest/EventResponse provides fallback for peers without events."
```

---

## Task 7: Execute Handlers on Sync-Applied Deltas

**Files:**
- Modify: `crates/node/src/sync/delta_request.rs:113-131` (where `add_delta()` is called for fetched deltas)
- Modify: `crates/node/src/sync/manager/mod.rs` (where DAG heads deltas are applied)

This is the critical task — making the sync path execute handlers when events are available. Currently these paths call `add_delta()` which passes `None` for events. We change them to use `add_delta_with_events()` when events are available.

- [ ] **Step 1: Update the missing-delta fetch loop in `delta_request.rs`**

In `crates/node/src/sync/delta_request.rs`, the delta fetch loop (around line 80-131) currently receives `DeltaResponse { delta }` and calls `delta_store.add_delta(dag_delta)`. Update it to:

1. Extract `events` from the new `DeltaResponse { delta, events }` field
2. If events are present, persist them to sidecar and call `add_delta_with_events()`
3. After delta is applied, execute handlers

```rust
// In the match arm for DeltaResponse (around line 85-131):
Ok(Some(StreamMessage::Message {
    payload: MessagePayload::DeltaResponse { delta, events },
    ..
})) => {
    let parent_delta: CausalDelta = borsh::from_slice(&delta)?;
    
    // ... existing parent traversal logic ...

    let dag_delta = calimero_dag::CausalDelta {
        id: parent_delta.id,
        parents: parent_delta.parents,
        payload: parent_delta.actions,
        hlc: parent_delta.hlc,
        expected_root_hash: parent_delta.expected_root_hash,
        kind: calimero_dag::DeltaKind::Regular,
    };

    let events_data = events.map(|e| e.into_owned());

    // Persist events to sidecar if available
    if let Some(ref ev) = events_data {
        if let Err(e) = delta_store.store_events(parent_delta.id, ev) {
            warn!(?e, %context_id, "Failed to store sync'd events in sidecar");
        }
    }

    // Use add_delta_with_events when events are available
    if let Err(e) = delta_store.add_delta_with_events(dag_delta, events_data.clone()).await {
        warn!(?e, %context_id, delta_id = ?missing_id, "Failed to persist fetched delta to DAG");
        continue;
    }
}
```

- [ ] **Step 2: Add handler execution after sync delta application**

After the delta fetch loop completes and deltas are applied, check which deltas have events and execute handlers. This requires extracting the `our_identity` for this context and calling `execute_event_handlers_parsed`.

Look at how `handle_state_delta()` does it (state_delta/mod.rs:376-404) and replicate the pattern for sync-applied deltas.

- [ ] **Step 3: Update DAG heads handling in `sync/manager/mod.rs`**

Find where `add_delta()` is called for DAG-heads deltas (around line 1705 based on the exploration). Apply the same pattern: check sidecar for events, use `add_delta_with_events()`.

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p calimero-node`
Expected: Compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/node/src/sync/delta_request.rs crates/node/src/sync/manager/mod.rs
git commit -m "feat(sync): execute handlers when sync-applied deltas have events

Part of #2139 — the core fix. Sync path now uses add_delta_with_events()
and executes handlers when events are available from sidecar or DeltaResponse."
```

---

## Task 8: Add EventRequest Fallback in Sync Path

**Files:**
- Modify: `crates/node/src/sync/delta_request.rs`

When a sync peer's `DeltaResponse` has `events: None`, the requesting node should try to fetch events from other context peers before applying.

- [ ] **Step 1: Track deltas that arrived without events**

In the delta fetch loop, collect delta IDs that had `events: None`:

```rust
let mut deltas_missing_events: Vec<[u8; 32]> = Vec::new();

// In the DeltaResponse match arm:
if events_data.is_none() {
    deltas_missing_events.push(parent_delta.id);
}
```

- [ ] **Step 2: After fetch loop, send EventRequest to other peers**

After the delta fetch loop completes:

```rust
if !deltas_missing_events.is_empty() {
    info!(
        %context_id,
        count = deltas_missing_events.len(),
        "Requesting events for deltas that arrived without them"
    );

    // Send EventRequest on the existing stream
    let event_req = StreamMessage::Message {
        payload: MessagePayload::EventRequest {
            delta_ids: deltas_missing_events.clone(),
        },
        next_nonce: super::helpers::generate_nonce(),
    };

    if let Ok(()) = super::stream::send(stream, &event_req, None).await {
        let timeout_budget = self.sync_config.timeout;
        if let Ok(Some(StreamMessage::Message {
            payload: MessagePayload::EventResponse { entries },
            ..
        })) = super::stream::recv(stream, None, timeout_budget).await
        {
            for (delta_id, events_data) in entries {
                if let Err(e) = delta_store.store_events(delta_id, &events_data) {
                    warn!(?e, %context_id, "Failed to store fallback events");
                }
                // Execute handlers for this delta now
                // (delta already applied, just need handler execution)
            }
        }
    }
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p calimero-node`
Expected: Compiles.

- [ ] **Step 4: Commit**

```bash
git add crates/node/src/sync/delta_request.rs
git commit -m "feat(sync): add EventRequest fallback for deltas arriving without events

Part of #2139 — when DeltaResponse has events=None, request events
from the same peer via EventRequest/EventResponse before executing handlers."
```

---

## Task 9: Remove the "Handlers Will NOT Execute" Warning

**Files:**
- Modify: `crates/node/src/handlers/state_delta/mod.rs:406-411`

Now that events are persisted and sync carries them, the warning about handlers not executing for buffered deltas is no longer accurate. Update it.

- [ ] **Step 1: Update the warning to reflect new behavior**

Replace lines 406-411:

```rust
// Before:
} else if !applied && events_payload.is_some() {
    warn!(
        %context_id,
        delta_id = ?delta_id,
        "Delta with events buffered as pending - handlers will NOT execute when delta is applied later!"
    );
}

// After:
} else if !applied && events_payload.is_some() {
    info!(
        %context_id,
        delta_id = ?delta_id,
        "Delta with events buffered as pending - events persisted to sidecar for later handler execution"
    );
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/node/src/handlers/state_delta/mod.rs
git commit -m "fix(node): update stale warning about handlers not executing for pending deltas

Part of #2139 — events are now persisted to sidecar, so handlers will
execute when the delta is later applied via sync."
```

---

## Task 10: Integration Testing — Build and Run Existing Tests

**Files:**
- Verify: `crates/node/tests/` (existing integration tests)

Before touching CI workflows, verify the implementation doesn't break existing tests.

- [ ] **Step 1: Run the full workspace build**

Run: `cargo build --workspace`
Expected: Builds with no errors.

- [ ] **Step 2: Run the full test suite**

Run: `cargo test --workspace`
Expected: All existing tests pass. New code paths are exercised when events flow through sync.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace -- -A warnings`
Expected: No new warnings from our changes.

- [ ] **Step 4: Run fmt check**

Run: `cargo fmt --check`
Expected: No formatting issues.

- [ ] **Step 5: Commit any fixes**

If any test failures or lint issues are found, fix them and commit:

```bash
git commit -m "fix: address test/lint issues from event sidecar implementation"
```

---

## Task 11: Update CI Workflow — Fuzzy Test Threshold

**Files:**
- Modify: `workflows/fuzzy-tests/kv-store-with-handlers/fuzzy-test.yml:69`

The fuzzy test's success threshold was set to 95% to accommodate handler failures. With the fix, handler execution should be reliable.

- [ ] **Step 1: Review the current threshold**

The current threshold at line 69:
```yaml
success_threshold: 95.0
```

Keep this threshold as-is initially. After the sidecar is deployed and validated in CI, a follow-up can raise it to 98.0 or higher. The fix makes handlers reliable, but the threshold should be validated empirically first.

- [ ] **Step 2: Add a comment explaining the threshold**

```yaml
    success_threshold: 95.0  # Can be raised after #2139 sidecar event store is validated in CI
```

- [ ] **Step 3: Commit**

```bash
git add workflows/fuzzy-tests/kv-store-with-handlers/fuzzy-test.yml
git commit -m "docs(ci): annotate fuzzy test threshold for post-#2139 adjustment"
```

---

## Task 12 (Deferred): Event Sidecar Garbage Collection

GC is specified in the design spec but deferred from this implementation round. The sidecar store will grow slowly (events are small, typically < 1KB per delta). Once the core fix is validated in CI, a follow-up PR should implement:

1. GC detection via DAG head comparison in sync heartbeats
2. Offline node timeout (24h default)
3. Hard retention cap (7 days)
4. `gc_events()` call on heartbeat cycle in `sync/manager/mod.rs`

The `delete_events()` method (Task 2) is already in place for when GC is added.

---

## Task 13: Final Verification and Cleanup

**Files:**
- All modified files

- [ ] **Step 1: Run full CI validation locally**

```bash
cargo fmt --check
cargo clippy --workspace -- -A warnings
cargo test --workspace
cargo deny check licenses sources  # Only if deps changed
```

- [ ] **Step 2: Verify the spec is still accurate**

Read `docs/superpowers/specs/2026-04-13-reliable-event-handler-sync-design.md` and confirm implementation matches all design decisions:
- [ ] Sidecar event store using `ContextDeltaEvents` key type
- [ ] `DeltaResponse` has optional `events` field
- [ ] `EventRequest`/`EventResponse` for fallback
- [ ] `handler_depth` on `BroadcastMessage::StateDelta`
- [ ] Events persisted on author and receiver
- [ ] Sync path uses `add_delta_with_events()` with sidecar events
- [ ] Handler execution atomic with delta application
- [ ] Warning updated to reflect new behavior

- [ ] **Step 3: Commit any final fixes**

```bash
git commit -m "chore: final cleanup for event sidecar implementation (#2139)"
```

- [ ] **Step 4: Push and update the PR**

```bash
git push origin spec/reliable-event-handler-sync
```
