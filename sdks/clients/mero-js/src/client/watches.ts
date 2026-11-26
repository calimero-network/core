/**
 * Live views: a view marked `#[app::view(subscribable)]` watched over the app's watch
 * stream (`GET /apps/{app}/events?watch=true`), so its answer arrives when what it read
 * changes instead of being polled.
 *
 * One stream per app per client carries every watch of that app. Its first frame
 * (`stream`) names it; each watch is then added with `POST /apps/{app}/watches`. The stream
 * sends `view-result` (the new answer), `view-changed` (the answer was too large to send:
 * the client reads the view again), `watch-ended` (access lost, or the view no longer reads
 * a bounded set) and `watch-reset` (the node lost the stream: every watch is added again).
 * A dropped connection resumes the same stream (`stream`, `last`); a stream the node no
 * longer holds comes back with a new id, and every watch is added again.
 */

import { AppError } from '../runtime/errors.js';
import type { NodeEvent } from '../runtime/transport.js';

/** What a watch stream needs of the client: its event stream and the three app routes. */
export interface WatchHost {
  transport: {
    events(path: string, options?: { query?: Record<string, string | number | boolean | undefined>; signal?: AbortSignal }): AsyncGenerator<NodeEvent>;
  };
  apps: {
    view(args: { app: string; method: string; args?: unknown; minIndex?: number }): Promise<{ index: number; result: unknown }>;
    watchAdd(args: { app: string; stream: string; view: string; args?: unknown; intervalMs?: number }): Promise<{ sub: string; index: number; answer: unknown }>;
    watchRemove(args: { app: string; sub: string }): Promise<unknown>;
  };
}

/**
 * Why the node ended a watch: the watcher lost access to the app or view, the view now
 * refuses the watcher, or what it reads is no longer bounded enough to watch.
 */
export type WatchEndReason = 'access-lost' | 'view-refused' | 'read-set-unbounded';

/** How a watch stream's connection stands. */
export type WatchConnectionState = 'idle' | 'connecting' | 'open' | 'reconnecting';

/** Options of {@link ViewWatches.watch} (`client.watchView`). */
export interface WatchViewOptions<T> {
  /** The view's answer: at once, then each time it changes. */
  onAnswer: (answer: T, index: number) => void;
  /** The node ended the watch; no answer follows. An unknown reason is passed as is. */
  onEnded?: (reason: WatchEndReason | undefined) => void;
  /**
   * Adding the watch or reading the view failed, or the view answered an app error
   * (an `AppError`); the watch carries on with the next change.
   */
  onError?: (error: unknown) => void;
  /** At most one answer per interval, in ms (the node clamps it to 250 ms–10 s; default 1 s). */
  intervalMs?: number;
  /** Ends the watch, as {@link WatchHandle.close} does. */
  signal?: AbortSignal;
}

/** A watch: `close()` ends it. */
export interface WatchHandle {
  close(): void;
}

interface Entry {
  view: string;
  args: unknown;
  intervalMs?: number;
  onAnswer: (answer: unknown, index: number) => void;
  onEnded?: (reason: WatchEndReason | undefined) => void;
  onError?: (error: unknown) => void;
  /** The node's id for it, once added on the current stream. */
  sub?: string;
  /** The newest index delivered: older answers are dropped. */
  index: number;
  closed: boolean;
}

/** Reconnect backoff: 200 ms doubling up to 5 s. */
const RETRY_BASE_MS = 200;
const RETRY_MAX_MS = 5_000;

/** One app's watch stream and the watches it carries. */
class AppStream {
  private readonly entries = new Set<Entry>();
  private readonly bySub = new Map<string, Entry>();
  private stream: string | undefined;
  private last: number | undefined;
  private abort: AbortController | undefined;
  private running = false;
  state: WatchConnectionState = 'idle';

  constructor(
    private readonly host: WatchHost,
    private readonly app: string,
    private readonly onState: (app: string, state: WatchConnectionState) => void,
    private readonly sleep: (ms: number) => Promise<void>,
  ) {}

  add(entry: Entry): void {
    this.entries.add(entry);
    if (this.stream && this.state === 'open') void this.register(entry);
    if (!this.running) void this.run();
  }

  remove(entry: Entry): void {
    entry.closed = true;
    this.entries.delete(entry);
    if (entry.sub) {
      this.bySub.delete(entry.sub);
      this.host.apps.watchRemove({ app: this.app, sub: entry.sub }).catch(() => undefined);
    }
    if (this.entries.size === 0) this.abort?.abort();
  }

  private setState(state: WatchConnectionState): void {
    if (this.state === state) return;
    this.state = state;
    this.onState(this.app, state);
  }

  private async run(): Promise<void> {
    this.running = true;
    let retry = 0;
    try {
      while (this.entries.size > 0) {
        this.abort = new AbortController();
        this.setState(this.stream ? 'reconnecting' : 'connecting');
        try {
          const frames = this.host.transport.events(`apps/${encodeURIComponent(this.app)}/events`, {
            query: { watch: true, stream: this.stream, last: this.stream ? this.last : undefined },
            signal: this.abort.signal,
          });
          for await (const frame of frames) {
            retry = 0;
            if (this.handle(frame) === 'reset') break;
          }
        } catch (error) {
          if (this.entries.size === 0) break;
          void error;
        }
        if (this.entries.size === 0) break;
        const delay = Math.min(RETRY_BASE_MS * 2 ** retry, RETRY_MAX_MS);
        retry += 1;
        await this.sleep(delay);
      }
    } finally {
      this.running = false;
      this.abort = undefined;
      this.stream = undefined;
      this.last = undefined;
      this.bySub.clear();
      this.setState('idle');
    }
  }

