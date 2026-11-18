import { useCallback, useEffect, useState } from 'react';
import type { CalimeroClient, NodeVerificationState, PackageCapabilities } from '@calimero-network/mero-js';
import type { AppCodeCheck } from './shieldView';

const RETRY_BASE_MS = 2_000;
const RETRY_MAX_MS = 30_000;
const NODE_POLL_MS = 1_000; // how often to notice that the client failed over

/** An unreachable node is asked again after 2 s, 4 s, 8 s, up to 30 s. */
const retryDelay = (attempt: number) => Math.min(RETRY_BASE_MS * 2 ** attempt, RETRY_MAX_MS);

/** The check's outcome, or `null` for a client that cannot be checked: it stays "checking" instead of taking the app down. */
async function check(client: CalimeroClient): Promise<NodeVerificationState | null> {
  try {
    return await client.verify();
  } catch {
    return null;
  }
}

function peek(client: CalimeroClient | null): NodeVerificationState {
  let known: NodeVerificationState | undefined;
  try {
    known = client?.verification();
  } catch {
    // A client that cannot say is still being checked.
  }
  return known ?? { status: 'checking', url: client?.nodeUrl ?? '' };
}

/** The check of the node in use, asked again after a failure to reach it, a failover, and whenever `refresh` is called. */
export function useNodeVerification(client: CalimeroClient | null): [NodeVerificationState, () => void] {
  // Kept with its client: a state from the previous client is never shown for the next one.
  const [held, setHeld] = useState(() => ({ client, state: peek(client) }));
  const [round, setRound] = useState(0);
  const [url, setUrl] = useState(client?.nodeUrl);
  useEffect(() => {
    setUrl(client?.nodeUrl);
    if (!client) return;
    const poll = setInterval(() => setUrl(client.nodeUrl), NODE_POLL_MS);
    return () => clearInterval(poll);
  }, [client]);
  useEffect(() => {
    if (!client) return;
    let live = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const run = async (attempt: number) => {
      const next = await check(client);
      if (!live || !next) return;
      setHeld({ client, state: next });
      if (next.status === 'unreachable') timer = setTimeout(() => void run(attempt + 1), retryDelay(attempt));
    };
    setHeld({ client, state: peek(client) });
    void run(0);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [client, round, url]);
  const state = held.client === client ? held.state : peek(client);
  // Never describe a node the client has moved on from: compared with the node in use now, not the last poll.
  const inUse = client?.nodeUrl;
  const shown: NodeVerificationState = inUse && state.url !== inUse ? { status: 'checking', url: inUse } : state;
  return [shown, useCallback(() => setRound((n) => n + 1), [])];
}

/** The app's own check that a node runs its code; memoise it, since a new function checks again. */
export type AppCodeChecker = (client: CalimeroClient, url: string) => Promise<AppCodeCheck>;

/** `appCode`'s answer for verified node `url`, shown only while the client, node and function are the ones it was asked for. */
export function useAppCode(client: CalimeroClient | null, appCode: AppCodeChecker | undefined, url: string | null): AppCodeCheck | null {
  const [result, setResult] = useState<{ client: CalimeroClient; url: string; appCode: AppCodeChecker; code: AppCodeCheck } | null>(null);
  useEffect(() => {
    if (!client || !appCode || !url) return;
    let live = true;
    void Promise.resolve()
      .then(() => appCode(client, url))
      .then(
        (code) => live && client.nodeUrl === url && setResult({ client, url, appCode, code }),
        () => undefined,
      );
    return () => {
      live = false;
    };
  }, [client, appCode, url]);
  const current = result?.client === client && result?.url === url && result?.appCode === appCode && client?.nodeUrl === url;
  return current ? result.code : null;
}

/** The package release a review is about. */
export interface ReviewRelease {
  package: string;
  version: string;
  /** The service of a multi-service release; the card is per service. */
  service?: string;
}

/** An app with no published release (a dev bundle, a package with no version): there is no signed card to read. */
export interface UnreleasedApp {
  package: string;
  version?: undefined;
  service?: undefined;
}

export type CardState =
  | { status: 'loading' }
  /** The app has no release, so there is nothing to ask for. */
  | { status: 'none' }
  | { status: 'ready'; card: PackageCapabilities }
  | { status: 'error'; message: string }
  /** The node did not pass its own check, so it is not asked and not retried. */
  | { status: 'refused' };

/** A plain sentence for a failed card request: never the node's raw error text. */
export function cardError(e: unknown): string {
  const status = typeof e === 'object' && e !== null && 'status' in e ? (e as { status: unknown }).status : undefined;
  if (status === 404) return 'This node cannot show app reviews yet.';
  if (status === 401 || status === 403) return "You don't have access to this app's details.";
  return 'The review of this app could not load.';
}

/** The release's capability card from the node, asked only once the node's own check verified it, and again on `retry`. */
export function useCard(client: CalimeroClient | null, release: ReviewRelease | UnreleasedApp, node: NodeVerificationState): [CardState, () => void] {
  const [state, setState] = useState<CardState>({ status: 'loading' });
  const [round, setRound] = useState(0);
  const gate = node.status === 'verified' ? 'verified' : node.status === 'refused' ? 'refused' : 'waiting';
  useEffect(() => {
    if (!release.version) {
      setState({ status: 'none' });
      return;
    }
    if (!client) {
      setState({ status: 'error', message: 'Not connected to a node.' });
      return;
    }
    if (gate !== 'verified') {
      setState(gate === 'refused' ? { status: 'refused' } : { status: 'loading' });
      return;
    }
    let live = true;
    setState({ status: 'loading' });
    const { package: pkg, version, service } = release;
    const request = service ? client.packageCapabilities(pkg, version, { service }) : client.packageCapabilities(pkg, version);
    request.then(
      (card) => live && setState({ status: 'ready', card }),
      (e: unknown) => live && setState({ status: 'error', message: cardError(e) }),
    );
    return () => {
      live = false;
    };
  }, [client, release.package, release.version, release.service, round, gate]);
  return [state, useCallback(() => setRound((n) => n + 1), [])];
}
