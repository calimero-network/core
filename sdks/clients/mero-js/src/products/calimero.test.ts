/**
 * `Calimero.fromEnv()` and its `kv()` against a stand-in node on a real local
 * HTTP server: which environment connects how, and what each mistake says.
 * Synthetic data only.
 */

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

import { StandInNode } from '../../tests/support/stand-in-node.js';
import { CalimeroClient } from '../client/index.js';
import { Calimero } from './calimero.js';
import { KvClient } from './kv/client.js';

const node = new StandInNode();
beforeAll(() => node.start());
afterAll(() => node.stop());
beforeEach(() => {
  node.requests.length = 0;
  node.answer = () => ({ status: 200, json: { index: 3, result: '7', events: [] } });
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllEnvs();
});

const devEnv = () => ({
  CALIMERO_NODE_URL: node.url,
  CALIMERO_ALLOW_UNATTESTED: '1',
  CALIMERO_API_KEY: 'cal_test',
  CALIMERO_KV_STORE: 'kv-1',
});
const failure = (p: Promise<unknown>) => p.then(() => undefined, (e: unknown) => e);

describe('Calimero.fromEnv: a dev node', () => {
  it('kv() sends the generated call to the store, with the API key', async () => {
    const kv = (await Calimero.fromEnv(devEnv())).kv();
    expect(kv).toBeInstanceOf(KvClient);
    expect(await kv.set({ bucket: 'default', key: 'flags/beta', value: '{"enabled":true}' })).toBe('7');
    const sent = node.requests.find((r) => r.head.path === '/apps/kv-1/intents');
    expect(sent?.body).toMatchObject({ method: 'set', args: { bucket: 'default', key: 'flags/beta', value: '{"enabled":true}' } });
    expect(sent?.head.headers).toContainEqual(['authorization', 'Bearer cal_test']);
  });

  it('kv(store) binds another store', async () => {
    node.answer = () => ({ status: 200, json: { index: 3, result: null } });
    expect(await (await Calimero.fromEnv(devEnv())).kv('kv-2').get({ bucket: 'b', key: 'k' })).toBeNull();
    expect(node.requests.map((r) => r.head.path)).toContain('/apps/kv-2/views');
  });

  it('kv() without CALIMERO_KV_STORE or a store names the variable', async () => {
    const calimero = await Calimero.fromEnv({ ...devEnv(), CALIMERO_KV_STORE: '' });
    expect(() => calimero.kv()).toThrow(/CALIMERO_KV_STORE/);
  });

  it('refuses the stand-in mock quote without CALIMERO_ALLOW_UNATTESTED=1', async () => {
    const env = { ...devEnv(), CALIMERO_ALLOW_UNATTESTED: undefined, CALIMERO_MIN_RELEASE: '2.3.109' };
    const error = await failure((await Calimero.fromEnv(env)).kv().whoami());
    expect(String(error)).toMatch(/mock quote refused/);
  });

  it('reads process.env by default', async () => {
    for (const [name, value] of Object.entries(devEnv())) vi.stubEnv(name, value);
    await (await Calimero.fromEnv()).kv().whoami();
    expect(node.requests.at(-1)?.head.path).toBe('/apps/kv-1/views');
  });
});

