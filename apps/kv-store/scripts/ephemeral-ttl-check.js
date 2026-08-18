#!/usr/bin/env node
// ephemeral-ttl-check.js — TTL eviction assertion for the ephemeral presence e2e.
//
// Runs AFTER the workflow has stopped node 1. Without node 1's heartbeats its
// entry in node 2's awareness store must expire within PRESENCE_TTL_MS (7 000
// ms). Node 2's own local entry (seeded here) is what drives the heartbeat tick
// that performs the sweep.
//
// Args: $1 node2_url  $2 context_id  $3 node1_key
//
// How eviction is observed now that there is no snapshot endpoint: a FRESH
// subscriber is seeded with exactly the entries the awareness store still
// holds, so the seed IS the snapshot. Subscribing after the TTL window and
// looking at what arrives answers the question directly.
//
// Two vacuity traps this closes:
//   * "no node-1 entry" is also what a broken subscription looks like — so we
//     require node 2's OWN entry to be in the same seed. If the seed is empty,
//     that is a failure, not a pass.
//   * a transport error yields no events at all, which reads identically to a
//     successful eviction — so those abort loudly instead.
//
// Both traps are checked once per transport: eviction is asserted over SSE (the
// transport mero-js uses in production) and over WS (still served, must not
// regress). The two subscribers are opened together after a single TTL wait, so
// covering both costs no extra wall-clock time.

import { bad, die, ok, rpc, sleep, subscribe, subscribeSse, summarize } from './ephemeral-lib.js';

const NODE2_URL = process.argv[2] || process.env.NODE2_URL || 'http://localhost:8931';
const CONTEXT_ID = process.argv[3] || process.env.CONTEXT_ID || '';
const NODE1_KEY = process.argv[4] || process.env.NODE1_KEY || '';

const NODE2_SLICE = [9, 8, 7];

console.log('=== ephemeral-ttl-check assertions ===');
console.log(`  node2_url   : ${NODE2_URL}`);
console.log(`  context_id  : ${CONTEXT_ID}`);
console.log(`  node1_key   : ${NODE1_KEY}`);

if (!CONTEXT_ID || !NODE1_KEY) die('CONTEXT_ID or NODE1_KEY is empty');

// Seed a local slice on node 2 so its heartbeat tick — and therefore the sweep
// that evicts stale remote entries — actually runs.
try {
  const resp = await rpc(NODE2_URL, 'set_ephemeral', { contextId: CONTEXT_ID, state: NODE2_SLICE }, 3);
  if (resp.error) die('seed node 2 local presence', `RPC error: ${JSON.stringify(resp.error)} — the sweep was never armed`);
  console.log(`  seeded node 2 local presence to trigger the sweep: ${JSON.stringify(resp)}`);
} catch (e) {
  die('seed node 2 local presence', `${e.message} — the request never completed, so the sweep was never armed`);
}

// TTL (7s) + two heartbeat ticks (2 × 2.5s) + margin. Node 1 is already
// stopped, so nothing can refresh its entry during the wait.
console.log('  waiting 13s for TTL (7s) + heartbeat sweeps (2×2.5s) ...');
await sleep(13000);

// A fresh subscriber is seeded with whatever the awareness store still holds —
// one per transport, opened together so the single TTL wait above covers both.
const watcher = subscribe(NODE2_URL, CONTEXT_ID);
try {
  await watcher.ready;
  ok('node 2 acknowledged the WS subscription');
} catch (e) {
  watcher.close();
  die('node 2 acknowledged the WS subscription', `${e.message} — the TTL guard cannot be evaluated`);
}

let sseWatcher;
try {
  sseWatcher = await subscribeSse(NODE2_URL, CONTEXT_ID);
  ok('node 2 accepted the SSE subscription (production transport)');
} catch (e) {
  watcher.close();
  die('node 2 accepted the SSE subscription', `${e.message} — the SSE TTL guard cannot be evaluated`);
}

const ownPresence = (p) => JSON.stringify(p.state) === JSON.stringify(NODE2_SLICE);

// Node 2's own entry is heartbeated locally and must survive; waiting for it
// both proves the seed arrived and gives the seed time to land in full. This is
// the non-vacuity guard, and it is required on EACH transport: a zero-entry
// result from a broken subscription must not read as a successful eviction.
try {
  await watcher.waitFor(ownPresence, 10000);
  ok("node 2's own presence is in the WS seed — the seed is non-empty, so the check below is not vacuous");
} catch (e) {
  watcher.close();
  sseWatcher.close();
  die("node 2's own presence is in the WS seed", `${e.message} — an empty seed would make the eviction check vacuous`);
}

try {
  await sseWatcher.waitFor(ownPresence, 10000, "node 2's own presence in the seed");
  ok("node 2's own presence is in the SSE seed — the seed is non-empty, so the check below is not vacuous");
} catch (e) {
  watcher.close();
  sseWatcher.close();
  die("node 2's own presence is in the SSE seed", `${e.message} — an empty seed would make the eviction check vacuous`);
}

// Give any further seed frames a moment to arrive before counting.
await sleep(1000);
watcher.close();

const seed = watcher.events;
console.log(`  entries in the WS seed : ${JSON.stringify(seed)}`);

const stale = seed.filter((p) => p.author === NODE1_KEY);
if (stale.length === 0) {
  ok('node 1 entry evicted from node 2 awareness after TTL, seen over WS (PRESENCE_TTL_MS=7000ms)', '0 remaining');
} else {
  bad('node 1 entry NOT evicted from node 2 awareness after TTL (WS)', `still present: ${JSON.stringify(stale)}`);
}

// `settle` (not a bare sleep) so a stream that died between the seed and the
// count is reported as a transport failure instead of as an empty — i.e.
// successfully evicted — seed.
let sseSeed;
try {
  sseSeed = await sseWatcher.settle(1000);
} catch (e) {
  sseWatcher.close();
  die('SSE seed readable for the eviction count', e.message);
}
sseWatcher.close();
console.log(`  entries in the SSE seed: ${JSON.stringify(sseSeed)}`);

const staleSse = sseSeed.filter((p) => p.author === NODE1_KEY);
if (staleSse.length === 0) {
  ok('node 1 entry evicted from node 2 awareness after TTL, seen over SSE (PRESENCE_TTL_MS=7000ms)', '0 remaining');
} else {
  bad('node 1 entry NOT evicted from node 2 awareness after TTL (SSE)', `still present: ${JSON.stringify(staleSse)}`);
}

summarize();
