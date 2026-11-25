/**
 * Finding the cluster an app runs on without naming it: a page a cluster
 * serves reads its own `/.well-known/calimero.json`, anything else (Vercel,
 * `localhost`, Node) uses the app's fallback, and the Calimero Cloud a page is
 * on is read from its hostname alone. No node is contacted: these check which
 * nodes the client would attest and talk to. Synthetic data only.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';

import { CLOUD_ENVIRONMENTS, CalimeroClient, DEFAULT_DIRECTORY, cloudEnvironment } from './index.js';

const policy = { allowMock: true };
const STAGING_DIRECTORY = CLOUD_ENVIRONMENTS.staging.directory;

/** A `fetch` answering each URL from `routes` (a body, or a status with no body), 404 otherwise; records what was asked. */
function fake(routes: Record<string, unknown>) {
  const asked: string[] = [];
  const fetcher = (async (input: string | URL | Request) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    asked.push(url);
    if (!(url in routes)) return new Response('not found', { status: 404 });
    const reply = routes[url];
    if (typeof reply === 'number') return new Response('', { status: reply });
    if (typeof reply === 'string') return new Response(reply, { status: 200, headers: { 'content-type': 'text/html' } });
    return new Response(JSON.stringify(reply), { status: 200, headers: { 'content-type': 'application/json' } });
  }) as unknown as typeof fetch;
  return { fetch: fetcher, asked };
}

const listing = (cluster: string, urls: string[]) => ({
  cluster,
  nodes: urls.map((url, id) => ({ id, url, weight: 1 })),
  refreshAfterSeconds: 60,
});

describe('cloudEnvironment', () => {
  it('reads the Calimero Cloud from the apps domain a hostname is under', () => {
    expect(cloudEnvironment('5eolix2xlsehjkbd2l5262oevb65zvq4dkgo66zlxpfc4os34una.apps.calimero.network')).toBe('production');
    expect(cloudEnvironment('abc.apps.relay.staging.cloud.calimero.network')).toBe('staging');
    expect(cloudEnvironment('ABC.APPS.CALIMERO.NETWORK.')).toBe('production');
  });

  it('is null for every other host, and for the apps domains themselves', () => {
    for (const host of [
      'my-app.vercel.app',
      'localhost',
      'kv.example.com',
      'apps.calimero.network',
      'evilapps.calimero.network',
      'apps.calimero.network.evil.com',
      'apps.relay.staging.cloud.calimero.network',
      'xapps.relay.staging.cloud.calimero.network',
      'evil-apps.relay.staging.cloud.calimero.network.attacker.com',
      '',
    ]) {
      expect(cloudEnvironment(host), host).toBeNull();
    }
  });
});

describe('CalimeroClient.connectHere', () => {
  it("connects a page a cluster serves to the nodes its manifest lists, and names that cluster", async () => {
    const origin = 'https://abc.apps.calimero.network';
    const { fetch, asked } = fake({
      [`${origin}/.well-known/calimero.json`]: {
        package: 'com.example.notes',
        cluster: 'eu-dedicated-7',
        deployment: null,
        versions: ['1.0.0'],
        deployments: [],
        api: ['https://n1.example', 'https://n2.example'],
      },
    });
    const client = await CalimeroClient.connectHere({
      policy,
      origin,
      href: `${origin}/1.0.0/?deployment=d-1`,
      fetch,
      fallback: { cluster: 'eu-shared-1' },
    });
    expect(client.nodes).toEqual(['https://n1.example', 'https://n2.example']);
    expect(client.cluster).toBe('eu-dedicated-7');
    expect(client.deployment).toBe('d-1');
    // The manifest is enough: no directory is asked, the fallback is not used.
    expect(asked).toEqual([`${origin}/.well-known/calimero.json`]);
  });

  it("takes the deployment from a deployment host's manifest, and leaves the cluster unknown on an older node", async () => {
    const origin = 'https://dlabel.apps.relay.staging.cloud.calimero.network';
    const { fetch } = fake({
      [`${origin}/.well-known/calimero.json`]: { package: 'p', deployment: 'd-9', versions: ['1.0.0'], api: ['https://n1.example'] },
    });
    const client = await CalimeroClient.connectHere({ policy, origin, href: `${origin}/1.0.0/`, fetch });
    expect(client.deployment).toBe('d-9');
    expect(client.cluster).toBeUndefined();
  });

  it("falls back to the cluster by name, through its environment's directory, on a page no cluster serves", async () => {
    const origin = 'https://my-app-git-main.vercel.app';
    const { fetch, asked } = fake({
      // Vercel answers a single-page app's index for every path.
      [`${origin}/.well-known/calimero.json`]: '<!doctype html><div id="root"></div>',
      [`${STAGING_DIRECTORY}/staging-1`]: listing('staging-1', ['https://s1.example']),
    });
    const client = await CalimeroClient.connectHere({
      policy,
      origin,
      href: `${origin}/`,
      fetch,
      fallback: { cluster: 'staging-1', environment: 'staging' },
    });
    expect(client.nodes).toEqual(['https://s1.example']);
    expect(client.cluster).toBe('staging-1');
    expect(client.deployment).toBeNull();
    expect(asked).toEqual([`${origin}/.well-known/calimero.json`, `${STAGING_DIRECTORY}/staging-1`]);
  });

  it('falls back to fixed nodes outside a browser, without asking any origin', async () => {
    const { fetch, asked } = fake({});
    const client = await CalimeroClient.connectHere({ policy, fetch, fallback: { baseUrl: 'http://127.0.0.1:2501', nodes: ['http://127.0.0.1:2502'] } });
    expect(client.nodes).toEqual(['http://127.0.0.1:2501', 'http://127.0.0.1:2502']);
    expect(client.cluster).toBeUndefined();
    expect(asked).toEqual([]);
  });

  it('never falls back on a Calimero Cloud app host whose manifest cannot be read', async () => {
    const origin = 'https://abc.apps.calimero.network';
    const { fetch } = fake({ [`${origin}/.well-known/calimero.json`]: 503 });
    await expect(
      CalimeroClient.connectHere({ policy, origin, fetch, fallback: { cluster: 'eu-shared-1' } }),
    ).rejects.toThrow(/Calimero Cloud app host/);
  });

  it('refuses a manifest with no nodes to talk to', async () => {
    const origin = 'https://abc.apps.calimero.network';
    const { fetch } = fake({ [`${origin}/.well-known/calimero.json`]: { package: 'p', versions: [], api: [] } });
    await expect(CalimeroClient.connectHere({ policy, origin, fetch })).rejects.toThrow(/served no usable/);
  });

  it('asks for a fallback when not on a page a cluster serves', async () => {
    const { fetch } = fake({});
    await expect(CalimeroClient.connectHere({ policy, origin: 'http://localhost:5173', fetch })).rejects.toThrow(/fallback/);
  });
});

