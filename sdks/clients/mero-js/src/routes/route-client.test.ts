/**
 * The generated client (`createRouteClient`, `client.api`): core's OpenAPI
 * routes, typed, sent through the real sealed channel to a stand-in node,
 * with the step-up (`412`) and operator-signature (`428`) middleware.
 * Synthetic data only.
 */

import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { StandInNode } from '../../tests/support/stand-in-node.js';
import { CalimeroClient, ConfirmCancelled, NodeTransport, NotFound, UnavailableError, createRouteClient, passkeyConfirm, type PasskeyProvider } from '../index.js';
import { ROUTE_BASE_URL, transportFetch, unwrap } from './route-client.js';
import { operatorSignature, stepUp } from './step-up.js';

const node = new StandInNode();
beforeAll(() => node.start());
afterAll(() => node.stop());
beforeEach(() => {
  node.requests.length = 0;
  node.answer = () => ({ status: 200, json: {} });
});

const transport = () => new NodeTransport({ baseUrl: node.url, policy: { allowMock: true }, retry: { baseDelayMs: 1 } });
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

describe('the generated client over the sealed transport', () => {
  it('sends a typed route through the sealed channel, path segments encoded, with the credential', async () => {
    const t = transport();
    t.useApiKey('cal_key');
    const api = createRouteClient(t);
    node.answer = () => ({ status: 200, json: { appId: 'a/b', name: 'kv' } });
    const { data, error } = await api.GET('/apps/{app}', { params: { path: { app: 'a/b' } } });
    expect(error).toBeUndefined();
    expect(data).toEqual({ appId: 'a/b', name: 'kv' });
    expect(sent()).toEqual([['GET', '/apps/a%2Fb']]);
    expect(header(0, 'authorization')).toBe('Bearer cal_key');
  });

  it('serialises the query and a JSON body', async () => {
    const api = createRouteClient(transport());
    await api.GET('/orgs/{org}/apps', { params: { path: { org: 'o-1' }, query: { after: 'x y', limit: 5 } } });
    await api.POST('/orgs', { body: { name: 'Acme' } });
    expect(sent()).toEqual([
      ['GET', '/orgs/o-1/apps?after=x+y&limit=5'],
      ['POST', '/orgs'],
    ]);
    expect(node.requests[1].body).toEqual({ name: 'Acme' });
    expect(header(1, 'content-type')).toBe('application/json');
  });

  it("answers a refusal as the result's error; unwrap throws it as the typed error", async () => {
    const api = createRouteClient(transport());
    node.answer = () => ({ status: 404, json: { status: 404, error: 'NOT_FOUND', message: 'no such app' } });
    const result = await api.GET('/apps/{app}', { params: { path: { app: 'a-1' } } });
    expect(result.data).toBeUndefined();
    expect(result.error).toEqual({ status: 404, error: 'NOT_FOUND', message: 'no such app' });
    expect(result.response.status).toBe(404);
    await expect(unwrap(api.GET('/apps/{app}', { params: { path: { app: 'a-1' } } }))).rejects.toBeInstanceOf(NotFound);
  });

  it('sends a raw body as it is, with its content type', async () => {
    const api = createRouteClient(transport());
    node.answer = () => ({ status: 200, json: { blobId: 'b', id: 'ab', size: 3, index: 1 } });
    const data = await unwrap(
      api.POST('/apps/{app}/blobs', {
        params: { path: { app: 'a-1' } },
        body: new Uint8Array([104, 105, 33]) as unknown as string,
        bodySerializer: (body: unknown) => body,
        headers: { 'Content-Type': 'text/plain' },
      }),
    );
    expect(data.size).toBe(3);
    expect(header(0, 'content-type')).toBe('text/plain');
    expect(node.requests[0].body).toEqual({ raw: 'hi!' });
  });

  it('reads a download as bytes', async () => {
    const api = createRouteClient(transport());
    node.answer = () => ({ status: 200, bytes: new Uint8Array([1, 2, 3]), contentType: 'image/png' });
    const { data, response } = await api.GET('/apps/{app}/blobs/{id}', {
      params: { path: { app: 'a-1', id: 'ab' } },
      parseAs: 'arrayBuffer',
    });
    expect(new Uint8Array(data as ArrayBuffer)).toEqual(new Uint8Array([1, 2, 3]));
    expect(response.headers.get('content-type')).toBe('image/png');
  });

  it('reads the attestation in the clear, without the credential', async () => {
    const t = transport();
    t.useApiKey('cal_key');
    const { data } = await createRouteClient(t).GET('/attestation', { params: { query: { nonce: '00'.repeat(32) } } });
    expect(data).toMatchObject({ mock: true });
    expect(node.requests).toEqual([]);
  });

  it('refuses a URL that is not one of its routes', async () => {
    const fetch = transportFetch(transport());
    await expect(fetch(new Request('https://elsewhere.example/apps'))).rejects.toThrow(/not a route/);
    expect(ROUTE_BASE_URL).toMatch(/\.invalid$/);
  });
});

