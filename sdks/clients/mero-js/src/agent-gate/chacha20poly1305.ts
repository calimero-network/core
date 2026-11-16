/**
 * ChaCha20-Poly1305 (RFC 8439), the AEAD mero-agent-gate's HPKE suite uses.
 * WebCrypto has no ChaCha20, and the SDK takes no runtime dependencies, so it
 * is written here from the RFC. Its tests check it against the RFC's vectors
 * and against Node's own implementation.
 *
 * Only for sealing a provisioning bundle once: short messages, one key per
 * message. Not constant-time in Poly1305's arithmetic (BigInt), which leaks
 * nothing here: the one-time Poly1305 key is never reused.
 */

const KEY_LEN = 32;
const NONCE_LEN = 12;
const TAG_LEN = 16;

function rotl(value: number, shift: number): number {
  return ((value << shift) | (value >>> (32 - shift))) >>> 0;
}

function quarterRound(s: Uint32Array, a: number, b: number, c: number, d: number): void {
  s[a] = (s[a] + s[b]) >>> 0;
  s[d] = rotl(s[d] ^ s[a], 16);
  s[c] = (s[c] + s[d]) >>> 0;
  s[b] = rotl(s[b] ^ s[c], 12);
  s[a] = (s[a] + s[b]) >>> 0;
  s[d] = rotl(s[d] ^ s[a], 8);
  s[c] = (s[c] + s[d]) >>> 0;
  s[b] = rotl(s[b] ^ s[c], 7);
}

function u32le(bytes: Uint8Array, offset: number): number {
  return (bytes[offset] | (bytes[offset + 1] << 8) | (bytes[offset + 2] << 16) | (bytes[offset + 3] << 24)) >>> 0;
}

/** One 64-byte ChaCha20 block (RFC 8439 §2.3). */
export function chacha20Block(key: Uint8Array, counter: number, nonce: Uint8Array): Uint8Array {
  const state = new Uint32Array(16);
  state[0] = 0x61707865;
  state[1] = 0x3320646e;
  state[2] = 0x79622d32;
  state[3] = 0x6b206574;
  for (let i = 0; i < 8; i += 1) state[4 + i] = u32le(key, i * 4);
  state[12] = counter >>> 0;
  for (let i = 0; i < 3; i += 1) state[13 + i] = u32le(nonce, i * 4);
  const working = state.slice();
  for (let round = 0; round < 10; round += 1) {
    quarterRound(working, 0, 4, 8, 12);
    quarterRound(working, 1, 5, 9, 13);
    quarterRound(working, 2, 6, 10, 14);
    quarterRound(working, 3, 7, 11, 15);
    quarterRound(working, 0, 5, 10, 15);
    quarterRound(working, 1, 6, 11, 12);
    quarterRound(working, 2, 7, 8, 13);
    quarterRound(working, 3, 4, 9, 14);
  }
  const out = new Uint8Array(64);
  for (let i = 0; i < 16; i += 1) {
    const word = (working[i] + state[i]) >>> 0;
    out[i * 4] = word & 0xff;
    out[i * 4 + 1] = (word >>> 8) & 0xff;
    out[i * 4 + 2] = (word >>> 16) & 0xff;
    out[i * 4 + 3] = (word >>> 24) & 0xff;
  }
  return out;
}

/** ChaCha20 encryption from block `counter` on (RFC 8439 §2.4). */
export function chacha20(key: Uint8Array, counter: number, nonce: Uint8Array, data: Uint8Array): Uint8Array {
  const out = new Uint8Array(data.length);
  for (let offset = 0, block = counter; offset < data.length; offset += 64, block += 1) {
    const stream = chacha20Block(key, block, nonce);
    const end = Math.min(offset + 64, data.length);
    for (let i = offset; i < end; i += 1) out[i] = data[i] ^ stream[i - offset];
  }
  return out;
}

function leToBigInt(bytes: Uint8Array): bigint {
  let value = 0n;
  for (let i = bytes.length - 1; i >= 0; i -= 1) value = (value << 8n) | BigInt(bytes[i]);
  return value;
}

const P1305 = (1n << 130n) - 5n;

/** Poly1305 over `message` with the one-time `key` (RFC 8439 §2.5). */
export function poly1305(key: Uint8Array, message: Uint8Array): Uint8Array {
  const r = leToBigInt(key.subarray(0, 16)) & 0x0ffffffc0ffffffc0ffffffc0fffffffn;
  const s = leToBigInt(key.subarray(16, 32));
  let acc = 0n;
  for (let offset = 0; offset < message.length; offset += 16) {
    const block = message.subarray(offset, Math.min(offset + 16, message.length));
    const n = leToBigInt(block) + (1n << BigInt(8 * block.length));
    acc = ((acc + n) * r) % P1305;
  }
  let tag = (acc + s) & ((1n << 128n) - 1n);
  const out = new Uint8Array(TAG_LEN);
  for (let i = 0; i < TAG_LEN; i += 1) {
    out[i] = Number(tag & 0xffn);
    tag >>= 8n;
  }
  return out;
}

function pad16(length: number): Uint8Array {
  return new Uint8Array((16 - (length % 16)) % 16);
}

function u64le(value: number): Uint8Array {
  const out = new Uint8Array(8);
  new DataView(out.buffer).setBigUint64(0, BigInt(value), true);
  return out;
}

function macData(aad: Uint8Array, ciphertext: Uint8Array): Uint8Array {
  const parts = [aad, pad16(aad.length), ciphertext, pad16(ciphertext.length), u64le(aad.length), u64le(ciphertext.length)];
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

function check(key: Uint8Array, nonce: Uint8Array): void {
  if (key.length !== KEY_LEN) throw new Error('ChaCha20-Poly1305 needs a 32-byte key');
  if (nonce.length !== NONCE_LEN) throw new Error('ChaCha20-Poly1305 needs a 12-byte nonce');
}

/** Seal: the ciphertext followed by its 16-byte tag (RFC 8439 §2.8). */
export function chacha20Poly1305Seal(
  key: Uint8Array,
  nonce: Uint8Array,
  plaintext: Uint8Array,
  aad: Uint8Array = new Uint8Array(0),
): Uint8Array {
  check(key, nonce);
  const otk = chacha20Block(key, 0, nonce).subarray(0, 32);
  const ciphertext = chacha20(key, 1, nonce, plaintext);
  const tag = poly1305(otk, macData(aad, ciphertext));
  const out = new Uint8Array(ciphertext.length + TAG_LEN);
  out.set(ciphertext);
  out.set(tag, ciphertext.length);
  return out;
}

/** Open what {@link chacha20Poly1305Seal} made; throws if it was altered. */
export function chacha20Poly1305Open(
  key: Uint8Array,
  nonce: Uint8Array,
  sealed: Uint8Array,
  aad: Uint8Array = new Uint8Array(0),
): Uint8Array {
  check(key, nonce);
  if (sealed.length < TAG_LEN) throw new Error('The sealed message is shorter than its tag');
  const ciphertext = sealed.subarray(0, sealed.length - TAG_LEN);
  const otk = chacha20Block(key, 0, nonce).subarray(0, 32);
  const expected = poly1305(otk, macData(aad, ciphertext));
  let diff = 0;
  for (let i = 0; i < TAG_LEN; i += 1) diff |= expected[i] ^ sealed[ciphertext.length + i];
  if (diff !== 0) throw new Error('The sealed message did not authenticate');
  return chacha20(key, 1, nonce, ciphertext);
}
