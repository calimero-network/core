// ephemeral-lib.js — shared helpers for the ephemeral-presence e2e scripts.
//
// Presence has no request/response read surface: a client learns presence ONLY
// by subscribing to the node's event stream, which seeds it with the context's
// current entries and then delivers live deltas. So every assertion here is
// made against the event stream, over a real WebSocket, the way a client sees
// it.
//
// Node 24 ships a global `WebSocket`, so there is no dependency to install.

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

const wsUrl = (httpUrl) => `${httpUrl.replace(/^http/, 'ws')}/ws`;

/** POST a JSON-RPC request, returning the parsed body. Throws on transport failure. */
export async function rpc(url, method, params, id = 1) {
  const res = await fetch(`${url}/jsonrpc`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id, method, params }),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status} from ${method}`);
  return res.json();
}

/** Read a context through the admin API. Throws on transport failure. */
export async function readContext(url, contextId) {
  const res = await fetch(`${url}/admin-api/contexts/${contextId}`);
  if (!res.ok) throw new Error(`HTTP ${res.status} reading context`);
  return res.json();
}

/**
 * Open a WS connection, subscribe to `contextId`, and collect every Ephemeral
 * event it delivers.
 *
 * The message handler is installed BEFORE the subscribe is sent, because the
 * seed a subscriber is given can arrive ahead of the subscribe acknowledgment
 * — it is queued by the same handler that records the subscription.
 *
 * Returns a handle with:
 *   `ready`     — resolves once the node has acknowledged the subscription
 *   `events`    — every Ephemeral payload seen so far, in arrival order
 *   `waitFor`   — resolve when an event matching a predicate arrives (or has)
 *   `close`     — shut the socket down
 */
export function subscribe(httpUrl, contextId, { timeoutMs = 15000 } = {}) {
  const events = [];
  const waiters = [];
  const ws = new WebSocket(wsUrl(httpUrl));

  let resolveReady;
  let rejectReady;
  const ready = new Promise((resolve, reject) => {
    resolveReady = resolve;
    rejectReady = reject;
  });
  const readyTimer = setTimeout(
    () => rejectReady(new Error(`subscribe not acknowledged within ${timeoutMs}ms`)),
    timeoutMs,
  );

  const deliver = (payload) => {
    events.push(payload);
    for (const waiter of [...waiters]) {
      if (waiter.pred(payload)) {
        clearTimeout(waiter.timer);
        waiters.splice(waiters.indexOf(waiter), 1);
        waiter.resolve(payload);
      }
    }
  };

  ws.addEventListener('open', () => {
    ws.send(JSON.stringify({ id: 1, method: 'subscribe', params: { contextIds: [contextId] } }));
  });

  ws.addEventListener('message', (ev) => {
    let msg;
    try {
      msg = JSON.parse(ev.data);
    } catch {
      return;
    }

    if (msg.id === 1 && msg.result) {
      const subscribed = msg.result.contextIds || msg.result.context_ids || [];
      clearTimeout(readyTimer);
      if (subscribed.includes(contextId)) {
        resolveReady(msg.result);
      } else {
        rejectReady(new Error(`subscribe did not include the context: ${JSON.stringify(msg.result)}`));
      }
      return;
    }

    const event = msg.result;
    if (event && event.type === 'Ephemeral') {
      deliver({ ...event.data, contextId: event.contextId ?? event.context_id });
    }
  });

  ws.addEventListener('error', (e) => {
    clearTimeout(readyTimer);
    rejectReady(new Error(`ws error: ${e.message || e.type}`));
  });

  return {
    ready,
    events,
    waitFor(pred, ms = timeoutMs) {
      const already = events.find(pred);
      if (already) return Promise.resolve(already);
      return new Promise((resolve, reject) => {
        const timer = setTimeout(
          () => reject(new Error(`no matching Ephemeral event within ${ms}ms`)),
          ms,
        );
        waiters.push({ pred, resolve, timer });
      });
    },
    close() {
      clearTimeout(readyTimer);
      for (const waiter of waiters) clearTimeout(waiter.timer);
      ws.close();
    },
  };
}

/**
 * Subscribe over SSE — the transport mero-js actually uses — and collect the
 * Ephemeral events delivered to it. SSE splits subscribe (a POST) from the
 * stream (a GET), so this is a materially different delivery path from WS and
 * is worth exercising in its own right.
 */
export async function subscribeSse(httpUrl, contextId, { timeoutMs = 15000 } = {}) {
  const controller = new AbortController();
  const res = await fetch(`${httpUrl}/sse`, {
    headers: { Accept: 'text/event-stream' },
    signal: controller.signal,
  });
  if (!res.ok || !res.body) throw new Error(`SSE connect failed: ${res.status}`);

  const events = [];
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buf = '';
  let sessionId = null;

  const pump = (async () => {
    while (true) {
      const { done, value } = await reader.read();
      if (done) return;
      buf += decoder.decode(value, { stream: true });

      let idx;
      while ((idx = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, idx).trim();
        buf = buf.slice(idx + 1);
        if (!line.startsWith('data:')) continue;
        let msg;
        try {
          msg = JSON.parse(line.slice(5).trim());
        } catch {
          continue;
        }

        if (msg.type === 'connect' && msg.session_id) {
          sessionId = msg.session_id;
          await fetch(`${httpUrl}/sse/subscription`, {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ id: sessionId, method: 'subscribe', params: { contextIds: [contextId] } }),
          });
          continue;
        }

        const event = msg.result;
        if (event && event.type === 'Ephemeral') {
          events.push({ ...event.data, contextId: event.contextId ?? event.context_id });
        }
      }
    }
  })().catch(() => {});

  return {
    events,
    async settle(ms = 3000) {
      await new Promise((r) => setTimeout(r, ms));
      return events;
    },
    close() {
      controller.abort();
      return pump;
    },
  };
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
