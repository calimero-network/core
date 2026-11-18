/**
 * The generated methods (`src/generated/methods.ts`, `client.orgs.get({ org })`):
 * one flattened argument object split back into path, query and body, sent
 * through the real sealed channel to a stand-in node, the payload answered
 * and a refusal thrown as the typed error. Synthetic data only.
 */

import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { StandInNode } from '../../tests/support/stand-in-node.js';
import {
  CalimeroClient,
  ConfirmRequired,
  NotFound,
  NodeTransport,
  UnavailableError,
  createRouteClient,
  presenceState,
  createRoutes,
  passkeyConfirm,
  type PasskeyProvider,
} from '../index.js';
import { route, type RouteShape } from './method.js';

const node = new StandInNode();
beforeAll(() => node.start());
afterAll(() => node.stop());
beforeEach(() => {
  node.requests.length = 0;
  node.answer = () => ({ status: 200, json: {} });
});

const client = (options: ConstructorParameters<typeof CalimeroClient>[1] = {}) =>
  new CalimeroClient({ baseUrl: node.url, policy: { allowMock: true }, retry: { baseDelayMs: 1 } }, options);
const sent = () => node.requests.map((r) => [r.head.method, r.head.path]);
const header = (i: number, name: string) => node.requests[i].head.headers.find(([k]) => k.toLowerCase() === name)?.[1];
const ASSERTION = { credentialId: 'cred', authenticatorData: 'ad', clientDataJson: 'cd', signature: 'sig' };
const passkey: PasskeyProvider = {
  create: async () => ({ attestationObject: 'ao', clientDataJson: 'cd' }),
  get: async () => ASSERTION,
};
const CHALLENGE = { challengeId: 'c-1', challenge: 'AAEC', rpId: 'localhost', allowCredentials: ['cred'] };
const confirming = (json: unknown) => {
  node.answer = (_, body) =>
    body.confirm
      ? { status: 200, json }
      : { status: 412, json: { status: 412, error: 'CONFIRM_REQUIRED', message: 'approve', details: CHALLENGE } };
};

