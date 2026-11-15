/**
 * The runtime's `Transport` interface on its own, as a generated route layer
 * would use it: against a stand-in cloud node on a real local HTTP server
 * (mock attestation, the sealed handshake, sealed and streamed answers).
 * Synthetic data only.
 */

import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

import { APP_BUNDLE_HASH, StandInNode, type Reply } from '../../tests/support/stand-in-node.js';
import { CalimeroClient, FILE_PART_BYTES } from '../client/calimero-client.js';
import { hex } from '../crypto/internal.js';
import {
  CalimeroError,
  ConfirmRequired,
  NodeTransport,
  NotFound,
  UnavailableError,
  queryString,
  routePath,
  type PasskeyProvider,
  type Transport,
} from './index.js';
import { Unanswered } from './attestation.js';

const node = new StandInNode();
beforeAll(() => node.start());
afterAll(() => node.stop());
beforeEach(() => {
  node.requests.length = 0;
  node.answer = () => ({ status: 200, json: {} });
});

const transport = () => new NodeTransport({ baseUrl: node.url, policy: { allowMock: true }, retry: { baseDelayMs: 1 } });
const refused = (status: number, error: string, details?: Record<string, unknown>): Reply => ({
  status,
  json: { error, message: `${error.toLowerCase()} (stand-in)`, ...(details ? { details } : {}) },
});
const failure = (p: Promise<unknown>) => p.then(() => undefined, (e: unknown) => e);
const header = (i: number, name: string) => node.requests[i].head.headers.find(([n]) => n.toLowerCase() === name)?.[1];
const passkey: PasskeyProvider = {
  create: async () => ({ attestationObject: '', clientDataJson: '' }),
  get: async () => ({ credentialId: 'cred', authenticatorData: 'ad', clientDataJson: 'cd', signature: 'sig' }),
};

describe('routePath and queryString', () => {
  it('fills a template with encoded segments, so an id cannot reach another route', () => {
    expect(routePath('apps/{app}/blobs/{id}', { app: 'a/../b', id: 7 })).toBe('apps/a%2F..%2Fb/blobs/7');
    expect(() => routePath('orgs/{org}', {})).toThrow(/org/);
  });

  it('encodes a query, repeating arrays and leaving out undefined and null', () => {
    expect(queryString({ from: 0, tag: ['a b', 'c&d'], none: undefined, nil: null, ok: true })).toBe(
      '?from=0&tag=a+b&tag=c%26d&ok=true',
    );
    expect(queryString({ none: undefined })).toBe('');
    expect(queryString(undefined)).toBe('');
  });
});

describe('Transport.request / json', () => {
  it('sends method, path and query over the sealed channel with the credential held now', async () => {
    node.answer = () => ({ status: 200, json: { appId: 'app-1' } });
    const t = transport();
    t.useApiKey('key-1');
    const answer = await t.json<{ appId: string }>('GET', '/apps/app-1', { query: { verbose: true } });
    expect(answer).toEqual({ appId: 'app-1' });
    expect(node.requests[0].head.method).toBe('GET');
    expect(node.requests[0].head.path).toBe('/apps/app-1?verbose=true');
    expect(header(0, 'authorization')).toBe('Bearer key-1');
  });

  it('sends a JSON body, and answers {} for an empty one', async () => {
    node.answer = () => ({ status: 200, bytes: new Uint8Array(), contentType: 'application/json' });
    const answer = await transport().json('POST', 'orgs', { body: { name: 'acme' } });
    expect(answer).toEqual({});
    expect(node.requests[0].body).toEqual({ name: 'acme' });
    expect(header(0, 'content-type')).toBe('application/json');
  });

  it('sends a raw body as its content type, and answers the response unread', async () => {
    node.answer = () => ({ status: 200, bytes: new Uint8Array([1, 2, 3]), contentType: 'image/png' });
    const response = await transport().request('POST', 'apps/app-1/blobs', {
      bytes: new Uint8Array([9]),
      contentType: 'image/png',
    });
    expect(response.headers.get('content-type')).toBe('image/png');
    expect(new Uint8Array(await response.arrayBuffer())).toEqual(new Uint8Array([1, 2, 3]));
    expect(header(0, 'content-type')).toBe('image/png');
  });

  it('throws a refusal as the typed error its code names', async () => {
    node.answer = () => refused(404, 'NOT_FOUND');
    const error = await failure(transport().request('GET', 'apps/nope'));
    expect(error).toBeInstanceOf(NotFound);
    expect(error).toBeInstanceOf(CalimeroError);
    expect(error).toMatchObject({ status: 404, code: 'NOT_FOUND' });
  });
});

