#!/usr/bin/env node
// ephemeral-open-subgroup-e2e.js - presence on a context in an Open subgroup,
// between its creator and a namespace member who only inherits access to it.
//
// Positional args: $1 node1_url  $2 node2_url  $3 context_id  $4 node1_key  $5 node2_key
//
// Each direction is asserted on both transports (WS and SSE) against the exact
// slice and author that were published.

import { check, die, rpc, subscribe, subscribeSse, summarize } from './ephemeral-lib.js';

const [NODE1_URL, NODE2_URL, CONTEXT_ID, NODE1_KEY, NODE2_KEY] = process.argv.slice(2);

const SLICE_FROM_NODE1 = [11, 22, 33];
const SLICE_FROM_NODE2 = [44, 55, 66];
const DELIVERY_TIMEOUT_MS = 15000;

for (const [name, value] of Object.entries({ NODE1_URL, NODE2_URL, CONTEXT_ID, NODE1_KEY, NODE2_KEY })) {
  if (!value) die(`${name} is empty - was the workflow output captured?`);
}

/** Publish `slice` from `publisherUrl` and assert `receiverUrl` sees exactly it, authored by `author`. */
async function assertDelivered(label, publisherUrl, receiverUrl, slice, author) {
  console.log(`\n-- ${label} --`);

  const ws = subscribe(receiverUrl, CONTEXT_ID);
  let sse;
  try {
    await ws.ready;
    sse = await subscribeSse(receiverUrl, CONTEXT_ID);
  } catch (e) {
    ws.close();
    die(`${label}: receiver subscription`, e.message);
  }

  let setResp;
  try {
    setResp = await rpc(publisherUrl, 'set_ephemeral', { contextId: CONTEXT_ID, state: slice });
  } catch (e) {
    die(`${label}: set_ephemeral`, `${e.message} - the request never completed`);
  }
  check(`${label}: set_ephemeral returns no error`, undefined, setResp.error);

  const matches = (p) => JSON.stringify(p.state) === JSON.stringify(slice);
  for (const [transport, sub] of [['WS', ws], ['SSE', sse]]) {
    try {
      const got = await sub.waitFor(matches, DELIVERY_TIMEOUT_MS);
      check(`${label}: ${transport} state`, slice, got.state);
      check(`${label}: ${transport} author`, author, got.author);
    } catch (e) {
      check(`${label}: ${transport} delivery`, slice, `nothing (${e.message})`);
    }
  }

  ws.close();
  await sse.close();
}

console.log('=== ephemeral-open-subgroup-e2e ===');
await assertDelivered('node 1 -> node 2 (inherited member)', NODE1_URL, NODE2_URL, SLICE_FROM_NODE1, NODE1_KEY);
await assertDelivered('node 2 (inherited member) -> node 1', NODE2_URL, NODE1_URL, SLICE_FROM_NODE2, NODE2_KEY);
summarize();
