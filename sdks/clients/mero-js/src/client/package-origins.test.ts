/**
 * Package origins (core `claude/package-origins`): a package's managers
 * register web origins so a frontend they host themselves signs users in with
 * passkeys, and sessions from there are bound to the package. Each route goes
 * through the real sealed channel to a stand-in node. Synthetic data only.
 */

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

import { StandInNode, type Reply } from '../../tests/support/stand-in-node.js';
import {
  BadOrigin,
  CalimeroClient,
  CalimeroError,
  ClusterNotUpgraded,
  ConfirmRequired,
  LoginRequired,
  NotFound,
  NotServing,
  OriginNotRegistered,
  OriginTaken,
  QuotaExceeded,
  type PackageOrigin,
} from './index.js';
import type { PasskeyProvider } from '../runtime/passkey.js';
import { passkeyConfirm } from '../runtime/passkey.js';

const node = new StandInNode();
beforeAll(() => node.start());
afterAll(() => node.stop());
beforeEach(() => {
  node.requests.length = 0;
  node.answer = () => ({ status: 200, json: {} });
});
afterEach(() => {
  vi.unstubAllGlobals();
});

const PKG = 'com.example.chat';
const SITE = 'https://chat.example.com';
const client = () => new CalimeroClient({ baseUrl: node.url, policy: { allowMock: true } });
const sent = () => node.requests.map((r) => [r.head.method, r.head.path]);
const last = () => node.requests[node.requests.length - 1];

/** A passkey that records the relying-party ids and credentials it was asked to sign for. */
const recording = () => {
  const asked: Array<{ rpId: string; allow?: string[] }> = [];
  const created: Array<Record<string, unknown>> = [];
  const pk: PasskeyProvider = {
    create: async (options) => {
      created.push(options);
      return { attestationObject: 'ao', clientDataJson: 'cd' };
    },
    get: async (_challenge, rpId, allow) => {
      asked.push({ rpId, allow });
      return { credentialId: 'cred', authenticatorData: 'ad', clientDataJson: 'cd', signature: 'sig' };
    },
  };
  return { pk, asked, created };
};

const STEP_UP = { challengeId: 'c-1', challenge: 'AAEC', rpId: 'cluster.example', allowCredentials: ['cred'] };
const refuse = (status: number, error: string, details: Record<string, unknown> = {}): Reply => ({
  status,
  json: { error, message: error.toLowerCase(), details },
});
/** 412 until the body carries a confirmation, then `then` (by default 200 `json`). */
const stepUp = (json: unknown, then?: () => Reply) => {
  node.answer = (_, body) =>
    body.confirm ? (then ? then() : { status: 200, json }) : refuse(412, 'CONFIRM_REQUIRED', STEP_UP);
};
const confirm = { challengeId: 'c-1', assertion: expect.objectContaining({ signature: 'sig' }) };

const ORIGIN: PackageOrigin = {
  package: PKG,
  origin: SITE,
  rpId: 'chat.example.com',
  addedBy: 'acc-1',
  addedAt: '2026-11-01T00:00:00Z',
  served: false,
};

describe('packageOrigins.list', () => {
  it('reads GET /package-origins/{package} and answers its origins', async () => {
    node.answer = () => ({ status: 200, json: { package: PKG, origins: [ORIGIN] } });
    expect(await (await client().packages.originsList({ package: PKG })).origins).toEqual([ORIGIN]);
    expect(sent()).toEqual([['GET', '/package-origins/com.example.chat']]);
  });

  it('encodes the package as one path segment', async () => {
    node.answer = () => ({ status: 200, json: { package: 'a/b', origins: [] } });
    expect(await (await client().packages.originsList({ package: 'a/b' })).origins).toEqual([]);
    expect(last().head.path).toBe('/package-origins/a%2Fb');
  });

  it('an unknown package is NotFound, a caller not managing it 403 FORBIDDEN', async () => {
    node.answer = () => refuse(404, 'NOT_FOUND');
    await expect(client().packages.originsList({ package: PKG })).rejects.toBeInstanceOf(NotFound);
    node.answer = () => refuse(403, 'FORBIDDEN');
    const error = await client().packages.originsList({ package: PKG }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CalimeroError);
    expect(error).toMatchObject({ status: 403, code: 'FORBIDDEN' });
  });
});