describe('the 412 step-up middleware', () => {
  it('signs the challenge and sends the same body again with confirm, once', async () => {
    const api = createRouteClient(transport());
    confirming({ token: 'tok' });
    const challenges: unknown[] = [];
    const data = await unwrap(
      api.POST('/orgs/{org}/invites', {
        params: { path: { org: 'o-1' } },
        body: { role: 'member', teams: [] },
        middleware: [
          stepUp(async (challenge) => {
            challenges.push(challenge);
            return ASSERTION;
          }),
        ],
      }),
    );
    expect(data).toEqual({ token: 'tok' });
    expect(challenges).toEqual([CHALLENGE]);
    expect(sent()).toEqual([
      ['POST', '/orgs/o-1/invites'],
      ['POST', '/orgs/o-1/invites'],
    ]);
    expect(node.requests[1].body).toEqual({ role: 'member', teams: [], confirm: { challengeId: 'c-1', assertion: ASSERTION } });
  });

  it('answers a DELETE without a body with a body of the confirmation alone', async () => {
    const api = createRouteClient(transport());
    confirming({});
    await unwrap(api.DELETE('/orgs/{org}', { params: { path: { org: 'o-1' } }, middleware: [stepUp(passkeyConfirm(passkey))] }));
    expect(node.requests[1].body).toEqual({ confirm: { challengeId: 'c-1', assertion: ASSERTION } });
  });

  it("is the client's own for every request when given to createRouteClient", async () => {
    const api = createRouteClient(transport(), { confirm: passkeyConfirm(passkey) });
    confirming({ orgId: 'o-1' });
    expect(await unwrap(api.POST('/auth/recovery', { body: {} }))).toEqual({ orgId: 'o-1' });
    expect(node.requests).toHaveLength(2);
  });

  it("leaves the 412 as the result's error without one", async () => {
    const api = createRouteClient(transport());
    confirming({});
    const { error, response } = await api.DELETE('/orgs/{org}', { params: { path: { org: 'o-1' } } });
    expect(response.status).toBe(412);
    expect(error).toMatchObject({ error: 'CONFIRM_REQUIRED', details: CHALLENGE });
    expect(node.requests).toHaveLength(1);
  });

  it('asks the passkey for a verified user over the challenge, and a dismissed prompt is ConfirmCancelled', async () => {
    const asked: unknown[] = [];
    const pk: PasskeyProvider = {
      ...passkey,
      get: async (challenge, rpId, allow, options) => {
        asked.push([Array.from(challenge), rpId, allow, options?.userVerification]);
        throw Object.assign(new Error('no'), { name: 'NotAllowedError' });
      },
    };
    const api = createRouteClient(transport());
    confirming({});
    const error = await api
      .DELETE('/orgs/{org}', { params: { path: { org: 'o-1' } }, middleware: [stepUp(passkeyConfirm(pk))] })
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ConfirmCancelled);
    expect(asked).toEqual([[[0, 1, 2], 'localhost', ['cred'], 'required']]);
  });

  it('gives a call one idempotency key, which its confirmed retry carries too', async () => {
    const api = createRouteClient(transport());
    confirming({ index: 1, result: null, events: [], receipt: {} });
    await unwrap(
      api.POST('/apps/{app}/intents', {
        params: { path: { app: 'a-1' } },
        body: { method: 'pay', args: {} },
        middleware: [stepUp(passkeyConfirm(passkey))],
      }),
    );
    const [first, second] = node.requests.map((r) => r.body.idempotencyKey);
    expect(first).toMatch(/^node:c-/);
    expect(second).toBe(first);
  });
});

