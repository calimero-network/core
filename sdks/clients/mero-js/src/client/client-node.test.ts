/**
 * CalimeroClient against a stand-in cloud node on a real local HTTP server,
 * through the real global `fetch`: mock attestation, the sealed handshake and
 * sealed, streamed answers, as core's cloud node speaks them. Synthetic data only.
 *
 * Covers the mapping of every refusal the node sends to the client's typed
 * errors (D64, D70). The abort test is in `events-abort.test.ts`.
 */

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

import { StandInNode, type Reply } from '../../tests/support/stand-in-node.js';
import {
  AppError,
  ArgsTooLarge,
  CalimeroClient,
  CalimeroError,
  ConfirmCancelled,
  ConfirmPromptBlocked,
  ConfirmRequired,
  IdempotencyMismatch,
  LoginRequired,
  NotFound,
  TrapError,
  UnavailableError,
  type Committed,
} from './index.js';
import { DEFAULT_RETRY, retryDelayMs } from './calimero-client.js';
import type { PasskeyProvider } from '../runtime/passkey.js';
import { passkeyConfirm } from '../runtime/passkey.js';

const node = new StandInNode();
beforeAll(() => node.start());
afterAll(() => node.stop());
beforeEach(() => {
  node.requests.length = 0;
  node.answer = () => ({ status: 200, json: {} });
});

const client = () =>
  new CalimeroClient({ baseUrl: node.url, policy: { allowMock: true }, retry: { baseDelayMs: 1 } });
const refused = (status: number, error: string, details?: Record<string, unknown>): Reply => ({
  status,
  json: { error, message: `${error.toLowerCase()} (stand-in)`, ...(details ? { details } : {}) },
});
const failure = (p: Promise<unknown>) => p.then(() => undefined, (e: unknown) => e);
const intents = () => node.requests.filter((r) => r.head.path === '/apps/app-1/intents');
const challenge = { challengeId: 'c-1', challenge: 'AAEC', rpId: 'localhost' };
const signingPasskey: PasskeyProvider = {
  create: async () => ({ attestationObject: '', clientDataJson: '' }),
  get: async () => ({ credentialId: 'cred', authenticatorData: 'ad', clientDataJson: 'cd', signature: 'sig' }),
};