describe('the 412 step-up', () => {
  const challenge = { challengeId: 'c-1', challenge: 'AAEC', rpId: 'localhost' };

  it('answers CONFIRM_REQUIRED with the passkey and sends the same body again with confirm', async () => {
    node.answer = (_, body) => (body.confirm ? { status: 200, json: { ok: true } } : refused(412, 'CONFIRM_REQUIRED', challenge));
    const answer = await transport().json('DELETE', 'orgs/o-1', { body: { reason: 'x' }, passkey });
    expect(answer).toEqual({ ok: true });
    expect(node.requests.map((r) => r.head.method)).toEqual(['DELETE', 'DELETE']);
    expect(node.requests[1].body).toMatchObject({ reason: 'x', confirm: { challengeId: 'c-1', assertion: { credentialId: 'cred' } } });
  });

  it('without a passkey, is ConfirmRequired carrying the challenge', async () => {
    node.answer = () => refused(412, 'CONFIRM_REQUIRED', challenge);
    const error = await failure(transport().json('DELETE', 'orgs/o-1', { body: {} }));
    expect(error).toBeInstanceOf(ConfirmRequired);
    expect((error as ConfirmRequired).details).toMatchObject(challenge);
  });
});

describe('D70 retries', () => {
  it('retry: true sends a 503 again until it is answered', async () => {
    let n = 0;
    node.answer = () => (++n < 3 ? refused(503, 'UNAVAILABLE') : { status: 200, json: { index: 1 } });
    const answer = await transport().json('POST', 'apps/app-1/intents', { body: { idempotencyKey: 'k' }, retry: true });
    expect(answer).toEqual({ index: 1 });
    expect(node.requests).toHaveLength(3);
    expect(new Set(node.requests.map((r) => r.body.idempotencyKey))).toEqual(new Set(['k']));
  });

  it('without retry, a 503 is sent once', async () => {
    node.answer = () => refused(503, 'UNAVAILABLE');
    const error = await failure(transport().json('POST', 'apps/app-1/intents', { body: {} }));
    expect(error).toBeInstanceOf(UnavailableError);
    expect(node.requests).toHaveLength(1);
  });
});

describe('Transport.events', () => {
  it('decodes server-sent events from a sealed stream', async () => {
    node.answer = () => ({
      status: 200,
      events: ['id: 1\nevent: committed\ndata: {"index":1}\n\n', 'event: note\ndata: plain\n\n'],
    });
    const abort = new AbortController();
    const seen: unknown[] = [];
    for await (const event of transport().events('apps/app-1/events', { query: { from: 0 }, signal: abort.signal })) {
      seen.push(event);
      if (seen.length === 2) break;
    }
    abort.abort();
    expect(seen).toEqual([
      { id: '1', kind: 'committed', data: { index: 1 } },
      { id: '', kind: 'note', data: 'plain' },
    ]);
    expect(node.requests[0].head.path).toBe('/apps/app-1/events?from=0');
  });
});

