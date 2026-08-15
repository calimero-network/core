// ephemeral-lib.js — shared helpers for the ephemeral-presence e2e scripts.
//
// Presence has no request/response read surface: a client learns presence ONLY
// by subscribing to the node's event stream, which seeds it with the context's
// current entries and then delivers live deltas. So every assertion here is
// made against the event stream, over a real WebSocket, the way a client sees
// it.
//
// Node 24 ships a global `WebSocket`, so there is no dependency to install.
//
// This file now holds only the assertion vocabulary; the transport (`rpc`,
// `readContext`, `subscribe`, `subscribeSse`, `sleep`) moved to
// ephemeral-transport.js and is re-exported at the bottom, so the e2e scripts
// and the runnable demo under tools/ephemeral-presence-demo share one client.

/** A passed/failed counter shared by a script's assertions. */
export const tally = { pass: 0, fail: 0 };

export const ok = (label, got) => {
  console.log(`ok   ${label}${got !== undefined ? ` (got: ${got})` : ''}`);
  tally.pass++;
};

export const bad = (label, detail) => {
  console.log(`FAIL ${label}${detail ? `: ${detail}` : ''}`);
  tally.fail++;
};

export const check = (label, expected, actual) =>
  JSON.stringify(actual) === JSON.stringify(expected)
    ? ok(label, JSON.stringify(actual))
    : bad(label, `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);

/**
 * Abort the whole script.
 *
 * A request that never completed is NOT a failed assertion — it is an unusable
 * run, and several assertions below are satisfied by an empty result (zero
 * entries, no event). Reporting a transport failure as a PASS is the exact
 * failure mode this guards.
 */
export function die(label, detail) {
  console.log(`FATAL ${label}${detail ? `: ${detail}` : ''}`);
  console.log(`\n=== ${tally.pass} passed, ${tally.fail} failed ===`);
  process.exit(1);
}

export function summarize() {
  console.log(`\n=== ${tally.pass} passed, ${tally.fail} failed ===`);
  process.exit(tally.fail === 0 ? 0 : 1);
}


// The transport helpers (`rpc`, `readContext`, `subscribe`, `subscribeSse`,
// `sleep`) now live in ephemeral-transport.js so the runnable demo under
// tools/ephemeral-presence-demo can drive a node exactly the way these
// assertions do, rather than carrying a second copy of the WebSocket client.
// They are re-exported here so every existing importer of this module keeps
// working unchanged.
export * from './ephemeral-transport.js';