describe('refusals map to typed errors in one place (D64)', () => {
  it('401 UNAUTHENTICATED is LoginRequired', async () => {
    node.answer = () => refused(401, 'UNAUTHENTICATED');
    const error = await failure(client().apps.view({ app: 'app-1', method: 'availability', args: {} }));
    expect(error).toBeInstanceOf(LoginRequired);
    expect(error).toBeInstanceOf(CalimeroError);
    expect(error).toMatchObject({ status: 401, code: 'UNAUTHENTICATED' });
  });

  it('404 NOT_FOUND is NotFound', async () => {
    node.answer = () => refused(404, 'NOT_FOUND');
    const error = await failure(client().apps.call({ app: 'app-1', method: 'book', args: {} }));
    expect(error).toBeInstanceOf(NotFound);
    expect(error).toMatchObject({ status: 404, code: 'NOT_FOUND' });
  });

  it('412 CONFIRM_REQUIRED is answered with the passkey and the call retried', async () => {
    const challenge = { challengeId: 'c-1', challenge: 'AAEC', rpId: 'localhost' };
    node.answer = (_, body) =>
      body.confirm ? { status: 200, json: { index: 7, result: 'ok', events: [] } } : refused(412, 'CONFIRM_REQUIRED', challenge);
    const passkey: PasskeyProvider = {
      create: async () => ({ attestationObject: '', clientDataJson: '' }),
      get: async () => ({ credentialId: 'cred', authenticatorData: 'ad', clientDataJson: 'cd', signature: 'sig' }),
    };
    const answer = await client().apps.call({ app: 'app-1', method: 'adminRelease', args: { id: 'b1' } }, { confirm: passkeyConfirm(passkey) });
    expect(answer.result).toBe('ok');
    const [first, second] = intents();
    expect(second.body.confirm).toEqual({ challengeId: 'c-1', assertion: expect.objectContaining({ signature: 'sig' }) });
    expect(second.body.idempotencyKey).toBe(first.body.idempotencyKey);
  });

  it('412 CONFIRM_REQUIRED without a passkey is ConfirmRequired, carrying the challenge', async () => {
    node.answer = () => refused(412, 'CONFIRM_REQUIRED', { challengeId: 'c-1', challenge: 'AAEC', rpId: 'localhost' });
    const error = await failure(client().apps.call({ app: 'app-1', method: 'adminRelease', args: {} }));
    expect(error).toBeInstanceOf(ConfirmRequired);
    expect(error).toMatchObject({ code: 'CONFIRM_REQUIRED', details: { challengeId: 'c-1' } });
  });

  it('412 CONFIRM_REQUIRED the user declines is ConfirmCancelled, and nothing more is sent', async () => {
    node.answer = () => refused(412, 'CONFIRM_REQUIRED', { challengeId: 'c-1', challenge: 'AAEC', rpId: 'localhost' });
    const declined = Object.assign(new Error('The operation either timed out or was not allowed.'), { name: 'NotAllowedError' });
    const passkey: PasskeyProvider = {
      create: async () => ({ attestationObject: '', clientDataJson: '' }),
      get: async () => {
        throw declined;
      },
    };
    const error = await failure(client().apps.call({ app: 'app-1', method: 'adminRelease', args: {} }, { confirm: passkeyConfirm(passkey) }));
    expect(error).toBeInstanceOf(ConfirmCancelled);
    expect((error as ConfirmCancelled).cause).toBe(declined);
    expect(intents()).toHaveLength(1);
  });

  it('a confirmation asks the passkey to verify the user (required), unless the node names otherwise', async () => {
    const asked: unknown[] = [];
    const passkey: PasskeyProvider = {
      create: async () => ({ attestationObject: '', clientDataJson: '' }),
      get: async (_c, _r, _a, options) => {
        asked.push(options?.userVerification);
        return { credentialId: 'cred', authenticatorData: 'ad', clientDataJson: 'cd', signature: 'sig' };
      },
    };
    node.answer = (_, body) =>
      body.confirm ? { status: 200, json: { index: 7, result: 'ok', events: [] } } : refused(412, 'CONFIRM_REQUIRED', challenge);
    await client().apps.call({ app: 'app-1', method: 'adminRelease', args: {} }, { confirm: passkeyConfirm(passkey) });
    node.answer = (_, body) =>
      body.confirm
        ? { status: 200, json: { index: 8, result: 'ok', events: [] } }
        : refused(412, 'CONFIRM_REQUIRED', { ...challenge, userVerification: 'preferred' });
    await client().apps.call({ app: 'app-1', method: 'adminRelease', args: {} }, { confirm: passkeyConfirm(passkey) });
    node.answer = (_, body) =>
      body.confirm
        ? { status: 200, json: { index: 9, result: 'ok', events: [] } }
        : refused(412, 'CONFIRM_REQUIRED', { ...challenge, userVerification: 'bogus' });
    await client().apps.call({ app: 'app-1', method: 'adminRelease', args: {} }, { confirm: passkeyConfirm(passkey) });
    expect(asked).toEqual(['required', 'preferred', 'required']);
  });

  it('a passkey written for the three-argument get still answers a confirmation', async () => {
    node.answer = (_, body) =>
      body.confirm ? { status: 200, json: { index: 7, result: 'ok', events: [] } } : refused(412, 'CONFIRM_REQUIRED', challenge);
    const legacy = {
      create: async () => ({ attestationObject: '', clientDataJson: '' }),
      get: async (_c: Uint8Array, _r: string, _a?: string[]) => ({
        credentialId: 'cred',
        authenticatorData: 'ad',
        clientDataJson: 'cd',
        signature: 'sig',
      }),
    };
    const answer = await client().apps.call({ app: 'app-1', method: 'adminRelease', args: {} }, { confirm: passkeyConfirm(legacy) });
    expect(answer.result).toBe('ok');
  });

  for (const [what, blocked] of [
    ['the page is not focused', Object.assign(new Error('The document is not focused.'), { name: 'NotAllowedError' })],
    ['another prompt is pending', Object.assign(new Error('A request is already pending.'), { name: 'NotAllowedError' })],
    ['the browser is in a bad state', Object.assign(new Error('x'), { name: 'InvalidStateError' })],
    ['an extension says a request is pending', Object.assign(new Error('A request is already pending.'), { name: 'OperationError' })],
  ] as const) {
    it(`a prompt refused because ${what} is ConfirmPromptBlocked, not a cancel, and nothing more is sent`, async () => {
      node.answer = () => refused(412, 'CONFIRM_REQUIRED', challenge);
      const passkey: PasskeyProvider = {
        create: async () => ({ attestationObject: '', clientDataJson: '' }),
        get: async () => {
          throw blocked;
        },
      };
      const error = await failure(client().apps.call({ app: 'app-1', method: 'adminRelease', args: {} }, { confirm: passkeyConfirm(passkey) }));
      expect(error).toBeInstanceOf(ConfirmPromptBlocked);
      expect(error).not.toBeInstanceOf(ConfirmCancelled);
      expect((error as ConfirmPromptBlocked).cause).toBe(blocked);
      expect(intents()).toHaveLength(1);
    });
  }

  it('an AbortError is still ConfirmCancelled; any other failure passes through', async () => {
    node.answer = () => refused(412, 'CONFIRM_REQUIRED', challenge);
    const aborted = Object.assign(new Error('aborted'), { name: 'AbortError' });
    const broken = new TypeError('boom');
    for (const [thrown, expected] of [
      [aborted, ConfirmCancelled],
      [broken, TypeError],
    ] as const) {
      const passkey: PasskeyProvider = {
        create: async () => ({ attestationObject: '', clientDataJson: '' }),
        get: async () => {
          throw thrown;
        },
      };
      const error = await failure(client().apps.call({ app: 'app-1', method: 'adminRelease', args: {} }, { confirm: passkeyConfirm(passkey) }));
      expect(error).toBeInstanceOf(expected);
    }
  });

  it('413 ARGS_TOO_LARGE is ArgsTooLarge', async () => {
    node.answer = () => refused(413, 'ARGS_TOO_LARGE');
    const error = await failure(client().apps.call({ app: 'app-1', method: 'book', args: {} }));
    expect(error).toBeInstanceOf(ArgsTooLarge);
    expect(error).toMatchObject({ status: 413, code: 'ARGS_TOO_LARGE' });
  });

  it('409 IDEMPOTENCY_MISMATCH is IdempotencyMismatch, and other 409s are not', async () => {
    node.answer = () => refused(409, 'IDEMPOTENCY_MISMATCH');
    expect(await failure(client().apps.call({ app: 'app-1', method: 'book', args: {}, idempotencyKey: 'k-1' }))).toBeInstanceOf(IdempotencyMismatch);
    for (const code of ['CONFLICT', 'NOT_INSTALLED']) {
      node.answer = () => refused(409, code);
      const error = await failure(client().apps.call({ app: 'app-1', method: 'book', args: {}, idempotencyKey: 'k-1' }));
      expect(error).not.toBeInstanceOf(IdempotencyMismatch);
      expect(error).toMatchObject({ name: 'CalimeroError', status: 409, code });
    }
  });

  it('422 APP_ERROR is AppError with the app code and its named fields', async () => {
    node.answer = () => refused(422, 'APP_ERROR', { index: 4, code: 'Taken', fields: { start: '1800000000000' } });
    const error = await failure(client().apps.call({ app: 'app-1', method: 'book', args: {} }));
    expect(error).toBeInstanceOf(AppError);
    expect(error).toMatchObject({ status: 422, code: 'APP_ERROR', appCode: 'Taken', fields: { start: '1800000000000' } });
  });

  it('422 TRAP is TrapError with its kind', async () => {
    node.answer = () => refused(422, 'TRAP', { index: 4, kind: 'OutOfFuel', detail: 'fuel' });
    const error = await failure(client().apps.call({ app: 'app-1', method: 'book', args: {} }));
    expect(error).toBeInstanceOf(TrapError);
    expect(error).toMatchObject({ status: 422, code: 'TRAP', kind: 'OutOfFuel' });
  });

  it('any other status and code is a plain CalimeroError carrying both', async () => {
    node.answer = () => refused(403, 'FORBIDDEN');
    const error = await failure(client().apps.call({ app: 'app-1', method: 'book', args: {} }));
    expect(error).toMatchObject({ name: 'CalimeroError', status: 403, code: 'FORBIDDEN' });
    expect(Object.getPrototypeOf(error)).toBe(CalimeroError.prototype);
  });

  it('an event stream refusal is mapped the same way', async () => {
    node.answer = () => refused(401, 'UNAUTHENTICATED');
    const error = await failure(client().events('app-1').next());
    expect(error).toBeInstanceOf(LoginRequired);
  });
});

