/**
 * What mero-react keeps between page loads: the node or cluster it talks to,
 * the app instance it opened last, and the passkey session.
 *
 * A Calimero 1.0 session is a short-lived session token (`v2.…`, 15 minutes)
 * plus a single-use refresh token (`rt_…`, 30 days). Both are kept together as
 * one {@link CalimeroSession} so a reload resumes where the page left off: the
 * client refreshes on the first `401` by itself.
 */

import type { CalimeroSession } from '@calimero-network/mero-js';

const NODE_URL_KEY = 'mero:nodeUrl';
const CLUSTER_KEY = 'mero:cluster';
const APP_ID_KEY = 'mero:appId';
const SESSION_KEY = 'mero:session';

/** Where the provider keeps the passkey session. */
export interface SessionStore {
  load(): CalimeroSession | null;
  save(session: CalimeroSession): void;
  clear(): void;
}

function storage(): Storage | null {
  try {
    return typeof window !== 'undefined' ? window.localStorage : null;
  } catch {
    return null;
  }
}

function read(key: string): string | null {
  try {
    return storage()?.getItem(key) ?? null;
  } catch {
    return null;
  }
}

function write(key: string, value: string | null): void {
  try {
    const s = storage();
    if (!s) return;
    if (value === null) s.removeItem(key);
    else s.setItem(key, value);
  } catch {
    /* storage full or blocked: the session lives in memory only */
  }
}

function isSession(value: unknown): value is CalimeroSession {
  const v = value as Partial<CalimeroSession> | null;
  return (
    !!v &&
    typeof v.accountId === 'string' &&
    typeof v.sessionToken === 'string' &&
    typeof v.refreshToken === 'string' &&
    typeof v.expiresAt === 'number'
  );
}

/**
 * The default store: `localStorage`, so a reload stays signed in.
 *
 * SECURITY: anything in `localStorage` is readable by every script on the
 * origin, the refresh token included. A frontend the node serves runs on its
 * own `{label}.{apps-domain}` origin under the node's CSP, which is the
 * intended setup. Pass {@link memorySessionStore} to keep the session for the
 * tab's lifetime only.
 */
export const localStorageSessionStore: SessionStore = {
  load() {
    const raw = read(SESSION_KEY);
    if (!raw) return null;
    try {
      const parsed: unknown = JSON.parse(raw);
      return isSession(parsed) ? parsed : null;
    } catch {
      return null;
    }
  },
  save(session) {
    write(SESSION_KEY, JSON.stringify(session));
  },
  clear() {
    write(SESSION_KEY, null);
  },
};

/** A store that forgets the session when the page unloads. */
export function memorySessionStore(initial: CalimeroSession | null = null): SessionStore {
  let held = initial;
  return {
    load: () => held,
    save: (session) => {
      held = session;
    },
    clear: () => {
      held = null;
    },
  };
}

export function getNodeUrl(): string | null {
  return read(NODE_URL_KEY);
}

export function setNodeUrl(url: string): void {
  write(NODE_URL_KEY, url);
}

export function clearNodeUrl(): void {
  write(NODE_URL_KEY, null);
}

/** The Calimero Cloud cluster (by name) chosen last, if a cluster was chosen. */
export function getCluster(): string | null {
  return read(CLUSTER_KEY);
}

export function setCluster(name: string): void {
  write(CLUSTER_KEY, name);
}

export function clearCluster(): void {
  write(CLUSTER_KEY, null);
}

/** The app instance the page opened last (an app id, `SHA-256(app)`, hex). */
export function getAppId(): string | null {
  return read(APP_ID_KEY);
}

export function setAppId(appId: string): void {
  write(APP_ID_KEY, appId);
}

export function clearAppId(): void {
  write(APP_ID_KEY, null);
}

/** Forget the node or cluster, the app and the session kept by the default store. */
export function clearAllStorage(): void {
  clearNodeUrl();
  clearCluster();
  clearAppId();
  localStorageSessionStore.clear();
}
