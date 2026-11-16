/**
 * Reaching a Calimero Cloud cluster by name: the public node directory picks
 * which nodes to try and in what order, and a call fails over between them
 * while a node is unavailable. Stand-in nodes on real local HTTP servers,
 * through the real sealed channel; the directory is an injected `fetch`.
 * Synthetic data only.
 */

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

import { StandInNode, type Reply } from '../../tests/support/stand-in-node.js';
import {
  CalimeroClient,
  DEFAULT_DIRECTORY,
  DirectoryError,
  UnavailableError,
  type CalimeroConnectOptions,
} from '../client/index.js';
import { passkeyConfirm } from './passkey.js';

const a = new StandInNode();
const b = new StandInNode();
const c = new StandInNode();
const all = [a, b, c];
beforeAll(() => Promise.all(all.map((n) => n.start())));
afterAll(() => Promise.all(all.map((n) => n.stop())));
beforeEach(() => {
  for (const node of all) {
    node.requests.length = 0;
    node.answer = () => ({ status: 200, json: {} });
  }
});
afterEach(() => vi.useRealTimers());

const DIRECTORY = 'https://directory.test/api/directory';
const ok = (json: unknown): Reply => ({ status: 200, json });
const unavailable = (): Reply => ({ status: 503, json: { error: 'UNAVAILABLE', message: 'unavailable (stand-in)' } });
const dropped = (): Reply => ({ status: 0, drop: true });
const failure = (p: Promise<unknown>) => p.then(() => undefined, (e: unknown) => e);
const intents = (node: StandInNode) => node.requests.filter((r) => r.head.path === '/apps/app-1/intents');

type Entry = { id?: number; url?: unknown; zone?: unknown; weight?: unknown };
const listing = (nodes: Entry[], refreshAfterSeconds = 60) => ({ cluster: 'eu-test-1', nodes, refreshAfterSeconds });

/**
 * A `fetch` that answers the directory from `answer()` (a body, or a status
 * with no body) and passes everything else to the real `fetch`.
 */
function withDirectory(answer: () => unknown) {
  const asked: string[] = [];
  const fetcher: typeof fetch = async (input, init) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    if (!url.startsWith(DIRECTORY)) return fetch(input, init);
    asked.push(url);
    const reply = answer();
    if (typeof reply === 'number') return new Response('', { status: reply });
    if (reply instanceof Error) throw reply;
    return new Response(JSON.stringify(reply), { status: 200, headers: { 'content-type': 'application/json' } });
  };
  return { fetch: fetcher, asked };
}

/** A `random()` answering `values` in turn, then 0. */
const sequence = (...values: number[]) => () => values.shift() ?? 0;

const connect = (answer: () => unknown, options: Partial<CalimeroConnectOptions> = {}) => {
  const directory = withDirectory(answer);
  const client = CalimeroClient.connect({
    cluster: 'eu-test-1',
    policy: { allowMock: true },
    directory: DIRECTORY,
    fetch: directory.fetch,
    retry: { retries: 2, baseDelayMs: 1 },
    random: () => 0,
    ...options,
  });
  return { client, asked: directory.asked };
};