describe('a route layer of its own shares the client', () => {
  it("sends through client.transport with the client's session, refreshed on a 401", async () => {
    const client = new CalimeroClient({ baseUrl: node.url, policy: { allowMock: true } });
    client.useSession({ accountId: 'acc', sessionToken: 'old', expiresAt: 0, refreshToken: 'r-1' });
    node.answer = (head, body) => {
      if (head.path === '/auth/refresh') {
        return { status: 200, json: { accountId: 'acc', sessionToken: 'new', expiresAt: 1, refreshToken: body.refreshToken === 'r-1' ? 'r-2' : 'x' } };
      }
      const auth = head.headers.find(([n]) => n.toLowerCase() === 'authorization')?.[1];
      return auth === 'Bearer new' ? { status: 200, json: { me: 'acc' } } : refused(401, 'UNAUTHENTICATED');
    };
    const routes: Transport = client.transport;
    expect(await routes.json('GET', 'auth/me')).toEqual({ me: 'acc' });
    expect(client.session?.sessionToken).toBe('new');
    expect(node.requests.map((r) => r.head.path)).toEqual(['/auth/me', '/auth/refresh', '/auth/me']);
  });

  it('a client made from a transport uses it', async () => {
    const t = transport();
    const client = new CalimeroClient(t);
    expect(client.transport).toBe(t);
    client.useApiKey('key-2');
    await client.auth.me();
    expect(header(0, 'authorization')).toBe('Bearer key-2');
  });
});

describe('the verification state of a node', () => {
  it('is unknown until the node is first used', () => {
    expect(transport().verification()).toBeUndefined();
  });

  it('is checking while the attestation is in flight', () => {
    const t = new NodeTransport({ baseUrl: node.url, policy: { allowMock: true }, fetch: () => new Promise<Response>(() => undefined) });
    void t.verify();
    expect(t.verification()).toEqual({ status: 'checking', url: node.url });
  });

  it('is the record of the check the first request ran, with no second check', async () => {
    const t = transport();
    await t.json('GET', 'apps/app-1');
    expect(t.verification()).toEqual({ status: 'verified', url: node.url, mock: true, checkedAt: expect.any(Number) });
    expect(await t.verify()).toBe(t.verification());
  });

  it('verify starts the check without a request and answers the outcome', async () => {
    const t = transport();
    expect(await t.verify()).toMatchObject({ status: 'verified', url: node.url, mock: true });
    expect(node.requests).toHaveLength(0);
  });

  it('carries the reason a node was refused, and the request still fails', async () => {
    const t = new NodeTransport({ baseUrl: node.url, policy: {} });
    expect(await t.verify()).toEqual({
      status: 'refused',
      url: node.url,
      reason: expect.stringMatching(/mock quote refused/),
      checkedAt: expect.any(Number),
    });
    await expect(t.json('GET', 'apps/app-1')).rejects.toThrow(/mock quote refused/);
    expect(node.requests).toHaveLength(0);
  });

  it('is kept per node', async () => {
    const t = transport();
    await t.verify();
    expect(t.verification('http://elsewhere.test')).toBeUndefined();
  });
});

describe('attesting one app on a connected node', () => {
  it('answers the record with the app hash the node bound, on the check the node already passed', async () => {
    const t = transport();
    expect(await t.attestApp('app-1')).toEqual({
      url: node.url,
      mock: true,
      appHash: hex(APP_BUNDLE_HASH),
      checkedAt: expect.any(Number),
    });
    expect(t.verification()).not.toHaveProperty('appHash');
  });

  it('throws the reason when the node itself was refused', async () => {
    await expect(new NodeTransport({ baseUrl: node.url, policy: {} }).attestApp('app-1')).rejects.toThrow(/mock quote refused/);
  });

  it('refuses an app attested under another transport key than the node channel', async () => {
    const keys = [new Uint8Array(32).fill(1), new Uint8Array(32).fill(2)];
    const fetched = vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(String(input));
      const key = keys[url.searchParams.has('app') ? 1 : 0];
      return new Response(
        JSON.stringify({ quote: btoa('real'), transportKey: hex(key), appHash: hex(new Uint8Array(32).fill(7)) }),
      );
    });
    const t = new NodeTransport({ baseUrl: node.url, policy: { verifyQuote: async () => true }, fetch: fetched as typeof fetch });
    await expect(t.attestApp('app-1')).rejects.toThrow(/another transport key/);
  });
});

