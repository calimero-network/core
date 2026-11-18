/**
 * The Noise NK handshake (`Noise_NK_25519_AESGCM_SHA256`) that opens a sealed
 * session, written against WebCrypto from the Noise specification
 * (revision 34, http://www.noiseprotocol.org/noise.html). The node runs the
 * other side with `snow`; core's published vectors check the two against each
 * other.
 *
 * NK: the initiator knows the responder's static key in advance — here the
 * transport key the node's quote commits to — and sends nothing but an
 * ephemeral key before both ephemerals are mixed in, so everything after the
 * handshake has forward secrecy.
 */

import { concat } from '../crypto/internal.js';

const PROTOCOL_NAME = 'Noise_NK_25519_AESGCM_SHA256';
const DH_LEN = 32;
const TAG_LEN = 16;
/** PKCS#8 wrapping of a raw X25519 private key (RFC 8410). */
const PKCS8_X25519_PREFIX = new Uint8Array([
  0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x6e, 0x04,
  0x22, 0x04, 0x20,
]);

/** Noise's `SymmetricState`: the chaining key, the handshake hash and the cipher key. */
export class SymmetricState {
  private ck: Uint8Array;
  private h: Uint8Array;
  private k: CryptoKey | null = null;
  private n = 0;

  private constructor(h: Uint8Array) {
    this.ck = h;
    this.h = h;
  }

  static async initialize(): Promise<SymmetricState> {
    // A name of 32 bytes or fewer is used as the hash, zero-padded.
    const name = new TextEncoder().encode(PROTOCOL_NAME);
    const h = new Uint8Array(32);
    h.set(name);
    return new SymmetricState(h);
  }

  async mixHash(data: Uint8Array): Promise<void> {
    this.h = await sha256(concat(this.h, data));
  }

  async mixKey(inputKeyMaterial: Uint8Array): Promise<void> {
    const [ck, key] = await hkdf2(this.ck, inputKeyMaterial);
    this.ck = ck;
    this.k = await crypto.subtle.importKey('raw', key, 'AES-GCM', false, ['encrypt', 'decrypt']);
    this.n = 0;
  }

  async encryptAndHash(plaintext: Uint8Array): Promise<Uint8Array> {
    if (!this.k) throw new Error('Noise: no cipher key yet');
    const ciphertext = new Uint8Array(
      await crypto.subtle.encrypt(
        { name: 'AES-GCM', iv: noiseNonce(this.n), additionalData: this.h },
        this.k,
        plaintext,
      ),
    );
    this.n += 1;
    await this.mixHash(ciphertext);
    return ciphertext;
  }

  async decryptAndHash(ciphertext: Uint8Array): Promise<Uint8Array> {
    if (!this.k) throw new Error('Noise: no cipher key yet');
    const plaintext = new Uint8Array(
      await crypto.subtle.decrypt(
        { name: 'AES-GCM', iv: noiseNonce(this.n), additionalData: this.h },
        this.k,
        ciphertext,
      ),
    );
    this.n += 1;
    await this.mixHash(ciphertext);
    return plaintext;
  }

  /** The initiator-to-responder key, then the responder-to-initiator key. */
  split(): Promise<[Uint8Array, Uint8Array]> {
    return hkdf2(this.ck, new Uint8Array());
  }
}

export interface X25519KeyPair {
  privateKey: CryptoKey;
  publicKey: Uint8Array;
}

/**
 * Start an NK handshake to `remoteStatic`. `fixedEphemeral` pins the
 * ephemeral secret, for the published test vectors only; leave it out.
 */
