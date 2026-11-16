/**
 * What a lost issuing node or a node on the wrong release does to a session,
 * against stand-in cloud nodes through the real sealed channel. Synthetic data
 * only.
 *
 * Session tokens are signed by the node that issued them (core #4774): once
 * that node is replaced or out of reach, a `401` means "refresh on any node",
 * not "signed out". A node whose release may not serve
 * the account answers `421 WRONG_NODE`: the client moves to another node.
 */

import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { StandInNode, type Reply } from '../../tests/support/stand-in-node.js';
import type { RequestHead } from '../sealed/sealed.js';
import { CalimeroClient, LoginRequired, WrongNode, type CalimeroSession } from './index.js';
import type { PasskeyProvider } from '../runtime/passkey.js';

const a = new StandInNode();
const b = new StandInNode();
beforeAll(() => Promise.all([a.start(), b.start()]));
afterAll(() => Promise.all([a.stop(), b.stop()]));
beforeEach(() => {
  for (const node of [a, b]) {
    node.requests.length = 0;
    node.answer = () => ({ status: 200, json: {} });
  }
});

const OLD: CalimeroSession = { accountId: 'acc-1', sessionToken: 'v2.old.sig', expiresAt: 0, refreshToken: 'rt_old' };
const NEW: CalimeroSession = { accountId: 'acc-1', sessionToken: 'v2.new.sig', expiresAt: 0, refreshToken: 'rt_new' };

const refused = (status: number, error: string, details?: Record<string, unknown>): Reply => ({
  status,
  json: { error, message: `${error.toLowerCase()} (stand-in)`, ...(details ? { details } : {}) },
});
const ok = (json: unknown): Reply => ({ status: 200, json });
const failure = (p: Promise<unknown>) => p.then(() => undefined, (e: unknown) => e);
const bearer = (head: RequestHead) => head.headers.find(([name]) => name.toLowerCase() === 'authorization')?.[1];
const at = (node: StandInNode, path: string) => node.requests.filter((r) => r.head.path === path);

const signedIn = (nodes?: string[]) => {
  const client = new CalimeroClient({ baseUrl: a.url, nodes, policy: { allowMock: true }, retry: { baseDelayMs: 1 } });
  client.useSession(OLD);
  return client;
};

/** A node that can no longer vouch for `OLD`'s issuer: it takes only `NEW`, and exchanges `rt_old` once. */
const restartedNode = (head: RequestHead, body: Record<string, unknown>): Reply => {
  if (head.path === '/auth/refresh') {
    return body.refreshToken === 'rt_old' ? ok(NEW) : refused(401, 'UNAUTHENTICATED', { reason: 'unknown refresh token' });
  }
  return bearer(head) === `Bearer ${NEW.sessionToken}`
    ? ok({ index: 3, result: 'v1', events: [] })
    : refused(401, 'UNAUTHENTICATED', { reason: 'signature does not verify' });
};