describe('CalimeroClient.connect: the node directory', () => {
  it('asks the directory for the cluster by name, unauthenticated', async () => {
    const { client, asked } = connect(() => listing([{ id: 1, url: a.url, zone: 'z1', weight: 1 }]), {
      cluster: 'eu test/1',
      directory: `${DIRECTORY}/`,
    });
    await client;
    expect(asked).toEqual([`${DIRECTORY}/eu%20test%2F1`]);
  });

  it('defaults to the Calimero Cloud directory', () => {
    expect(DEFAULT_DIRECTORY).toBe('https://manager.cloud.calimero.network/api/directory');
  });

  it('orders the nodes in the zone first, each group by weight', async () => {
    const nodes = [
      { url: 'https://n1.test', zone: 'far', weight: 100 },
      { url: 'https://n2.test', zone: 'near', weight: 1 },
      { url: 'https://n3.test', zone: 'near', weight: 3 },
      { url: 'https://n4.test', zone: 'far', weight: 1 },
    ];
    // near: weights [1, 3], total 4; 0.5 * 4 = 2 lands on n3, then n2.
    // far: weights [100, 1], total 101; 0.995 * 101 lands on n4, then n1.
    const { client } = connect(() => listing(nodes), { zone: 'near', random: sequence(0.5, 0, 0.995, 0) });
    const cloud = await client;
    expect(cloud.nodes).toEqual(['https://n3.test', 'https://n2.test', 'https://n4.test', 'https://n1.test']);
    expect(cloud.nodeUrl).toBe('https://n3.test');
  });

  it('without a zone, orders every node by weight alone', async () => {
    const nodes = [
      { url: 'https://n1.test', zone: 'z1', weight: 1 },
      { url: 'https://n2.test', zone: 'z2', weight: 1 },
      { url: 'https://n3.test', zone: 'z3', weight: 2 },
    ];
    // total 4: 0.6 * 4 = 2.4 lands on n3; then total 2: 0.75 * 2 = 1.5 lands on n2.
    const { client } = connect(() => listing(nodes), { random: sequence(0.6, 0.75) });
    expect((await client).nodes).toEqual(['https://n3.test', 'https://n2.test', 'https://n1.test']);
  });

  it('a heavier node comes first more often', async () => {
    const nodes = [
      { url: 'https://light.test', weight: 1 },
      { url: 'https://heavy.test', weight: 9 },
    ];
    let first = 0;
    for (let i = 0; i < 100; i++) {
      const { client } = connect(() => listing(nodes), { random: () => i / 100 });
      if ((await client).nodeUrl === 'https://heavy.test') first++;
    }
    expect(first).toBe(90);
  });

  it('drops entries that are not http(s) URLs with a positive integer weight', async () => {
    const nodes: Entry[] = [
      { url: 'ftp://bad.test', weight: 1 },
      { url: 'not a url', weight: 1 },
      { url: 42, weight: 1 },
      { weight: 1 },
      { url: 'https://zero.test', weight: 0 },
      { url: 'https://negative.test', weight: -1 },
      { url: 'https://fraction.test', weight: 1.5 },
      { url: 'https://text.test', weight: '1' },
      { url: 'https://good.test/', weight: 1 },
      { url: 'http://also-good.test', weight: 2, zone: 7 },
      { url: 'https://good.test', weight: 5 },
    ];
    const { client } = connect(() => ({ ...listing([]), nodes: [...nodes, null, 'x'] }));
    expect([...(await client).nodes].sort()).toEqual(['http://also-good.test', 'https://good.test']);
  });

  it('is DirectoryError when no usable node remains', async () => {
    const error = await failure(connect(() => listing([{ url: 'ftp://bad.test', weight: 1 }])).client);
    expect(error).toBeInstanceOf(DirectoryError);
    expect(error).toMatchObject({ cluster: 'eu-test-1', reason: 'empty' });
  });

  it('is DirectoryError for an unknown cluster (404)', async () => {
    const error = await failure(connect(() => 404).client);
    expect(error).toBeInstanceOf(DirectoryError);
    expect(error).toMatchObject({ cluster: 'eu-test-1', reason: 'not-found', status: 404 });
  });

  it('is DirectoryError when the directory fails or answers something else', async () => {
    const unreachable = new TypeError('fetch failed');
    const cases: Array<[() => unknown, string]> = [
      [() => unreachable, 'unreachable'],
      [() => 500, 'unreachable'],
      [() => ['not', 'a', 'listing'], 'bad-response'],
      [() => ({ cluster: 'eu-test-1' }), 'bad-response'],
    ];
    for (const [answer, reason] of cases) {
      const error = await failure(connect(answer).client);
      expect(error).toBeInstanceOf(DirectoryError);
      expect(error).toMatchObject({ reason });
    }
  });

  it('attests the chosen node under the caller policy before the first request', async () => {
    a.answer = () => ok({ index: 1, result: 'v', events: [] });
    const { client } = connect(() => listing([{ url: a.url, weight: 1 }]), { policy: {} });
    // A stand-in sends a mock quote, which a policy without `allowMock` refuses.
    const error = await failure((await client).apps.view({ app: 'app-1', method: 'get', args: {} }));
    expect(error).toBeDefined();
    expect(error).not.toBeInstanceOf(UnavailableError);
    expect(a.requests).toHaveLength(0);
  });
});