const attestations = (inner: typeof fetch, change: (call: number, response: Response) => Promise<Response> | Response | Error) => {
  let calls = 0;
  const seen: string[] = [];
  const wrapped = (async (input: RequestInfo | URL, init?: RequestInit) => {
    if (!new URL(String(input)).pathname.endsWith('/attestation')) return inner(input, init);
    const call = ++calls;
    const outcome = change(call, await inner(input, init));
    if (outcome instanceof Error) throw outcome;
    return outcome;
  }) as typeof fetch;
  return { wrapped, seen };
};
const sleep = (ms: number) => new Promise((ok) => setTimeout(ok, ms));

describe('a node that cannot be reached is not a node that refused', () => {
  it('is unreachable, then verified once the next attempt gets through', async () => {
    const { wrapped } = attestations(fetch, (call, response) => (call === 1 ? new TypeError('fetch failed') : response));
    const t = new NodeTransport({ baseUrl: node.url, policy: { allowMock: true }, fetch: wrapped });
    expect(await t.verify()).toEqual({
      status: 'unreachable',
      url: node.url,
      reason: 'fetch failed',
      checkedAt: expect.any(Number),
    });
    expect(await t.verify()).toMatchObject({ status: 'verified', url: node.url });
  });

  it('lets the next request attest again, without a reload', async () => {
    const { wrapped } = attestations(fetch, (call, response) => (call === 1 ? new TypeError('fetch failed') : response));
    const t = new NodeTransport({ baseUrl: node.url, policy: { allowMock: true }, fetch: wrapped });
    await expect(t.json('GET', 'apps/app-1')).rejects.toThrow(/fetch failed/);
    expect(t.verification()).toMatchObject({ status: 'unreachable' });
    await t.json('GET', 'apps/app-1');
    expect(t.verification()).toMatchObject({ status: 'verified' });
  });

  it('is unreachable while the node answers its attestation with a 5xx, then verified', async () => {
    const { wrapped } = attestations(fetch, (call, response) => (call === 1 ? new Response('', { status: 502 }) : response));
    const t = new NodeTransport({ baseUrl: node.url, policy: { allowMock: true }, fetch: wrapped });
    expect(await t.verify()).toMatchObject({ status: 'unreachable', reason: expect.stringMatching(/HTTP 502/) });
    expect(await t.verify()).toMatchObject({ status: 'verified' });
  });

  it('is unreachable when the connection drops while the attestation is read, then verified', async () => {
    const dropped = () =>
      new Response(new ReadableStream({ start: (c) => c.error(new TypeError('terminated')) }), { status: 200 });
    const { wrapped } = attestations(fetch, (call, response) => (call === 1 ? dropped() : response));
    const t = new NodeTransport({ baseUrl: node.url, policy: { allowMock: true }, fetch: wrapped });
    expect(await t.verify()).toMatchObject({ status: 'unreachable', reason: expect.stringMatching(/terminated/) });
    expect(await t.verify()).toMatchObject({ status: 'verified' });
  });

  it('keeps refused for a node that answers its attestation with malformed JSON', async () => {
    const { wrapped } = attestations(fetch, () => new Response('{', { status: 200 }));
    const t = new NodeTransport({ baseUrl: node.url, policy: { allowMock: true }, fetch: wrapped });
    await t.verify();
    expect(await t.verify()).toMatchObject({ status: 'refused' });
  });

  it('is unreachable while the node answers its attestation with 429, then verified', async () => {
    const { wrapped } = attestations(fetch, (call, response) => (call === 1 ? new Response('', { status: 429 }) : response));
    const t = new NodeTransport({ baseUrl: node.url, policy: { allowMock: true }, fetch: wrapped });
    expect(await t.verify()).toMatchObject({ status: 'unreachable', reason: expect.stringMatching(/HTTP 429/) });
    expect(await t.verify()).toMatchObject({ status: 'verified' });
  });

  it('keeps refused for a node that answers its attestation with a 4xx', async () => {
    const { wrapped } = attestations(fetch, () => new Response('', { status: 404 }));
    const t = new NodeTransport({ baseUrl: node.url, policy: { allowMock: true }, fetch: wrapped });
    await t.verify();
    expect(await t.verify()).toMatchObject({ status: 'refused', reason: expect.stringMatching(/HTTP 404/) });
  });

  it('keeps refused for a node that failed the policy', async () => {
    const t = new NodeTransport({ baseUrl: node.url, policy: {} });
    await t.verify();
    expect(await t.verify()).toMatchObject({ status: 'refused' });
  });
});

