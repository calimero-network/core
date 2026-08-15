# Ephemeral presence demo — live cursors on `apps/collaborative-editor`

A runnable demo, not a test. Two real `merod` nodes share one context of the
**unmodified** `apps/collaborative-editor` app; each node runs a terminal client
that publishes a cursor position and watches everyone else's move in real time.

It exists to make four things visible on screen:

| # | Property | What you watch |
|---|---|---|
| 1 | **Live cursors** | Two clients on two nodes, each seeing the other's cursor move |
| 2 | **Seed-on-subscribe** | A late client is handed the cursors that already exist, tagged `SEED` with the server's `ageMs` — no waiting for the next movement |
| 3 | **TTL eviction** | Stop a node and its cursor disappears within `PRESENCE_TTL_MS` (7 s) — nothing said goodbye |
| 4 | **No DAG growth** | `contextStateHash` sits still through thousands of cursor updates, then jumps the instant a real editor write lands |

## Why there is no presence code in the app

A WASM app **cannot** use ephemeral presence, and does not need to. There is no
SDK surface for it and the node never invokes WASM on the presence path — that
is the entire point: no WASM run and no DAG op per cursor move. Presence is a
**client-side** capability:

* **publish** — JSON-RPC `set_ephemeral` (`{ contextId, state: <bytes> }`)
* **receive** — the node's WS (or SSE) event stream, which replays the context's
  current presence to a new subscriber and then streams live deltas

`state` is opaque bytes. This demo puts `{"v":1,"name":"alice","line":3,"col":17}`
in there; a real editor would put a selection range or a Yjs awareness frame.
`apps/collaborative-editor` is untouched and knows nothing about any of it.

## Prerequisites

Node 24 (global `WebSocket` + `fetch`; **no npm install, no dependencies**),
`merobox`, and a debug build of `merod`.

Build the binaries (single line):

```
cargo build -p merod -p meroctl
```

Build the app's WASM (single line):

```
cargo build -p collaborative-editor --target wasm32-unknown-unknown --profile app-release
```

## 1. Bring it up (one command)

Run every command below from the repo root.

```
merobox bootstrap run tools/ephemeral-presence-demo/workflow.yml --binary-path "$PWD/target/debug/merod"
```

That boots two native nodes (RPC on `:8940` and `:8941`), installs
`collaborative-editor`, creates a namespace + context, joins node 2, waits for
the gossip mesh, and **leaves both nodes running**. It writes the per-run ids to
`tools/ephemeral-presence-demo/.demo-env` and prints the commands below with the
real context id filled in.

## 2. Open three terminals

In **each** terminal, first load the ids:

```
source tools/ephemeral-presence-demo/.demo-env
```

Terminal 1 — alice, on node 1:

```
node tools/ephemeral-presence-demo/cursor-client.js --node "$NODE1_URL" --context "$CONTEXT_ID" --name alice --label node-1
```

Terminal 2 — bob, on node 2:

```
node tools/ephemeral-presence-demo/cursor-client.js --node "$NODE2_URL" --context "$CONTEXT_ID" --name bob --label node-2
```

**Property 1 is now on screen**: both terminals show both cursors moving, across
two nodes, over gossip.

Terminal 3 — a late watcher. Start it **after** the other two have been running
for a while:

```
node tools/ephemeral-presence-demo/cursor-client.js --node "$NODE2_URL" --context "$CONTEXT_ID" --watch --name watcher --label node-2
```

**Property 2**: its first two feed lines are `SEED`, not `LIVE`, and each carries
the server's `ageMs`:

```
SEED  alice was ALREADY present when we subscribed — cursor 2:24, ageMs=297 (0.3s old, TTL 7000ms)
SEED  bob   was ALREADY present when we subscribed — cursor 1:57, ageMs=31  (0.0s old, TTL 7000ms)
```

`ageMs` is the tell, and it comes from the node, not from a timer here: it is
*absent* on a live delta and present *only* on the replay a subscriber is seeded
with. The roster marks those rows `SEED` until the peer next moves.

## 3. Property 4 — no DAG growth

Watch the `CONTEXT STATE HASH` panel while the cursors fly. It reads
`contextStateHash` straight from the node's admin API once a second, and it does
not move — the counter next to it says how many presence events went by in the
meantime.

Then press **`w`** in any publishing terminal. That runs a real application
write, `execute(insert_text, position=0, text="a")`, and the hash moves within
milliseconds:

```
DAG   editor write: execute(insert_text, position=0, text="a") — hash before: 79qVVrGe4oeauaam5YsmyotuB8W1WjeieCv2pd15BTR6
DAG   contextStateHash MOVED  79qVVrGe4oea… -> DadQ79zK6sB4…
DAG   hash after  editor write: DadQ79zK6sB4X2r29rEFLLBSmNetbf9bNnw8FkqzhmTt  (moved — a real op WAS appended)
```

The panel is falsifiable in both directions: it stays put for cursors, it moves
for a write, and an unreachable node prints `UNREADABLE` rather than a
comfortable-looking frozen hash.

## 4. Property 3 — TTL eviction

**Presence belongs to the node, not to your client socket.** The node keeps
heartbeating a slice you set (every `PRESENCE_HEARTBEAT_MS` = 2.5 s) for as long
as the node itself is alive, so quitting a cursor client does **not** retract its
cursor — the other side keeps seeing it, frozen where you left it. To watch an
eviction you have to stop the peer's *node*:

```
merobox stop presence-demo-node-2 --no-docker
```

Within ~7 s (`PRESENCE_TTL_MS`) the surviving terminal logs, unprompted:

```
GONE  bob evicted by the node after PRESENCE_TTL_MS=7000ms (removed=true, author=Ha7BKPAsXT…) — no goodbye was ever sent
```

Nothing was sent on bob's behalf. Node 1 simply stopped hearing from him and
swept the entry, emitting `removed: true` to its subscribers.

Stopping a node is the end of that node's run, so start the demo over when you
want node 2 back (the bring-up nukes and rebuilds from scratch, which takes
about a minute):

```
merobox bootstrap run tools/ephemeral-presence-demo/workflow.yml --binary-path "$PWD/target/debug/merod"
```

## 5. Tear down

```
merobox nuke
```

## Client options

| Flag | Meaning |
|---|---|
| `--node <url>` | Node RPC base URL (`http://localhost:8940`) |
| `--context <id>` | Context to join (from `.demo-env`) |
| `--name <label>` | Name shown in the roster and embedded in the cursor payload |
| `--watch` | Subscribe only; publish nothing. Best for showing the seed |
| `--label <text>` | Cosmetic node label in the header |
| `--write-after <s>` | Fire the editor write automatically after N seconds |
| `--duration <s>` | Exit after N seconds |
| `--plain` | Line-oriented log instead of the full-screen UI (also automatic when stdout is not a TTY, so `| tee` works) |

Keys: **`w`** real editor write, **`q`** quit.

## One caveat about identity

Presence is keyed by the **node's context member key**, so there is exactly one
presence entry per node per context. Two publishing clients pointed at the same
node will overwrite each other's cursor. Run one publisher per node (plus as many
`--watch` clients as you like).

## Related

* `workflows/ephemeral-presence-e2e.yml` — the e2e that *asserts* these
  properties (delivery, replay-on-subscribe over WS and SSE, per-connection seed
  delivery, no-DAG-growth, TTL eviction).
* `workflows/scripts/ephemeral-transport.js` — the WS/SSE + JSON-RPC client
  shared by that e2e and this demo, so both talk to the node the same way.