export async function initiate(
  remoteStatic: Uint8Array,
  prologue: Uint8Array,
  fixedEphemeral?: Uint8Array,
): Promise<{
  message1: Uint8Array;
  finish(message2: Uint8Array): Promise<{ payload: Uint8Array; split: [Uint8Array, Uint8Array] }>;
}> {
  const state = await SymmetricState.initialize();
  await state.mixHash(prologue);
  // <- s (known in advance)
  await state.mixHash(remoteStatic);
  // -> e, es
  const ephemeral = await x25519KeyPair(fixedEphemeral);
  await state.mixHash(ephemeral.publicKey);
  await state.mixKey(await dh(ephemeral.privateKey, remoteStatic));
  const message1 = concat(ephemeral.publicKey, await state.encryptAndHash(new Uint8Array()));

  return {
    message1,
    async finish(message2) {
      if (message2.length < DH_LEN + TAG_LEN) throw new Error('Noise: message 2 is too short');
      // <- e, ee
      const remoteEphemeral = message2.slice(0, DH_LEN);
      await state.mixHash(remoteEphemeral);
      await state.mixKey(await dh(ephemeral.privateKey, remoteEphemeral));
      const payload = await state.decryptAndHash(message2.slice(DH_LEN));
      return { payload, split: await state.split() };
    },
  };
}

export async function x25519KeyPair(secret?: Uint8Array): Promise<X25519KeyPair> {
  if (!secret) {
    let pair: CryptoKeyPair;
    try {
      pair = (await crypto.subtle.generateKey({ name: 'X25519' }, true, [
        'deriveBits',
      ])) as CryptoKeyPair;
    } catch (error) {
      throw new Error(
        'Sealed transport needs WebCrypto with X25519, which this runtime lacks ' +
          `(${(error as Error)?.name ?? 'no crypto.subtle'}). Current browsers and Node have ` +
          'it; in React Native, install react-native-quick-crypto as the global crypto.',
      );
    }
    const publicKey = new Uint8Array(await crypto.subtle.exportKey('raw', pair.publicKey));
    return { privateKey: pair.privateKey, publicKey };
  }
  const privateKey = await crypto.subtle.importKey(
    'pkcs8',
    concat(PKCS8_X25519_PREFIX, secret),
    { name: 'X25519' },
    true,
    ['deriveBits'],
  );
  const jwk = await crypto.subtle.exportKey('jwk', privateKey);
  return { privateKey, publicKey: base64UrlDecode(jwk.x as string) };
}

export async function dh(privateKey: CryptoKey, publicKey: Uint8Array): Promise<Uint8Array> {
  const peer = await crypto.subtle.importKey('raw', publicKey, { name: 'X25519' }, false, []);
  return new Uint8Array(
    await crypto.subtle.deriveBits({ name: 'X25519', public: peer } as EcdhKeyDeriveParams, privateKey, 256),
  );
}

/** Noise's AESGCM nonce: 32 zero bits, then the counter as a big-endian u64. */
function noiseNonce(n: number): Uint8Array {
  const nonce = new Uint8Array(12);
  new DataView(nonce.buffer).setBigUint64(4, BigInt(n), false);
  return nonce;
}

async function sha256(data: Uint8Array): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest('SHA-256', data));
}

async function hmacSha256(key: Uint8Array, data: Uint8Array): Promise<Uint8Array> {
  const hmacKey = await crypto.subtle.importKey('raw', key, { name: 'HMAC', hash: 'SHA-256' }, false, [
    'sign',
  ]);
  return new Uint8Array(await crypto.subtle.sign('HMAC', hmacKey, data));
}

/** Noise's `HKDF` with two outputs. */
async function hkdf2(chainingKey: Uint8Array, inputKeyMaterial: Uint8Array): Promise<[Uint8Array, Uint8Array]> {
  const temp = await hmacSha256(chainingKey, inputKeyMaterial);
  const first = await hmacSha256(temp, new Uint8Array([1]));
  const second = await hmacSha256(temp, concat(first, new Uint8Array([2])));
  return [first, second];
}

function base64UrlDecode(value: string): Uint8Array {
  const base64 = value.replace(/-/g, '+').replace(/_/g, '/');
  const binary = atob(base64 + '='.repeat((4 - (base64.length % 4)) % 4));
  return Uint8Array.from(binary, (char) => char.charCodeAt(0));
}
