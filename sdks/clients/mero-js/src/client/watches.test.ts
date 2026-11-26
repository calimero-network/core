/**
 * Live views over a stand-in host: the stream's frames are pushed by the test, the
 * three app routes are recorded. Synthetic data only.
 */

import { describe, expect, it } from 'vitest';

import type { NodeEvent } from '../runtime/transport.js';
import { AppError } from '../runtime/errors.js';
import { ViewWatches, type WatchConnectionState, type WatchHost } from './watches.js';

/** One open stream: frames the test pushes, ended by `end()` or the client's abort. */
class Conn {
  private queue: NodeEvent[] = [];
  private wake: (() => void) | undefined;
  private ended = false;
  constructor(
    readonly query: Record<string, unknown>,
    signal?: AbortSignal,
  ) {
    signal?.addEventListener('abort', () => this.end());
  }
  push(kind: string, data: unknown, id = ''): void {
    this.queue.push({ id, kind, data });
    this.wake?.();
  }
  end(): void {
    this.ended = true;
    this.wake?.();
  }
  async *frames(): AsyncGenerator<NodeEvent> {
    for (;;) {
      while (this.queue.length) yield this.queue.shift()!;
      if (this.ended) return;
      await new Promise<void>((resolve) => (this.wake = resolve));
      this.wake = undefined;
    }
  }
}

class StandIn implements WatchHost {
  conns: Conn[] = [];
  added: { stream: string; view: string; args?: unknown; intervalMs?: number }[] = [];
  removed: string[] = [];
  reads: { method: string; minIndex?: number }[] = [];
  nextSub = 0;
  /** What `watchAdd` answers as the view's first answer. */
  addAnswer = (view: string): unknown => `${view}@1`;
  viewResult = (method: string, minIndex = 0) => ({ index: minIndex, result: `${method}@${minIndex}` });

  transport = {
    events: (_path: string, options: { query?: Record<string, unknown>; signal?: AbortSignal } = {}) => {
      const conn = new Conn(options.query ?? {}, options.signal);
      this.conns.push(conn);
      return conn.frames();
    },
  };

  apps = {
    view: async (args: { app: string; method: string; minIndex?: number }) => {
      this.reads.push({ method: args.method, minIndex: args.minIndex });
      return this.viewResult(args.method, args.minIndex);
    },
    watchAdd: async (args: { app: string; stream: string; view: string; args?: unknown; intervalMs?: number }) => {
      this.added.push(args);
      return { sub: `w${++this.nextSub}`, index: 1, answer: this.addAnswer(args.view) };
    },
    watchRemove: async (args: { app: string; sub: string }) => {
      this.removed.push(args.sub);
      return {};
    },
  };

