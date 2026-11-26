/**
 * One app instance: its record, calls (`/apps/{app}/intents`) and views
 * (`/apps/{app}/views`).
 *
 * A call carries an idempotency key (yours, or one the client mints) and is
 * retried with the same key while the node answers `503`; a call that
 * committed is answered from its record, never run twice. A method the app
 * marks `#[app::confirm]` answers `412 CONFIRM_REQUIRED`: pass `confirm: true`
 * and the passkey prompt answers it.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import type { CalimeroClient, CallResult, ViewResult, WatchConnectionState } from '@calimero-network/mero-js';
import { describeError } from '../errors';
import { useMero } from '../context';
import {
  confirmWith,
  NoAppError,
  NotConnectedError,
  toError,
  useAppId,
  useAsyncAction,
  useResource,
  useSignedInClient,
  type MutationOptions,
  type ResourceState,
} from './internal';

export interface CallOptions {
  /** Your own idempotency key (kept 24 hours by the node). */
  idempotencyKey?: string;
  /** Answer a `412 CONFIRM_REQUIRED` with the passkey. */
  confirm?: boolean;
}

export interface ViewOptions {
  /** Read from this node's replica without asking the leader. */
  stale?: boolean;
  /** With `stale`: wait until the replica has applied this log index (a call's `index`). */
  minIndex?: number;
}

/**
 * `POST /apps/{app}/views`: strong unless `stale`. Strong is what the node
 * reads for any `consistency` but `stale`, so it is sent as no `consistency`.
 */
function viewOf(client: CalimeroClient, app: string, method: string, args: unknown, options: ViewOptions): Promise<ViewResult> {
  return client.apps.view({
    app,
    method,
    args,
    consistency: options.stale ? 'stale' : undefined,
    minIndex: options.minIndex,
  });
}

/**
 * The app the page works on, with its record and bound `call` / `view`.
 *
 * `call` and `view` throw the client's typed errors as they are; for loading
 * and error state around a call, use {@link useExecute}.
 */
export function useApp(appId?: string | null) {
  const { client: anyClient, passkey } = useMero();
  const client = useSignedInClient();
  const id = useAppId(appId);

  const info = useResource<Record<string, unknown> | null>(
    client && id ? () => client.apps.get({ app: id }) : null,
    null,
    [client, id],
  );

  const call = useCallback(
    (method: string, args: unknown = {}, options: CallOptions = {}): Promise<CallResult> => {
      if (!anyClient) return Promise.reject(new NotConnectedError('connected'));
      if (!id) return Promise.reject(new NoAppError());
      // Without `idempotencyKey` the client mints one, so its own retries of a
      // `503` share it. A confirmed call goes out once: its challenge is
      // single-use.
      return anyClient.apps.call(
        { app: id, method, args, idempotencyKey: options.idempotencyKey },
        options.confirm ? confirmWith(passkey) : undefined,
      );
    },
    [anyClient, id, passkey],
  );

  const view = useCallback(
    (method: string, args: unknown = {}, options: ViewOptions = {}): Promise<ViewResult> => {
      if (!anyClient) return Promise.reject(new NotConnectedError('connected'));
      if (!id) return Promise.reject(new NoAppError());
      return viewOf(anyClient, id, method, args, options);
    },
    [anyClient, id],
  );

  return { appId: id, app: info.data, loading: info.loading, error: info.error, failure: info.failure, refetch: info.refetch, call, view };
}

/**
 * Calls with loading and error state. Was `useExecute(contextId, executorId)`:
 * there is no executor any more (the caller is the signed-in account), and the
 * context is now an app id.
 *
 * `execute` resolves the method's result, `call` the whole `CallResult`
 * (`index`, `events`), and both resolve `null` on failure unless
 * `throwOnError`.
 */
export function useExecute(appId?: string | null, options: MutationOptions = {}) {
  const { call: bound } = useApp(appId);
  const { run, loading, error, failure, reset } = useAsyncAction(options);

  const call = useCallback(
    (method: string, args?: unknown, callOptions?: CallOptions) => run(() => bound(method, args, callOptions)),
    [run, bound],
  );

  const execute = useCallback(
    async <T = unknown>(method: string, args?: unknown, callOptions?: CallOptions): Promise<T | null> => {
      const answered = await call(method, args, callOptions);
      return answered ? (answered.result as T) : null;
    },
    [call],
  );

  return { execute, call, loading, error, failure, reset };
}

