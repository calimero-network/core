#!/usr/bin/env node
// cursor-client.js — a live, watchable client for Calimero ephemeral presence.
//
// This is the demo the feature exists for: multiple people moving a cursor
// around a shared document, seeing each other in real time, WITHOUT any of it
// touching the DAG. It runs against a context of the ordinary
// `apps/collaborative-editor` app — the app is unmodified and knows nothing
// about presence, because presence is a client-side capability:
//
//   * publish  — JSON-RPC `set_ephemeral` (a slice of opaque bytes)
//   * receive  — the node's WS event stream, which SEEDS a new subscriber with
//                the context's current presence and then streams live deltas
//
// The node never runs WASM on this path and never writes an op, which is what
// the state-hash panel is here to make falsifiable on screen.
//
// No dependencies: Node 24 has a global `WebSocket` and `fetch`. The transport
// is shared with the e2e assertions (workflows/scripts/ephemeral-transport.js)
// so the demo and the test drive the node through the same client.
//
// Usage (see README.md for the full script):
//   node cursor-client.js --node http://localhost:8940 --context <id> --name alice
//   node cursor-client.js --node http://localhost:8941 --context <id> --watch
//
// Keys (TTY only):  [w] real editor write (DAG must move)   [q] quit
//
// Non-TTY / piped output degrades to a line-oriented log, and --duration /
// --write-after make the whole thing scriptable.

import {
  readContext,
  rpc,
  subscribe,
} from '../../workflows/scripts/ephemeral-transport.js';

// ---------------------------------------------------------------------------
// Args
// ---------------------------------------------------------------------------

const argv = process.argv.slice(2);
const flag = (name, fallback = null) => {
  const i = argv.indexOf(`--${name}`);
  return i >= 0 && argv[i + 1] && !argv[i + 1].startsWith('--') ? argv[i + 1] : fallback;
};
const has = (name) => argv.includes(`--${name}`);

// 8940/8941 are the demo workflow's RPC ports (the e2e uses 8930/8931).
const NODE_URL = flag('node', process.env.NODE_URL || 'http://localhost:8940');
const CONTEXT_ID = flag('context', process.env.CONTEXT_ID || '');
const NAME = flag('name', process.env.NAME || `client@${NODE_URL.split(':').pop()}`);
const NODE_LABEL = flag('label', '');
const WATCH_ONLY = has('watch');
// A fixed exit time and a scheduled write make the demo reproducible in a
// pipe (which is how the run in the README's "what you should see" was
// captured); interactively, both are driven by keystrokes instead.
const DURATION_S = Number(flag('duration', '0'));
const WRITE_AFTER_S = Number(flag('write-after', '0'));
const PLAIN = has('plain') || !process.stdout.isTTY;

if (!CONTEXT_ID) {
  console.error('error: --context <id> is required (source .demo-env first — see README.md)');
  process.exit(2);
}

// ---------------------------------------------------------------------------
// Constants that mirror the node
// ---------------------------------------------------------------------------

/// `PRESENCE_TTL_MS` in calimero-node. Nothing here enforces or infers it — the
/// node evicts, and announces it with `removed: true`. It is quoted in the
/// roster and the feed only so the eviction you watch land can be checked
/// against the number that caused it.
const PRESENCE_TTL_MS = 7000;
/// base58 of [0u8; 32] — "nothing has ever been written to this context".
const NULL_HASH = '11111111111111111111111111111111';

const CURSOR_INTERVAL_MS = 400;
const HASH_POLL_MS = 1000;
const FRAME_MS = 125;
const DOC_LINES = 12;
const DOC_COLS = 60;

// ---------------------------------------------------------------------------
// Terminal helpers
// ---------------------------------------------------------------------------

