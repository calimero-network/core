import { describe, expect, it } from 'vitest';

import { browserSessionHolder, generateHolderKeyPair, holderFromKeyPair, holderMessage } from './holder.js';
import { fromB64url } from './passkey.js';

describe('session holder', () => {
  it('signs the bytes core verifies: the domain, the token hash and the time, each zero-terminated', async () => {
    const message = new TextDecoder().decode(await holderMessage('rt_token', 1700000000));
    // b64url(SHA-256("rt_token")), as core's `proof::refresh_holder_message` names the token.
    const hash = 'aAWBVfU5NO7kZuxMRjnJyWZf9oZu9RF25AkhoU0wLO0';
    expect(message).toBe(`calimero.refresh-holder.v1\0${hash}\x001700000000\0`);
  });

  it('keeps its private key unexportable and gives its public key as an uncompressed point', async () => {
    const pair = await generateHolderKeyPair();
    expect(pair.privateKey.extractable).toBe(false);
    await expect(crypto.subtle.exportKey('pkcs8', pair.privateKey)).rejects.toThrow();
    const raw = fromB64url(await holderFromKeyPair(pair).publicKey());
    expect(raw.length).toBe(65);
    expect(raw[0]).toBe(4);
  });

  it('proves a token with a signature its public key verifies, at the current time', async () => {
    const pair = await generateHolderKeyPair();
    const holder = holderFromKeyPair(pair);
    const before = Math.floor(Date.now() / 1000);
    const proof = await holder.prove('rt_token');
    expect(proof.at).toBeGreaterThanOrEqual(before);
    const signature = fromB64url(proof.signature);
    expect(signature.length).toBe(64);
    const verify = (token: string) =>
      holderMessage(token, proof.at).then((m) =>
        crypto.subtle.verify({ name: 'ECDSA', hash: 'SHA-256' }, pair.publicKey, signature, m),
      );
    expect(await verify('rt_token')).toBe(true);
    expect(await verify('rt_other')).toBe(false);
  });

  it('falls back to a key held in memory where there is no IndexedDB', async () => {
    const holder = browserSessionHolder();
    expect(holder).toBeDefined();
    const key = await holder!.publicKey();
    expect(fromB64url(key).length).toBe(65);
    expect(await holder!.publicKey()).toBe(key);
  });
});
