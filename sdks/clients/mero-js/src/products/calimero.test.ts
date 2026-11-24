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
    ['a profile without a release floor', { CALIMERO_NODE_URL: 'http://127.0.0.1:1', CALIMERO_API_KEY: 'cal_test', CALIMERO_ALLOW_UNATTESTED: '1', CALIMERO_TEE_PROFILE: 'debug' }, /CALIMERO_TEE_PROFILE needs CALIMERO_MIN_RELEASE/],
  ])('%s', async (_, env, message) => {
    const connect = vi.spyOn(CalimeroClient, 'connect').mockRejectedValue(new Error('connected'));
    expect(await failure(Calimero.fromEnv(env))).toMatchObject({ message: expect.stringMatching(message) });
    expect(connect).not.toHaveBeenCalled();
  });
});