describe('packageOrigins.add', () => {
  it('answers the 412 over __add_origin with the passkey and sends the same body with confirm', async () => {
    const { pk, asked } = recording();
    const added = { package: PKG, ...ORIGIN };
    stepUp(added);
    expect(await client().packages.originsAdd({ package: PKG, origin: SITE }, { confirm: passkeyConfirm(pk) })).toEqual(added);
    expect(sent()).toEqual([
      ['POST', '/package-origins/com.example.chat'],
      ['POST', '/package-origins/com.example.chat'],
    ]);
    // Only the fields the node takes: it refuses unknown ones.
    expect(node.requests[0].body).toEqual({ origin: SITE });
    expect(last().body).toEqual({ origin: SITE, confirm });
    expect(asked).toEqual([{ rpId: 'cluster.example', allow: ['cred'] }]);
  });

  it('sends an explicit rpId in both attempts', async () => {
    stepUp({ package: PKG, ...ORIGIN, rpId: 'example.com' });
    const added = await client().packages.originsAdd({ package: PKG, origin: SITE, rpId: 'example.com' }, { confirm: passkeyConfirm(recording().pk) });
    expect(added.rpId).toBe('example.com');
    expect(node.requests[0].body).toEqual({ origin: SITE, rpId: 'example.com' });
    expect(last().body).toEqual({ origin: SITE, rpId: 'example.com', confirm });
  });

  it('without a passkey the 412 is ConfirmRequired, carrying the challenge', async () => {
    stepUp({});
    const error = await client().packages.originsAdd({ package: PKG, origin: SITE }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ConfirmRequired);
    expect(error).toMatchObject({ details: STEP_UP });
    expect(sent()).toEqual([['POST', '/package-origins/com.example.chat']]);
  });

  it.each([
    [400, 'BAD_ORIGIN', BadOrigin],
    [400, 'BAD_RP_ID', BadOrigin],
    [409, 'ORIGIN_TAKEN', OriginTaken],
    [429, 'QUOTA_EXCEEDED', QuotaExceeded],
  ] as const)('a confirmed %i %s is a typed error', async (status, code, type) => {
    stepUp({}, () => refuse(status, code));
    const error = await client().packages.originsAdd({ package: PKG, origin: SITE }, { confirm: passkeyConfirm(recording().pk) }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(type);
    expect(error).toBeInstanceOf(CalimeroError);
    expect(error).toMatchObject({ status, code });
  });

  it('a refusal before any step-up reaches the caller without a prompt', async () => {
    const { pk, asked } = recording();
    node.answer = () => refuse(400, 'BAD_ORIGIN');
    await expect(client().packages.originsAdd({ package: PKG, origin: 'not an origin' }, { confirm: passkeyConfirm(pk) })).rejects.toBeInstanceOf(BadOrigin);
    expect(asked).toEqual([]);
    expect(sent()).toHaveLength(1);
  });
});

describe('packageOrigins.add served (a custom domain)', () => {
  const DOMAIN = 'https://kv.example.com';
  const SERVED = { package: PKG, origin: DOMAIN, rpId: 'kv.example.com', addedBy: 'acc-1', addedAt: '2026-11-05T10:00:00Z', served: true };

  it('sends served: true in both attempts and answers the served registration', async () => {
    const { pk, asked } = recording();
    stepUp(SERVED);
    expect(await client().packages.originsAdd({ package: PKG, origin: DOMAIN, served: true }, { confirm: passkeyConfirm(pk) })).toEqual(SERVED);
    expect(node.requests[0].body).toEqual({ origin: DOMAIN, served: true });
    expect(last().body).toEqual({ origin: DOMAIN, served: true, confirm });
    expect(asked).toEqual([{ rpId: 'cluster.example', allow: ['cred'] }]);
  });

  it('sends rpId and served together', async () => {
    stepUp({ ...SERVED, rpId: 'example.com' });
    await client().packages.originsAdd({ package: PKG, origin: DOMAIN, rpId: 'example.com', served: true }, { confirm: passkeyConfirm(recording().pk) });
    expect(node.requests[0].body).toEqual({ origin: DOMAIN, rpId: 'example.com', served: true });
  });

  it('a plain add sends no served, so a node that predates served origins takes it', async () => {
    stepUp({ package: PKG, ...ORIGIN });
    await client().packages.originsAdd({ package: PKG, origin: SITE }, { confirm: passkeyConfirm(recording().pk) });
    expect(node.requests[0].body).toEqual({ origin: SITE });
    expect(last().body).toEqual({ origin: SITE, confirm });
  });

  it.each([
    [400, 'NOT_SERVING', NotServing],
    [400, 'BAD_ORIGIN', BadOrigin],
    [409, 'ORIGIN_TAKEN', OriginTaken],
    [409, 'CLUSTER_NOT_UPGRADED', ClusterNotUpgraded],
    [429, 'QUOTA_EXCEEDED', QuotaExceeded],
  ] as const)('a %i %s refusal before any step-up is a typed error, without a prompt', async (status, code, type) => {
    const { pk, asked } = recording();
    node.answer = () => refuse(status, code);
    const error = await client().packages.originsAdd({ package: PKG, origin: DOMAIN, served: true }, { confirm: passkeyConfirm(pk) }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(type);
    expect(error).toBeInstanceOf(CalimeroError);
    expect(error).toMatchObject({ status, code });
    expect(asked).toEqual([]);
    expect(sent()).toHaveLength(1);
  });

  it('list answers served on each origin', async () => {
    const { package: _p, ...listed } = SERVED;
    node.answer = () => ({ status: 200, json: { package: PKG, origins: [{ ...ORIGIN, served: false }, listed] } });
    const { origins } = await client().packages.originsList({ package: PKG });
    expect(origins.map((o) => o.served)).toEqual([false, true]);
  });
});

describe('opsServedOrigins', () => {
  it('reads GET /ops/served-origins with the operator token', async () => {
    const report = {
      fresh: true,
      origins: [{ origin: 'https://kv.example.com', host: 'kv.example.com', label: 'a'.repeat(52), addedAt: '2026-11-05T10:00:00Z' }],
    };
    node.answer = () => ({ status: 200, json: report });
    const c = client();
    c.useApiKey('ops-token');
    expect(await c.ops.served()).toEqual(report);
    expect(sent()).toEqual([['GET', '/ops/served-origins']]);
  });
});

describe('packageOrigins.remove', () => {
  it('answers the 412 over __remove_origin and DELETEs with confirm', async () => {
    const removed = { package: PKG, origin: SITE, removed: true };
    stepUp(removed);
    expect(await client().packages.originsRemove({ package: PKG, origin: SITE }, { confirm: passkeyConfirm(recording().pk) })).toEqual(removed);
    expect(sent()).toEqual([
      ['DELETE', '/package-origins/com.example.chat'],
      ['DELETE', '/package-origins/com.example.chat'],
    ]);
    expect(node.requests[0].body).toEqual({ origin: SITE });
    expect(last().body).toEqual({ origin: SITE, confirm });
  });

  it('an origin that is not registered is NotFound', async () => {
    stepUp({}, () => refuse(404, 'NOT_FOUND'));
    await expect(client().packages.originsRemove({ package: PKG, origin: SITE }, { confirm: passkeyConfirm(recording().pk) })).rejects.toBeInstanceOf(NotFound);
  });

  it('without a passkey the 412 is ConfirmRequired', async () => {
    stepUp({});
    await expect(client().packages.originsRemove({ package: PKG, origin: SITE })).rejects.toBeInstanceOf(ConfirmRequired);
  });

  it("a session opened on a removed origin gets 401, which a refused refresh makes LoginRequired", async () => {
    const c = client();
    c.useSession({ accountId: 'acc-2', sessionToken: 'tok', expiresAt: 9e9, refreshToken: 'r' });
    node.answer = () => refuse(401, 'UNAUTHENTICATED');
    await expect(c.auth.me()).rejects.toBeInstanceOf(LoginRequired);
    expect(sent()).toEqual([
      ['GET', '/auth/me'],
      ['POST', '/auth/refresh'],
    ]);
  });
});

describe('signing in from a registered origin', () => {
  const SESSION = { accountId: 'acc-2', sessionToken: 'tok', expiresAt: 9e9, refreshToken: 'r' };
  /** begin answers `options`, finish a session; `begin` may refuse instead. */
  const ceremony = (options: Record<string, unknown>, begin?: Reply) => {
    node.answer = (head) => {
      if (head.path.endsWith('/begin')) return begin ?? { status: 200, json: { challengeId: 'l-1', options } };
      return { status: 200, json: SESSION };
    };
  };

  it("signIn sends the page's origin and signs for the rpId the node answers", async () => {
    const { pk, asked } = recording();
    ceremony({ challenge: 'AAEC', rpId: 'chat.example.com' });
    expect(await client().signIn(pk, { origin: SITE })).toEqual(SESSION);
    expect(node.requests[0].body).toEqual({ origin: SITE });
    expect(asked).toEqual([{ rpId: 'chat.example.com', allow: undefined }]);
  });

  it('signIn takes the rpId from options.rp.id when options.rpId is absent', async () => {
    const { pk, asked } = recording();
    ceremony({ challenge: 'AAEC', rp: { id: 'chat.example.com' } });
    await client().signIn(pk, { origin: SITE });
    expect(asked[0].rpId).toBe('chat.example.com');
  });

  it("signUp sends the page's origin, then signs the 412 on finish with the new passkey for the origin's rpId", async () => {
    const { pk, created, asked } = recording();
    const options = { challenge: 'AAEC', rp: { id: 'chat.example.com', name: 'chat' }, user: { id: 'AA', name: 'bob' } };
    const proof = { challengeId: 'p-1', challenge: 'AAEC', rpId: 'chat.example.com', allowCredentials: ['cred'] };
    node.answer = (head, body) => {
      if (head.path.endsWith('/begin')) return { status: 200, json: { challengeId: 'r-1', options } };
      return body.confirm ? { status: 200, json: SESSION } : refuse(412, 'CONFIRM_REQUIRED', proof);
    };
    expect(await client().signUp(pk, 'bob', { origin: SITE })).toEqual(SESSION);
    expect(sent()).toEqual([
      ['POST', '/auth/passkey/register/begin'],
      ['POST', '/auth/passkey/register/finish'],
      ['POST', '/auth/passkey/register/finish'],
    ]);
    expect(node.requests[0].body).toEqual({ name: 'bob', origin: SITE });
    expect(created).toEqual([options]);
    const registration = { attestationObject: 'ao', clientDataJson: 'cd' };
    expect(node.requests[1].body).toEqual({ challengeId: 'r-1', registration });
    expect(last().body).toEqual({
      challengeId: 'r-1',
      registration,
      confirm: { challengeId: 'p-1', assertion: expect.objectContaining({ signature: 'sig' }) },
    });
    // The same new passkey signs, for the origin's relying party, not the cluster's.
    expect(asked).toEqual([{ rpId: 'chat.example.com', allow: ['cred'] }]);
  });

  it("in a browser, defaults to window.location.origin when it is not a node's", async () => {
    vi.stubGlobal('location', { origin: SITE });
    ceremony({ challenge: 'AAEC', rpId: 'chat.example.com' });
    await client().signIn(recording().pk);
    expect(node.requests[0].body).toEqual({ origin: SITE });
  });

  it("sends no origin from a page on the node's own origin, outside a browser, or with origin: null", async () => {
    ceremony({ challenge: 'AAEC', rpId: 'cluster.example' });
    await client().signIn(recording().pk);
    vi.stubGlobal('location', { origin: new URL(node.url).origin });
    await client().signUp(recording().pk, 'bob');
    vi.stubGlobal('location', { origin: SITE });
    await client().signIn(recording().pk, { origin: null });
    const begins = node.requests.filter((r) => r.head.path.endsWith('/begin')).map((r) => r.body);
    expect(begins).toEqual([{}, { name: 'bob' }, {}]);
  });

  it('an unregistered origin is OriginNotRegistered, naming the site and the package', async () => {
    const { pk, asked } = recording();
    ceremony({}, refuse(403, 'ORIGIN_NOT_REGISTERED', { package: PKG }));
    const error = await client().signIn(pk, { origin: SITE }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(OriginNotRegistered);
    expect(error).toMatchObject({ status: 403, code: 'ORIGIN_NOT_REGISTERED', origin: SITE, package: PKG });
    expect((error as Error).message).toContain(`this site (${SITE}) is not registered for ${PKG}`);
    expect(asked).toEqual([]);
    expect(sent()).toEqual([['POST', '/auth/passkey/login/begin']]);
  });

  it('OriginNotRegistered keeps the origin the node names', async () => {
    ceremony({}, refuse(403, 'ORIGIN_NOT_REGISTERED', { origin: 'https://other.example', package: PKG }));
    const error = await client().signUp(recording().pk, 'bob', { origin: SITE }).catch((e: unknown) => e);
    expect(error).toMatchObject({ origin: 'https://other.example', package: PKG });
  });

  it("a step-up of a session from such an origin signs for that origin's rpId", async () => {
    const { pk, asked } = recording();
    const c = client();
    c.useSession(SESSION);
    node.answer = (_, body) =>
      body.confirm
        ? { status: 200, json: { keyId: 'k', key: 'secret' } }
        : refuse(412, 'CONFIRM_REQUIRED', { ...STEP_UP, rpId: 'chat.example.com' });
    await c.auth.apikeyCreate({ label: 'x' }, { confirm: passkeyConfirm(pk) });
    expect(asked).toEqual([{ rpId: 'chat.example.com', allow: ['cred'] }]);
  });

  it("me() answers the package (app) and origin the session is bound to", async () => {
    node.answer = () => ({ status: 200, json: { accountId: 'acc-2', via: 'session', app: PKG, origin: SITE } });
    const me = await client().auth.me();
    expect(me.app).toBe(PKG);
    expect(me.origin).toBe(SITE);
  });
});