describe('failover: a node that stays unavailable', () => {
  const cluster = (...urls: string[]) =>
    new CalimeroClient({ baseUrl: urls[0], nodes: urls.slice(1), policy: { allowMock: true }, retry: { retries: 2, baseDelayMs: 1 } });

  it('a call answered 503 until its retries run out moves to the next node, with the same key', async () => {
    a.answer = unavailable;
    b.answer = () => ok({ index: 9, result: 'b1', events: [] });
    const client = cluster(a.url, b.url);
    const answer = await client.apps.call({ app: 'app-1', method: 'book', args: { roomId: 'r1' } });
    expect(answer.result).toBe('b1');
    expect(intents(a)).toHaveLength(3);
    expect(intents(b)).toHaveLength(1);
    const keys = new Set([...intents(a), ...intents(b)].map((r) => r.body.idempotencyKey));
    expect(keys.size).toBe(1);
    expect(client.nodeUrl).toBe(b.url);
  });

  it('a connection that keeps failing moves to the next node too, and retries start again there', async () => {
    let attempts = 0;
    a.answer = dropped;
    b.answer = () => (++attempts <= 2 ? unavailable() : ok({ index: 2, result: 'v', events: [] }));
    const answer = await cluster(a.url, b.url).apps.call({ app: 'app-1', method: 'book', args: {}, idempotencyKey: 'caller-key' });
    expect(answer.result).toBe('v');
    expect(intents(a)).toHaveLength(3);
    expect(intents(b).map((r) => r.body.idempotencyKey)).toEqual(['caller-key', 'caller-key', 'caller-key']);
  });

  it('a view fails over too', async () => {
    a.answer = unavailable;
    b.answer = () => ok({ index: 1, result: [] });
    expect((await cluster(a.url, b.url).apps.view({ app: 'app-1', method: 'list' })).result).toEqual([]);
  });

  it('tries every node once, then is the last UnavailableError', async () => {
    a.answer = unavailable;
    b.answer = dropped;
    c.answer = unavailable;
    const client = cluster(a.url, b.url, c.url);
    const error = await failure(client.apps.call({ app: 'app-1', method: 'book', args: {} }));
    expect(error).toBeInstanceOf(UnavailableError);
    expect(error).toMatchObject({ code: 'UNAVAILABLE' });
    expect([intents(a).length, intents(b).length, intents(c).length]).toEqual([3, 3, 3]);
  });

  it('with a single node, a 503 is retried there only, then UnavailableError', async () => {
    a.answer = unavailable;
    const client = cluster(a.url);
    expect(await failure(client.apps.call({ app: 'app-1', method: 'book', args: {} }))).toBeInstanceOf(UnavailableError);
    expect(intents(a)).toHaveLength(3);
    expect(client.nodeUrl).toBe(a.url);
  });

  it('a request after another already moved does not move again', async () => {
    a.answer = unavailable;
    b.answer = () => ok({ index: 1, result: 'v', events: [] });
    const client = cluster(a.url, b.url, c.url);
    const answers = await Promise.all([client.apps.call({ app: 'app-1', method: 'x' }), client.apps.call({ app: 'app-1', method: 'y' })]);
    expect(answers.map((x) => x.result)).toEqual(['v', 'v']);
    expect(client.nodeUrl).toBe(b.url);
    expect(intents(c)).toHaveLength(0);
  });

  it('the confirmed attempt of callConfirmed is sent once, not failed over', async () => {
    a.answer = (_, body) =>
      body.confirm
        ? unavailable()
        : { status: 412, json: { error: 'CONFIRM_REQUIRED', message: 'x', details: { challengeId: 'c-1', challenge: 'AAEC', rpId: 'localhost' } } };
    const passkey = {
      create: async () => ({ attestationObject: '', clientDataJson: '' }),
      get: async () => ({ credentialId: 'cred', authenticatorData: 'ad', clientDataJson: 'cd', signature: 'sig' }),
    };
    const error = await failure(cluster(a.url, b.url).apps.call({ app: 'app-1', method: 'adminRelease', args: {} }, { confirm: passkeyConfirm(passkey) }));
    expect(error).toBeInstanceOf(UnavailableError);
    expect(intents(a)).toHaveLength(2);
    expect(b.requests).toHaveLength(0);
  });
});

