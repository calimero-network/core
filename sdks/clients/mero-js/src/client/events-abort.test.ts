/**
 * events() ends promptly on abort(), through the real global `fetch` and the
 * sealed transport, against a stand-in cloud node. Synthetic data only.
 */

import { setFlagsFromString } from 'node:v8';
import { runInNewContext } from 'node:vm';

import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { StandInNode } from '../../tests/support/stand-in-node.js';
import { CalimeroClient, type NodeEvent } from './index.js';

setFlagsFromString('--expose-gc');
const gc = runInNewContext('gc') as () => void;

const node = new StandInNode();
beforeAll(() => node.start());
afterAll(() => node.stop());

const client = () => new CalimeroClient({ baseUrl: node.url, policy: { allowMock: true } });

describe('events() and abort()', () => {
  // The bug: the sealed fetch handed the transport `new Request(input, init).signal`,
  // a signal that follows the caller's only through a weak reference. Once the
  // Request was garbage collected, abort() no longer reached the open fetch, so the
  // stream stayed open. The GC here makes that deterministic.
  it.each(Array.from({ length: 10 }, (_, i) => i + 1))('ends within 500 ms of abort(), run %i of 10', async () => {
    node.answer = (head) =>
      head.path.startsWith('/apps/app-1/events')
        ? { status: 200, events: ['id: 1\nevent: Booked\ndata: {"id":"b1"}\n\n'] }
        : { status: 200, json: {} };
    const seen: NodeEvent[] = [];
    const ac = new AbortController();
    let resolveFirst!: () => void;
    const first = new Promise<void>((ok) => (resolveFirst = ok));
    const done = (async () => {
      try {
        for await (const e of client().events('app-1', 0, ac.signal)) {
          seen.push(e);
          resolveFirst();
        }
      } catch (error) {
        if (!ac.signal.aborted) throw error;
      }
    })();
    await first;
    expect(seen).toEqual([{ id: '1', kind: 'Booked', data: { id: 'b1' } }]);
    for (let i = 0; i < 3; i++) {
      gc();
      await new Promise((r) => setTimeout(r, 10));
    }

    const aborted = Date.now();
    ac.abort();
    const ended = await Promise.race([done.then(() => true), new Promise<boolean>((r) => setTimeout(() => r(false), 500))]);
    expect(ended).toBe(true);
    expect(Date.now() - aborted).toBeLessThan(500);
    // The node sees the connection go, too.
    const deadline = Date.now() + 500;
    while (node.openStreams > 0 && Date.now() < deadline) await new Promise((r) => setTimeout(r, 10));
    expect(node.openStreams).toBe(0);
  });
});