describe('an attestation left unanswered', () => {
  const unanswered = (ok: (call: number) => boolean) => {
    const other: string[] = [];
    let calls = 0;
    const fetched = (async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input));
      if (url.origin === 'http://other.test') {
        other.push(url.pathname);
        throw new TypeError('fetch failed');
      }
      if (url.pathname.endsWith('/attestation') && !ok(++calls)) return new Response('', { status: 503 });
      return fetch(input, init);
    }) as typeof fetch;
    return { fetched, other, calls: () => calls };
  };

  it('does not move the transport to another node', async () => {
    const { fetched, other } = unanswered(() => false);
    const t = new NodeTransport({
      baseUrl: node.url,
      nodes: ['http://other.test'],
      policy: { allowMock: true },
      fetch: fetched,
      retry: { baseDelayMs: 1 },
    });
    await expect(t.json('GET', 'apps/app-1', { retry: true })).rejects.toBeInstanceOf(Unanswered);
    expect(t.nodeUrl).toBe(node.url);
    expect(other).toEqual([]);
  });

  it('is retried by the client until the attestation is answered', async () => {
    const { fetched, calls } = unanswered((call) => call > 2);
    node.answer = () => refused(404, 'NOT_FOUND');
    const client = new CalimeroClient({ baseUrl: node.url, policy: { allowMock: true }, fetch: fetched, retry: { baseDelayMs: 1, maxDelayMs: 2 } });
    await expect(client.uploadFile('app', new Uint8Array(FILE_PART_BYTES + 1))).rejects.toBeInstanceOf(NotFound);
    expect(calls()).toBe(3);
    expect(node.requests).toHaveLength(1);
  });

  it('is retried by the client only up to its limit', async () => {
    const { fetched, calls } = unanswered(() => false);
    const client = new CalimeroClient({ baseUrl: node.url, policy: { allowMock: true }, fetch: fetched, retry: { baseDelayMs: 1, maxDelayMs: 2 } });
    await expect(client.uploadFile('app', new Uint8Array(FILE_PART_BYTES + 1))).rejects.toBeInstanceOf(Unanswered);
    expect(calls()).toBeGreaterThan(1);
    expect(node.requests).toHaveLength(0);
  });
});