  /** Handles one frame; `'reset'` asks for a new stream. */
  private handle(frame: NodeEvent): 'reset' | undefined {
    const data = (frame.data ?? {}) as Record<string, unknown>;
    const index = Number(String(frame.id ?? '').split(':')[0]);
    if (frame.id && Number.isFinite(index)) this.last = Math.max(this.last ?? 0, index);
    switch (frame.kind) {
      case 'stream': {
        const id = String(data.stream ?? '');
        const resumed = id === this.stream;
        this.stream = id;
        this.setState('open');
        if (!resumed) {
          // A new stream holds none of our watches: add every one again.
          this.last = undefined;
          this.bySub.clear();
          for (const entry of this.entries) {
            entry.sub = undefined;
            void this.register(entry);
          }
        }
        return undefined;
      }
      case 'view-result': {
        const entry = this.bySub.get(String(data.sub));
        if (!entry) return undefined;
        if ('error' in data) this.fail(entry, data.error, Number(data.index));
        else this.deliver(entry, data.answer, Number(data.index));
        return undefined;
      }
      case 'view-changed': {
        const entry = this.bySub.get(String(data.sub));
        if (entry) void this.read(entry, Number(data.index));
        return undefined;
      }
      case 'watch-ended': {
        const entry = this.bySub.get(String(data.sub));
        if (entry) {
          this.bySub.delete(String(data.sub));
          this.entries.delete(entry);
          entry.closed = true;
          entry.onEnded?.(typeof data.reason === 'string' ? (data.reason as WatchEndReason) : undefined);
          if (this.entries.size === 0) this.abort?.abort();
        }
        return undefined;
      }
      case 'watch-reset':
        this.stream = undefined;
        this.last = undefined;
        return 'reset';
      default:
        return undefined;
    }
  }

  private async register(entry: Entry): Promise<void> {
    const stream = this.stream;
    if (!stream || entry.closed) return;
    try {
      const added = await this.host.apps.watchAdd({
        app: this.app,
        stream,
        view: entry.view,
        args: entry.args,
        ...(entry.intervalMs === undefined ? {} : { intervalMs: entry.intervalMs }),
      });
      if (entry.closed || this.stream !== stream) {
        this.host.apps.watchRemove({ app: this.app, sub: added.sub }).catch(() => undefined);
        return;
      }
      entry.sub = added.sub;
      this.bySub.set(added.sub, entry);
      // `index` is the app's last applied entry, the one the stream's frames carry.
      this.deliver(entry, added.answer, added.index);
    } catch (error) {
      entry.onError?.(error);
    }
  }

  private async read(entry: Entry, minIndex: number): Promise<void> {
    try {
      const answer = await this.host.apps.view({ app: this.app, method: entry.view, args: entry.args, minIndex });
      this.deliver(entry, answer.result, answer.index);
    } catch (error) {
      if (!entry.closed) entry.onError?.(error);
    }
  }

  /** The view answered app error `error` at `index`: reported, unless an answer after it was delivered. */
  private fail(entry: Entry, error: unknown, index: number): void {
    if (entry.closed || (Number.isFinite(index) && index < entry.index)) return;
    if (Number.isFinite(index)) entry.index = index;
    entry.onError?.(new AppError('the watched view answered an app error', { error }));
  }

  private deliver(entry: Entry, answer: unknown, index: number): void {
    if (entry.closed) return;
    const at = Number.isFinite(index) ? index : entry.index;
    if (at < entry.index) return;
    entry.index = at;
    entry.onAnswer(answer, at);
  }
}

/** Every app's watch stream of one client (`client.watches`). */
export class ViewWatches {
  private readonly streams = new Map<string, AppStream>();
  private readonly listeners = new Set<(app: string, state: WatchConnectionState) => void>();

  constructor(
    private readonly host: WatchHost,
    private readonly sleep: (ms: number) => Promise<void> = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
  ) {}

  /**
   * Watches view `view` of `app` with `args`: `onAnswer` gets its answer at once, then each
   * time it changes, until `close()` (or `signal`).
   */
  watch<T = unknown>(app: string, view: string, args: unknown, options: WatchViewOptions<T>): WatchHandle {
    let stream = this.streams.get(app);
    if (!stream) {
      stream = new AppStream(this.host, app, (a, s) => this.emit(a, s), this.sleep);
      this.streams.set(app, stream);
    }
    const entry: Entry = {
      view,
      args,
      intervalMs: options.intervalMs,
      onAnswer: options.onAnswer as (answer: unknown, index: number) => void,
      onEnded: options.onEnded,
      onError: options.onError,
      index: 0,
      closed: false,
    };
    const owner = stream;
    const close = () => {
      if (!entry.closed) owner.remove(entry);
      options.signal?.removeEventListener('abort', close);
    };
    if (options.signal?.aborted) return { close: () => undefined };
    options.signal?.addEventListener('abort', close);
    owner.add(entry);
    return { close };
  }

  /** How `app`'s watch stream stands (`idle` with no watches). */
  state(app: string): WatchConnectionState {
    return this.streams.get(app)?.state ?? 'idle';
  }

  /** Calls `listener` on every change of any app's stream state; returns its removal. */
  onState(listener: (app: string, state: WatchConnectionState) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private emit(app: string, state: WatchConnectionState): void {
    for (const listener of this.listeners) listener(app, state);
  }
}
