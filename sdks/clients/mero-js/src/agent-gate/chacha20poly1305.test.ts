import { createCipheriv, randomBytes } from 'node:crypto';

import { describe, expect, it } from 'vitest';

import { chacha20Block, chacha20Poly1305Open, chacha20Poly1305Seal, poly1305 } from './chacha20poly1305.js';

const hex = (bytes: Uint8Array) => Buffer.from(bytes).toString('hex');
const fromHex = (value: string) => new Uint8Array(Buffer.from(value.replace(/\s+/g, ''), 'hex'));

describe('ChaCha20-Poly1305 (RFC 8439)', () => {
  it('matches the block function vector (§2.3.2)', () => {
    const key = fromHex('000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f');
    const nonce = fromHex('000000090000004a00000000');
    expect(hex(chacha20Block(key, 1, nonce))).toBe(
      '10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4e' +
        'd2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e',
    );
  });

  it('matches the Poly1305 vector (§2.5.2)', () => {
    const key = fromHex('85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b');
    const message = new TextEncoder().encode('Cryptographic Forum Research Group');
    expect(hex(poly1305(key, message))).toBe('a8061dc1305136c6c22b8baf0c0127a9');
  });

  it('matches the AEAD vector (§2.8.2)', () => {
    const key = fromHex('808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f');
    const nonce = fromHex('070000004041424344454647');
    const aad = fromHex('50515253c0c1c2c3c4c5c6c7');
    const plaintext = new TextEncoder().encode(
      "Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.",
    );
    const sealed = chacha20Poly1305Seal(key, nonce, plaintext, aad);
    expect(hex(sealed.subarray(sealed.length - 16))).toBe('1ae10b594f09e26a7e902ecbd0600691');
    expect(new TextDecoder().decode(chacha20Poly1305Open(key, nonce, sealed, aad))).toBe(new TextDecoder().decode(plaintext));
  });

  it("agrees with Node's implementation on random inputs of every length around a block", () => {
    for (const length of [0, 1, 15, 16, 17, 63, 64, 65, 200, 4096]) {
      const key = new Uint8Array(randomBytes(32));
      const nonce = new Uint8Array(randomBytes(12));
      const aad = new Uint8Array(randomBytes(length % 37));
      const plaintext = new Uint8Array(randomBytes(length));
      const cipher = createCipheriv('chacha20-poly1305', key, nonce, { authTagLength: 16 });
      cipher.setAAD(aad, { plaintextLength: length });
      const theirs = Buffer.concat([cipher.update(plaintext), cipher.final(), cipher.getAuthTag()]);
      expect(hex(chacha20Poly1305Seal(key, nonce, plaintext, aad))).toBe(theirs.toString('hex'));
    }
  });

  it('refuses a message altered by one bit', () => {
    const key = new Uint8Array(32).fill(7);
    const nonce = new Uint8Array(12).fill(1);
    const sealed = chacha20Poly1305Seal(key, nonce, new TextEncoder().encode('secret'));
    sealed[0] ^= 1;
    expect(() => chacha20Poly1305Open(key, nonce, sealed)).toThrow('did not authenticate');
  });
});
