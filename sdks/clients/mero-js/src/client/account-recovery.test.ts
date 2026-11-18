/**
 * Account recovery (core #4844, `docs/specs/account-recovery.md`): each method
 * sends the route and body core's `crates/node/src/api.rs` takes, through the
 * real sealed channel to a stand-in node. Synthetic data only.
 */

import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { StandInNode } from '../../tests/support/stand-in-node.js';
import { CalimeroClient, NotFound, RecoveryNotReady, WrongRecoveryCode } from './index.js';
import type { PasskeyProvider } from '../runtime/passkey.js';
import { passkeyConfirm } from '../runtime/passkey.js';

const node = new StandInNode();
beforeAll(() => node.start());
afterAll(() => node.stop());
beforeEach(() => {
  node.requests.length = 0;
  node.answer = () => ({ status: 200, json: {} });
});

const client = () => new CalimeroClient({ baseUrl: node.url, policy: { allowMock: true } });
const sent = () => node.requests.map((r) => [r.head.method, r.head.path]);
const last = () => node.requests[node.requests.length - 1];
const asked: string[] = [];
const passkey: PasskeyProvider = {
  create: async () => (asked.push('create'), { attestationObject: 'ao', clientDataJson: 'cd' }),
  get: async (_c, rpId, allow) => (asked.push(`get ${rpId} ${(allow || []).join(',')}`), { credentialId: 'new', authenticatorData: 'ad', clientDataJson: 'cd', signature: 'sig' }),
};
const CHALLENGE = { challengeId: 'c-1', challenge: 'AAEC', rpId: 'localhost' };
const confirmed = { challengeId: 'c-1', assertion: expect.objectContaining({ signature: 'sig' }) };
const PENDING = { accountId: 'acc-1', credentialId: 'new', startedAt: 100, readyAt: 100 + 72 * 3600 };
const SESSION = { accountId: 'acc-1', sessionToken: 'tok', expiresAt: 9e9, refreshToken: 'r' };
const confirming = (json: unknown, details: Record<string, unknown> = CHALLENGE) => {
  node.answer = (_, body) =>
    body.confirm ? { status: 200, json } : { status: 412, json: { error: 'CONFIRM_REQUIRED', message: 'approve', details } };
};
beforeEach(() => {
  asked.length = 0;
});

describe('the recovery code, signed in', () => {
  it('is made with a passkey and answered once, and removed with one', async () => {
    const c = client();
    confirming({ code: 'ABCD-EFGH-JKMN-PQRS-TVWX-YZ01-2345-6789' });
    expect(await (await c.auth.recoveryCreate({}, { confirm: passkeyConfirm(passkey) })).code).toBe('ABCD-EFGH-JKMN-PQRS-TVWX-YZ01-2345-6789');
    expect(last().body).toEqual({ confirm: confirmed });
    confirming({});
    await c.auth.recoveryRemove({}, { confirm: passkeyConfirm(passkey) });
    expect(sent()).toEqual([
      ['POST', '/auth/recovery'],
      ['POST', '/auth/recovery'],
      ['DELETE', '/auth/recovery'],
      ['DELETE', '/auth/recovery'],
    ]);
  });

  it('a recovery under way shows on me() and a passkey the account has cancels it', async () => {
    const c = client();
    node.answer = () => ({ status: 200, json: { accountId: 'acc-1', recoveryCode: false, recoveryPending: PENDING } });
    expect((await c.auth.me()).recoveryPending).toEqual(PENDING);
    confirming({});
    await c.auth.accountRecoveryCancel({}, { confirm: passkeyConfirm(passkey) });
    expect(last().body).toEqual({ confirm: confirmed });
    expect(sent().slice(1)).toEqual([
      ['POST', '/auth/recovery/cancel'],
      ['POST', '/auth/recovery/cancel'],
    ]);
  });
});

describe('recovering, signed out', () => {
  it('starts with the code and a new passkey: begin, create, start; no session', async () => {
    const c = client();
    node.answer = (head) =>
      head.path === '/auth/recovery/begin'
        ? { status: 200, json: { challengeId: 'r-1', options: { challenge: 'AAEC' } } }
        : { status: 200, json: PENDING };
    expect(await c.startAccountRecovery('abcd efgh', passkey)).toEqual(PENDING);
    expect(sent()).toEqual([
      ['POST', '/auth/recovery/begin'],
      ['POST', '/auth/recovery/start'],
    ]);
    expect(node.requests[0].body).toEqual({ code: 'abcd efgh' });
    expect(last().body).toEqual({ challengeId: 'r-1', code: 'abcd efgh', registration: { attestationObject: 'ao', clientDataJson: 'cd' } });
    expect(asked).toEqual(['create']);
    expect(c.session).toBeUndefined();
  });

  it('a wrong or spent code is WrongRecoveryCode, before any passkey prompt, even signed in', async () => {
    const c = client();
    c.useSession(SESSION);
    node.answer = () => ({ status: 401, json: { error: 'UNAUTHENTICATED', message: 'unknown or used recovery code' } });
    const error = await c.startAccountRecovery('nope', passkey).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(WrongRecoveryCode);
    expect((error as Error).message).toContain('unknown or used recovery code');
    expect(asked).toEqual([]);
    // No refresh was tried: the 401 is the code's.
    expect(sent()).toEqual([['POST', '/auth/recovery/begin']]);
  });

  it('finishes with the new passkey once ready, and signs in', async () => {
    const c = client();
    confirming(SESSION, { ...CHALLENGE, allowCredentials: ['new'] });
    expect(await c.finishAccountRecovery(passkey, 'acc-1')).toEqual(SESSION);
    expect(c.session).toEqual(SESSION);
    expect(asked).toEqual(['get localhost new']);
    expect(node.requests.map((r) => r.body)).toEqual([{ accountId: 'acc-1' }, { accountId: 'acc-1', confirm: confirmed }]);
  });

  it('too early is RecoveryNotReady with readyAt; none under way is NotFound', async () => {
    const c = client();
    node.answer = () => ({
      status: 409,
      json: { error: 'RECOVERY_NOT_READY', message: 'the recovery may finish only once its delay has passed', details: { readyAt: PENDING.readyAt } },
    });
    const early = await c.finishAccountRecovery(passkey, 'acc-1').catch((e: unknown) => e);
    expect(early).toBeInstanceOf(RecoveryNotReady);
    expect((early as RecoveryNotReady).readyAt).toBe(PENDING.readyAt);
    node.answer = () => ({ status: 404, json: { error: 'NOT_FOUND', message: 'no recovery of this account is under way' } });
    expect(await c.finishAccountRecovery(passkey).catch((e: unknown) => e)).toBeInstanceOf(NotFound);
    expect(last().body).toEqual({});
    expect(asked).toEqual([]);
  });
});
