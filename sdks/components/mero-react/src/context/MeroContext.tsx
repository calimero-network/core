/**
 * `MeroProvider`: one `CalimeroClient` (mero-js 28, Calimero 1.0) for the tree,
 * the passkey session it holds, and the app instance hooks default to.
 *
 * The client talks to one node (`nodeUrl`, plus `nodes` to fail over to) or
 * to a Calimero Cloud cluster by name (`cluster`), whose nodes the public node
 * directory names; either way each node is attested under the policy before
 * anything is sent to it.
 *
 * There is no redirect, no callback and no token in the URL any more. Signing
 * in is a WebAuthn ceremony against the node itself (`/auth/passkey/login`),
 * over a channel sealed to the node's attested key. The session it opens is a
 * 15-minute token plus a 30-day single-use refresh token; the client refreshes
 * on a `401` by itself (one refresh shared by every request waiting on it, on
 * another node when the issuing one is gone) and moves to the next node on
 * `421 WRONG_NODE`. The provider only keeps the newest session in its store and
 * drops it when a refresh is refused.
 */

import React, { useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { browserPasskey, CalimeroClient, LoginRequired } from '@calimero-network/mero-js';
import type { CalimeroSession, PasskeyProvider } from '@calimero-network/mero-js';
import { reportSessions, resolvePolicy, SessionReportingClient } from '../client';
import { describeError, type Failure } from '../errors';
import {
  clearCluster,
  clearNodeUrl,
  getAppId,
  getCluster,
  getNodeUrl,
  localStorageSessionStore,
  setAppId as storeAppId,
  clearAppId,
  setCluster as storeCluster,
  setNodeUrl as storeNodeUrl,
} from '../storage';
import type { MeroContextValue, MeroProviderConfig } from '../types';

import { MeroContext } from './context';

export { MeroContext };

export interface MeroProviderProps extends MeroProviderConfig {
  children: React.ReactNode;
}

const isBrowser = typeof window !== 'undefined';

function trimUrl(url: string): string {
  return url.replace(/\/+$/, '');
}

/** What the client talks to: one node (and `nodes`), or a cluster by name. */
type Target = { node: string } | { cluster: string } | null;

export function MeroProvider({
  children,
  nodeUrl: nodeUrlProp,
  nodes,
  cluster: clusterProp,
  zone,
  directory,
  policy,
  minReleaseVersion,
  allowMock = false,
  servedByNode = false,
  appId: appIdProp,
  sessionStore = localStorageSessionStore,
  passkey: passkeyProp,
  fetch: fetchProp,
  retry,
  client: injectedClient,
  onError,
}: MeroProviderProps) {
  const ownOrigin = servedByNode && isBrowser ? window.location.origin : null;

  const [chosen, setChosen] = useState<Target>(() => {
    if (ownOrigin) return { node: ownOrigin };
    if (nodeUrlProp) return { node: trimUrl(nodeUrlProp) };
    if (clusterProp) return { cluster: clusterProp };
    const keptCluster = getCluster();
    if (keptCluster) return { cluster: keptCluster };
    const keptNode = getNodeUrl();
    return keptNode ? { node: keptNode } : null;
  });
  const target: Target = ownOrigin ? { node: ownOrigin } : chosen;
  const chosenNode = target && 'node' in target ? target.node : null;
  const chosenCluster = target && 'cluster' in target ? target.cluster : null;

  const [session, setSession] = useState<CalimeroSession | null>(() => sessionStore.load());
  const [appId, setAppIdState] = useState<string | null>(() => appIdProp ?? getAppId());
  const [isLoading, setIsLoading] = useState(false);
  const [failure, setFailure] = useState<Failure | null>(null);
  // Bumped to throw the client away: it has no way to forget a credential.
  const [generation, setGeneration] = useState(0);

  useEffect(() => {
    if (appIdProp) setAppIdState(appIdProp);
  }, [appIdProp]);

  const passkey = useMemo<PasskeyProvider>(() => passkeyProp ?? browserPasskey(), [passkeyProp]);

  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;
  const storeRef = useRef(sessionStore);
  storeRef.current = sessionStore;

  const resolvedPolicy = useMemo(
    () => (injectedClient ? null : resolvePolicy({ policy, minReleaseVersion, allowMock })),
    // A policy object literal changes identity every render; its content does not.
    [injectedClient, JSON.stringify(policy ?? null), policy?.verifyQuote, minReleaseVersion, allowMock],
  );
  const nodesKey = JSON.stringify(nodes ?? []);
  const retryKey = JSON.stringify(retry ?? {});

  const keep = useCallback((fresh: CalimeroSession) => {
    storeRef.current.save(fresh);
    setSession(fresh);
  }, []);

  const nodeClient = useMemo<CalimeroClient | null>(() => {
    if (injectedClient) return injectedClient;
    if (!chosenNode || !resolvedPolicy) return null;
    const built = new SessionReportingClient(
      {
        baseUrl: chosenNode,
        nodes: JSON.parse(nodesKey) as string[],
        policy: resolvedPolicy,
        fetch: fetchProp,
        retry: JSON.parse(retryKey) as typeof retry,
      },
      keep,
    );
    const kept = storeRef.current.load();
    if (kept) built.restore(kept);
    return built;
  }, [injectedClient, chosenNode, nodesKey, resolvedPolicy, fetchProp, retryKey, generation, keep]);

  // A cluster by name: the directory is read first, so its client comes later.
  const clusterKey =
    !injectedClient && chosenCluster && resolvedPolicy
      ? JSON.stringify([chosenCluster, zone ?? null, directory ?? null, retryKey, generation])
      : null;
  const [connected, setConnected] = useState<{ key: string; client: CalimeroClient } | null>(null);
  const [connectingKey, setConnectingKey] = useState<string | null>(null);

  useEffect(() => {
    if (!clusterKey || !chosenCluster || !resolvedPolicy) return;
    let current = true;
    setConnectingKey(clusterKey);
    CalimeroClient.connect({
      cluster: chosenCluster,
      zone,
      directory,
      policy: resolvedPolicy,
      fetch: fetchProp,
      retry: JSON.parse(retryKey) as typeof retry,
    })
      .then((built) => {
        if (!current) return;
        const restore = reportSessions(built, keep);
        const kept = storeRef.current.load();
        if (kept) restore(kept);
        setConnected({ key: clusterKey, client: built });
      })
      .catch((error: unknown) => {
        if (!current) return;
        const described = describeError(error);
        setFailure(described);
        onErrorRef.current?.(described);
      })
      .finally(() => {
        if (current) setConnectingKey(null);
      });
    return () => {
      current = false;
    };
    // `clusterKey` stands for the cluster, zone, directory, retry and generation.
  }, [clusterKey, resolvedPolicy, fetchProp, keep]);

  const clusterClient = clusterKey && connected?.key === clusterKey ? connected.client : null;
  const client = injectedClient ?? (chosenCluster ? clusterClient : nodeClient);
  const isConnecting = !!clusterKey && connectingKey === clusterKey;
  const nodeUrl = chosenCluster ? (clusterClient?.nodeUrl ?? null) : chosenNode;

  const endSession = useCallback(() => {
    storeRef.current.clear();
    setSession(null);
    setGeneration((g) => g + 1);
  }, []);

  const reportError = useCallback(
    (error: unknown) => {
      const described = describeError(error);
      if (error instanceof LoginRequired) {
        endSession();
        setFailure(described);
      }
      onErrorRef.current?.(described);
    },
    [endSession],
  );

  const ceremony = useCallback(
    async (run: (c: CalimeroClient) => Promise<CalimeroSession>): Promise<CalimeroSession | null> => {
      if (!client) {
        const described = describeError(
          new Error(
            chosenCluster
              ? `Not connected to cluster ${chosenCluster} yet.`
              : 'No node to sign in to: call connectToNode or connectToCluster first.',
          ),
        );
        setFailure(described);
        onErrorRef.current?.(described);
        return null;
      }
      setIsLoading(true);
      setFailure(null);
      try {
        const fresh = await run(client);
        // An injected client does not report; keep what it answered.
        storeRef.current.save(fresh);
        setSession(fresh);
        return fresh;
      } catch (error) {
        const described = describeError(error);
        setFailure(described);
        onErrorRef.current?.(described);
        return null;
      } finally {
        setIsLoading(false);
      }
    },
    [client, chosenCluster],
  );

  const signIn = useCallback(() => ceremony((c) => c.signIn(passkey)), [ceremony, passkey]);
  const signUp = useCallback((name: string) => ceremony((c) => c.signUp(passkey, name)), [ceremony, passkey]);

  const signOutWith = useCallback(
    async (everywhere: boolean) => {
      setIsLoading(true);
      try {
        if (client?.session) {
          // The node may be unreachable; the session ends here either way.
          await (everywhere ? client.signOutEverywhere() : client.signOut()).catch(() => undefined);
        }
      } finally {
        endSession();
        setFailure(null);
        setIsLoading(false);
      }
    },
    [client, endSession],
  );
  const signOut = useCallback(() => signOutWith(false), [signOutWith]);
  const signOutEverywhere = useCallback(() => signOutWith(true), [signOutWith]);
  const logout = useCallback(() => {
    void signOutWith(false);
  }, [signOutWith]);

  const connectToNode = useCallback(
    (url: string) => {
      if (ownOrigin) return;
      const next = trimUrl(url);
      if (next === chosenNode) return;
      storeNodeUrl(next);
      clearCluster();
      // A session belongs to the cluster that issued it.
      storeRef.current.clear();
      setSession(null);
      setFailure(null);
      setChosen({ node: next });
    },
    [ownOrigin, chosenNode],
  );

  const connectToCluster = useCallback(
    (name: string) => {
      if (ownOrigin) return;
      const next = name.trim();
      if (!next || next === chosenCluster) return;
      storeCluster(next);
      clearNodeUrl();
      storeRef.current.clear();
      setSession(null);
      setFailure(null);
      setChosen({ cluster: next });
    },
    [ownOrigin, chosenCluster],
  );

  const disconnect = useCallback(() => {
    void signOutWith(false);
    if (!ownOrigin) {
      clearNodeUrl();
      clearCluster();
      setChosen(null);
    }
  }, [ownOrigin, signOutWith]);

  const setAppId = useCallback((next: string | null) => {
    if (next) storeAppId(next);
    else clearAppId();
    setAppIdState(next);
  }, []);

  const value = useMemo<MeroContextValue>(
    () => ({
      client,
      mero: client,
      nodeUrl,
      cluster: chosenCluster,
      isConnecting,
      connectToNode,
      connectToCluster,
      disconnect,
      session,
      accountId: session?.accountId ?? null,
      isAuthenticated: !!client && !!session,
      isLoading,
      signIn,
      signUp,
      signOut,
      signOutEverywhere,
      logout,
      appId,
      setAppId,
      passkey,
      servedByNode,
      allowMock,
      failure,
      reportError,
    }),
    [
      client,
      nodeUrl,
      chosenCluster,
      isConnecting,
      connectToNode,
      connectToCluster,
      disconnect,
      session,
      isLoading,
      signIn,
      signUp,
      signOut,
      signOutEverywhere,
      logout,
      appId,
      setAppId,
      passkey,
      servedByNode,
      allowMock,
      failure,
      reportError,
    ],
  );

  return <MeroContext.Provider value={value}>{children}</MeroContext.Provider>;
}

/** The provider's client, session and actions. Throws outside a `MeroProvider`. */
export function useMero(): MeroContextValue {
  const value = useContext(MeroContext);
  if (!value) throw new Error('useMero must be used within a MeroProvider');
  return value;
}