describe('generated methods: one argument object', () => {
  it('puts path parameters in the path and the rest in the body', async () => {
    node.answer = () => ({ status: 200, json: { orgId: 'o/1', name: 'Acme', owner: 'acc-1', members: [], teams: [], metadata: null } });
    const c = client();
    const org = await c.orgs.get({ org: 'o/1' });
    expect(org.name).toBe('Acme');
    await c.orgs.memberMetadata({ org: 'o/1', account: 'acc 2', metadata: { name: 'Bo' } });
    expect(sent()).toEqual([
      ['GET', '/orgs/o%2F1'],
      ['POST', '/orgs/o%2F1/members/acc%202/metadata'],
    ]);
    expect(node.requests[1].body).toEqual({ metadata: { name: 'Bo' } });
  });

  it('puts query parameters in the query', async () => {
    node.answer = () => ({ status: 200, json: { apps: [], next: null } });
    await client().orgs.apps({ org: 'o-1', after: 'a-9', limit: 5 });
    await client().apps.list({ package: 'com.example.kv' });
    expect(sent()).toEqual([
      ['GET', '/orgs/o-1/apps?after=a-9&limit=5'],
      ['GET', '/apps?package=com.example.kv'],
    ]);
  });

  it('answers the typed payload of a call', async () => {
    node.answer = () => ({ status: 200, json: { index: 3, result: { ok: true }, events: [], receipt: {} } });
    const { index, result } = await client().apps.call({ app: 'a-1', method: 'set', args: { k: 'v' } });
    expect([index, result]).toEqual([3, { ok: true }]);
    expect(node.requests[0].body).toMatchObject({ method: 'set', args: { k: 'v' } });
    expect(node.requests[0].body.idempotencyKey).toMatch(/^node:c-/);
  });

  it('sends a raw body as it is, with its query and content type', async () => {
    node.answer = () => ({ status: 200, json: { blobId: 'b', id: 'ab', size: 3, index: 1 } });
    const { size } = await client().apps.blobUpload({
      app: 'a-1',
      sealed: true,
      body: new Uint8Array([104, 105, 33]),
      contentType: 'text/plain',
    });
    expect(size).toBe(3);
    expect(sent()).toEqual([['POST', '/apps/a-1/blobs?sealed=true']]);
    expect(header(0, 'content-type')).toBe('text/plain');
    expect(node.requests[0].body).toEqual({ raw: 'hi!' });
  });

  it("sends a raw body with the route's own media type when none is given", async () => {
    node.answer = () => ({ status: 200, json: { hash: 'h' } });
    expect(await client().packages.bundlePut({ body: new Uint8Array([0, 97, 115, 109]) })).toEqual({ hash: 'h' });
    expect(header(0, 'content-type')).toBe('application/wasm');
  });

  it('reads a download as a Blob of its type', async () => {
    node.answer = () => ({ status: 200, bytes: new Uint8Array([1, 2, 3]), contentType: 'image/png' });
    const blob = await client().apps.blobDownload({ app: 'a-1', id: 'ab', grant: 'g' });
    expect(new Uint8Array(await blob.arrayBuffer())).toEqual(new Uint8Array([1, 2, 3]));
    expect(blob.type).toBe('image/png');
    expect(sent()).toEqual([['GET', '/apps/a-1/blobs/ab?grant=g']]);
  });

  it('flattens the app attestation query beside its path parameter', async () => {
    // core 92ac297a1: `GET /apps/{app}/attestation` documents only `nonce`, so
    // nothing collides with the path's `app` and the query is flattened.
    node.answer = () => ({ status: 200, json: { appId: 'a-1', replicas: [] } });
    await client().apps.attestation({ app: 'a-1', nonce: '00' });
    expect(sent()).toEqual([['GET', '/apps/a-1/attestation?nonce=00']]);
  });

  it('takes the query whole where a query parameter is named like a path parameter', async () => {
    // No route of the current document has such a pair; the generator keeps the
    // shape for one that does, so it is held here with a synthetic route.
    node.answer = () => ({ status: 200, json: {} });
    const api = createRouteClient(new NodeTransport({ baseUrl: node.url, policy: { allowMock: true } }));
    const shape: RouteShape = {
      method: 'GET',
      path: '/apps/{app}/attestation',
      pathParams: ['app'],
      query: ['nonce', 'app'],
      queryWhole: true,
      body: 'none',
      optionalBody: false,
      reply: 'json',
    };
    await route(api, shape, { app: 'a-1', query: { nonce: '00', app: 'other' } });
    expect(sent()).toEqual([['GET', '/apps/a-1/attestation?nonce=00&app=other']]);
  });

  it('leaves out an optional body with nothing in it', async () => {
    await client().orgs.delete({ org: 'o-1' });
    expect(header(0, 'content-type')).toBeUndefined();
  });

  it('refuses a call without a path parameter before sending it', async () => {
    await expect(client().orgs.get({} as { org: string })).rejects.toThrow(/no value for org/);
    expect(node.requests).toEqual([]);
  });
});

describe('generated methods: refusals and challenges', () => {
  it('throws a refusal as the typed error', async () => {
    node.answer = () => ({ status: 404, json: { status: 404, error: 'NOT_FOUND', message: 'no such org' } });
    const error = await client()
      .orgs.get({ org: 'o-1' })
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(NotFound);
    expect(error).toMatchObject({ status: 404, code: 'NOT_FOUND' });
  });

  it('answers a 412 with the call’s own confirm and sends the same fields again with confirm', async () => {
    confirming({ token: 'tok' });
    const { token } = await client().orgs.inviteCreate({ org: 'o-1', role: 'member' }, { confirm: passkeyConfirm(passkey) });
    expect(token).toBe('tok');
    expect(sent()).toEqual([
      ['POST', '/orgs/o-1/invites'],
      ['POST', '/orgs/o-1/invites'],
    ]);
    expect(node.requests[1].body).toEqual({ role: 'member', confirm: { challengeId: 'c-1', assertion: ASSERTION } });
  });

  it("answers a 412 with the client's confirm when the call gives none", async () => {
    confirming({});
    await client({ confirm: passkeyConfirm(passkey) }).orgs.delete({ org: 'o-1' });
    expect(node.requests[1].body).toEqual({ confirm: { challengeId: 'c-1', assertion: ASSERTION } });
  });

  it('throws the 412 as ConfirmRequired with no confirm at all', async () => {
    confirming({});
    await expect(client().orgs.delete({ org: 'o-1' })).rejects.toBeInstanceOf(ConfirmRequired);
  });

  it('answers a 428 with the call’s operatorProof', async () => {
    node.answer = (_, body) =>
      body.operatorProof
        ? { status: 200, json: { index: 2 } }
        : { status: 428, json: { status: 428, error: 'OPERATOR_SIGNATURE_REQUIRED', message: 'sign', details: { nonce: 'n-1' } } };
    await client().ops.releaseDeadline(
      { version: '2.3.0', after: '2030-01-01T00:00:00Z' },
      { operatorProof: async (s) => ({ nonce: s.nonce, signature: '00' }) },
    );
    expect(node.requests[1].body).toEqual({ after: '2030-01-01T00:00:00Z', operatorProof: { nonce: 'n-1', signature: '00' } });
  });

  it('aborts with the call’s signal', async () => {
    const controller = new AbortController();
    controller.abort();
    await expect(client().auth.me({}, { signal: controller.signal })).rejects.toThrow();
  });
});

