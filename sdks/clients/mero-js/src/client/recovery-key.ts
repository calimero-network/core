/**
 * An organization's recovery key (core `docs/specs/org-recovery.md`): an
 * Ed25519 key whose 32-byte seed the owner keeps offline. The node stores only
 * the public half; the seed signs, here and never on a node, the statement
 * that hands the organization to one account. Byte-for-byte core's
 * `calimero_identity::recovery`.
 */

import { b64url } from '../runtime/passkey.js';

/** What a recovery signature covers before the organization, the account and the nonce. */
export const RECOVERY_DOMAIN = 'calimero.org-recovery.v1\0';

/** The longest nonce a recovery carries, in bytes (core's `MAX_RECOVERY_NONCE_BYTES`). */
export const MAX_RECOVERY_NONCE_BYTES = 64;

/** PKCS#8 prefix of an Ed25519 private key; the 32-byte seed follows. */
const PKCS8_ED25519 = Uint8Array.from([
  0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
]);

/** A new random seed. */
export function newRecoverySeed(): Uint8Array {
  return crypto.getRandomValues(new Uint8Array(32));
}

/** The seed as people write it down: 64 hex digits in eight groups of eight, joined by `-`. */
export function recoverySeedText(seed: Uint8Array): string {
  checkSeed(seed);
  const hex = Array.from(seed, (b) => b.toString(16).padStart(2, '0')).join('');
  const groups: string[] = [];
  for (let i = 0; i < hex.length; i += 8) groups.push(hex.slice(i, i + 8));
  return groups.join('-');
}

/** A seed from its written form: 64 hex digits; `-`, whitespace and case are ignored. */
export function parseRecoverySeed(text: string): Uint8Array {
  const digits = text.replace(/[-\s]/g, '');
  if (!/^[0-9a-fA-F]{64}$/.test(digits)) throw new Error('a recovery seed is 64 hex digits');
  return Uint8Array.from({ length: 32 }, (_, i) => parseInt(digits.slice(2 * i, 2 * i + 2), 16));
}

/** The bytes a recovery signs: `"calimero.org-recovery.v1\0" || org || 0 || account || 0 || nonce`. */
export function recoveryMessage(org: string, account: string, nonce: string): Uint8Array {
  return new TextEncoder().encode(`${RECOVERY_DOMAIN}${org}\0${account}\0${nonce}`);
}

/** The seed's public key, base64url: what the organization stores. */
export async function recoveryPublicKey(seed: Uint8Array): Promise<string> {
  const key = await privateKey(seed, true);
  const jwk = await crypto.subtle.exportKey('jwk', key);
  if (!jwk.x) throw new Error('this runtime cannot export an Ed25519 public key');
  return jwk.x;
}

/** The seed's signature (base64url) handing `org` to `account` under `nonce`. */
export async function signRecovery(seed: Uint8Array, org: string, account: string, nonce: string): Promise<string> {
  const nonceBytes = new TextEncoder().encode(nonce).length;
  if (nonceBytes === 0 || nonceBytes > MAX_RECOVERY_NONCE_BYTES) {
    throw new Error(`a recovery nonce is 1 to ${MAX_RECOVERY_NONCE_BYTES} bytes`);
  }
  const key = await privateKey(seed, false);
  const sig = await crypto.subtle.sign('Ed25519', key, buf(recoveryMessage(org, account, nonce)));
  return b64url(new Uint8Array(sig));
}

function checkSeed(seed: Uint8Array): void {
  if (seed.length !== 32) throw new Error('a recovery seed is 32 bytes');
}

export function privateKey(seed: Uint8Array, extractable: boolean): Promise<CryptoKey> {
  checkSeed(seed);
  const pkcs8 = new Uint8Array(PKCS8_ED25519.length + 32);
  pkcs8.set(PKCS8_ED25519);
  pkcs8.set(seed, PKCS8_ED25519.length);
  return crypto.subtle.importKey('pkcs8', buf(pkcs8), { name: 'Ed25519' }, extractable, ['sign']);
}

export const buf = (b: Uint8Array): ArrayBuffer => b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength) as ArrayBuffer;