describe('a client made by connect keeps its node list fresh', () => {
  it('re-reads the directory after a failover: the node in use first, then the directory order', async () => {
    a.answer = unavailable;
    b.answer = () => ok({ index: 1, result: 'v', events: [] });
    let answer = listing([
      { url: a.url, weight: 1 },
      { url: b.url, weight: 1 },
    ]);
    const { client, asked } = connect(() => answer);
    const cloud = await client;
    expect(cloud.nodes).toEqual([a.url, b.url]);
    // By the time it is read again, `a` is gone and `c` has joined.
    answer = listing([
      { url: c.url, weight: 1 },
      { url: b.url, weight: 1 },
    ]);
    await cloud.apps.call({ app: 'app-1', method: 'book', args: {} });
    expect(cloud.nodeUrl).toBe(b.url);
    await vi.waitFor(() => expect(cloud.nodes).toEqual([b.url, c.url]));
    expect(asked).toHaveLength(2);
    expect(cloud.nodeUrl).toBe(b.url);
  });

  it('a failed re-read is ignored', async () => {
    a.answer = unavailable;
    b.answer = () => ok({ index: 1, result: 'v', events: [] });
    let reads = 0;
    const { client, asked } = connect(() =>
      ++reads === 1
        ? listing([
            { url: a.url, weight: 1 },
            { url: b.url, weight: 1 },
          ])
        : 500,
    );
    const cloud = await client;
    await cloud.apps.call({ app: 'app-1', method: 'book', args: {} });
    await vi.waitFor(() => expect(asked).toHaveLength(2));
    await cloud.apps.call({ app: 'app-1', method: 'book', args: {} });
    expect(cloud.nodes).toEqual([a.url, b.url]);
    expect(cloud.nodeUrl).toBe(b.url);
  });

  it('re-reads it lazily once refreshAfterSeconds has passed', async () => {
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(1_000_000);
    b.answer = () => ok({ index: 1, result: 'v' });
    let answer = listing([{ url: b.url, weight: 1 }], 30);
    const { client, asked } = connect(() => answer);
    const cloud = await client;
    answer = listing([
      { url: c.url, weight: 1 },
      { url: b.url, weight: 1 },
    ]);
    vi.setSystemTime(1_000_000 + 29_000);
    await cloud.apps.view({ app: 'app-1', method: 'get' });
    expect(asked).toHaveLength(1);
    vi.setSystemTime(1_000_000 + 31_000);
    await cloud.apps.view({ app: 'app-1', method: 'get' });
    await vi.waitFor(() => expect(cloud.nodes).toEqual([b.url, c.url]));
    expect(asked).toHaveLength(2);
  });

  it('a client made with new CalimeroClient never reads a directory', async () => {
    a.answer = unavailable;
    b.answer = () => ok({ index: 1, result: 'v', events: [] });
    const directory = withDirectory(() => listing([]));
    const client = new CalimeroClient({
      baseUrl: a.url,
      nodes: [b.url],
      policy: { allowMock: true },
      fetch: directory.fetch,
      retry: { retries: 0 },
    });
    await client.apps.call({ app: 'app-1', method: 'book', args: {} });
    expect(directory.asked).toHaveLength(0);
    expect(client.nodes).toEqual([a.url, b.url]);
  });
});
