/**
 * Live events (`GET /apps/{app}/events`, server-sent events over the sealed
 * channel).
 *
 * Each subscribed app gets one stream with an `AbortController`; unmounting,
 * or changing the app ids, aborts it (mero-js ends an aborted stream within a
 * few hundred milliseconds, mero-js#312). A stream that ends or fails by itself
 * reopens with backoff, from the last event seen: the node numbers events
 * `index:seq`, and anything at or before the last one delivered is skipped, so
 * a reconnect neither drops nor repeats what this page already saw.
 */

import { useEffect, useRef } from 'react';
import { LoginRequired, NotFound } from '@calimero-network/mero-js';
import type { CalimeroClient } from '@calimero-network/mero-js';
import { useMero } from '../context';
import { useAppId, useSignedInClient } from './internal';

/** One event as `useSubscription` delivers it. */
export interface AppEvent {
  /** The app it came from. */
  appId: string;
  /** The node's id for it, `index:seq`. */
  id: string;
  /** The log index of the call that emitted it. */
  index: number;
  /** Its position among that call's events. */
  seq: number;
  /** The event's name (`ItemAdded`, ...). */
  kind: string;
  /** Its payload, parsed from JSON. */
  data: unknown;
}

/** @deprecated The 0.x name; events are {@link AppEvent}s now. */
export type SubscriptionEventData = AppEvent;

export interface SubscriptionOptions {
  /**
   * Where to start: a log index to replay from (`0` is the whole history), or
   * `'live'` (default) for events after the stream opens.
   */
  from?: number | 'live';
  /** Do not subscribe while false. Default true. */
  enabled?: boolean;
  /** Called when a stream fails; it reopens unless the session ended or the app is gone. */
  onError?: (error: unknown, appId: string) => void;
  /** Called each time a stream (re)opens. Refetch views here to cover a gap. */
  onOpen?: (appId: string) => void;
  /** Longest wait between reconnects. Default 30 s. */
  maxBackoffMs?: number;
}

/**
 * The node takes `from` as the first log index to replay. A number past every
 * index replays nothing and leaves only what arrives live.
 */
const LIVE = Number.MAX_SAFE_INTEGER;

function position(id: string): [number, number] | null {
  const [index, seq] = id.split(':').map(Number);
  return Number.isFinite(index) && Number.isFinite(seq) ? [index, seq] : null;
}

function after(a: [number, number], b: [number, number]): boolean {
  return a[0] > b[0] || (a[0] === b[0] && a[1] > b[1]);
}

function sleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    signal.addEventListener(
      'abort',
      () => {
        clearTimeout(timer);
        resolve();
      },
      { once: true },
    );
  });
}

interface Follow {
  client: CalimeroClient;
  appId: string;
  from: number | 'live';
  signal: AbortSignal;
  maxBackoffMs: number;
  deliver: (event: AppEvent) => void;
  opened: (appId: string) => void;
  failed: (error: unknown, appId: string) => void;
  sessionEnded: (error: unknown) => void;
}

/** Follow one app's events until `signal` aborts. Exported for tests. */
export async function followEvents(f: Follow): Promise<void> {
  let last: [number, number] | null = null;
  let backoff = 1000;
  while (!f.signal.aborted) {
    const from = last ? last[0] : f.from === 'live' ? LIVE : f.from;
    try {
      const stream = f.client.events(f.appId, from, f.signal);
      let opened = false;
      for await (const frame of stream) {
        if (!opened) {
          opened = true;
          backoff = 1000;
          f.opened(f.appId);
        }
        if (f.signal.aborted) return;
        const at = position(frame.id);
        if (at && last && !after(at, last)) continue;
        if (at) last = at;
        f.deliver({
          appId: f.appId,
          id: frame.id,
          index: at ? at[0] : -1,
          seq: at ? at[1] : -1,
          kind: frame.kind,
          data: frame.data,
        });
      }
      // The node closed the stream (a leader change, a restart): open it again.
    } catch (error) {
      if (f.signal.aborted) return;
      if (error instanceof LoginRequired) {
        f.sessionEnded(error);
        return;
      }
      f.failed(error, f.appId);
      if (error instanceof NotFound) return;
    }
    await sleep(backoff, f.signal);
    backoff = Math.min(backoff * 2, f.maxBackoffMs);
  }
}

/**
 * Call `callback` for every event of the given app(s). Pass `null` (or an
 * empty list) to subscribe to nothing. Without app ids it follows the
 * provider's `appId`.
 *
 * Was `useSubscription(contextIds, callback)`: same shape, app ids in place of
 * context ids. Group subscriptions are gone (teams emit no app events).
 */
export function useSubscription(
  appIds: string | string[] | null | undefined,
  callback: (event: AppEvent) => void,
  options: SubscriptionOptions = {},
): void {
  const client = useSignedInClient();
  const { reportError } = useMero();
  const fallback = useAppId(undefined);
  const ids = appIds === undefined ? (fallback ? [fallback] : []) : appIds === null ? [] : Array.isArray(appIds) ? appIds : [appIds];
  const idsKey = JSON.stringify([...new Set(ids)].sort());
  const { from = 'live', enabled = true, maxBackoffMs = 30_000 } = options;

  const callbackRef = useRef(callback);
  callbackRef.current = callback;
  const optionsRef = useRef(options);
  optionsRef.current = options;

  useEffect(() => {
    const list = JSON.parse(idsKey) as string[];
    if (!client || !enabled || list.length === 0) return;
    const controller = new AbortController();
    for (const appId of list) {
      void followEvents({
        client,
        appId,
        from,
        signal: controller.signal,
        maxBackoffMs,
        deliver: (event) => callbackRef.current(event),
        opened: (id) => optionsRef.current.onOpen?.(id),
        failed: (error, id) => optionsRef.current.onError?.(error, id),
        sessionEnded: (error) => {
          optionsRef.current.onError?.(error, appId);
          reportError(error);
        },
      });
    }
    return () => controller.abort();
  }, [client, idsKey, from, enabled, maxBackoffMs, reportError]);
}
