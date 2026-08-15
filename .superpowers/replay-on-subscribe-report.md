# Replay-on-subscribe — report

Branch `feat/ephemeral-presence-rebased`, base `3eafbd3b`, HEAD `6ff71bd0`.

| # | SHA | Subject |
|---|-----|---------|
| 1 | `dca929d2` | feat(primitives): carry optional age on the ephemeral payload |
| 2 | `0e001d5e` | feat(server): replay current presence when a client subscribes |
| 3 | `66e57057` | refactor(server)!: delete the get_ephemeral JSON-RPC method |
| 4 | `6ff71bd0` | test(workflows): observe presence on the event stream, not a snapshot RPC |

22 files, +1636 / −961.

---

## 1. Per-connection delivery mechanism

The requirement is that a subscriber's seed reaches **that connection only**. Each
transport already owns a per-connection sink; the replay writes to it directly and
never touches `NodeClient::send_event` (the node-wide broadcast).

The payload construction is shared — `crates/server/src/ephemeral_replay.rs`,
`presence_replay(node_client, context_id) -> Vec<NodeEvent>` — so both transports
emit an identical wire shape. Only delivery differs.

### WebSocket

`ConnectionState` already holds `commands: mpsc::Sender<Command>`, the channel
`handle_commands` drains into the socket. Added
`ConnectionState::try_push_event(&NodeEvent)` (`crates/server/src/ws.rs`), which
serializes the event and `try_send`s `Command::Send(Response { id: None, body })`
— the same envelope shape (`id: None`) the broadcast fan-out uses, so a replayed
event is indistinguishable from a live one at the envelope level.

`try_send`, not `send`: a slow client's full channel drops the seed with a
`debug!` instead of blocking the subscribe handler, matching the skip-on-lag
contract `fan_out_node_events` already has.

The subscribe handler (`crates/server/src/ws/subscribe.rs`) is a child module of
`crate::ws`, so it reaches the private field through the new method without
widening any visibility.

### SSE

Structurally harder, and worth spelling out: SSE splits subscribe (a `POST
/sse/subscription`) from the stream (a long-lived `GET /sse`). They are two
different requests, and the POST handler had no handle on the connection it needs
to seed. It was **not** infeasible, though — the session already tracks the
current connection's *event task* (`SessionState::bind_event_task`, which aborts a
superseded task on reconnect). The connection's sink is exactly the same kind of
per-connection fact, so it binds in the same place.

- `SessionState` gained `connection: Arc<Mutex<Option<mpsc::WeakSender<Command>>>>`.
- `bind_event_task` is now private, wrapped by `bind_connection(abort_handle, sink)`,
  which sets both together — the sink and the task can never describe different
  connections.
- `sse_handler` passes `commands_sender.downgrade()` before moving the strong
  sender into the event task.
- `SessionState::try_push(Response) -> bool` upgrades the weak sender and
  `try_send`s.

**Weak, not strong, on purpose.** A session outlives its connections. A strong
clone parked on the session would keep a dead connection's channel and its
buffered frames alive until the next connection replaced it, and would make "is
there a live connection?" unanswerable. A failed upgrade means precisely "no live
connection to seed", which is the right answer. Two tests pin this
(`a_session_with_no_live_connection_is_not_seeded`,
`rebinding_a_session_redirects_the_seed_to_the_new_connection`).

Neither transport required a broadcast fallback.

### One thing I added that was not in the brief

`presence_replay` bounds the snapshot read at **2s**
(`SNAPSHOT_TIMEOUT`). The subscribe handler awaits it inline, and
`NodeClient::ephemeral_snapshot` goes through `LazyRecipient`, which **queues
messages until the actor is initialized** — an uninitialized or saturated node
actor would otherwise hang the subscribe handler forever, so the client would get
neither an acknowledgment nor a live stream, over an optional seed. This is not
hypothetical: it is exactly what happened to two pre-existing WS tests the moment
the snapshot read was added. Anything slower than 2s is worthless as a seed
anyway, since presence republishes every 2.5s.

---

## 2. Ordering: subscribe first, snapshot second

```
1. authorize (caller_may_observe_context)  — unchanged gate
2. insert into `subscriptions` (write lock)  → live deltas start reaching this connection
3. read the snapshot (no lock held)
4. push the replay events onto this connection's sink
```

**Why this direction.** The two failure modes are not symmetric:

- *Snapshot-then-subscribe* **loses** a delta landing in the window: it is in
  neither the already-read snapshot nor the not-yet-live stream. Since a heartbeat
  that re-sends identical bytes produces **no diff** (`AwarenessStore::apply`
  returns `None` when the slice is unchanged), a lost delta leaves the client
  stale until that author's slice actually *changes* — which may be never.
- *Subscribe-then-snapshot* can only **duplicate**. A delta delivered in the window
  is followed by a replayed entry that is at least as new, because the node writes
  the awareness store **before** emitting the diff (`handlers::ephemeral::inbound`
  calls `apply`, then `emit_ephemeral_diff` on its result). Presence is
  last-writer-wins idempotent state, so applying the replay after the delta is
  correct.

I chose the direction that cannot lose data.

**Residual, stated honestly.** Between step 3 (snapshot response received) and step
4 (enqueue) there are no awaits, but the window is not zero. A delta emitted inside
it could be enqueued by the fan-out task *ahead* of the replay, in which case the
client's last message is a replay carrying a value one delta old. Closing this
completely would require the subscribe handler to hold the per-connection lock
across the actor round-trip while the fan-out sends under that same lock — which
would let one stalled node actor head-of-line-block event delivery to every
connected client. That trade is not worth it for transient presence with a 2.5s
republish and a 7s TTL. Documented in `ephemeral_replay.rs`.

**Test.** `ws::tests::a_delta_landing_during_the_snapshot_read_is_not_lost`. The
stubbed node actor broadcasts a delta *while serving the snapshot request* — an
instant only reachable once the subscription is live — and the subscriber must end
up with both the delta and the seed. Verified falsifiable: moving the replay block
above the subscription insert makes it fail (`0 passed; 1 failed`), and it passes
again when restored.

---

## 3. Payload

`EphemeralPayload` gained `age_ms: Option<u64>` with
`#[serde(default, skip_serializing_if = "Option::is_none")]`. Absent (not `null`)
on live deltas, present on replayed entries. Its presence is itself the "this is
replayed state" signal, and `default` keeps every pre-existing decoder working.

`emit_ephemeral_diff` passes `None` on both arms; `presence_replay` passes
`Some(age_ms)` from `ephemeral_snapshot`'s `(author, slice, age_ms)` tuple.

---

## 4. `get_ephemeral` deletion

Removed: `crates/server/src/jsonrpc/get_ephemeral.rs` (293 lines), its `mod`
registration, the `RequestPayload::GetEphemeral` dispatch arm and variant, and
`GetEphemeralRequest` / `GetEphemeralResponse` / `GetEphemeralError` /
`EphemeralEntryValue` plus the `Validate` impl and the now-unused `BTreeMap`
import from `crates/server/primitives/src/jsonrpc.rs`.

Kept: `set_ephemeral` (SSE is server→client only, so publishing still needs a
request path) and `NodeClient::ephemeral_snapshot`, now documented as an internal
accessor backing the replay rather than an RPC-backed read.

The author-keyed map and the mandatory `ageMs` went with `GetEphemeralResponse`;
the surviving shape is the single `EphemeralPayload`.

**Cross-repo:** `calimero-server-primitives` is `publish = true` and pinned by
mero-tee at rev `1a418f46`. This is a **breaking change to its public surface**.
mero-tee consumes only `Quote`-related items from that crate, so a rev bump should
be mechanical — but per the repo contract this needs flagging rather than assuming.

### Collateral refactor (folded into commits 2/3)

- The uncommitted WIP already on the worktree extracted
  `caller_may_observe_context` as the shared WS/SSE gate. Kept and folded into
  commit 2, with its doc updated to stop naming `get_ephemeral`. Its dead sibling
  `CallerIdentity::observe_parts` (never called; would have been the get_ephemeral
  caller) was deleted per the no-dead-code rule.
- `stub_node_manager` / `seed_context_member` moved from `jsonrpc/test_support.rs`
  to a new crate-level `crates/server/src/test_support.rs`, since WS and SSE tests
  now need the same node stub. `test_node_client` was extracted there too, and
  `jsonrpc`'s `state_with` now uses it.

---

## 5. Tests added

`crates/primitives/src/events.rs` (3): live delta omits `ageMs`; replay carries it
camelCase; a payload without the field still deserializes.

`crates/server/src/ws.rs` (4):
- `presence_replay_reaches_only_the_subscribing_connection` — A is seeded, then B
  subscribes and is seeded; **A receives nothing**.
- `replayed_entry_carries_age_and_a_live_delta_does_not` — seed has `ageMs: 4200`,
  an injected broadcast delta has no `ageMs` key at all.