describe('Calimero.fromEnv: a cluster', () => {
  it('connects by name under the release floor, with the API key', async () => {
    const stand = new CalimeroClient({ baseUrl: node.url, policy: { allowMock: true } });
    const connect = vi.spyOn(CalimeroClient, 'connect').mockResolvedValue(stand);
    const calimero = await Calimero.fromEnv({ CALIMERO_CLUSTER: 'eu-shared-1', CALIMERO_MIN_RELEASE: '2.3.109', CALIMERO_API_KEY: 'cal_test' });
    expect(connect).toHaveBeenCalledWith({ cluster: 'eu-shared-1', policy: { release: { minReleaseVersion: '2.3.109' } } });
    expect(calimero.client).toBe(stand);
    await calimero.kv('kv-1').whoami();
    expect(node.requests.at(-1)?.head.headers).toContainEqual(['authorization', 'Bearer cal_test']);
  });

  it.each([
    ['debug', { profile: 'debug', allowDebugProfile: true }],
    ['debug-read-only', { profile: 'debug-read-only', allowDebugProfile: true }],
    ['locked-read-only', { profile: 'locked-read-only', allowDebugProfile: false }],
    ['staging', { profile: 'staging', allowDebugProfile: false }],
  ])('CALIMERO_TEE_PROFILE=%s trusts that image profile', async (profile, release) => {
    const connect = vi.spyOn(CalimeroClient, 'connect').mockResolvedValue(new CalimeroClient({ baseUrl: node.url, policy: { allowMock: true } }));
    await Calimero.fromEnv({ CALIMERO_CLUSTER: 'eu-shared-1', CALIMERO_MIN_RELEASE: '2.3.109', CALIMERO_API_KEY: 'cal_test', CALIMERO_TEE_PROFILE: profile });
    expect(connect).toHaveBeenCalledWith({ cluster: 'eu-shared-1', policy: { release: { minReleaseVersion: '2.3.109', ...release } } });
  });
});

describe('Calimero.fromEnv: CALIMERO_ENVIRONMENT and CALIMERO_DIRECTORY', () => {
  const cluster = { CALIMERO_CLUSTER: 'staging-1', CALIMERO_MIN_RELEASE: '2.3.109', CALIMERO_API_KEY: 'cal_test' };
  const stub = () => vi.spyOn(CalimeroClient, 'connect').mockResolvedValue(new CalimeroClient({ baseUrl: node.url, policy: { allowMock: true } }));

  it("staging: staging's directory, mirrors and image profiles, as a staging page gets them", async () => {
    const connect = stub();
    await Calimero.fromEnv({ ...cluster, CALIMERO_ENVIRONMENT: 'staging' });
    expect(connect).toHaveBeenCalledWith({
      cluster: 'staging-1',
      environment: 'staging',
      policy: {
        release: {
          minReleaseVersion: '2.3.109',
          profiles: ['locked-read-only', 'debug', 'debug-read-only'],
          allowDebugProfile: true,
          releaseSource: 'https://staging.cloud.calimero.network',
          collateralSource: 'https://staging.cloud.calimero.network/api/tee/pcs',
        },
      },
    });
  });

  it("staging asks staging's directory and nothing of production's", async () => {
    const asked: string[] = [];
    vi.stubGlobal('fetch', async (input: string | URL | Request) => {
      asked.push(String(input));
      return new Response('not found', { status: 404 });
    });
    try {
      await failure(Calimero.fromEnv({ ...cluster, CALIMERO_ENVIRONMENT: 'staging' }));
    } finally {
      vi.unstubAllGlobals();
    }
    expect(asked).toEqual(['https://manager.staging.cloud.calimero.network/api/directory/staging-1']);
  });

  it('an explicit CALIMERO_TEE_PROFILE wins over the environment\'s profiles', async () => {
    const connect = stub();
    await Calimero.fromEnv({ ...cluster, CALIMERO_ENVIRONMENT: 'staging', CALIMERO_TEE_PROFILE: 'locked-read-only' });
    expect(connect.mock.calls[0][0].policy.release).toEqual({
      minReleaseVersion: '2.3.109',
      profile: 'locked-read-only',
      allowDebugProfile: false,
      releaseSource: 'https://staging.cloud.calimero.network',
      collateralSource: 'https://staging.cloud.calimero.network/api/tee/pcs',
    });
  });

  it("production names production's mirrors and adds no trust", async () => {
    const connect = stub();
    await Calimero.fromEnv({ ...cluster, CALIMERO_ENVIRONMENT: 'production' });
    expect(connect).toHaveBeenCalledWith({
      cluster: 'staging-1',
      environment: 'production',
      policy: {
        release: {
          minReleaseVersion: '2.3.109',
          releaseSource: 'https://cloud.calimero.network',
          collateralSource: 'https://cloud.calimero.network/api/tee/pcs',
        },
      },
    });
  });

  it('unset leaves the connection exactly as before', async () => {
    const connect = stub();
    await Calimero.fromEnv({ ...cluster, CALIMERO_ENVIRONMENT: '' });
    await Calimero.fromEnv(cluster);
    expect(connect).toHaveBeenCalledTimes(2);
    for (const [options] of connect.mock.calls) {
      expect(options).toEqual({ cluster: 'staging-1', policy: { release: { minReleaseVersion: '2.3.109' } } });
    }
  });

  it('CALIMERO_DIRECTORY names the directory, with or without an environment', async () => {
    const connect = stub();
    await Calimero.fromEnv({ ...cluster, CALIMERO_DIRECTORY: 'https://dir.example/api/directory' });
    await Calimero.fromEnv({ ...cluster, CALIMERO_ENVIRONMENT: 'staging', CALIMERO_DIRECTORY: 'https://dir.example/api/directory' });
    expect(connect.mock.calls.map(([o]) => [o.directory, o.environment])).toEqual([
      ['https://dir.example/api/directory', undefined],
      ['https://dir.example/api/directory', 'staging'],
    ]);
  });
});