describe('a node that changed its transport key (re-attested on stale_transport_key)', () => {
  it('is checking while it is attested again, then verified later', async () => {
    const states: Array<string | undefined> = [];
    let t!: NodeTransport;
    const { wrapped } = attestations(fetch, (call, response) => {
      if (call === 2) states.push(t.verification()?.status);
      return response;
    });
    t = new NodeTransport({ baseUrl: node.url, policy: { allowMock: true }, fetch: wrapped });
    const first = await t.verify();
    await sleep(5);
    await node.rotateKey();
    await t.json('GET', 'apps/app-1');
    expect(states).toEqual(['checking']);
    const second = t.verification();
    expect(second).toMatchObject({ status: 'verified' });
    expect((second as { checkedAt: number }).checkedAt).toBeGreaterThan((first as { checkedAt: number }).checkedAt);
  });

  it('is refused when the new attestation fails, and the request fails with it', async () => {
    const { wrapped } = attestations(fetch, async (call, response) => {
      if (call !== 2) return response;
      const doc = await response.json();
      return new Response(JSON.stringify({ ...doc, transportKey: hex(new Uint8Array(32).fill(9)) }));
    });
    const t = new NodeTransport({ baseUrl: node.url, policy: { allowMock: true }, fetch: wrapped });
    await t.verify();
    await node.rotateKey();
    await expect(t.json('GET', 'apps/app-1')).rejects.toThrow(/does not bind/);
    expect(t.verification()).toMatchObject({ status: 'refused', reason: expect.stringMatching(/does not bind/) });
  });
});

describe('verify and attestApp around a restarted node', () => {
  it('verify answers the settled outcome, not a check a re-key started meanwhile', async () => {
    let release!: () => void;
    const gate = new Promise<void>((ok) => (release = ok));
    const { wrapped } = attestations(fetch, async (call, response) => {
      if (call === 2) await gate;
      return response;
    });
    const t = new NodeTransport({ baseUrl: node.url, policy: { allowMock: true }, fetch: wrapped });
    await t.verify();
    await node.rotateKey();
    const rekey = t.json('GET', 'apps/app-1');
    while (t.verification()?.status !== 'checking') await sleep(1);
    const settled = t.verify();
    release();
    expect(await settled).toMatchObject({ status: 'verified' });
    await rekey;
  });

  it('attestApp attests the channel again once when the node restarted before any request re-keyed', async () => {
    const t = transport();
    await t.verify();
    await node.rotateKey();
    expect(await t.attestApp('app-1')).toMatchObject({ appHash: hex(APP_BUNDLE_HASH) });
  });

  it('attestApp does not hold the client-wide pinned app hash against the app the node binds', async () => {
    const fetched = (async (input: RequestInfo | URL) => {
      const key = hex(new Uint8Array(32).fill(1));
      const bound = new URL(String(input)).searchParams.has('app') ? new Uint8Array(32).fill(7) : new Uint8Array(32);
      return new Response(JSON.stringify({ quote: btoa('real'), transportKey: key, appHash: hex(bound) }));
    }) as typeof fetch;
    const t = new NodeTransport({
      baseUrl: node.url,
      policy: { verifyQuote: async () => true, appHash: 'ab'.repeat(32) },
      fetch: fetched,
    });
    expect(await t.attestApp('app-1')).toMatchObject({ appHash: '07'.repeat(32) });
  });
});