- `unauthorized_caller_receives_no_presence_replay` — auth on, non-member: the
  subscribe ack is the first frame, `contextIds: []`, and nothing follows.
- `a_delta_landing_during_the_snapshot_read_is_not_lost` — the ordering test above.

`crates/server/src/sse/handlers.rs` (4): seed reaches only the subscribing session;
replayed entry carries age; a session with no live connection is not seeded;
rebinding redirects the seed to the new connection.

Two pre-existing WS tests (`subscribe_and_unsubscribe_round_trip`,
`events_only_reach_subscribers`) moved from `#[tokio::test]` to `#[actix::test]`
with an empty-snapshot stub, because a context subscription now reads the snapshot.

---

## 6. e2e rework

`get_ephemeral` polling is gone, and there is nothing to poll in its place —
presence is observable only by subscribing. Assertions moved to Node scripts
driving real WS/SSE subscribers, with one-line `.sh` wrappers (`exec node
"$(dirname "$0")/x.js" "$@"`) because merobox runs script steps through `sh`. Node
24's global `WebSocket` means no dependency to install.

- `workflows/scripts/ephemeral-lib.js` — shared helpers: `rpc`, `readContext`,
  `subscribe` (WS), `subscribeSse`, and a `die()` that aborts on transport failure.
- `workflows/scripts/ephemeral-presence-e2e.js` — phases 0–4.
- `workflows/scripts/ephemeral-ttl-check.js` — TTL eviction.
- `.sh` files reduced to wrappers; `.yml` description and step comments updated.

**Delivery** is now asserted the way it works: subscribe first, publish second,
await the event — and match specifically on the *live* shape (`ageMs === undefined`)
so a replayed entry cannot satisfy it.

**Replay** (new): a second subscriber connecting after the publish must be seeded,
with `ageMs` inside the TTL window — checked over WS *and* over SSE (the transport
mero-js uses). Plus the per-connection property: the Phase-2 watcher, subscribed
long before and with nothing published since, must see **nothing** when the
latecomer subscribes.

**TTL eviction** is read off a fresh subscriber's seed, which is exactly the
awareness store's remaining contents — the replay gives back the observability
`get_ephemeral` provided. Vacuity guard: the check also requires node 2's **own**
entry to be in that same seed, so a broken subscription (empty seed) fails instead
of passing as an eviction.

**No-DAG-growth guard** is unchanged in substance and still load-bearing: a real
`kv-store` `execute(set)` on node 1 establishes a **NON-NULL** baseline
(asserted `!= 11111111111111111111111111111111`), and node 1's `contextStateHash`
must be byte-identical after `set_ephemeral`.

Transport failures abort with `FATAL` + exit 1 rather than degrading into an empty
result that reads as a pass — the discipline from `3eafbd3b` is preserved.

---

## 7. Commands and output

### Workflow (real 2-node run, native merod built from this tree)

```
$ cargo build --release -p merod          # exit 0
$ PATH="$PWD/target/release:$PATH" merobox bootstrap run workflows/ephemeral-presence-e2e.yml
```

```
=== ephemeral-presence-e2e assertions ===
-- Phase 0: context reachable on both nodes --
ok   node1 context API returns the context (got: "2orJs8h9MjAWHBeEb7PN7EJ3KGHBvRQnQL1nZB7E2fsZ")
ok   node2 context API returns the context (got: "2orJs8h9MjAWHBeEb7PN7EJ3KGHBvRQnQL1nZB7E2fsZ")
-- Phase 1: advance node 1 DAG with a real kv-store write --
ok   kv-store execute(set) on node 1 succeeded (no RPC error)
ok   node1 contextStateHash is NON-NULL after kv write — DAG advanced (got: BM4XyjFjVdbnNJmSULMqG1nSxHLfoLb8aQxSQAo2bSUP)
-- Phase 2: a subscriber on node 2 receives node 1 presence (gossip) --
ok   node 2 acknowledged the WS subscription
ok   set_ephemeral on node 1 succeeded (no RPC error)
ok   node 2 subscriber received node 1 presence over gossip
ok   live delta state equals the published slice (got: [1,2,3])
ok   live delta author is node 1 (got: "ADqXhh7JA3VUv9i5gzj8GQtKJNc3NwdNRwzM6mv6aZkf")
ok   live delta omits ageMs (absent, not null) — it is fresh at emission
ok   live delta is an upsert (removed absent-or-false)
-- Phase 3: a NEW subscriber is seeded with the existing presence --
ok   a subscriber joining an already-populated context is seeded with the entry
ok   replayed entry state equals the published slice (got: [1,2,3])
ok   replayed entry author is node 1 (got: "ADqXhh7JA3VUv9i5gzj8GQtKJNc3NwdNRwzM6mv6aZkf")
ok   replayed entry carries an ageMs inside the TTL window (got: 1)
ok   another client's seed did not reach the already-connected subscriber
-- Phase 3b: SSE subscriber (production transport) is seeded too --
ok   SSE subscriber was seeded with the existing presence
ok   SSE seed carries ageMs (got: 173)
-- Phase 4: no-DAG-growth guard (node 1, falsifiable) --
  node1 hash before set_ephemeral (NON-NULL baseline): BM4XyjFjVdbnNJmSULMqG1nSxHLfoLb8aQxSQAo2bSUP
  node1 hash after  set_ephemeral                    : BM4XyjFjVdbnNJmSULMqG1nSxHLfoLb8aQxSQAo2bSUP
ok   no DAG growth: node1 contextStateHash unchanged by set_ephemeral (LOAD-BEARING)

=== 19 passed, 0 failed ===
```

