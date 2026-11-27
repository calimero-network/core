/**
 * A client with a holder key ({@link SessionHolder}): a sign-in binds the
 * session to it and every refresh proves it, against a stand-in node through
 * the real sealed channel. Synthetic data only.
 */

import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { StandInNode, type Reply } from '../../tests/support/stand-in-node.js';
import { CalimeroClient, type CalimeroSession } from './index.js';
import { generateHolderKeyPair, holderFromKeyPair, holderMessage } from '../runtime/holder.js';
import { fromB64url, type PasskeyProvider } from '../runtime/passkey.js';

const node = new StandInNode();
beforeAll(() => node.start());
afterAll(() => node.stop());
beforeEach(() => {
  node.requests.length = 0;
  node.answer = () => ({ status: 200, json: {} });
});

const SESSION: CalimeroSession = { accountId: 'acc-1', sessionToken: 'v2.s.sig', expiresAt: 0, refreshToken: 'rt_held' };
const ok = (json: unknown): Reply => ({ status: 200, json });
const at = (path: string) => node.requests.filter((r) => r.head.path === path);
const passkey: PasskeyProvider = {
  create: async () => ({ attestationObject: '', clientDataJson: '' }),
  get: async () => ({ credentialId: 'cred', authenticatorData: 'ad', clientDataJson: 'cd', signature: 'sig' }),
};

const holding = async () => {
  const pair = await generateHolderKeyPair();
  const holder = holderFromKeyPair(pair);
  const client = new CalimeroClient({ baseUrl: node.url, policy: { allowMock: true }, retry: { baseDelayMs: 1 }, holder });
  return { pair, holder, client };
};

describe('a session bound to a holder key', () => {
  it('names the holder key when a sign-in begins', async () => {
    node.answer = (head) =>
      head.path === '/auth/passkey/login/begin'
        ? ok({ challengeId: 'c-1', options: { challenge: 'AAEC', rpId: 'localhost' } })
        : ok(SESSION);
    const { holder, client } = await holding();
    await client.signIn(passkey, { origin: null });
    expect(at('/auth/passkey/login/begin').map((r) => r.body)).toEqual([{ holderKey: await holder.publicKey() }]);
  });

  it('signs in unbound on a cluster that does not take a holder key yet', async () => {
    node.answer = (head, body) => {
      if (head.path === '/auth/passkey/login/begin') {
        return 'holderKey' in body
          ? { status: 409, json: { error: 'FEATURE_NOT_ACTIVE', message: 'not yet (stand-in)' } }
          : ok({ challengeId: 'c-1', options: { challenge: 'AAEC', rpId: 'localhost' } });
      }
      return ok(SESSION);
    };
    const { client } = await holding();
    expect(await client.signIn(passkey, { origin: null })).toEqual(SESSION);
    expect(at('/auth/passkey/login/begin').map((r) => 'holderKey' in r.body)).toEqual([true, false]);
  });

  it('proves the key on every refresh, over the token it presents', async () => {
    node.answer = () => ok({ ...SESSION, sessionToken: 'v2.next.sig' });
    const { pair, client } = await holding();
    client.useSession(SESSION);
    await client.refresh();
    const [sent] = at('/auth/refresh').map((r) => r.body as { refreshToken: string; holderProof: { at: number; signature: string } });
    expect(sent.refreshToken).toBe('rt_held');
    const message = await holderMessage('rt_held', sent.holderProof.at);
    const verified = await crypto.subtle.verify(
      { name: 'ECDSA', hash: 'SHA-256' },
      pair.publicKey,
      fromB64url(sent.holderProof.signature),
      message,
    );
    expect(verified).toBe(true);
  });

  it('sends no proof from a client without a holder', async () => {
    node.answer = () => ok(SESSION);
    const client = new CalimeroClient({ baseUrl: node.url, policy: { allowMock: true } });
    client.useSession(SESSION);
    await client.refresh();
    expect(at('/auth/refresh').map((r) => r.body)).toEqual([{ refreshToken: 'rt_held' }]);
  });
});