describe('generated methods: what the document marks', () => {
  const unavailable = { status: 503, json: { status: 503, error: 'UNAVAILABLE', message: 'electing' } };

  it('retries a route marked x-calimero-retry-safe, with the idempotency key it requires', async () => {
    let n = 0;
    node.answer = () => (++n < 3 ? unavailable : { status: 200, json: { index: 1, result: 2, events: [], receipt: {} } });
    const { result } = await client().apps.method({ app: 'a-1', method: 'get', args: {} });
    expect(result).toBe(2);
    expect(sent()).toEqual(Array(3).fill(['POST', '/apps/a-1/methods/get']));
    const keys = node.requests.map((r) => r.body.idempotencyKey);
    expect(keys[0]).toMatch(/^node:c-/);
    expect(new Set(keys).size).toBe(1);
  });

  it('sends a route not marked retry-safe once', async () => {
    node.answer = () => unavailable;
    await expect(client().orgs.create({ name: 'Acme' })).rejects.toBeInstanceOf(UnavailableError);
    expect(node.requests).toHaveLength(1);
  });

  it("asks the passkey for the route's own user verification (a sign-up's: preferred)", async () => {
    const asked: unknown[] = [];
    const pk: PasskeyProvider = {
      ...passkey,
      get: async (_c, _r, _a, options) => {
        asked.push(options?.userVerification);
        return ASSERTION;
      },
    };
    confirming({ accountId: 'acc-1', sessionToken: 't', expiresAt: 1, refreshToken: 'r' });
    await client().auth.registerFinish(
      { challengeId: 'r-1', registration: { attestationObject: 'ao', clientDataJson: 'cd' } },
      { confirm: passkeyConfirm(pk) },
    );
    confirming({});
    await client().orgs.delete({ org: 'o-1' }, { confirm: passkeyConfirm(pk) });
    expect(asked).toEqual(['preferred', 'required']);
  });

  it('moves to the next node on a 421 WRONG_NODE', async () => {
    const other = new StandInNode();
    await other.start();
    try {
      node.answer = () => ({ status: 421, json: { status: 421, error: 'WRONG_NODE', message: 'use another node' } });
      other.answer = () => ({ status: 200, json: { accountId: 'acc-1' } });
      const c = new CalimeroClient({ baseUrl: node.url, nodes: [other.url], policy: { allowMock: true } });
      expect(await c.auth.me()).toEqual({ accountId: 'acc-1' });
      expect(c.nodeUrl).toBe(other.url);
    } finally {
      await other.stop();
    }
  });

  it('sends a presence state as hex, and null to retract', async () => {
    await client().apps.presencePublish({ app: 'a-1', seq: 1, state: presenceState(new Uint8Array([1, 255])) });
    await client().apps.presencePublish({ app: 'a-1', seq: 2, state: presenceState(null) });
    expect(node.requests.map((r) => r.body)).toEqual([
      { seq: 1, state: '01ff' },
      { seq: 2, state: null },
    ]);
  });
});

describe('createRoutes', () => {
  it('builds the groups on any transport', async () => {
    const t = new NodeTransport({ baseUrl: node.url, policy: { allowMock: true } });
    t.useApiKey('cal_key');
    const routes = createRoutes(createRouteClient(t));
    node.answer = () => ({ status: 200, json: { accountId: 'acc-1' } });
    expect(await routes.auth.me()).toEqual({ accountId: 'acc-1' });
    expect(header(0, 'authorization')).toBe('Bearer cal_key');
    expect(Object.keys(routes).sort()).toEqual(['apps', 'auth', 'consent', 'deployments', 'ops', 'orgs', 'packages', 'public']);
  });

  it('lists packages and deployments through their groups', async () => {
    node.answer = (head) =>
      head.path === '/packages' ? { status: 200, json: { packages: [{ package: 'p' }] } } : { status: 200, json: { deployments: [] } };
    const c = client();
    expect(await c.packages.list()).toEqual({ packages: [{ package: 'p' }] });
    expect(await c.deployments.list()).toEqual({ deployments: [] });
  });
});