  get conn(): Conn {
    return this.conns[this.conns.length - 1]!;
  }
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const settle = async () => {
  for (let i = 0; i < 10; i++) await tick();
};
const noSleep = () => Promise.resolve();

describe('ViewWatches', () => {
  it('shares one stream per app and delivers the first answer, then each result', async () => {
    const host = new StandIn();
    const watches = new ViewWatches(host, noSleep);
    const a: unknown[] = [];
    const b: unknown[] = [];
    watches.watch('app', 'get', { key: 'a' }, { onAnswer: (x) => a.push(x), intervalMs: 500 });
    watches.watch('app', 'entries', {}, { onAnswer: (x) => b.push(x) });
    await settle();
    expect(host.conns).toHaveLength(1);
    expect(host.conn.query.watch).toBe(true);
    host.conn.push('stream', { stream: 's1' });
    await settle();
    expect(host.added.map((x) => x.view)).toEqual(['get', 'entries']);
    expect(host.added[0]).toMatchObject({ stream: 's1', args: { key: 'a' }, intervalMs: 500 });
    expect(host.added[1]).not.toHaveProperty('intervalMs');
    expect(a).toEqual(['get@1']);
    expect(b).toEqual(['entries@1']);

    host.conn.push('view-result', { sub: 'w1', index: 5, answer: 'a5' }, '5:1');
    host.conn.push('view-result', { sub: 'w1', index: 3, answer: 'stale' }, '3:2');
    await settle();
    expect(a).toEqual(['get@1', 'a5']);
    expect(b).toEqual(['entries@1']);
  });

  it('takes the first answer from the add and reads the view only on view-changed', async () => {
    const host = new StandIn();
    const watches = new ViewWatches(host, noSleep);
    const seen: unknown[] = [];
    watches.watch('app', 'big', {}, { onAnswer: (x) => seen.push(x) });
    await settle();
    host.conn.push('stream', { stream: 's1' });
    await settle();
    expect(seen).toEqual(['big@1']);
    host.conn.push('view-changed', { sub: 'w1', index: 9 }, '9:1');
    await settle();
    expect(host.reads).toEqual([{ method: 'big', minIndex: 9 }]);
    expect(seen).toEqual(['big@1', 'big@9']);
  });

  it('reports an app error answer through onError and keeps watching', async () => {
    const host = new StandIn();
    const watches = new ViewWatches(host, noSleep);
    const seen: unknown[] = [];
    const errors: unknown[] = [];
    watches.watch('app', 'get', {}, { onAnswer: (x) => seen.push(x), onError: (e) => errors.push(e) });
    await settle();
    host.conn.push('stream', { stream: 's1' });
    await settle();
    host.conn.push('view-result', { sub: 'w1', index: 4, error: { code: 'NotFound', fields: { key: 'a' } } }, '4:1');
    await settle();
    expect(errors).toHaveLength(1);
    expect(errors[0]).toBeInstanceOf(AppError);
    expect(errors[0]).toMatchObject({ appCode: 'NotFound', fields: { key: 'a' } });
    host.conn.push('view-result', { sub: 'w1', index: 3, error: { code: 'Old' } }, '3:2');
    host.conn.push('view-result', { sub: 'w1', index: 5, answer: 'back' }, '5:3');
    await settle();
    expect(errors).toHaveLength(1);
    expect(seen).toEqual(['get@1', 'back']);
  });

  it('ends a watch on watch-ended and closes the stream with the last watch', async () => {
    const host = new StandIn();
    const watches = new ViewWatches(host, noSleep);
    const ended: (string | undefined)[] = [];
    const seen: unknown[] = [];
    watches.watch('app', 'get', {}, { onAnswer: (x) => seen.push(x), onEnded: (r) => ended.push(r) });
    await settle();
    host.conn.push('stream', { stream: 's1' });
    await settle();
    host.conn.push('watch-ended', { sub: 'w1', reason: 'access-lost' });
    host.conn.push('view-result', { sub: 'w1', index: 7, answer: 'after' });
    await settle();
    expect(ended).toEqual(['access-lost']);
    expect(seen).toEqual(['get@1']);
    expect(watches.state('app')).toBe('idle');
  });

  it('resumes the same stream after a drop, with the last index seen', async () => {
    const host = new StandIn();
    const watches = new ViewWatches(host, noSleep);
    const seen: unknown[] = [];
    watches.watch('app', 'get', {}, { onAnswer: (x) => seen.push(x) });
    await settle();
    host.conn.push('stream', { stream: 's1' });
    await settle();
    host.conn.push('view-result', { sub: 'w1', index: 4, answer: 'a4' }, '4:1');
    await settle();
    host.conn.end();
    await settle();
    expect(host.conns).toHaveLength(2);
    expect(host.conn.query).toMatchObject({ watch: true, stream: 's1', last: 4 });
    host.conn.push('stream', { stream: 's1' });
    host.conn.push('view-result', { sub: 'w1', index: 6, answer: 'a6' }, '6:2');
    await settle();
    // Resumed: the watch was not added again.
    expect(host.added).toHaveLength(1);
    expect(seen).toEqual(['get@1', 'a4', 'a6']);
  });

  it('adds every watch again on a new stream id or a watch-reset', async () => {
    const host = new StandIn();
    const watches = new ViewWatches(host, noSleep);
    watches.watch('app', 'get', {}, { onAnswer: () => undefined });
    await settle();
    host.conn.push('stream', { stream: 's1' });
    await settle();
    host.conn.end();
    await settle();
    // The node no longer holds s1: it answers with a new stream.
    host.conn.push('stream', { stream: 's2' });
    await settle();
    expect(host.added.map((x) => x.stream)).toEqual(['s1', 's2']);

    host.conn.push('watch-reset', {});
    await settle();
    expect(host.conn.query.stream).toBeUndefined();
    host.conn.push('stream', { stream: 's3' });
    await settle();
    expect(host.added.map((x) => x.stream)).toEqual(['s1', 's2', 's3']);
  });

  it('keeps a watch that replaces the last one in the same tick, as new arguments do', async () => {
    const host = new StandIn();
    const watches = new ViewWatches(host, noSleep);
    const a: unknown[] = [];
    const b: unknown[] = [];
    const first = watches.watch('app', 'get', { key: 'a' }, { onAnswer: (x) => a.push(x) });
    await settle();
    host.conn.push('stream', { stream: 's1' });
    await settle();
    first.close();
    watches.watch('app', 'get', { key: 'b' }, { onAnswer: (x) => b.push(x) });
    await settle();
    expect(host.removed).toEqual(['w1']);
    expect(host.added.map((x) => x.args)).toEqual([{ key: 'a' }, { key: 'b' }]);
    // The stream comes back as itself: the new watch is not added twice.
    host.conn.push('stream', { stream: 's1' });
    host.conn.push('view-result', { sub: 'w1', index: 7, answer: 'old' }, '7:1');
    host.conn.push('view-result', { sub: 'w2', index: 8, answer: 'b8' }, '8:2');
    await settle();
    expect(host.added).toHaveLength(2);
    expect(a).toEqual(['get@1']);
    expect(b).toEqual(['get@1', 'b8']);
  });

  it('adds a watch made while the stream connects once it names itself', async () => {
    const host = new StandIn();
    const watches = new ViewWatches(host, noSleep);
    const seen: unknown[] = [];
    watches.watch('app', 'get', {}, { onAnswer: (x) => seen.push(x) });
    watches.watch('app', 'entries', {}, { onAnswer: (x) => seen.push(x) });
    await settle();
    expect(host.added).toHaveLength(0);
    host.conn.push('stream', { stream: 's1' });
    await settle();
    expect(host.added.map((x) => [x.stream, x.view])).toEqual([
      ['s1', 'get'],
      ['s1', 'entries'],
    ]);
    expect(seen).toEqual(['get@1', 'entries@1']);
  });

  it('removes a closed watch on the node and reports the connection state', async () => {
    const host = new StandIn();
    const watches = new ViewWatches(host, noSleep);
    const states: WatchConnectionState[] = [];
    watches.onState((_app, s) => states.push(s));
    const ac = new AbortController();
    const one = watches.watch('app', 'get', {}, { onAnswer: () => undefined });
    watches.watch('app', 'entries', {}, { onAnswer: () => undefined, signal: ac.signal });
    await settle();
    host.conn.push('stream', { stream: 's1' });
    await settle();
    expect(watches.state('app')).toBe('open');
    one.close();
    await settle();
    expect(host.removed).toEqual(['w1']);
    ac.abort();
    await settle();
    expect(host.removed).toEqual(['w1', 'w2']);
    expect(states).toEqual(['connecting', 'open', 'idle']);
  });
});