describe('503s are retried with the same key, then UnavailableError (D70)', () => {
  const cases: Array<[string, Record<string, unknown> | undefined, string, Committed, number | undefined]> = [
    ['TIMEOUT', { retryAfter: 0.001 }, 'timeout', false, 0.001],
    ['MIGRATING', { retryAfter: 0.002 }, 'migrating', false, 0.002],
    ['UNAVAILABLE', undefined, 'unavailable', 'unknown', undefined],
  ];

  it.each(cases)('503 %s: 1 + 6 attempts with one key, then reason and committed', async (code, details, reason, committed, retryAfter) => {
    node.answer = () => refused(503, code, details);
    const error = await failure(client().apps.call({ app: 'app-1', method: 'book', args: { roomId: 'r1' }, idempotencyKey: 'caller-key' }));
    expect(error).toBeInstanceOf(UnavailableError);
    expect(error).toMatchObject({ status: 503, code, reason, committed, retryAfter });
    expect(intents().map((r) => r.body.idempotencyKey)).toEqual(Array(7).fill('caller-key'));
  });

  it('a call without a key gets one before the first attempt; a 503 then success sends it both times', async () => {
    let attempts = 0;
    node.answer = () =>
      ++attempts === 1 ? refused(503, 'UNAVAILABLE') : { status: 200, json: { index: 3, result: 'b1', events: [] } };
    const answer = await client().apps.call({ app: 'app-1', method: 'book', args: { roomId: 'r1' } });
    expect(answer.result).toBe('b1');
    const keys = intents().map((r) => r.body.idempotencyKey);
    expect(keys).toHaveLength(2);
    // `node:c-` and 16 random bytes in base64url without padding (22 characters), as the Rust client mints it.
    expect(keys[0]).toMatch(/^node:c-[A-Za-z0-9_-]{22}$/);
    expect(keys[1]).toBe(keys[0]);
  });

  it('a non-503 refusal is final: sent once', async () => {
    node.answer = () => refused(422, 'APP_ERROR', { code: 'Taken', fields: {} });
    await failure(client().apps.call({ app: 'app-1', method: 'book', args: {}, idempotencyKey: 'k-1' }));
    expect(intents()).toHaveLength(1);
  });

  it('a view retries a 503 too', async () => {
    let attempts = 0;
    node.answer = () => (++attempts < 3 ? refused(503, 'UNAVAILABLE') : { status: 200, json: { index: 1, result: [] } });
    expect((await client().apps.view({ app: 'app-1', method: 'availability', args: {} })).result).toEqual([]);
    expect(attempts).toBe(3);
  });
});