describe('a 401 once the issuing node is gone', () => {
  it('refreshes once and repeats the call with the same idempotency key', async () => {
    a.answer = restartedNode;
    const client = signedIn();
    const answer = await client.apps.call({ app: 'app-1', method: 'set', args: { key: 'k' } });
    expect(answer.result).toBe('v1');
    expect(at(a, '/auth/refresh').map((r) => r.body)).toEqual([{ refreshToken: 'rt_old' }]);
    const calls = at(a, '/apps/app-1/intents');
    expect(calls.map((r) => bearer(r.head))).toEqual([`Bearer ${OLD.sessionToken}`, `Bearer ${NEW.sessionToken}`]);
    expect(calls[0].body.idempotencyKey).toBe(calls[1].body.idempotencyKey);
    expect(client.session).toEqual(NEW);
  });

  it('surfaces LoginRequired when the refresh is refused, without repeating the call', async () => {
    a.answer = (head, body) =>
      head.path === '/auth/refresh'
        ? refused(401, 'UNAUTHENTICATED', { reason: 'refresh token already used' })
        : restartedNode(head, body);
    const error = await failure(signedIn().apps.view({ app: 'app-1', method: 'get', args: {} }));
    expect(error).toBeInstanceOf(LoginRequired);
    expect(at(a, '/auth/refresh')).toHaveLength(1);
    expect(at(a, '/apps/app-1/views')).toHaveLength(1);
  });

  it('shares one refresh among requests refused at the same time', async () => {
    a.answer = restartedNode;
    const client = signedIn();
    const answers = await Promise.all(Array.from({ length: 6 }, () => client.apps.view({ app: 'app-1', method: 'get', args: {} })));
    expect(answers.map((x) => x.result)).toEqual(Array(6).fill('v1'));
    // A refresh token is single use: sent twice, the second would revoke the session.
    expect(at(a, '/auth/refresh')).toHaveLength(1);
  });

  it('refreshes at most once per request: a second 401 is LoginRequired', async () => {
    a.answer = (head) => (head.path === '/auth/refresh' ? ok(NEW) : refused(401, 'UNAUTHENTICATED'));
    const error = await failure(signedIn().auth.me());
    expect(error).toBeInstanceOf(LoginRequired);
    expect(at(a, '/auth/refresh')).toHaveLength(1);
    expect(at(a, '/auth/me')).toHaveLength(2);
  });

  it('signs out the refreshed session rather than leaving it live', async () => {
    a.answer = restartedNode;
    const client = signedIn();
    await client.signOut();
    expect(at(a, '/auth/logout').map((r) => bearer(r.head))).toEqual([`Bearer ${OLD.sessionToken}`, `Bearer ${NEW.sessionToken}`]);
    expect(client.session).toBeUndefined();
  });

  it('does not refresh for an API key', async () => {
    a.answer = () => refused(401, 'UNAUTHENTICATED');
    const client = new CalimeroClient({ baseUrl: a.url, policy: { allowMock: true } });
    client.useApiKey('cal_key');
    expect(await failure(client.auth.me())).toBeInstanceOf(LoginRequired);
    expect(at(a, '/auth/refresh')).toHaveLength(0);
  });
});