describe('the 428 operator-signature middleware', () => {
  it('signs the statement with the operator key and sends the body again with operatorProof', async () => {
    const statement = {
      subject: 'cluster',
      method: '__release_deadline',
      args: { version: '2.3.0' },
      nonce: 'n-1',
      cluster: 'c',
      message: 'AAEC',
    };
    node.answer = (_, body) =>
      body.operatorProof
        ? { status: 200, json: { index: 4 } }
        : { status: 428, json: { status: 428, error: 'OPERATOR_SIGNATURE_REQUIRED', message: 'sign', details: statement } };
    const signed: unknown[] = [];
    const api = createRouteClient(transport(), {
      operatorProof: async (s) => {
        signed.push(s);
        return { nonce: s.nonce, signature: 'ab'.repeat(64) };
      },
    });
    const data = await unwrap(
      api.POST('/ops/releases/{version}/deadline', { params: { path: { version: '2.3.0' } }, body: { after: '2030-01-01T00:00:00Z' } }),
    );
    expect(data).toEqual({ index: 4 });
    expect(signed).toEqual([statement]);
    expect(node.requests[1].body).toEqual({ after: '2030-01-01T00:00:00Z', operatorProof: { nonce: 'n-1', signature: 'ab'.repeat(64) } });
  });

  it('answers per request as well', async () => {
    node.answer = (_, body) =>
      body.operatorProof
        ? { status: 200, json: { key: 'account:a' } }
        : { status: 428, json: { status: 428, error: 'OPERATOR_SIGNATURE_REQUIRED', message: 'sign', details: { nonce: 'n' } } };
    const api = createRouteClient(transport());
    const { data } = await api.DELETE('/ops/deployers/{key}', {
      params: { path: { key: 'account:a' } },
      middleware: [operatorSignature(async (s) => ({ nonce: s.nonce, signature: '00' }))],
    });
    expect(data).toEqual({ key: 'account:a' });
    expect(node.requests[1].body).toEqual({ operatorProof: { nonce: 'n', signature: '00' } });
  });
});

describe('D70 retries through the generated client', () => {
  const unavailable = { status: 503, json: { status: 503, error: 'UNAVAILABLE', message: 'electing' } };

  it('retries a view on 503 until it is answered', async () => {
    let n = 0;
    node.answer = () => (++n < 3 ? unavailable : { status: 200, json: { result: 1 } });
    const { data } = await createRouteClient(transport()).POST('/apps/{app}/views', {
      params: { path: { app: 'a-1' } },
      body: { method: 'get' },
    });
    expect(data).toEqual({ result: 1 });
    expect(node.requests).toHaveLength(3);
  });

  it('answers the last 503 once the retries ran out, as UnavailableError through unwrap', async () => {
    node.answer = () => unavailable;
    const api = createRouteClient(transport());
    const error = await unwrap(api.POST('/apps/{app}/views', { params: { path: { app: 'a-1' } }, body: { method: 'get' } })).catch(
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(UnavailableError);
    expect(node.requests).toHaveLength(7);
  });

  it('sends any other route once', async () => {
    node.answer = () => unavailable;
    const { response } = await createRouteClient(transport()).POST('/orgs', { body: { name: 'Acme' } });
    expect(response.status).toBe(503);
    expect(node.requests).toHaveLength(1);
  });
});

describe('a route client on a CalimeroClient transport', () => {
  it('shares the client session and sends the same route as its generated methods', async () => {
    const client = new CalimeroClient({ baseUrl: node.url, policy: { allowMock: true } });
    client.useSession({ accountId: 'acc-1', sessionToken: 'tok', expiresAt: 9e9, refreshToken: 'r' });
    node.answer = () => ({ status: 200, json: { accountId: 'acc-1' } });
    const viaApi = await unwrap(createRouteClient(client.transport).GET('/auth/me'));
    const viaWrapper = await client.auth.me();
    expect(viaApi).toEqual(viaWrapper);
    expect(sent()).toEqual([
      ['GET', '/auth/me'],
      ['GET', '/auth/me'],
    ]);
    expect(header(0, 'authorization')).toBe('Bearer tok');
  });
});