describe('attestApp and the channel requests go through', () => {
  it('uses the key the dropped channel is attested to afresh, so the next request goes through it', async () => {
    const t = transport();
    await t.json('GET', 'apps/app-1');
    await node.rotateKey();
    expect(await t.attestApp('app-1')).toMatchObject({ appHash: hex(APP_BUNDLE_HASH) });
    await t.json('GET', 'apps/app-1');
    expect(t.verification()).toMatchObject({ status: 'verified' });
  });

  it('refuses when the app quote and the channel check keep coming from nodes with different keys', async () => {
    const calls = { node: 0, app: 0 };
    const fetched = (async (input: RequestInfo | URL) => {
      const app = new URL(String(input)).searchParams.has('app');
      calls[app ? 'app' : 'node'] += 1;
      const key = hex(new Uint8Array(32).fill(app ? 2 : 1));
      return new Response(JSON.stringify({ quote: btoa('real'), transportKey: key, appHash: hex(new Uint8Array(32).fill(7)) }));
    }) as typeof fetch;
    const t = new NodeTransport({ baseUrl: node.url, policy: { verifyQuote: async () => true }, fetch: fetched });
    await expect(t.attestApp('app-1')).rejects.toThrow(/another transport key/);
    expect(calls).toEqual({ node: 2, app: 2 });
  });

  const pinned = (bound: [number, number]) => {
    let calls = 0;
    const fetched = (async () => {
      const hash = new Uint8Array(32).fill(bound[Math.min(calls++, 1)]);
      return new Response(JSON.stringify({ quote: btoa('real'), transportKey: hex(new Uint8Array(32).fill(1)), appHash: hex(hash) }));
    }) as typeof fetch;
    return new NodeTransport({
      baseUrl: node.url,
      policy: { verifyQuote: async () => true, app: 'app-1', appHash: '07'.repeat(32) },
      fetch: fetched,
    });
  };

  it('keeps a pin meant for this app', async () => {
    await expect(pinned([7, 8]).attestApp('app-1')).rejects.toThrow(/not the pinned/);
  });

  it('drops a pin meant for another app', async () => {
    expect(await pinned([7, 8]).attestApp('app-2')).toMatchObject({ appHash: '08'.repeat(32) });
  });
});

describe('a release mirror that cannot be reached', () => {
  const mirrored = (mirror: () => Response) =>
    new NodeTransport({
      baseUrl: node.url,
      policy: { release: { minReleaseVersion: '2.3.87', releaseSource: 'http://mirror.test' } },
      fetch: (async (input: RequestInfo | URL) => {
        if (String(input).startsWith('http://mirror.test')) return mirror();
        return attestation();
      }) as typeof fetch,
    });
  const attestation = () =>
    new Response(
      JSON.stringify({
        quote: btoa('real'),
        transportKey: hex(new Uint8Array(32).fill(1)),
        release: { version: '2.3.87', profile: 'locked-read-only' },
      }),
    );

  it('is unreachable, named as the mirror and not the node', async () => {
    const fetched = (async (input: RequestInfo | URL) => {
      if (String(input).startsWith('http://mirror.test')) throw new TypeError('fetch failed');
      return new Response(
        JSON.stringify({
          quote: btoa('real'),
          transportKey: hex(new Uint8Array(32).fill(1)),
          release: { version: '2.3.87', profile: 'locked-read-only' },
        }),
      );
    }) as typeof fetch;
    const t = new NodeTransport({
      baseUrl: node.url,
      policy: { release: { minReleaseVersion: '2.3.87', releaseSource: 'http://mirror.test' } },
      fetch: fetched,
    });
    expect(await t.verify()).toEqual({
      status: 'unreachable',
      url: node.url,
      reason: 'the release mirror could not be reached: fetch failed',
      checkedAt: expect.any(Number),
    });
  });

  it('is unreachable when the mirror answers with a 5xx', async () => {
    const state = await mirrored(() => new Response('', { status: 503 })).verify();
    expect(state).toMatchObject({ status: 'unreachable', reason: expect.stringMatching(/HTTP 503/) });
  });

  const dropped = () => new Response(new ReadableStream({ start: (c) => c.error(new TypeError('terminated')) }));
  const github = (answer: (url: string) => Response) =>
    new NodeTransport({
      baseUrl: node.url,
      policy: { release: { minReleaseVersion: '2.3.87', releaseSource: 'github' } },
      fetch: (async (input: RequestInfo | URL) =>
        String(input).startsWith('https://github.com') ? answer(String(input)) : attestation()) as typeof fetch,
    });

  it('is unreachable when the mirror answers with 429', async () => {
    expect(await mirrored(() => new Response('', { status: 429 })).verify()).toMatchObject({
      status: 'unreachable',
      reason: expect.stringMatching(/HTTP 429/),
    });
  });

  it('is unreachable when the connection drops while the mirror answer is read', async () => {
    expect(await mirrored(dropped).verify()).toMatchObject({ status: 'unreachable', reason: expect.stringMatching(/terminated/) });
    expect(await github(dropped).verify()).toMatchObject({ status: 'unreachable', reason: expect.stringMatching(/terminated/) });
  });

  it('is refused when the mirror answers malformed JSON', async () => {
    const t = mirrored(() => new Response('{'));
    await t.verify();
    expect(await t.verify()).toMatchObject({ status: 'refused', reason: expect.stringMatching(/did not answer with JSON/) });
  });

  it("is refused when GitHub answers 403", async () => {
    const t = github(() => new Response('', { status: 403 }));
    await t.verify();
    expect(await t.verify()).toMatchObject({ status: 'refused', reason: expect.stringMatching(/HTTP 403/) });
  });

  it('is refused when the mirror answers with a 4xx', async () => {
    const t = mirrored(() => new Response('', { status: 404 }));
    await t.verify();
    expect(await t.verify()).toMatchObject({ status: 'refused', reason: expect.stringMatching(/HTTP 404/) });
  });
});