describe('421 WRONG_NODE', () => {
  const wrongNode = () => refused(421, 'WRONG_NODE', { node: 1 });

  it('moves to the next node and repeats the call there once', async () => {
    a.answer = wrongNode;
    b.answer = () => ok({ index: 4, result: 'v2', events: [] });
    const client = signedIn([b.url]);
    const answer = await client.apps.call({ app: 'app-1', method: 'set', args: { key: 'k' } });
    expect(answer.result).toBe('v2');
    const [first] = at(a, '/apps/app-1/intents');
    const [second] = at(b, '/apps/app-1/intents');
    expect(second.body.idempotencyKey).toBe(first.body.idempotencyKey);
    expect(bearer(second.head)).toBe(`Bearer ${OLD.sessionToken}`);
    // The next requests go to the new node too.
    expect(client.nodeUrl).toBe(b.url);
    await client.auth.me();
    expect(at(a, '/auth/me')).toHaveLength(0);
    expect(at(b, '/auth/me')).toHaveLength(1);
  });

  it('is WrongNode, with its details, when the client knows no other node', async () => {
    a.answer = wrongNode;
    const error = await failure(signedIn().apps.view({ app: 'app-1', method: 'get', args: {} }));
    expect(error).toBeInstanceOf(WrongNode);
    expect(error).toMatchObject({ status: 421, code: 'WRONG_NODE', details: { node: 1 } });
    expect(at(a, '/apps/app-1/views')).toHaveLength(1);
  });

  it('is WrongNode when the next node refuses too, after one move', async () => {
    a.answer = wrongNode;
    b.answer = wrongNode;
    const error = await failure(signedIn([b.url]).apps.view({ app: 'app-1', method: 'get', args: {} }));
    expect(error).toBeInstanceOf(WrongNode);
    expect(a.requests.length + b.requests.length).toBe(2);
  });

  it('is not NOT_LEADER: another 421 code is not a move', async () => {
    a.answer = () => refused(421, 'NOT_LEADER', { leaderUrl: 'http://elsewhere' });
    const client = signedIn([b.url]);
    const error = await failure(client.ops.placement());
    expect(error).not.toBeInstanceOf(WrongNode);
    expect(error).toMatchObject({ status: 421, code: 'NOT_LEADER' });
    expect(client.nodeUrl).toBe(a.url);
    expect(b.requests).toHaveLength(0);
  });

  it('does not repeat a sign-in: its challenge is spent; the next one goes to the next node', async () => {
    a.answer = (head) =>
      head.path === '/auth/passkey/login/begin'
        ? ok({ challengeId: 'c-1', options: { challenge: 'AAEC', rpId: 'localhost' } })
        : wrongNode();
    const passkey: PasskeyProvider = {
      create: async () => ({ attestationObject: '', clientDataJson: '' }),
      get: async () => ({ credentialId: 'cred', authenticatorData: 'ad', clientDataJson: 'cd', signature: 'sig' }),
    };
    const client = new CalimeroClient({ baseUrl: a.url, nodes: [b.url], policy: { allowMock: true } });
    expect(await failure(client.signIn(passkey))).toBeInstanceOf(WrongNode);
    expect(at(a, '/auth/passkey/login/finish')).toHaveLength(1);
    expect(b.requests).toHaveLength(0);
    expect(client.nodeUrl).toBe(b.url);
  });

  it('a refresh refused 421 goes to the next node with the same token, unspent', async () => {
    a.answer = (head) => (head.path === '/auth/refresh' ? wrongNode() : refused(401, 'UNAUTHENTICATED'));
    b.answer = restartedNode;
    const client = signedIn([b.url]);
    const answer = await client.apps.view({ app: 'app-1', method: 'get', args: {} });
    expect(answer.result).toBe('v1');
    expect(at(a, '/auth/refresh').map((r) => r.body)).toEqual([{ refreshToken: 'rt_old' }]);
    expect(at(b, '/auth/refresh').map((r) => r.body)).toEqual([{ refreshToken: 'rt_old' }]);
    expect(at(b, '/apps/app-1/views').map((r) => bearer(r.head))).toEqual([`Bearer ${NEW.sessionToken}`]);
    expect(client.nodeUrl).toBe(b.url);
    expect(client.session).toEqual(NEW);
  });

  it('a refresh every node refuses 421 is WrongNode, tried once per node', async () => {
    a.answer = wrongNode;
    b.answer = wrongNode;
    const client = signedIn([b.url]);
    expect(await failure(client.refresh())).toBeInstanceOf(WrongNode);
    expect(at(a, '/auth/refresh')).toHaveLength(1);
    expect(at(b, '/auth/refresh')).toHaveLength(1);
    expect(client.session).toEqual(OLD);
  });

  it('a refresh refused 421 with no other node is WrongNode', async () => {
    a.answer = wrongNode;
    const client = signedIn();
    expect(await failure(client.refresh())).toBeInstanceOf(WrongNode);
    expect(at(a, '/auth/refresh')).toHaveLength(1);
  });
});

describe('sign-out when the node cannot be reached', () => {
  for (const [method, path] of [
    ['signOut', '/auth/logout'],
    ['signOutEverywhere', '/auth/logout-all'],
  ] as const) {
    it(`${method} still clears the session, and reports the failure`, async () => {
      a.answer = (head) => (head.path === path ? { status: 200, drop: true } : ok({}));
      const client = signedIn();
      const error = await failure(client[method]());
      expect(error).toBeDefined();
      // The revoke was attempted with the session's token.
      expect(at(a, path).length).toBeGreaterThanOrEqual(1);
      expect(bearer(at(a, path)[0].head)).toBe(`Bearer ${OLD.sessionToken}`);
      expect(client.session).toBeUndefined();
      // The next request goes without the old bearer token.
      await client.transport.json('GET', 'health');
      expect(bearer(at(a, '/health')[0].head)).toBeUndefined();
    });
  }
});
