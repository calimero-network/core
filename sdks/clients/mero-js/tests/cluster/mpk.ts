/**
 * A signed single-service `.mpk`, made the way core's `calimero-bundle` signs
 * one (`sign_manifest_json`): `signerId` is the key's `did:key`, the signature
 * is Ed25519 over `SHA-256("calimero.bundle.manifest.v1\0" || JCS(manifest))`
 * with `signature` and `_`-prefixed fields left out, and the archive is a
 * gzipped tar holding `manifest.json` and `app.wasm`.
 */

import { createHash, generateKeyPairSync, sign, type KeyObject } from 'node:crypto';
import { gzipSync } from 'node:zlib';

const B58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

function base58(bytes: Uint8Array): string {
  let n = 0n;
  for (const b of bytes) n = (n << 8n) | BigInt(b);
  let out = '';
  while (n > 0n) {
    out = B58[Number(n % 58n)] + out;
    n /= 58n;
  }
  for (const b of bytes) {
    if (b !== 0) break;
    out = `1${out}`;
  }
  return out;
}

/** RFC 8785 for what a manifest holds: objects, arrays, strings, integers, booleans. */
function jcs(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(jcs).join(',')}]`;
  if (value && typeof value === 'object') {
    const entries = Object.entries(value as Record<string, unknown>)
      .filter(([, v]) => v !== undefined)
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
    return `{${entries.map(([k, v]) => `${JSON.stringify(k)}:${jcs(v)}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

function tar(entries: Array<[string, Uint8Array]>): Uint8Array {
  const blocks: Buffer[] = [];
  for (const [name, body] of entries) {
    const h = Buffer.alloc(512);
    h.write(name, 0, 'utf8');
    h.write('0000644\0', 100);
    h.write('0000000\0', 108);
    h.write('0000000\0', 116);
    h.write(`${body.length.toString(8).padStart(11, '0')}\0`, 124);
    h.write('00000000000\0', 136);
    h.write('        ', 148);
    h.write('0', 156);
    h.write('ustar\0', 257);
    h.write('00', 263);
    let sum = 0;
    for (const b of h) sum += b;
    h.write(`${sum.toString(8).padStart(6, '0')}\0 `, 148);
    blocks.push(h, Buffer.from(body), Buffer.alloc((512 - (body.length % 512)) % 512));
  }
  blocks.push(Buffer.alloc(1024));
  return new Uint8Array(gzipSync(Buffer.concat(blocks)));
}

export interface Publisher {
  privateKey: KeyObject;
  publicKey: Uint8Array;
  signerId: string;
}

export function publisher(): Publisher {
  const { privateKey, publicKey } = generateKeyPairSync('ed25519');
  const raw = new Uint8Array(publicKey.export({ format: 'der', type: 'spki' }).subarray(-32));
  return { privateKey, publicKey: raw, signerId: `did:key:z${base58(new Uint8Array([0xed, 0x01, ...raw]))}` };
}

const b64url = (b: Uint8Array) => Buffer.from(b).toString('base64url');

export function signedMpk(pkg: string, version: string, wasm: Uint8Array, key: Publisher): Uint8Array {
  const manifest: Record<string, unknown> = {
    version: '1.0',
    package: pkg,
    appVersion: version,
    signerId: key.signerId,
    minRuntimeVersion: '0.1.0',
    wasm: { path: 'app.wasm', hash: createHash('sha256').update(wasm).digest('hex'), size: wasm.length },
  };
  const payload = createHash('sha256')
    .update(Buffer.from('calimero.bundle.manifest.v1\0'))
    .update(jcs(manifest))
    .digest();
  manifest.signature = {
    algorithm: 'ed25519',
    publicKey: b64url(key.publicKey),
    signature: b64url(new Uint8Array(sign(null, payload, key.privateKey))),
  };
  return tar([
    ['manifest.json', new TextEncoder().encode(JSON.stringify(manifest))],
    ['app.wasm', wasm],
  ]);
}

/** `wasm` with a custom section named `tag`: another module that runs the same. */
export function variant(wasm: Uint8Array, tag: string): Uint8Array {
  const name = new TextEncoder().encode(tag);
  return new Uint8Array([...wasm, 0, name.length + 1, name.length, ...name]);
}