describe('the retry budget is the Rust client\'s (D70)', () => {
  afterEach(() => vi.restoreAllMocks());

  it('6 retries, from 200 ms doubling to at most 2 s, no jitter', () => {
    expect(DEFAULT_RETRY).toEqual({ retries: 6, baseDelayMs: 200, maxDelayMs: 2000 });
    const schedule = Array.from({ length: DEFAULT_RETRY.retries }, (_, retry) => retryDelayMs(retry, DEFAULT_RETRY));
    expect(schedule).toEqual([200, 400, 800, 1600, 2000, 2000]);
    // The same every time: no jitter.
    expect(Array.from({ length: 6 }, (_, retry) => retryDelayMs(retry, DEFAULT_RETRY))).toEqual(schedule);
  });

  it('a call without retry options sends 1 + 6 attempts, waiting the schedule between them', async () => {
    // The default delays, scaled down by a hundred so the test stays fast.
    const waits: number[] = [];
    const setTimeout = globalThis.setTimeout;
    vi.spyOn(globalThis, 'setTimeout').mockImplementation(((fn: () => void, ms?: number, ...rest: unknown[]) => {
      if (typeof ms === 'number' && ms >= 200 && ms <= 2000) {
        waits.push(ms);
        return setTimeout(fn, ms / 100, ...rest);
      }
      return setTimeout(fn, ms, ...rest);
    }) as typeof globalThis.setTimeout);
    node.answer = () => refused(503, 'UNAVAILABLE');
    const plain = new CalimeroClient({ baseUrl: node.url, policy: { allowMock: true } });
    expect(await failure(plain.apps.call({ app: 'app-1', method: 'book', args: {}, idempotencyKey: 'k-1' }))).toBeInstanceOf(UnavailableError);
    expect(intents()).toHaveLength(7);
    expect(waits).toEqual([200, 400, 800, 1600, 2000, 2000]);
  });
});