describe('Calimero.fromEnv: what each mistake says', () => {
  const cluster = { CALIMERO_CLUSTER: 'eu-shared-1', CALIMERO_MIN_RELEASE: '2.3.109', CALIMERO_API_KEY: 'cal_test' };
  it.each([
    ['neither cluster nor node', { CALIMERO_API_KEY: 'cal_test' }, /CALIMERO_CLUSTER/],
    ['both cluster and node', { ...cluster, CALIMERO_NODE_URL: 'http://127.0.0.1:1' }, /CALIMERO_CLUSTER or CALIMERO_NODE_URL, not both/],
    ['unattested with a cluster', { ...cluster, CALIMERO_ALLOW_UNATTESTED: '1' }, /CALIMERO_ALLOW_UNATTESTED.*CALIMERO_NODE_URL/],
    ['unattested other than 1', { CALIMERO_NODE_URL: 'http://127.0.0.1:1', CALIMERO_API_KEY: 'cal_test', CALIMERO_ALLOW_UNATTESTED: 'true' }, /CALIMERO_ALLOW_UNATTESTED must be 1/],
    ['no API key', { ...cluster, CALIMERO_API_KEY: '' }, /CALIMERO_API_KEY/],
    ['no release floor for a cluster', { ...cluster, CALIMERO_MIN_RELEASE: undefined }, /CALIMERO_MIN_RELEASE/],
    ['no release floor for an attested node', { CALIMERO_NODE_URL: 'http://127.0.0.1:1', CALIMERO_API_KEY: 'cal_test' }, /CALIMERO_MIN_RELEASE/],
    ['an unknown environment', { ...cluster, CALIMERO_ENVIRONMENT: 'prod' }, /CALIMERO_ENVIRONMENT must be production or staging.*"prod"/],
    ['an environment in another case', { ...cluster, CALIMERO_ENVIRONMENT: 'Staging' }, /CALIMERO_ENVIRONMENT must be production or staging/],
    ['a directory for a dev node', { CALIMERO_NODE_URL: 'http://127.0.0.1:1', CALIMERO_API_KEY: 'cal_test', CALIMERO_ALLOW_UNATTESTED: '1', CALIMERO_DIRECTORY: 'https://dir.example' }, /CALIMERO_DIRECTORY is for a cluster/],
    ['a directory that is not a URL', { ...cluster, CALIMERO_DIRECTORY: 'manager.staging.cloud.calimero.network' }, /CALIMERO_DIRECTORY must be an http\(s\) URL/],
    ['a profile without a release floor', { CALIMERO_NODE_URL: 'http://127.0.0.1:1', CALIMERO_API_KEY: 'cal_test', CALIMERO_ALLOW_UNATTESTED: '1', CALIMERO_TEE_PROFILE: 'debug' }, /CALIMERO_TEE_PROFILE needs CALIMERO_MIN_RELEASE/],
  ])('%s', async (_, env, message) => {
    const connect = vi.spyOn(CalimeroClient, 'connect').mockRejectedValue(new Error('connected'));
    expect(await failure(Calimero.fromEnv(env))).toMatchObject({ message: expect.stringMatching(message) });
    expect(connect).not.toHaveBeenCalled();
  });
});