describe('a dropped channel never writes the node state', () => {
  type Internals = { channels: Map<string, typeof fetch>; keys: Map<string, Uint8Array> };
  const keyOf = (n: number) => new Uint8Array(32).fill(n);

  /** Node-level attestations numbered from 1, each answering key `n`; call `held` waits for `release(outcome)`. */
  function replicas(held?: number) {
    let calls = 0;
    let release!: (outcome: 'unreachable' | 'ok') => void;
    const gate = new Promise<'unreachable' | 'ok'>((ok) => (release = ok));
    const fetched = (async (input: RequestInfo | URL) => {
      const path = new URL(String(input)).pathname;
      if (path.endsWith('/sealed/v2/handshake')) {
        return new Response(JSON.stringify({ error: { code: 'stale_transport_key' } }), { status: 409 });
      }
      const call = ++calls;
      if (call === held && (await gate) === 'unreachable') throw new TypeError('fetch failed');
      return new Response(JSON.stringify({ quote: btoa('real'), transportKey: hex(keyOf(call)), appHash: hex(keyOf(9)) }));
    }) as typeof fetch;
    const t = new NodeTransport({ baseUrl: node.url, policy: { verifyQuote: async () => true }, fetch: fetched });
    return { t, release, internals: t as unknown as Internals, calls: () => calls };
  }
  const tick = () => sleep(5);

  it('ignores an older attempt that fails after a newer one on a new channel verified', async () => {
    const { t, release, internals } = replicas(1);
    const first = t.verify();
    internals.channels.delete(node.url);
    expect(await t.verify()).toMatchObject({ status: 'verified' });
    const second = internals.channels.get(node.url);
    release('unreachable');
    await first;
    await tick();
    expect(t.verification()).toMatchObject({ status: 'verified', url: node.url });
    expect(internals.keys.get(node.url)).toEqual(keyOf(2));
    expect(internals.channels.get(node.url)).toBe(second);
  });

  it('ignores an older attempt that verifies after a newer one, keeping the newer key', async () => {
    const { t, release, internals } = replicas(1);
    const first = t.verify();
    internals.channels.delete(node.url);
    await t.verify();
    release('ok');
    await first;
    await tick();
    expect(internals.keys.get(node.url)).toEqual(keyOf(2));
  });

  it('keeps the new channel state when the dropped channel re-keys after the drop', async () => {
    const { t, internals, calls } = replicas();
    await t.verify();
    const dropped = internals.channels.get(node.url) as typeof fetch;
    internals.channels.delete(node.url);
    await t.verify();
    await expect(dropped(`${node.url}/apps/app-1`)).rejects.toThrow();
    await tick();
    expect(calls()).toBeGreaterThan(2);
    expect(t.verification()).toMatchObject({ status: 'verified', url: node.url });
    expect(internals.keys.get(node.url)).toEqual(keyOf(2));
  });
});