describe('a connection that fails is retried like a 503 (D70)', () => {
  it('a dropped connection is sent again with the same key, then succeeds', async () => {
    let attempts = 0;
    node.answer = () => (++attempts <= 2 ? { status: 0, drop: true } : { status: 200, json: { index: 5, result: 'b1', events: [] } });
    const answer = await client().apps.call({ app: 'app-1', method: 'book', args: { roomId: 'r1' } });
    expect(answer.result).toBe('b1');
    const keys = intents().map((r) => r.body.idempotencyKey);
    expect(keys).toHaveLength(3);
    expect(new Set(keys).size).toBe(1);
    expect(keys[0]).toMatch(/^node:c-/);
  });

  it('after the last attempt it is UnavailableError, reason network, committed unknown', async () => {
    node.answer = () => ({ status: 0, drop: true });
    const error = await failure(client().apps.call({ app: 'app-1', method: 'book', args: {}, idempotencyKey: 'caller-key' }));
    expect(error).toBeInstanceOf(UnavailableError);
    expect(error).toMatchObject({ code: 'NETWORK', reason: 'network', committed: 'unknown', retryAfter: undefined });
    // What `fetch` rejected with, as it reached the client through the sealed fetch.
    expect((error as UnavailableError).cause).toBeInstanceOf(TypeError);
    expect(intents().map((r) => r.body.idempotencyKey)).toEqual(Array(7).fill('caller-key'));
  });

  it('a view retries a dropped connection too', async () => {
    let attempts = 0;
    node.answer = () => (++attempts < 3 ? { status: 0, drop: true } : { status: 200, json: { index: 1, result: [] } });
    expect((await client().apps.view({ app: 'app-1', method: 'availability', args: {} })).result).toEqual([]);
    expect(attempts).toBe(3);
  });

  it('an admin route is not retried: a dropped connection is sent once and rejects as fetch did', async () => {
    node.answer = () => ({ status: 0, drop: true });
    const error = await failure(client().apps.create({ name: 'rooms', bundle: 'b' }));
    expect(error).toBeInstanceOf(TypeError);
    expect(node.requests.filter((r) => r.head.path === '/apps')).toHaveLength(1);
  });
});

