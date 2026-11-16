/**
 * RFC 9180 HPKE, the one suite and mode mero-agent-gate takes: Auth mode,
 * DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, ChaCha20-Poly1305, single-shot
 * (one message per context, sequence number 0). Auth mode proves the sender:
 * the gate opens a bundle only under the X25519 key it names, so nobody can
 * seal in an owner's name.
 *
 * X25519 and HMAC are WebCrypto; ChaCha20-Poly1305 is ./chacha20poly1305.
 * The tests open what mero-agent-gate (Rust, the `hpke` crate) sealed, and the
 * gate opens what this seals.
 */

import { concat } from '../crypto/internal.js';
import { dh, x25519KeyPair } from '../sealed/noise.js';
import { chacha20Poly1305Open, chacha20Poly1305Seal } from './chacha20poly1305.js';

const KEM_ID = 0x0020; // DHKEM(X25519, HKDF-SHA256)
const KDF_ID = 0x0001; // HKDF-SHA256
const AEAD_ID = 0x0003; // ChaCha20-Poly1305
const MODE_AUTH = 0x02;
const N_SECRET = 32;
const N_KEY = 32;
const N_NONCE = 12;

const utf8 = (s: string) => new TextEncoder().encode(s);
const i2osp2 = (n: number) => new Uint8Array([(n >> 8) & 0xff, n & 0xff]);

const KEM_SUITE = concat(utf8('KEM'), i2osp2(KEM_ID));
const HPKE_SUITE = concat(utf8('HPKE'), i2osp2(KEM_ID), i2osp2(KDF_ID), i2osp2(AEAD_ID));

async function hmac(key: Uint8Array, data: Uint8Array): Promise<Uint8Array> {
  // An empty salt is HashLen zeros (RFC 5869); WebCrypto refuses a zero-length HMAC key.
  const raw = key.length === 0 ? new Uint8Array(32) : key;
  const k = await crypto.subtle.importKey('raw', raw, { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
  return new Uint8Array(await crypto.subtle.sign('HMAC', k, data));
}

async function expand(prk: Uint8Array, info: Uint8Array, length: number): Promise<Uint8Array> {
  const out = new Uint8Array(length);
  let previous = new Uint8Array(0);
  for (let i = 1, offset = 0; offset < length; i += 1) {
    previous = await hmac(prk, concat(previous, info, new Uint8Array([i])));
    out.set(previous.subarray(0, Math.min(previous.length, length - offset)), offset);
    offset += previous.length;
  }
  return out;
}

function labeledExtract(suite: Uint8Array, salt: Uint8Array, label: string, ikm: Uint8Array): Promise<Uint8Array> {
  return hmac(salt, concat(utf8('HPKE-v1'), suite, utf8(label), ikm));
}

function labeledExpand(suite: Uint8Array, prk: Uint8Array, label: string, info: Uint8Array, length: number) {
  return expand(prk, concat(i2osp2(length), utf8('HPKE-v1'), suite, utf8(label), info), length);
}

async function extractAndExpand(dhValue: Uint8Array, kemContext: Uint8Array): Promise<Uint8Array> {
  const prk = await labeledExtract(KEM_SUITE, new Uint8Array(0), 'eae_prk', dhValue);
  return labeledExpand(KEM_SUITE, prk, 'shared_secret', kemContext, N_SECRET);
}

async function keySchedule(sharedSecret: Uint8Array, info: Uint8Array): Promise<{ key: Uint8Array; nonce: Uint8Array }> {
  const empty = new Uint8Array(0);
  const pskIdHash = await labeledExtract(HPKE_SUITE, empty, 'psk_id_hash', empty);
  const infoHash = await labeledExtract(HPKE_SUITE, empty, 'info_hash', info);
  const context = concat(new Uint8Array([MODE_AUTH]), pskIdHash, infoHash);
  const secret = await labeledExtract(HPKE_SUITE, sharedSecret, 'secret', empty);
  return {
    key: await labeledExpand(HPKE_SUITE, secret, 'key', context, N_KEY),
    nonce: await labeledExpand(HPKE_SUITE, secret, 'base_nonce', context, N_NONCE),
  };
}

export interface HpkeSealed {
  /** The encapsulated key: the sender's ephemeral X25519 public key. */
  enc: Uint8Array;
  ciphertext: Uint8Array;
}

/**
 * Seal `plaintext` to `recipientPublic` in Auth mode as the holder of
 * `senderSecret` (32 raw X25519 bytes). `ephemeralSecret` is for tests.
 */
export async function hpkeSealAuth(
  recipientPublic: Uint8Array,
  senderSecret: Uint8Array,
  info: Uint8Array,
  plaintext: Uint8Array,
  aad: Uint8Array = new Uint8Array(0),
  ephemeralSecret?: Uint8Array,
): Promise<HpkeSealed> {
  if (recipientPublic.length !== 32) throw new Error('The recipient key is not 32 bytes of X25519');
  const ephemeral = await x25519KeyPair(ephemeralSecret);
  const sender = await x25519KeyPair(senderSecret);
  const dhValue = concat(await dh(ephemeral.privateKey, recipientPublic), await dh(sender.privateKey, recipientPublic));
  const shared = await extractAndExpand(dhValue, concat(ephemeral.publicKey, recipientPublic, sender.publicKey));
  const { key, nonce } = await keySchedule(shared, info);
  return { enc: ephemeral.publicKey, ciphertext: chacha20Poly1305Seal(key, nonce, plaintext, aad) };
}

/** Open what {@link hpkeSealAuth} (or the gate's own sealer) made, as the recipient. */
export async function hpkeOpenAuth(
  recipientSecret: Uint8Array,
  senderPublic: Uint8Array,
  info: Uint8Array,
  sealed: HpkeSealed,
  aad: Uint8Array = new Uint8Array(0),
): Promise<Uint8Array> {
  const recipient = await x25519KeyPair(recipientSecret);
  const dhValue = concat(await dh(recipient.privateKey, sealed.enc), await dh(recipient.privateKey, senderPublic));
  const shared = await extractAndExpand(dhValue, concat(sealed.enc, recipient.publicKey, senderPublic));
  const { key, nonce } = await keySchedule(shared, info);
  return chacha20Poly1305Open(key, nonce, sealed.ciphertext, aad);
}
