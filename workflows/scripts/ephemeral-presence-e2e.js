#!/usr/bin/env node
// ephemeral-presence-e2e.js — assertions for the ephemeral presence e2e.
//
// Driven by merobox as a `script` step (target: local) through the .sh wrapper
// next to this file. Positional args mirror the old shell script:
//   $1 node1_url   $2 node2_url   $3 context_id   $4 node1_key
//
// Presence has no read endpoint: a client learns presence only by subscribing
// to the event stream, which seeds it with the context's current entries and
// then streams live deltas. Every assertion below is therefore made over a real
// subscriber socket.
//
// What this proves:
//   1. DELIVERY  — node 1 publishes, a subscriber on node 2 receives it (gossip).
//   2. REPLAY    — a subscriber that connects AFTER the fact is seeded with the
//                  existing entry, carrying `ageMs`. This is the behaviour that
//                  replaced the get_ephemeral RPC.
//   3. SHAPE     — a live delta omits `ageMs`; a replayed entry carries it. That
//                  one field is how a client tells the two apart.
//   4. NO DAG    — node 1's contextStateHash is unchanged by the presence write,
//                  measured against a NON-NULL baseline so it is falsifiable.
//
// Auth: nodes run in Proxy mode (default), so no Bearer token is required.

import { bad, check, die, ok, readContext, rpc, sleep, subscribe, subscribeSse, summarize, tally } from './ephemeral-lib.js';

const NODE1_URL = process.argv[2] || process.env.NODE1_URL || 'http://localhost:8930';
const NODE2_URL = process.argv[3] || process.env.NODE2_URL || 'http://localhost:8931';
const CONTEXT_ID = process.argv[4] || process.env.CONTEXT_ID || '';
const NODE1_KEY = process.argv[5] || process.env.NODE1_KEY || '';

// base58 of [0u8; 32] — the genesis / "nothing has been written" state hash.
const NULL_HASH = '11111111111111111111111111111111';
// PRESENCE_TTL_MS in calimero-node: no replayed entry may claim to be older.
const PRESENCE_TTL_MS = 7000;

const SLICE = [1, 2, 3];

console.log('=== ephemeral-presence-e2e assertions ===');
console.log(`  node1_url   : ${NODE1_URL}`);
console.log(`  node2_url   : ${NODE2_URL}`);
console.log(`  context_id  : ${CONTEXT_ID}`);
console.log(`  node1_key   : ${NODE1_KEY}`);

if (!CONTEXT_ID) die('CONTEXT_ID is empty — was the create_context step output captured?');

// --- Phase 0: both nodes have the context ----------------------------------

console.log('\n-- Phase 0: context reachable on both nodes --');

let ctx1;
let ctx2;
try {
  ctx1 = await readContext(NODE1_URL, CONTEXT_ID);
  ctx2 = await readContext(NODE2_URL, CONTEXT_ID);
} catch (e) {
  die('context API responds on both nodes', `${e.message} — downstream assertions would be meaningless`);
}

check('node1 context API returns the context', CONTEXT_ID, ctx1?.data?.id);
check('node2 context API returns the context', CONTEXT_ID, ctx2?.data?.id);
console.log(`  node1 contextStateHash : ${ctx1?.data?.contextStateHash}`);
console.log(`  node2 contextStateHash : ${ctx2?.data?.contextStateHash}`);

// --- Phase 1: advance node 1's DAG so the guard has something to protect ----
//
// The no-DAG-growth guard is only meaningful against a NON-NULL baseline. Drive
// a genuine persisted write on node 1 (which installed kv-store and created the
// context, so no cross-node sync is needed) and assert the hash actually moved
// off genesis. Phase 4 then re-reads it: any DAG op emitted by the presence
// path would move this hash and fail the test.

console.log('\n-- Phase 1: advance node 1 DAG with a real kv-store write --');

let execResp;
try {
  execResp = await rpc(NODE1_URL, 'execute', {
    contextId: CONTEXT_ID,
    method: 'set',
    argsJson: { key: 'dag-guard', value: '1' },
  }, 10);
} catch (e) {
  die('kv-store execute(set) on node 1', `${e.message} — the DAG baseline was never established`);
}
console.log(`  execute(set) response: ${JSON.stringify(execResp)}`);
if (execResp.error) bad('kv-store execute(set) on node 1', `RPC error: ${JSON.stringify(execResp.error)}`);
else ok('kv-store execute(set) on node 1 succeeded (no RPC error)');

let hashBefore;
try {
  hashBefore = (await readContext(NODE1_URL, CONTEXT_ID))?.data?.contextStateHash;
} catch (e) {
  die('read node1 contextStateHash after the kv write', e.message);
}
console.log(`  node1 contextStateHash after kv write: ${hashBefore}`);
if (hashBefore && hashBefore !== NULL_HASH) {
  ok('node1 contextStateHash is NON-NULL after kv write — DAG advanced', hashBefore);
} else {
  bad('node1 contextStateHash is NON-NULL after kv write', `still genesis/null: ${hashBefore} — the guard would be vacuous`);
}

// --- Phase 2: delivery — node 2's subscriber receives node 1's presence ----
//
// Subscribe FIRST, publish second: the subscriber has to be live to witness the
// delta. (This is also why the old poll-a-snapshot approach is gone — there is
// nothing to poll.)

console.log('\n-- Phase 2: a subscriber on node 2 receives node 1 presence (gossip) --');

const watcher = subscribe(NODE2_URL, CONTEXT_ID);
try {
  await watcher.ready;
  ok('node 2 acknowledged the WS subscription');
} catch (e) {
  watcher.close();
  die('node 2 acknowledged the WS subscription', e.message);
}

