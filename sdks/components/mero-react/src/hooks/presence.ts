/**
 * Presence (`/apps/{app}/presence`): who is in an app now, and a small state
 * each of them shares (a cursor, "typing"). In memory on every node, never
 * logged.
 *
 * Each device publishes `(seq, state)`; the newest `seq` wins, and an entry
 * the device does not repeat within 7 seconds is gone. The hook heartbeats
 * every 5 seconds and retracts (`state: null`) on unmount. The stream answers
 * a `snapshot` first, then a `presence` event per change.
 *
 * Same name as before (`useEphemeral`); it now takes an app id where it took a
 * context id, and keys peers by account.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { LoginRequired, presenceState } from '@calimero-network/mero-js';
import { useMero } from '../context';
import { useAppId, useSignedInClient } from './internal';

/** How a state goes to bytes and back (at most 4 KiB). Default: JSON. */
export interface Codec<T> {
  encode(value: T): Uint8Array;
  decode(bytes: Uint8Array): T;
}

export const jsonCodec: Codec<any> = {
  encode: (value) => new TextEncoder().encode(JSON.stringify(value)),
  decode: (bytes) => JSON.parse(new TextDecoder().decode(bytes)),
};

/** One device present in the app. */
export interface EphemeralEntry<T> {
  account: string;
  device: string;
  state: T;
}

export interface UseEphemeralOptions<T> {
  codec?: Codec<T>;
  /** The state to start from for the first `setPresence`. */
  initial?: T;
  /** Least time between publishes; the last value wins. Default 100 ms. */
  throttleMs?: number;
  /** Include this account's own entries in `peers`. Default false. */
  includeSelf?: boolean;
  /** Publish and follow only while true. Default true. */
  enabled?: boolean;
}

export interface UseEphemeralResult<T> {
  /** Account → its newest state (one entry per account; see `entries` for devices). */
  peers: Map<string, T>;
  /** Every device present. */
  entries: EphemeralEntry<T>[];
  /** Merge into this device's state and publish it. */
  setPresence: (partial: Partial<T>) => void;
  /** Retract this device's presence now (it also happens on unmount). */
  leave: () => void;
  error: Error | null;
}

const HEARTBEAT_MS = 5000;

function hexToBytes(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}

interface Diff {
  account: string;
  device: string;
  state: string | null;
}

function key(d: { account: string; device: string }): string {
  return `${d.account}\u0000${d.device}`;
}

export function useEphemeral<T extends object = Record<string, unknown>>(
  appId: string | null | undefined,
  options: UseEphemeralOptions<T> = {},
): UseEphemeralResult<T> {
  const client = useSignedInClient();
  const { accountId, reportError } = useMero();
  const id = useAppId(appId);
  const { throttleMs = 100, includeSelf = false, enabled = true } = options;
  const codec = (options.codec ?? jsonCodec) as Codec<T>;
  const codecRef = useRef(codec);
  codecRef.current = codec;

  const [entries, setEntries] = useState<Map<string, EphemeralEntry<T>>>(new Map());
  const [error, setError] = useState<Error | null>(null);

  // Follow the stream.
  useEffect(() => {
    setEntries(new Map());
    if (!client || !id || !enabled) return;
    const controller = new AbortController();
    const decode = (d: Diff): EphemeralEntry<T> | null => {
      if (d.state === null) return null;
      try {
        return { account: d.account, device: d.device, state: codecRef.current.decode(hexToBytes(d.state)) };
      } catch {
        return null;
      }
    };
    void (async () => {
      let backoff = 1000;
      while (!controller.signal.aborted) {
        try {
          for await (const frame of client.presenceStream(id, controller.signal)) {
            backoff = 1000;
            if (frame.kind === 'snapshot' && Array.isArray(frame.data)) {
              const next = new Map<string, EphemeralEntry<T>>();
              for (const d of frame.data as Diff[]) {
                const entry = decode(d);
                if (entry) next.set(key(d), entry);
              }
              setEntries(next);
            } else if (frame.kind === 'presence' && frame.data && typeof frame.data === 'object') {
              const d = frame.data as Diff;
              const entry = decode(d);
              setEntries((prev) => {
                const next = new Map(prev);
                if (entry) next.set(key(d), entry);
                else next.delete(key(d));
                return next;
              });
            }
          }
        } catch (e) {
          if (controller.signal.aborted) return;
          if (e instanceof LoginRequired) {
            reportError(e);
            return;
          }
          setError(e instanceof Error ? e : new Error(String(e)));
        }
        await new Promise((resolve) => setTimeout(resolve, backoff));
        backoff = Math.min(backoff * 2, 30_000);
      }
    })();
    return () => controller.abort();
  }, [client, id, enabled, reportError]);

  // Publish.
  const state = useRef<T | undefined>(options.initial);
  const seq = useRef(Date.now());
  const published = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastSent = useRef(0);

  const send = useCallback(
    (value: T | null) => {
      if (!client || !id) return;
      seq.current += 1;
      const bytes = value === null ? null : codecRef.current.encode(value);
      lastSent.current = Date.now();
      // `state: null` retracts.
      const state = presenceState(bytes);
      client.apps.presencePublish({ app: id, seq: seq.current, state }).then(
        () => setError(null),
        (e: unknown) => setError(e instanceof Error ? e : new Error(String(e))),
      );
    },
    [client, id],
  );

  const setPresence = useCallback(
    (partial: Partial<T>) => {
      state.current = { ...(state.current ?? ({} as T)), ...partial } as T;
      published.current = true;
      if (timer.current) return;
      const wait = Math.max(0, throttleMs - (Date.now() - lastSent.current));
      timer.current = setTimeout(() => {
        timer.current = null;
        if (enabled && state.current) send(state.current);
      }, wait);
    },
    [send, throttleMs, enabled],
  );

  const leave = useCallback(() => {
    if (!published.current) return;
    published.current = false;
    send(null);
  }, [send]);

  // Heartbeat, and retract on unmount or when the app changes.
  useEffect(() => {
    if (!client || !id || !enabled) return;
    const beat = setInterval(() => {
      if (published.current && state.current && Date.now() - lastSent.current >= HEARTBEAT_MS - 50) send(state.current);
    }, HEARTBEAT_MS);
    return () => {
      clearInterval(beat);
      if (timer.current) {
        clearTimeout(timer.current);
        timer.current = null;
      }
      if (published.current) {
        published.current = false;
        send(null);
      }
    };
  }, [client, id, enabled, send]);

  const list = [...entries.values()].filter((e) => includeSelf || e.account !== accountId);
  const peers = new Map<string, T>();
  for (const e of list) peers.set(e.account, e.state);

  return { peers, entries: list, setPresence, leave, error };
}
