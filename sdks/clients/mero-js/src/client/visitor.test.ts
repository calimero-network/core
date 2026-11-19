import { describe, expect, it } from 'vitest';

import { publicLinkKey, Visitor } from './visitor.js';

// The vector core checks too (`calimero_client::visitor`, `system::tests`): Ed25519 signs
// deterministically, so the credential is the same byte for byte.
const LINK = 'd4205f1ac7bb3fe43278131b6e621ebf952f0a5f51a38505327572d863c46c42';
const KEY = 'fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618';
const BEARER =
  'vis1.pl_token.fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618.1000.9b5205e4deb4b534a612c3b03be773b60aea16192f2fb6d99496d194f2dd259173173731e56a46ec7b37cc3d2d2482fe7be5d70c0fadda54c22f5ea2b8fa4e09';

describe('visitor', () => {
  it('names the link and the visitor and signs the credential core expects', async () => {
    expect(await publicLinkKey('pl_token')).toBe(LINK);
    const v = await Visitor.create('pl_token', new Uint8Array(32).fill(9));
    expect(v.link).toBe(LINK);
    expect(v.publicKey).toBe(KEY);
    expect(await v.bearer(1000)).toBe(BEARER);
  });

  it('gives two seeds two keys', async () => {
    const one = await Visitor.create('pl_token', new Uint8Array(32).fill(1));
    const two = await Visitor.create('pl_token', new Uint8Array(32).fill(2));
    expect(one.publicKey).not.toBe(two.publicKey);
    expect(one.link).toBe(two.link);
  });
});
