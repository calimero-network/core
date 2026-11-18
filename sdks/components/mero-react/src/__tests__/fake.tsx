import React from 'react';
import { expect, vi } from 'vitest';
import type {
  CalimeroClient,
  CalimeroSession,
  NodeEvent,
  OpenConsentOptions,
  PasskeyProvider,
} from '@calimero-network/mero-js';
import { MeroProvider } from '../context';
import { memorySessionStore, type SessionStore } from '../storage';

export const SESSION: CalimeroSession = {
  accountId: 'acct-1',
  sessionToken: 'v2.claims.sig',
  expiresAt: 2_000_000_000,
  refreshToken: 'rt_1',
};

export const passkey: PasskeyProvider = {
  create: vi.fn(async (..._args: unknown[]) => ({}) as never),
  get: vi.fn(async (..._args: unknown[]) => ({}) as never),
};

/**
 * A stand-in for CalimeroClient: only what the hooks use, each a spy. The
 * routes are the generated method groups (`client.apps.call`, ...); the rest
 * are the hand-written flows and streams.
 */
export function fakeClient(overrides: Record<string, unknown> = {}, groups: FakeGroupOverrides = {}) {
  const apps = {
    get: vi.fn(async ({ app }: { app: string }) => ({ appId: app, name: 'kv' })),
    call: vi.fn(async (..._args: unknown[]) => ({ index: 7, result: 'ok', events: [] })),
    view: vi.fn(async (..._args: unknown[]) => ({ index: 7, result: { value: 1 } })),
    abi: vi.fn(async (..._args: unknown[]) => ({})),
    list: vi.fn(async (..._args: unknown[]) => ({ apps: [] as unknown[] })),
    create: vi.fn(async (..._args: unknown[]) => ({ appId: 'app-new' })),
    delete: vi.fn(async (..._args: unknown[]) => ({})),
    metadata: vi.fn(async (..._args: unknown[]) => ({})),
    upgrade: vi.fn(async (..._args: unknown[]) => ({ index: 1, status: 'ok', error: null })),
    blobUpload: vi.fn(async (..._args: unknown[]) => ({ blobId: 'b58', id: 'hex', size: 3 })),
    blobInfo: vi.fn(async (..._args: unknown[]) => ({ blobId: 'b58', id: 'hex', size: 3 })),
    blobList: vi.fn(async (..._args: unknown[]) => ({ blobs: [] as unknown[] })),
    blobDelete: vi.fn(async (..._args: unknown[]) => ({})),
    presencePublish: vi.fn(async (..._args: unknown[]) => ({})),
    ...groups.apps,
  };
  const orgs = {
    get: vi.fn(async (..._args: unknown[]) => ({ releaseApprovers: [], publishes: [], newWorkStopped: null })),
    create: vi.fn(async (..._args: unknown[]) => ({ orgId: 'org-1' })),
    delete: vi.fn(async (..._args: unknown[]) => ({})),
    setMember: vi.fn(async (..._args: unknown[]) => ({})),
    removeMember: vi.fn(async (..._args: unknown[]) => ({})),
    metadata: vi.fn(async (..._args: unknown[]) => ({})),
    memberMetadata: vi.fn(async (..._args: unknown[]) => ({})),
    capabilities: vi.fn(async (..._args: unknown[]) => ({})),
    teamGet: vi.fn(async (..._args: unknown[]) => ({})),
    teamCreate: vi.fn(async (..._args: unknown[]) => ({ teamId: 'team-1' })),
    teamUpdate: vi.fn(async (..._args: unknown[]) => ({})),
    teamDelete: vi.fn(async (..._args: unknown[]) => ({})),
    teamSetMember: vi.fn(async (..._args: unknown[]) => ({})),
    teamRemoveMember: vi.fn(async (..._args: unknown[]) => ({})),
    inviteCreate: vi.fn(async (..._args: unknown[]) => ({ token: 'tok-1' })),
    inviteRedeem: vi.fn(async (..._args: unknown[]) => ({ org: 'org-1' })),
    ...groups.orgs,
  };
  const auth = {
    me: vi.fn(async (..._args: unknown[]) => ({ accountId: SESSION.accountId })),
    passkeyRemove: vi.fn(async (..._args: unknown[]) => ({})),
    passkeyLabel: vi.fn(async (..._args: unknown[]) => ({})),
    apikeyList: vi.fn(async (..._args: unknown[]) => ({ keys: [{ keyId: 'k-1' }] })),
    apikeyCreate: vi.fn(async (..._args: unknown[]) => ({ keyId: 'k-2', key: 'ak_secret' })),
    apikeyRevoke: vi.fn(async (..._args: unknown[]) => ({})),
    ...groups.auth,
  };
  const consent = {
    pageCreate: vi.fn(async (..._args: unknown[]) => ({
      requestId: 'req-1',
      url: 'https://node.test/consent/req-1',
      expiresAt: 9e9,
      code: 'ABC-123',
    })),
    pageResult: vi.fn(async (..._args: unknown[]) => ({ requestId: 'req-1', kind: 'upgrade', status: 'approved', decidedAt: 1 })),
    ...groups.consent,
  };
  const packages = {
    list: vi.fn(async (..._args: unknown[]) => ({ packages: [] as unknown[] })),
    ...groups.packages,
  };
  const base = {
    nodeUrl: 'https://node.test',
    session: SESSION as CalimeroSession | undefined,
    apps,
    orgs,
    auth,
    consent,
    packages,
    signIn: vi.fn(async (..._args: unknown[]) => SESSION),
    signUp: vi.fn(async (..._args: unknown[]) => SESSION),
    signOut: vi.fn(async (..._args: unknown[]) => undefined),
    signOutEverywhere: vi.fn(async (..._args: unknown[]) => undefined),
    events: vi.fn(async function* (): AsyncGenerator<NodeEvent> {}),
    addPasskey: vi.fn(async (..._args: unknown[]) => 'cred-2'),
    openConsentPage: vi.fn(async (_req: unknown, how?: ((url: string) => void) | OpenConsentOptions) => {
      const started = { requestId: 'req-1', url: 'https://node.test/consent/req-1', expiresAt: 9e9, code: 'ABC-123' };
      const options = typeof how === 'function' ? { navigate: how } : (how ?? {});
      if (options.confirm && !(await options.confirm(started))) return started;
      options.navigate?.(started.url);
      return started;
    }),
    waitForConsent: vi.fn(async (..._args: unknown[]) => ({ requestId: 'req-1', kind: 'upgrade', status: 'approved', decidedAt: 1 })),
    presenceStream: vi.fn(async function* (): AsyncGenerator<NodeEvent> {}),
    ...overrides,
  };
  return base as typeof base & CalimeroClient;
}

