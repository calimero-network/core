/**
 * The two shapes every hook here is built from: a read that refetches when its
 * inputs change (latest request wins), and an action with loading / error
 * state. Both keep the client's typed error as it is in `error` and add
 * `describeError`'s reading in `failure`, and both hand the error to the
 * provider, which ends the session on a `LoginRequired`.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { passkeyConfirm } from '@calimero-network/mero-js';
import type { CalimeroClient, MethodOptions, PasskeyProvider } from '@calimero-network/mero-js';
import { useMero } from '../context';
import { describeError, type Failure } from '../errors';

/** Options every action hook takes. */
export interface MutationOptions {
  /**
   * Rethrow a failure from the action after recording it. By default an action
   * resolves `null` when it fails, which is enough for a button; set this when
   * the caller branches on the outcome.
   */
  throwOnError?: boolean;
}

export interface AsyncState {
  loading: boolean;
  /** The error as the client threw it: `instanceof AppError` etc. still work. */
  error: Error | null;
  /** {@link describeError}'s reading of `error`. */
  failure: Failure | null;
}

export class NotConnectedError extends Error {
  constructor(what = 'signed in') {
    super(`Not ${what}: sign in through MeroProvider first.`);
    this.name = 'NotConnectedError';
  }
}

export class NoAppError extends Error {
  constructor() {
    super('No app instance: pass an appId, or set one on MeroProvider (`appId`) or with `setAppId`.');
    this.name = 'NoAppError';
  }
}

/**
 * The per-call options of a generated method that answer the node's
 * `412 CONFIRM_REQUIRED` with `passkey`.
 */
export function confirmWith(passkey: PasskeyProvider): MethodOptions {
  return { confirm: passkeyConfirm(passkey) };
}

function toError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}

export function useMountedRef() {
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  return mounted;
}

/** The client when signed in, else `null`. */
export function useSignedInClient(): CalimeroClient | null {
  const { client, isAuthenticated } = useMero();
  return isAuthenticated ? client : null;
}

/** The app a hook works on: its own argument, else the provider's. */
export function useAppId(appId?: string | null): string | null {
  const { appId: fallback } = useMero();
  return appId === undefined ? fallback : appId;
}

export function useAsyncAction({ throwOnError = false }: MutationOptions = {}) {
  const { reportError } = useMero();
  const mounted = useMountedRef();
  const [state, setState] = useState<AsyncState>({ loading: false, error: null, failure: null });

  const run = useCallback(
    async <T>(action: () => Promise<T>): Promise<T | null> => {
      if (mounted.current) setState({ loading: true, error: null, failure: null });
      try {
        const result = await action();
        if (mounted.current) setState({ loading: false, error: null, failure: null });
        return result;
      } catch (error) {
        reportError(error);
        if (mounted.current) setState({ loading: false, error: toError(error), failure: describeError(error) });
        if (throwOnError) throw error;
        return null;
      }
    },
    [mounted, reportError, throwOnError],
  );

  const reset = useCallback(() => setState({ loading: false, error: null, failure: null }), []);

  return { ...state, run, reset };
}

export interface ResourceState<T> extends AsyncState {
  data: T;
  refetch: () => Promise<void>;
}

/**
 * A read. `fetcher` is `null` while the read cannot run (not signed in, no app):
 * `data` is then `initial`. When `deps` change, the old data is dropped during
 * render and any request still in flight for them is ignored.
 */
export function useResource<T>(
  fetcher: (() => Promise<T>) | null,
  initial: T,
  deps: readonly unknown[],
): ResourceState<T> {
  const { reportError } = useMero();
  const mounted = useMountedRef();
  const [data, setData] = useState<T>(initial);
  const [state, setState] = useState<AsyncState>({ loading: false, error: null, failure: null });
  const seq = useRef(0);
  const fetcherRef = useRef(fetcher);
  fetcherRef.current = fetcher;
  const initialRef = useRef(initial);

  const refetch = useCallback(async () => {
    const mine = ++seq.current;
    const fn = fetcherRef.current;
    if (!fn) {
      if (mounted.current) {
        setData(initialRef.current);
        setState({ loading: false, error: null, failure: null });
      }
      return;
    }
    if (mounted.current) setState({ loading: true, error: null, failure: null });
    try {
      const result = await fn();
      if (mounted.current && mine === seq.current) {
        setData(result);
        setState({ loading: false, error: null, failure: null });
      }
    } catch (error) {
      reportError(error);
      if (mounted.current && mine === seq.current) {
        setState({ loading: false, error: toError(error), failure: describeError(error) });
      }
    }
    // `deps` stand for everything `fetcher` closes over.
  }, deps);

  const previous = useRef(deps);
  if (deps.length !== previous.current.length || deps.some((d, i) => !Object.is(d, previous.current[i]))) {
    previous.current = deps;
    seq.current += 1;
    setData(initialRef.current);
  }

  useEffect(() => {
    void refetch();
  }, [refetch]);

  return { data, ...state, refetch };
}

/**
 * A small helper for the action hooks below: run `fn` with the signed-in
 * client, or fail with `NotConnectedError`.
 */
export function useClientAction(options?: MutationOptions) {
  const client = useSignedInClient();
  const action = useAsyncAction(options);
  const withClient = useCallback(
    <T>(fn: (c: NonNullable<typeof client>) => Promise<T>) =>
      action.run(() => {
        if (!client) throw new NotConnectedError();
        return fn(client);
      }),
    [client, action.run],
  );
  return { withClient, loading: action.loading, error: action.error, failure: action.failure };
}