const C = {
  reset: '\x1b[0m',
  dim: '\x1b[2m',
  bold: '\x1b[1m',
  red: '\x1b[31m',
  green: '\x1b[32m',
  yellow: '\x1b[33m',
  blue: '\x1b[34m',
  magenta: '\x1b[35m',
  cyan: '\x1b[36m',
};
const paint = (color, s) => (PLAIN ? s : `${color}${s}${C.reset}`);
const short = (s, n = 10) => (typeof s === 'string' && s.length > n ? `${s.slice(0, n)}…` : String(s));
// Local time, deliberately: the feed is read side-by-side with `merobox stop`
// in another terminal, and a UTC timestamp there makes "did the eviction land
// inside the TTL?" needlessly hard to answer.
const hhmmss = (t = Date.now()) => {
  const d = new Date(t);
  const pad = (n, w = 2) => String(n).padStart(w, '0');
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(d.getMilliseconds(), 3)}`;
};
const secs = (ms) => `${(ms / 1000).toFixed(1)}s`;

// ---------------------------------------------------------------------------
// Presence payload
// ---------------------------------------------------------------------------
//
// The wire carries opaque bytes (`state: Vec<u8>`) — the node neither parses
// nor validates them. We put a small JSON object in there; a real editor would
// put a selection range or a Yjs awareness frame.

const encodeCursor = (cursor) => Array.from(new TextEncoder().encode(JSON.stringify(cursor)));

const decodeCursor = (state) => {
  if (!Array.isArray(state)) return null;
  try {
    return JSON.parse(new TextDecoder().decode(Uint8Array.from(state)));
  } catch {
    return null;
  }
};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/** author pubkey -> { name, line, col, src, seededAgeMs, lastSeen, updates } */
const roster = new Map();
const feed = [];
const stats = {
  published: 0,
  received: 0,
  seeded: 0,
  evicted: 0,
  hash: null,
  hashSince: Date.now(),
  hashChanges: 0,
  hashError: null,
  writes: 0,
};

const log = (tag, color, text) => {
  const line = { t: Date.now(), tag, color, text };
  feed.push(line);
  if (feed.length > 200) feed.shift();
  if (PLAIN) console.log(`${hhmmss(line.t)}  ${paint(color, tag.padEnd(5))} ${text}`);
};

// My own cursor, moved by a bounded random walk so the roster is visibly alive.
// The walk is mean-reverting: an unbiased walk pins itself against a margin
// within a minute or so and then sits there, which reads on screen as "the
// demo froze".
const clamp = (v, hi) => Math.max(0, Math.min(hi, v));
const me = { line: Math.floor(Math.random() * DOC_LINES), col: Math.floor(Math.random() * DOC_COLS) };
const stepCursor = () => {
  if (Math.random() < 0.3) {
    me.line = clamp(me.line + Math.sign(Math.random() - 0.5) - Math.sign(me.line - DOC_LINES / 2) * (Math.random() < 0.3 ? 1 : 0), DOC_LINES - 1);
  }
  const drift = Math.round((Math.random() - 0.5) * 7) - Math.round((me.col - DOC_COLS / 2) / 12);
  me.col = clamp(me.col + drift, DOC_COLS - 1);
};

// ---------------------------------------------------------------------------
// Event handling
// ---------------------------------------------------------------------------
//
// `ageMs` is the whole tell. The node omits it on a live delta (emitted at the
// instant of change) and carries it ONLY on the replay a subscriber is seeded
// with at subscribe time. So the field's presence — not any timing heuristic
// on our side — is what makes seed-on-subscribe visible.

const onEphemeral = (payload) => {
  const { author, removed, ageMs } = payload;
  const isSeed = ageMs !== undefined && ageMs !== null;

  if (removed) {
    const gone = roster.get(author);
    roster.delete(author);
    stats.evicted++;
    log(
      'GONE',
      C.red,
      `${paint(C.bold, gone?.name ?? short(author))} evicted by the node after PRESENCE_TTL_MS=${PRESENCE_TTL_MS}ms ` +
        `(removed=true, author=${short(author)}) — no goodbye was ever sent`,
    );
    return;
  }

  const cursor = decodeCursor(payload.state);
  if (!cursor) return;

  const prev = roster.get(author);
  const entry = {
    name: cursor.name ?? short(author),
    line: cursor.line ?? 0,
    col: cursor.col ?? 0,
    src: isSeed ? 'SEED' : 'LIVE',
    seededAgeMs: isSeed ? ageMs : prev?.seededAgeMs ?? null,
    lastSeen: Date.now(),
    updates: (prev?.updates ?? 0) + 1,
    mine: cursor.name === NAME && !WATCH_ONLY,
  };
  roster.set(author, entry);
  stats.received++;

  if (isSeed) {
    stats.seeded++;
    log(
      'SEED',
      C.yellow,
      `${paint(C.bold, entry.name)} was ALREADY present when we subscribed — ` +
        `cursor ${entry.line}:${entry.col}, ageMs=${ageMs} (${secs(ageMs)} old, TTL ${PRESENCE_TTL_MS}ms), author=${short(author)}`,
    );
  } else if (!prev || prev.line !== entry.line || prev.col !== entry.col) {
    log('LIVE', C.green, `${entry.name} moved to ${entry.line}:${entry.col} (author=${short(author)})`);
  }
};

// ---------------------------------------------------------------------------
// The DAG panel
// ---------------------------------------------------------------------------
//
// The load-bearing claim: cursors do not grow the DAG. `contextStateHash` is
// read straight from the admin API on a timer, so it is the node's own answer,
// not ours. It must sit still while presence flies, and jump the moment a real
// `insert_text` op lands.

async function pollHash() {
  try {
    const hash = (await readContext(NODE_URL, CONTEXT_ID))?.data?.contextStateHash ?? null;
    stats.hashError = null;
    if (hash !== stats.hash) {
      const from = stats.hash;
      stats.hash = hash;
      stats.hashSince = Date.now();
      if (from !== null) {
        stats.hashChanges++;
        log('DAG', C.magenta, `contextStateHash MOVED  ${short(from, 12)} -> ${short(hash, 12)}`);
      }
    }
  } catch (e) {
    // A dead node reads identically to "the hash never moved", which is exactly
    // the false-positive this demo must not produce. Say so on screen.
    stats.hashError = e.message;
  }
}

/** A real application write: an op, a WASM run, and a hash that must move. */
async function editorWrite() {
  const before = stats.hash;
  const text = `${NAME[0] ?? '*'}`;
  log('DAG', C.magenta, `editor write: execute(insert_text, position=0, text="${text}") — hash before: ${before}`);
  try {
    const resp = await rpc(
      NODE_URL,
      'execute',
      { contextId: CONTEXT_ID, method: 'insert_text', argsJson: { position: 0, text } },
      99,
    );
    if (resp.error) {
      log('DAG', C.red, `editor write FAILED: ${JSON.stringify(resp.error)}`);
      return;
    }
    stats.writes++;
  } catch (e) {
    log('DAG', C.red, `editor write FAILED: ${e.message}`);
    return;
  }

  // Poll until the node reports a different hash, so the demo shows the real
  // before/after rather than asserting one.
  const deadline = Date.now() + 8000;
  while (Date.now() < deadline) {
    await pollHash();
    if (stats.hash !== before) {
      log('DAG', C.magenta, `hash after  editor write: ${stats.hash}  (moved — a real op WAS appended)`);
      return;
    }
    await new Promise((r) => setTimeout(r, 250));
  }
  log('DAG', C.red, `hash did NOT move within 8s of the editor write (still ${stats.hash}) — that is a finding, not a demo`);
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

function frame() {
  const now = Date.now();
  const lines = [];
  const rule = '─'.repeat(96);

  lines.push(paint(C.bold, `Calimero ephemeral presence — live cursors on apps/collaborative-editor`));
  lines.push(rule);
  lines.push(
    `  node    ${NODE_URL}${NODE_LABEL ? ` (${NODE_LABEL})` : ''}    ` +
      `me  ${paint(C.cyan, WATCH_ONLY ? `${NAME} [watch-only, publishes nothing]` : NAME)}`,
  );
  lines.push(`  context ${CONTEXT_ID}`);
  lines.push(
    `  presence  published ${paint(C.cyan, stats.published)}   received ${paint(C.cyan, stats.received)}   ` +
      `seeded ${paint(C.yellow, stats.seeded)}   evicted ${paint(C.red, stats.evicted)}`,
  );
  lines.push('');

  // --- DAG panel ---------------------------------------------------------
  const held = secs(now - stats.hashSince);
  const hashLine = stats.hashError
    ? paint(C.red, `UNREADABLE (${stats.hashError}) — a dead node is not a still hash`)
    : `${paint(C.bold, stats.hash ?? '(null)')}${stats.hash === NULL_HASH ? paint(C.dim, '  [genesis — nothing written yet]') : ''}`;
  lines.push(paint(C.bold, '  CONTEXT STATE HASH (the DAG)'));
  lines.push(`    ${hashLine}`);
  lines.push(
    `    unchanged for ${paint(stats.hashChanges === 0 ? C.green : C.reset, held)}   ` +
      `changes since start: ${stats.hashChanges}   editor writes: ${stats.writes}   ` +
      paint(C.dim, `(${stats.received} presence events received in the meantime)`),
  );
  lines.push('');

  // --- Roster ------------------------------------------------------------
  lines.push(paint(C.bold, '  PRESENCE ROSTER  (from the WS event stream — there is no read endpoint)'));
  lines.push(
    paint(
      C.dim,
      '    a row leaves ONLY when the node sends removed=true. Presence is held by the NODE, not by this',
    ),
  );
  lines.push(
    paint(
      C.dim,
      '    socket: a peer whose client has quit keeps its cursor until its NODE stops heartbeating.',
    ),
  );
  lines.push(paint(C.dim, '    who         author         cursor    last move   src    note'));
  if (roster.size === 0) {
    lines.push(paint(C.dim, '    (nobody yet — start a second client, or wait for gossip)'));
  }
  for (const [author, e] of [...roster.entries()].sort((a, b) => a[1].name.localeCompare(b[1].name))) {
    // Time since this peer's cursor last CHANGED — not since it was last
    // alive. An unchanged heartbeat produces no diff and therefore no event,
    // so a peer sitting still goes quiet on this socket while remaining
    // perfectly fresh in the node's awareness store. Marking such a row
    // "expiring" would be a lie the node never told us.
    const age = now - e.lastSeen;
    const idle = age > PRESENCE_TTL_MS;
    const cursorCell = `${String(e.line).padStart(2)}:${String(e.col).padStart(2)}`;
    const note = e.mine
      ? paint(C.cyan, 'you')
      : e.src === 'SEED'
        ? paint(C.yellow, `seeded at subscribe, ageMs=${e.seededAgeMs}`)
        : paint(C.dim, `${e.updates} updates`);
    const row =
      `    ${e.name.padEnd(11)} ${short(author, 12).padEnd(14)} ${cursorCell.padEnd(9)} ` +
      `${secs(age).padStart(6)}     ${e.src === 'SEED' ? paint(C.yellow, 'SEED') : paint(C.green, 'LIVE')}   ${note}`;
    lines.push(idle ? paint(C.dim, `${row}  ${paint(C.dim, '← idle, still held by its node')}`) : row);
  }
  lines.push('');

  // --- Cursor map --------------------------------------------------------
  lines.push(paint(C.bold, '  DOCUMENT (cursor positions)'));
  const grid = Array.from({ length: DOC_LINES }, () => Array(DOC_COLS).fill(' '));
  let i = 0;
  const marks = ['A', 'B', 'C', 'D', 'E', 'F'];
  for (const e of roster.values()) {
    grid[Math.min(DOC_LINES - 1, e.line)][Math.min(DOC_COLS - 1, e.col)] = e.mine ? '@' : marks[i % marks.length];
    i++;
  }
  for (const row of grid) lines.push(paint(C.dim, `    │${row.join('')}│`));
  lines.push('');

  // --- Feed --------------------------------------------------------------
  lines.push(paint(C.bold, '  EVENT FEED'));
  for (const l of feed.slice(-9)) {
    lines.push(`    ${paint(C.dim, hhmmss(l.t))}  ${paint(l.color, l.tag.padEnd(5))} ${l.text}`);
  }
  lines.push('');
  lines.push(paint(C.dim, `  [w] real editor write (DAG must move)   [q] quit`));

  process.stdout.write(`\x1b[H${lines.map((l) => `${l}\x1b[K`).join('\n')}\x1b[J`);
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

const timers = [];
let watcher = null;

function shutdown(code = 0) {
  for (const t of timers) clearInterval(t);
  watcher?.close();
  if (!PLAIN) process.stdout.write('\x1b[?25h\x1b[2J\x1b[H');
  if (process.stdin.isTTY) process.stdin.setRawMode(false);
  // Nothing is sent on the way out. A cursor disappears because the peer's node
  // stopped heartbeating and the TTL expired it — never because of a goodbye.
  console.log(
    `\nexited without sending anything — published ${stats.published}, received ${stats.received}, ` +
      `hash changes ${stats.hashChanges}, editor writes ${stats.writes}`,
  );
  process.exit(code);
}

async function main() {
  console.log(`connecting to ${NODE_URL} for context ${CONTEXT_ID} as "${NAME}"${WATCH_ONLY ? ' (watch-only)' : ''} ...`);
  await pollHash();

  watcher = subscribe(NODE_URL, CONTEXT_ID);
  // Every Ephemeral event, seed and live alike, arrives through this one hook.
  const drain = () => {
    while (watcher.events.length) onEphemeral(watcher.events.shift());
  };
  try {
    await watcher.ready;
    log('WS', C.blue, `subscribed to ${short(CONTEXT_ID, 12)} — the node replays current presence to THIS connection now`);
  } catch (e) {
    console.error(`could not subscribe: ${e.message}`);
    process.exit(1);
  }
  timers.push(setInterval(drain, 50));

  if (!WATCH_ONLY) {
    timers.push(
      setInterval(async () => {
        stepCursor();
        try {
          const resp = await rpc(
            NODE_URL,
            'set_ephemeral',
            { contextId: CONTEXT_ID, state: encodeCursor({ v: 1, name: NAME, line: me.line, col: me.col }) },
            7,
          );
          if (resp.error) log('ERR', C.red, `set_ephemeral: ${JSON.stringify(resp.error)}`);
          else stats.published++;
        } catch (e) {
          log('ERR', C.red, `set_ephemeral: ${e.message}`);
        }
      }, CURSOR_INTERVAL_MS),
    );
  }

  timers.push(setInterval(pollHash, HASH_POLL_MS));
  if (!PLAIN) {
    process.stdout.write('\x1b[?25l\x1b[2J');
    timers.push(setInterval(frame, FRAME_MS));
  } else {
    timers.push(
      setInterval(() => {
        const roles = [...roster.values()].map((e) => `${e.name}@${e.line}:${e.col}(${e.src})`).join(' ');
        console.log(
          `${hhmmss()}  STATUS hash=${stats.hash} unchanged=${secs(Date.now() - stats.hashSince)} ` +
            `changes=${stats.hashChanges} published=${stats.published} received=${stats.received} | ${roles}`,
        );
      }, 2000),
    );
  }

  if (WRITE_AFTER_S > 0) setTimeout(editorWrite, WRITE_AFTER_S * 1000);
  if (DURATION_S > 0) setTimeout(() => shutdown(0), DURATION_S * 1000);

  if (process.stdin.isTTY) {
    process.stdin.setRawMode(true);
    process.stdin.resume();
    process.stdin.on('data', (buf) => {
      const key = buf.toString();
      if (key === 'q' || key === '\u0003') shutdown(0);
      if (key === 'w') void editorWrite();
    });
  }
}

process.on('SIGINT', () => shutdown(0));
process.on('SIGTERM', () => shutdown(0));

await main();