```
=== ephemeral-ttl-check assertions ===
ok   node 2 acknowledged the WS subscription
ok   node 2's own presence is in the seed — the seed is non-empty, so the check below is not vacuous
  entries in the seed: [{"author":"FnbYgk2k94H6...","state":[9,8,7],"ageMs":545,...}]
ok   node 1 entry evicted from node 2 awareness after TTL (PRESENCE_TTL_MS=7000ms) (got: 0 remaining)

=== 3 passed, 0 failed ===

🎉 Workflow '...' completed successfully!     # exit 0
```

Both scripts were also run against a dead endpoint to confirm the transport-failure
path: `FATAL ... fetch failed`, exit 1.

### Rust

```
$ cargo check --workspace
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.06s     # clean

$ cargo test --workspace --no-run
    Finished                                                                # clean

$ cargo test --workspace
TOTAL passed=4816 failed=0 ignored=77        (aggregated over all test binaries)
0 binaries reported FAILED; no `failures:` block anywhere in the log

$ cargo clippy --workspace --all-targets
   3 warning: very complex type used ...            (calimero-node / node-primitives)
   2 warning: this function has too many arguments (9/7)
   1 warning: this function has too many arguments (8/7)
   1 warning: this lint expectation is unfulfilled  (node-primitives/sync/snapshot.rs)
   1 warning: doc list item without indentation     (node/handlers/ephemeral/outbound.rs)
   → zero warnings in calimero-server / calimero-server-primitives / calimero-primitives.
   → all remaining warnings are pre-existing; the `doc list item` one was verified
     by reverting outbound.rs to HEAD~1 and re-running clippy (still 1).

$ rustup run 1.88.0 cargo fmt --check
FMT CLEAN
```

The two known flakes (`autonat_v2::tests::test_dynamic_protocol_change`,
`macros::macros_tests::test`) did not fire in this run.

---

## 8. Concerns

1. **Cross-repo, needs a decision.** Deleting the `GetEphemeral*` types breaks
   `calimero-server-primitives`' public surface. mero-tee pins rev `1a418f46` and
   uses only the `Quote` side, so a bump should be mechanical — but it is a
   coordinated change, not mine to make.
2. **Replay arrives before the subscribe ack on WS.** The seed is enqueued inside
   the subscribe handler, and the ack is enqueued by the message loop after it
   returns. Clients that discriminate on `id` (events are `id: null`) are fine —
   both the new e2e client and the mero-js shape handle it — but a client that
   ignores frames until it sees its ack would drop the seed. Emitting after the ack
   would need a hook the `mount_method!` macro does not have. Flagging rather than
   reworking the macro.
3. **The ordering residual in §2.** Narrow, self-correcting on the next *change*
   to a slice, but not on an unchanged heartbeat. If it ever matters, the fix is a
   sequence number on `EphemeralPayload` so clients can LWW-dedupe — deliberately
   not added, since the brief asked for `age_ms` only.
4. **No per-context replay cap.** A context with many peers seeds one frame per
   author into a 32-slot command channel; beyond that the tail is dropped
   (`debug!`) and the client converges on later deltas. Fine at presence scale,
   worth remembering if slices get big or peers numerous.
5. **The workflow still carries the pre-existing `wait_for_sync` note** about
   snapshot sync returning `applied_records=0` on this branch. Unrelated to this
   change; the DAG guard is unaffected (it compares node 1 against itself).