type Spy = ReturnType<typeof vi.fn>;

/** Replacements for some of a generated group's methods. */
export interface FakeGroupOverrides {
  apps?: Record<string, Spy>;
  orgs?: Record<string, Spy>;
  auth?: Record<string, Spy>;
  consent?: Record<string, Spy>;
  packages?: Record<string, Spy>;
}

/** The `412 CONFIRM_REQUIRED` challenge `expectConfirmedBy` answers. */
const CHALLENGE = { challengeId: 'ch-1', challenge: 'AAAA', rpId: 'node.test', allowCredentials: [] as string[] };

/**
 * Assert that a generated method's per-call options answer a `412` with
 * `provider`: their `confirm` signs the challenge with it.
 */
export async function expectConfirmedBy(options: unknown, provider: PasskeyProvider): Promise<void> {
  const confirm = (options as { confirm?: (c: typeof CHALLENGE, ctx: { request: Request }) => Promise<unknown> } | undefined)
    ?.confirm;
  expect(typeof confirm).toBe('function');
  const get = provider.get as unknown as Spy;
  get.mockClear();
  await confirm!(CHALLENGE, { request: new Request('https://node.test/') });
  expect(get).toHaveBeenCalledTimes(1);
  expect(get.mock.calls[0][1]).toBe('node.test');
}

export function wrapper(
  client: CalimeroClient,
  { store = memorySessionStore(SESSION), appId = 'app-1' }: { store?: SessionStore; appId?: string } = {},
) {
  return function Wrapper({ children }: { children: React.ReactNode }) {
    return (
      <MeroProvider client={client} sessionStore={store} appId={appId} passkey={passkey}>
        {children}
      </MeroProvider>
    );
  };
}