describe('the confirmed attempt is sent exactly once (its challenge is single-use)', () => {
  it('a 503 on the confirmed attempt is not retried, and is UnavailableError', async () => {
    node.answer = (_, body) => (body.confirm ? refused(503, 'UNAVAILABLE') : refused(412, 'CONFIRM_REQUIRED', challenge));
    const error = await failure(client().apps.call({ app: 'app-1', method: 'adminRelease', args: { id: 'b1' } }, { confirm: passkeyConfirm(signingPasskey) }));
    expect(error).toBeInstanceOf(UnavailableError);
    expect(error).toMatchObject({ code: 'UNAVAILABLE', committed: 'unknown' });
    expect(intents().filter((r) => r.body.confirm)).toHaveLength(1);
    expect(intents()).toHaveLength(2);
  });

  it('a dropped connection on the confirmed attempt is not retried, and is UnavailableError', async () => {
    node.answer = (_, body) => (body.confirm ? { status: 0, drop: true } : refused(412, 'CONFIRM_REQUIRED', challenge));
    const error = await failure(client().apps.call({ app: 'app-1', method: 'adminRelease', args: { id: 'b1' } }, { confirm: passkeyConfirm(signingPasskey) }));
    expect(error).toMatchObject({ name: 'UnavailableError', reason: 'network', committed: 'unknown' });
    expect(intents().filter((r) => r.body.confirm)).toHaveLength(1);
  });

  it('the first, unconfirmed attempt still retries a 503 before the challenge', async () => {
    let attempts = 0;
    node.answer = (_, body) => {
      if (body.confirm) return { status: 200, json: { index: 9, result: 'ok', events: [] } };
      return ++attempts === 1 ? refused(503, 'UNAVAILABLE') : refused(412, 'CONFIRM_REQUIRED', challenge);
    };
    const answer = await client().apps.call({ app: 'app-1', method: 'adminRelease', args: { id: 'b1' } }, { confirm: passkeyConfirm(signingPasskey) });
    expect(answer.result).toBe('ok');
    const sent = intents();
    expect(sent.map((r) => r.body.confirm === undefined)).toEqual([true, true, false]);
    expect(new Set(sent.map((r) => r.body.idempotencyKey)).size).toBe(1);
  });
});

describe('what the client verified about its node', () => {
  it('is the record of the attestation check, and none before the node is used', async () => {
    const c = client();
    expect(c.verification()).toBeUndefined();
    expect(await c.verify()).toEqual({ status: 'verified', url: node.url, mock: true, checkedAt: expect.any(Number) });
    expect(c.verification()).toMatchObject({ status: 'verified', url: node.url });
  });

  it('is a refusal with its reason when the node fails the policy', async () => {
    const c = new CalimeroClient({ baseUrl: node.url, policy: {} });
    expect(await c.verify()).toMatchObject({ status: 'refused', url: node.url, reason: expect.stringMatching(/mock quote refused/) });
  });
});

describe('packageCapabilities', () => {
  const capabilities = {
    package: 'com.example.notes',
    version: '1.2.0',
    service: null,
    bundle: 'b1',
    mpk: 'm1',
    manifestHash: 'h1',
    signer: 's1',
    name: 'Notes',
    author: null,
    outbound: ['api.example.com'],
    abi: {
      abiVersion: '1',
      world: 'notes',
      state: 'State',
      methods: {
        add: { kind: 'call', params: [['text', 'string']], returns: null, confirm: false, public: true, allowSystem: false, doc: 'Add a note.' },
      },
      errors: [],
      events: [],
    },
  };

  it('reads the package version capabilities, encoding each path segment', async () => {
    node.answer = () => ({ status: 200, json: capabilities });
    const answer = await client().packageCapabilities('com.example.notes', '1.2.0');
    expect(answer).toEqual(capabilities);
    expect(node.requests[0].head.method).toBe('GET');
    expect(node.requests[0].head.path).toBe('/packages/com.example.notes/1.2.0/capabilities');
  });

  it('cannot reach another route through the package or version', async () => {
    node.answer = () => ({ status: 200, json: capabilities });
    await client().packageCapabilities('a/../b', '1.0.0?x');
    expect(node.requests[0].head.path).toBe('/packages/a%2F..%2Fb/1.0.0%3Fx/capabilities');
  });

  it('asks for one service of the release only when it names one', async () => {
    node.answer = () => ({ status: 200, json: capabilities });
    await client().packageCapabilities('p', '1.0.0', { service: 'web ui' });
    await client().packageCapabilities('p', '1.0.0');
    expect(node.requests.map((r) => r.head.path)).toEqual([
      '/packages/p/1.0.0/capabilities?service=web+ui',
      '/packages/p/1.0.0/capabilities',
    ]);
  });

  it('throws a refusal as its typed error', async () => {
    node.answer = () => refused(404, 'NOT_FOUND');
    expect(await failure(client().packageCapabilities('nope', '1.0.0'))).toBeInstanceOf(NotFound);
  });
});

describe('attestApp', () => {
  it('answers the record of the app the node bound, through the client', async () => {
    const record = await client().attestApp('app-1');
    expect(record).toMatchObject({ url: node.url, mock: true, appHash: expect.stringMatching(/^[0-9a-f]{64}$/) });
  });
});