try {
  const setResp = await rpc(NODE1_URL, 'set_ephemeral', { contextId: CONTEXT_ID, state: SLICE }, 1);
  console.log(`  set_ephemeral response: ${JSON.stringify(setResp)}`);
  if (setResp.error) bad('set_ephemeral on node 1', `RPC error: ${JSON.stringify(setResp.error)}`);
  else ok('set_ephemeral on node 1 succeeded (no RPC error)');
} catch (e) {
  watcher.close();
  die('set_ephemeral on node 1', `${e.message} — the request never completed`);
}

let live;
try {
  // Match on the LIVE shape specifically: a delta carries no age. Without this
  // the assertion could be satisfied by a replayed entry.
  live = await watcher.waitFor((p) => JSON.stringify(p.state) === JSON.stringify(SLICE) && p.ageMs === undefined);
  ok('node 2 subscriber received node 1 presence over gossip');
} catch (e) {
  bad('node 2 subscriber received node 1 presence over gossip', `${e.message} — gossip not delivered`);
}

if (live) {
  check('live delta state equals the published slice', SLICE, live.state);
  if (NODE1_KEY) check('live delta author is node 1', NODE1_KEY, live.author);
  if ('ageMs' in live) {
    bad('live delta omits ageMs', `a live delta must not carry an age: ${JSON.stringify(live)}`);
  } else {
    ok('live delta omits ageMs (absent, not null) — it is fresh at emission');
  }
  if (live.removed !== undefined && live.removed !== false) {
    bad('live delta is an upsert', `removed=${live.removed}`);
  } else {
    ok('live delta is an upsert (removed absent-or-false)');
  }
}

// --- Phase 3: replay-on-subscribe — the NEW behaviour ----------------------
//
// A client connecting after the fact must be seeded with what is already there.
// Nothing is published between here and Phase 2, and an unchanged heartbeat
// produces no diff, so anything this connection sees can only be the seed.

console.log('\n-- Phase 3: a NEW subscriber is seeded with the existing presence --');

const latecomer = subscribe(NODE2_URL, CONTEXT_ID);
let replayed;
try {
  await latecomer.ready;
  replayed = await latecomer.waitFor((p) => JSON.stringify(p.state) === JSON.stringify(SLICE), 10000);
  ok('a subscriber joining an already-populated context is seeded with the entry');
} catch (e) {
  bad('a subscriber joining an already-populated context is seeded with the entry', e.message);
}

if (replayed) {
  check('replayed entry state equals the published slice', SLICE, replayed.state);
  if (NODE1_KEY) check('replayed entry author is node 1', NODE1_KEY, replayed.author);
  if (typeof replayed.ageMs === 'number' && replayed.ageMs >= 0 && replayed.ageMs < PRESENCE_TTL_MS) {
    ok('replayed entry carries an ageMs inside the TTL window', replayed.ageMs);
  } else {
    bad('replayed entry carries an ageMs inside the TTL window', `got: ${JSON.stringify(replayed.ageMs)}`);
  }
}

// The seed must be addressed to the connection that asked for it. If it were
// broadcast, the Phase-2 watcher — subscribed long before, and with nothing
// published since — would see the entry a second time.
const beforeCount = watcher.events.length;
await sleep(1500);
const leaked = watcher.events.slice(beforeCount);
if (leaked.length === 0) {
  ok("another client's seed did not reach the already-connected subscriber");
} else {
  bad("another client's seed did not reach the already-connected subscriber", `saw ${leaked.length} extra event(s): ${JSON.stringify(leaked)}`);
}

latecomer.close();

// --- Phase 3b: the same seed over SSE, the transport mero-js uses ----------

console.log('\n-- Phase 3b: SSE subscriber (production transport) is seeded too --');

try {
  const sse = await subscribeSse(NODE2_URL, CONTEXT_ID);
  const seen = await sse.settle(4000);
  sse.close();
  const seed = seen.find((p) => JSON.stringify(p.state) === JSON.stringify(SLICE));
  if (seed) {
    ok('SSE subscriber was seeded with the existing presence');
    if (typeof seed.ageMs === 'number') ok('SSE seed carries ageMs', seed.ageMs);
    else bad('SSE seed carries ageMs', JSON.stringify(seed));
  } else {
    bad('SSE subscriber was seeded with the existing presence', `events seen: ${JSON.stringify(seen)}`);
  }
} catch (e) {
  bad('SSE subscriber was seeded with the existing presence', e.message);
}

// --- Phase 4: no-DAG-growth guard (LOAD-BEARING) --------------------------
//
// Re-read node 1's hash — the node the presence write hit — and require it to
// equal the NON-NULL baseline from Phase 1. Because the baseline is a real,
// DAG-advanced hash, this is directly falsifiable: any DAG op emitted by the
// presence handlers would move it.

console.log('\n-- Phase 4: no-DAG-growth guard (node 1, falsifiable) --');

let hashAfter;
try {
  hashAfter = (await readContext(NODE1_URL, CONTEXT_ID))?.data?.contextStateHash;
} catch (e) {
  watcher.close();
  die('read node1 contextStateHash after set_ephemeral', `${e.message} — the load-bearing guard cannot be evaluated`);
}

console.log(`  node1 hash before set_ephemeral (NON-NULL baseline): ${hashBefore}`);
console.log(`  node1 hash after  set_ephemeral                    : ${hashAfter}`);
check('no DAG growth: node1 contextStateHash unchanged by set_ephemeral (LOAD-BEARING)', hashBefore, hashAfter);

watcher.close();

// Phase 5 (TTL expiry) is a separate workflow step: it runs after node 1 has
// been stopped, so no heartbeat can refresh its entry. See
// ephemeral-ttl-check.js.
summarize(tally);
