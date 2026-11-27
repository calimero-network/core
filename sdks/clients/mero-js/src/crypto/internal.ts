/**
 * Byte-level primitives shared by the sealed transport, the attestation check
 * and the agent gate: hex handling, fixed-width integers and concatenation.
 * Internal — not exported from the package root.
 */

/** Little-endian u32, for borsh fixed-width fields and epoch counters. */
export function u32le(value: number): Uint8Array {
  const out = new Uint8Array(4);
  new DataView(out.buffer).setUint32(0, value, true);
  return out;
}

export function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

export function fromHex(value: string, label: string, expectedBytes: number): Uint8Array {
  const clean = value.trim();
  if (!/^[0-9a-fA-F]*$/.test(clean) || clean.length !== expectedBytes * 2) {
    throw new Error(
      `${label} must be ${expectedBytes * 2} hex characters, got ${clean.length}`,
    );
  }
  // Indexed rather than `match(/../g)!`: the length is already validated above,
  // so the assertion would only be telling the compiler what the guard proved,
  // and this needs no assertion at all.
  const bytes = new Uint8Array(expectedBytes);
  for (let i = 0; i < expectedBytes; i += 1) {
    bytes[i] = Number.parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

/**
 * Decode hex of an unknown length.
 *
 * The sized {@link fromHex} is the right default — every key and digest here
 * has a fixed width, and checking it catches a truncated paste at the edge
 * rather than deep inside a signature check. This one exists for the already-
 * encoded structures whose length is genuinely variable: a device certificate,
 * a login statement, a request signature. It still rejects odd-length and
 * non-hex input, so the only check it gives up is the one that cannot apply.
 */
export function fromHexUnsized(value: string, label: string): Uint8Array {
  const clean = value.trim();
  if (!/^[0-9a-fA-F]*$/.test(clean) || clean.length % 2 !== 0) {
    throw new Error(`${label} must be an even number of hex characters`);
  }
  const bytes = new Uint8Array(clean.length / 2);
  for (let i = 0; i < bytes.length; i += 1) {
    bytes[i] = Number.parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

export function u64le(value: number | bigint): Uint8Array {
  const out = new Uint8Array(8);
  new DataView(out.buffer).setBigUint64(0, BigInt(value), true);
  return out;
}

export function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((total, p) => total + p.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

/** `b`'s bytes as an `ArrayBuffer` of their own, for WebCrypto. */
function arrayBuffer(b: Uint8Array): ArrayBuffer {
  return b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength) as ArrayBuffer;
}

// The Ed25519 group order: an S at or above it is a malleated signature, refused as verify_strict does.
const L = 2n ** 252n + 27742317777372353535851937790883648493n;
// The Ed25519 points of order dividing 8, for `verify_strict`'s refusal of a small-order key or R.
const P = 2n ** 255n - 19n;
const D = (((P - 121665n) % P) * modPow(121666n, P - 2n)) % P;
function modPow(base: bigint, exp: bigint): bigint {
  let result = 1n;
  base %= P;
  for (; exp > 0n; exp >>= 1n, base = (base * base) % P) if (exp & 1n) result = (result * base) % P;
  return result;
}

function isSmallOrder(encoded: Uint8Array): boolean {
  let y = 0n;
  for (let i = 31; i >= 0; i--) y = (y << 8n) | BigInt(encoded[i] & (i === 31 ? 0x7f : 0xff));
  y %= P;
  const sign = (encoded[31] >> 7) & 1;
  const y2 = (y * y) % P;
  const x2 = (((y2 - 1n + P) % P) * modPow((D * y2 + 1n) % P, P - 2n)) % P;
  let x = modPow(x2, (P + 3n) / 8n);
  if ((x * x) % P !== x2) x = (x * modPow(2n, (P - 1n) / 4n)) % P;
  if ((x * x) % P !== x2) return false;
  if (Number(x & 1n) !== sign) x = (P - x) % P;
  let pt: [bigint, bigint] = [x, y];
  for (let i = 0; i < 3; i++) {
    const [x1, y1] = pt;
    const k = (D * x1 * x1 * y1 * y1) % P;
    pt = [(2n * x1 * y1 * modPow((1n + k) % P, P - 2n)) % P, ((y1 * y1 + x1 * x1) * modPow((1n - k + P) % P, P - 2n)) % P];
  }
  return pt[0] === 0n && pt[1] === 1n;
}

/** Ed25519 as core's `verify_strict` reads it: a non-canonical S and a small-order key or R are refused as well as a bad signature. */
export async function verifyStrict(key: Uint8Array, message: Uint8Array, signature: Uint8Array): Promise<boolean> {
  let s = 0n;
  for (let i = 63; i >= 32; i--) s = (s << 8n) | BigInt(signature[i]);
  if (s >= L || isSmallOrder(key) || isSmallOrder(signature.slice(0, 32))) return false;
  const publicKey = await crypto.subtle.importKey('raw', arrayBuffer(key), { name: 'Ed25519' }, false, ['verify']);
  return crypto.subtle.verify('Ed25519', publicKey, arrayBuffer(signature), arrayBuffer(message));
}