export interface UseViewOptions extends ViewOptions {
  appId?: string | null;
  /** Do not read while false. Default true. */
  enabled?: boolean;
  /**
   * Watch the view (one marked `#[app::view(subscribable)]`): the answer updates on its
   * own when what the view read changes, over the app's shared watch stream, instead of
   * being read once.
   */
  live?: boolean;
  /** With `live`: a longer interval than the app's, in ms; it can only slow updates (up to 10 s). */
  intervalMs?: number;
}

export interface UseViewResult<T> extends ResourceState<T | undefined> {
  /** The log index the answer reflects. */
  index: number | null;
  /** With `live`: why the node ended the watch (access lost, ...); no update follows. */
  ended: string | null;
}

interface LiveState {
  data: unknown;
  index: number | null;
  loading: boolean;
  error: Error | null;
  ended: string | null;
}

const NOT_LIVE: LiveState = { data: undefined, index: null, loading: false, error: null, ended: null };

/**
 * A view's answer, read again when the method or its arguments change. With `live`, it
 * is watched and updates on its own; otherwise pair it with `useSubscription` and call
 * `refetch` on the events that matter.
 */
export function useView<T = unknown>(method: string, args: unknown = {}, options: UseViewOptions = {}): UseViewResult<T> {
  const client = useSignedInClient();
  const { reportError } = useMero();
  const id = useAppId(options.appId);
  const argsKey = JSON.stringify(args ?? {});
  const { stale, minIndex, enabled = true, live = false, intervalMs } = options;

  const resource = useResource<ViewResult | null>(
    client && id && enabled && !live ? () => viewOf(client, id, method, JSON.parse(argsKey), { stale, minIndex }) : null,
    null,
    [client, id, method, argsKey, stale, minIndex, enabled, live],
  );

  const [watched, setWatched] = useState<LiveState>(NOT_LIVE);
  const reportRef = useRef(reportError);
  reportRef.current = reportError;
  useEffect(() => {
    if (!live || !client || !id || !enabled) {
      setWatched(NOT_LIVE);
      return;
    }
    setWatched({ ...NOT_LIVE, loading: true });
    const handle = client.watchView(id, method, JSON.parse(argsKey), {
      intervalMs,
      onAnswer: (answer, index) => setWatched({ data: answer, index, loading: false, error: null, ended: null }),
      onError: (error) => {
        reportRef.current(error);
        setWatched((prev) => ({ ...prev, loading: false, error: toError(error) }));
      },
      onEnded: (reason) => setWatched((prev) => ({ ...prev, loading: false, ended: reason ?? 'ended' })),
    });
    return () => handle.close();
  }, [client, id, method, argsKey, enabled, live, intervalMs]);

  const refetchLive = useCallback(async () => {
    if (!client || !id) return;
    try {
      const answer = await viewOf(client, id, method, JSON.parse(argsKey), {});
      setWatched((prev) => ({ ...prev, data: answer.result, index: answer.index, error: null }));
    } catch (error) {
      reportRef.current(error);
      setWatched((prev) => ({ ...prev, error: toError(error) }));
    }
  }, [client, id, method, argsKey]);

  if (live) {
    return {
      data: watched.data as T | undefined,
      index: watched.index,
      loading: watched.loading,
      error: watched.error,
      failure: watched.error ? describeError(watched.error) : null,
      refetch: refetchLive,
      ended: watched.ended,
    };
  }
  return {
    ...resource,
    data: resource.data ? (resource.data.result as T) : undefined,
    index: resource.data ? resource.data.index : null,
    ended: null,
  };
}

/**
 * How the app's watch stream stands (`idle` with no live view, `connecting`, `open`,
 * `reconnecting`), for a "reconnecting…" hint beside live views.
 */
export function useConnectionState(appId?: string | null): WatchConnectionState {
  const client = useSignedInClient();
  const id = useAppId(appId);
  const [state, setState] = useState<WatchConnectionState>(() => (client && id ? client.watches.state(id) : 'idle'));
  useEffect(() => {
    if (!client || !id) {
      setState('idle');
      return;
    }
    setState(client.watches.state(id));
    return client.watches.onState((app, next) => {
      if (app === id) setState(next);
    });
  }, [client, id]);
  return state;
}

/** The app's ABI (methods, events, errors), as the node reports it. */
export function useAppAbi(appId?: string | null) {
  const client = useSignedInClient();
  const id = useAppId(appId);
  return useResource<Record<string, unknown> | null>(
    client && id ? () => client.apps.abi({ app: id }) : null,
    null,
    [client, id],
  );
}