describe('CalimeroClient.connect', () => {
  it("asks the production directory by default and the staging one for environment 'staging'", async () => {
    const { fetch, asked } = fake({
      [`${DEFAULT_DIRECTORY}/eu-shared-1`]: listing('eu-shared-1', ['https://p1.example']),
      [`${STAGING_DIRECTORY}/staging-1`]: listing('staging-1', ['https://s1.example']),
    });
    const prod = await CalimeroClient.connect({ cluster: 'eu-shared-1', policy, fetch });
    const staging = await CalimeroClient.connect({ cluster: 'staging-1', environment: 'staging', policy, fetch });
    expect(prod.nodes).toEqual(['https://p1.example']);
    expect(prod.cluster).toBe('eu-shared-1');
    expect(staging.nodes).toEqual(['https://s1.example']);
    expect(asked).toEqual([`${DEFAULT_DIRECTORY}/eu-shared-1`, `${STAGING_DIRECTORY}/staging-1`]);
  });
});

describe("CalimeroClient.connect's default directory follows the page's Calimero Cloud", () => {
  afterEach(() => vi.unstubAllGlobals());

  const routes = {
    [`${DEFAULT_DIRECTORY}/c1`]: listing('c1', ['https://p1.example']),
    [`${STAGING_DIRECTORY}/c1`]: listing('c1', ['https://s1.example']),
    ['https://dir.example/c1']: listing('c1', ['https://x1.example']),
  };

  it("a page on staging's apps domain asks staging's directory", async () => {
    vi.stubGlobal('location', { hostname: 'dlabel.apps.relay.staging.cloud.calimero.network' });
    const { fetch, asked } = fake(routes);
    const client = await CalimeroClient.connect({ cluster: 'c1', policy, fetch });
    expect(asked).toEqual([`${STAGING_DIRECTORY}/c1`]);
    expect(client.nodes).toEqual(['https://s1.example']);
  });

  it("production pages, every other host and Node (no location) ask production's", async () => {
    expect(globalThis.location).toBeUndefined();
    const hosts = [
      undefined,
      'abc.apps.calimero.network',
      'localhost',
      'evil-apps.relay.staging.cloud.calimero.network.attacker.com',
      'xapps.relay.staging.cloud.calimero.network',
    ];
    for (const hostname of hosts) {
      if (hostname) vi.stubGlobal('location', { hostname });
      const { fetch, asked } = fake(routes);
      await CalimeroClient.connect({ cluster: 'c1', policy, fetch });
      expect(asked, hostname).toEqual([`${DEFAULT_DIRECTORY}/c1`]);
    }
  });

  it('an explicit directory or environment wins over the page', async () => {
    vi.stubGlobal('location', { hostname: 'dlabel.apps.relay.staging.cloud.calimero.network' });
    const { fetch, asked } = fake(routes);
    await CalimeroClient.connect({ cluster: 'c1', directory: 'https://dir.example', policy, fetch });
    await CalimeroClient.connect({ cluster: 'c1', environment: 'production', policy, fetch });
    expect(asked).toEqual(['https://dir.example/c1', `${DEFAULT_DIRECTORY}/c1`]);
  });
});
