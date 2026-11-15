/**
 * Building and signing a compute-only pipeline package (`.mpk`), byte-compatible
 * with core's `calimero-bundle`: the manifest, its RFC 8785 canonical form, the
 * domain-tagged signing payload and the archive layout are the ones core's
 * `scripts/pack-component.py` and `mero-sign` produce and its node verifies.
 *
 * A publisher key is an Ed25519 seed. Its backup ({@link exportPublisherKey})
 * is the JSON `mero-sign` reads as a key file: `{ private_key, public_key,
 * signer_id }`, the seed and public key base64url, the signer id a `did:key`.
 */

import { sha256 } from '@noble/hashes/sha2.js';

import { privateKey, buf, recoveryPublicKey } from '../client/recovery-key.js';
import { b64url, fromB64url } from '../runtime/passkey.js';

export const MAX_MANIFEST_BYTES = 1024 * 1024;
export const MAX_PIPELINES_PER_PACKAGE = 16;
export const MAX_PIPELINE_BYTES = 64 * 1024;
export const MAX_ARCHIVE_BYTES = 512 * 1024 * 1024; // decompressed
export const MAX_MPK_BYTES = ((64 * 1024 * 1024 - 1024 * 1024) / 4) * 3; // what a node takes, compressed
const DEV_SIGNER_ID = 'did:key:z6MknF3p5L5FDHJQ7FREUapuX4Wmp4MtF6WrHYaXS2B3eZQd'; // mero-sign --dev's public key

const MANIFEST_SIGNING_DOMAIN = new TextEncoder().encode('calimero.bundle.manifest.v1\0');
const MIN_RUNTIME_VERSION = '0.1.0';
const ED25519_PUB_MULTICODEC = [0xed, 0x01];
const BASE58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
const COMPONENT_HEADER = [0, 0x61, 0x73, 0x6d, 0x0d, 0, 0x01, 0]; // "\0asm" + component version 13, layer 1
const PIPELINE_PATH = /^pipelines\/[A-Za-z0-9._-]+\.yaml$/;
const SEED_B64URL = /^[A-Za-z0-9_-]{43}$/;
const SEMVER =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$/;

export interface PublisherKey {
  /** The 32-byte Ed25519 seed. Keep it secret. */
  readonly seed: Uint8Array;
  /** The 32-byte Ed25519 public key. */
  readonly publicKey: Uint8Array;
}

/** A non-extractable signing key (for a console that keeps it in IndexedDB) and its public key. */
export interface WebCryptoPublisherKey {
  readonly privateKey: CryptoKey;
  readonly publicKey: Uint8Array;
}

export type PackKey = PublisherKey | WebCryptoPublisherKey;

export interface PackageMetadata {
  name: string;
  description?: string;
  author?: string;
  icon?: string;
  tags?: string[];
  license?: string;
  category?: string;
  guide?: string;
}

export interface ComputePackageInput {
  /** The package id, e.g. `com.example.pipeline`. */
  package: string;
  /** The release's version, a semver such as `1.0.0`. */
  version: string;
  /** The compute component (a `calimero:app` WASM component). */
  compute: Uint8Array;
  /** Pipeline YAML files, each at `pipelines/<name>.yaml`. */
  pipelines: { path: string; source: string }[];
  metadata?: PackageMetadata;
}

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

const utf8 = (s: string) => new TextEncoder().encode(s);
const hex = (b: Uint8Array) => Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
const concat = (...parts: Uint8Array[]) => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let off = 0;
  for (const p of parts) (out.set(p, off), (off += p.length));
  return out;
};

async function requireEd25519(): Promise<void> {
  const unsupported = new Error('this runtime cannot sign Ed25519 packages (WebCrypto Ed25519 is missing)');
  if (!globalThis.crypto?.subtle) throw unsupported;
  try {
    await privateKey(new Uint8Array(32), false);
  } catch {
    throw unsupported;
  }
}

async function keyOfSeed(seed: Uint8Array): Promise<PublisherKey> {
  await requireEd25519();
  if (seed.length !== 32) throw new Error('a publisher seed is 32 bytes');
  const publicKey = fromB64url(await recoveryPublicKey(seed));
  return { seed: Uint8Array.from(seed), publicKey };
}

export async function generatePublisherKey(): Promise<PublisherKey> {
  await requireEd25519();
  return keyOfSeed(crypto.getRandomValues(new Uint8Array(32)));
}

/** The same key as a non-extractable WebCrypto key, after which the seed can be dropped. */
export async function webCryptoPublisherKey(key: PublisherKey): Promise<WebCryptoPublisherKey> {
  await requireEd25519();
  return { privateKey: await privateKey(Uint8Array.from(key.seed), false), publicKey: Uint8Array.from(key.publicKey) };
}

function base58(bytes: Uint8Array): string {
  let n = 0n;
  for (const b of bytes) n = (n << 8n) | BigInt(b);
  let out = '';
  for (; n > 0n; n /= 58n) out = BASE58[Number(n % 58n)] + out;
  return '1'.repeat(bytes.findIndex((b) => b !== 0) === -1 ? bytes.length : bytes.findIndex((b) => b !== 0)) + out;
}

/** The `did:key` of the key, as core's `derive_signer_id_did_key` derives it. */
export function signerIdOf(key: Pick<PublisherKey, 'publicKey'>): string {
  return `did:key:z${base58(concat(Uint8Array.from(ED25519_PUB_MULTICODEC), key.publicKey))}`;
}

/** Whether this is `mero-sign --dev`'s public key: a registry and a production node refuse what it signs. */
export function isDevPublisherKey(key: Pick<PublisherKey, 'publicKey'>): boolean {
  return signerIdOf(key) === DEV_SIGNER_ID;
}

/** The key's backup: UTF-8 JSON in `mero-sign`'s key-file shape. */
export function exportPublisherKey(key: PublisherKey): Uint8Array {
  return utf8(
    JSON.stringify({ private_key: b64url(key.seed), public_key: b64url(key.publicKey), signer_id: signerIdOf(key) }, null, 2),
  );
}

/** Restores a key from its backup; the public key and signer id, when present, must be the seed's. Other fields are ignored, as `mero-sign` does. */
export async function importPublisherKey(bytes: Uint8Array): Promise<PublisherKey> {
  let file: { private_key?: unknown; public_key?: unknown; signer_id?: unknown };
  try {
    file = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
  } catch {
    throw new Error('a publisher key backup is a JSON document');
  }
  const seed = file?.private_key;
  if (typeof seed !== 'string' || !SEED_B64URL.test(seed) || b64url(fromB64url(seed)) !== seed) {
    throw new Error('the backup private_key is not an unpadded base64url 32-byte seed');
  }
  const key = await keyOfSeed(fromB64url(seed));
  if (file.public_key !== undefined && file.public_key !== b64url(key.publicKey)) {
    throw new Error("the backup public_key is not the seed's");
  }
  if (file.signer_id !== undefined && file.signer_id !== signerIdOf(key)) {
    throw new Error("the backup signer_id is not the seed's");
  }
  return key;
}

/** RFC 8785 canonical bytes of the manifest without `signature` and `_`-prefixed fields, as core's `canonicalize_manifest`. */
export function canonicalizeManifest(manifest: { [key: string]: Json }): Uint8Array {
  const signed = Object.fromEntries(Object.entries(manifest).filter(([k]) => k !== 'signature' && !k.startsWith('_')));
  return utf8(jcs(signed));
}

// Manifests hold only strings, safe integers, arrays and objects, where JSON.stringify's
// string and integer forms are RFC 8785's; keys sort by UTF-16 code unit, the default order.
// With `anyNumber` (a node's receipt, which carries user params) every finite number is allowed:
// RFC 8785 writes numbers as ECMAScript does, which `String` gives, and core's canonicalizer
// converts every number to f64 first, so a number JSON.parse rounded canonicalizes as core's does.
export function jcs(value: Json, anyNumber = false): string {
  if (typeof value === 'string') {
    if (/[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/.test(value)) {
      throw new Error('a lone surrogate cannot be canonicalized');
    }
    return JSON.stringify(value);
  }
  if (typeof value === 'number') {
    if (anyNumber ? !Number.isFinite(value) : !Number.isSafeInteger(value)) {
      throw new Error(anyNumber ? 'a non-finite number cannot be canonicalized' : 'only safe integers can be canonicalized');
    }
    return String(value);
  }
  if (value === null || typeof value === 'boolean') return String(value);
  if (typeof value !== 'object') throw new Error(`a ${typeof value} cannot be canonicalized`);
  if (Array.isArray(value)) return `[${value.map((v) => jcs(v, anyNumber)).join(',')}]`;
  return `{${Object.keys(value)
    .sort()
    .map((k) => `${jcs(k, anyNumber)}:${jcs(value[k], anyNumber)}`)
    .join(',')}}`;
}

/** What Ed25519 signs: `sha256(domain || canonical bytes)`, as core's `compute_signing_payload`. */
export async function signingPayload(manifest: { [key: string]: Json }): Promise<Uint8Array> {
  return sha256(concat(MANIFEST_SIGNING_DOMAIN, canonicalizeManifest(manifest)));
}

/**
 * Checks a manifest's signature and that its `signerId` is the signing key's; returns the signer id.
 * Not core's `verify_strict`, and it does not look at artifact digests: use {@link checkPackage} for those.
 */
export async function verifyManifestSignature(manifest: { [key: string]: Json }): Promise<string> {
  await requireEd25519();
  const sig = manifest.signature as { algorithm?: string; publicKey?: string; signature?: string } | undefined;
  if (!sig) throw new Error("manifest is missing required 'signature' field");
  if (sig.algorithm !== 'ed25519') throw new Error(`unsupported signature algorithm: '${sig.algorithm}'`);
  const publicKey = fromB64url(sig.publicKey ?? '');
  const signature = fromB64url(sig.signature ?? '');
  if (publicKey.length !== 32 || signature.length !== 64) throw new Error('malformed signature field');
  const key = await crypto.subtle.importKey('raw', buf(publicKey), { name: 'Ed25519' }, false, ['verify']);
  const ok = await crypto.subtle.verify('Ed25519', key, buf(signature), buf(await signingPayload(manifest)));
  if (!ok) throw new Error('Ed25519 signature verification failed');
  const derived = signerIdOf({ publicKey });
  if (manifest.signerId !== derived) {
    throw new Error(`signerId mismatch: manifest declares '${manifest.signerId}' but signature public key derives to '${derived}'`);
  }
  return derived;
}

function checkPathComponent(value: string, field: string): void {
  if (!value) throw new Error(`${field} is empty`);
  if (value.includes('..')) throw new Error(`${field} contains path traversal sequence '..'`);
  if (value.includes('/') || value.includes('\\')) throw new Error(`${field} contains directory separator`);
  if (value.includes('\0')) throw new Error(`${field} contains null byte`);
  if (value.length >= 2 && value[1] === ':') throw new Error(`${field} appears to be an absolute path`);
}

// core's validate_artifact_path, then the layout a pipeline file must have.
function checkPipelinePath(value: string, field: string): void {
  if (!value) throw new Error(`${field} is empty`);
  if (value.includes('\0')) throw new Error(`${field} contains null byte`);
  if (value.includes('\\')) throw new Error(`${field} contains backslash (use forward slashes)`);
  if (value.startsWith('/')) throw new Error(`${field} is an absolute path`);
  if (value[1] === ':') throw new Error(`${field} appears to be an absolute Windows path`);
  if (value.split('/').includes('..')) throw new Error(`${field} contains path traversal component '..'`);
  if (!PIPELINE_PATH.test(value) || utf8(value).length > 100) {
    throw new Error(`${field} must be pipelines/<name>.yaml (A-Z a-z 0-9 . _ -), at most 100 bytes`);
  }
}

const METADATA_STRINGS = ['name', 'description', 'author', 'icon', 'license', 'category', 'guide'] as const;

// Copies only the known string fields into a fresh object, so what is signed is what is archived.
function normalizeMetadata(m: PackageMetadata): { [key: string]: Json } {
  const bad = (what: string) => new Error(`metadata ${what}`);
  if (typeof m !== 'object' || m === null || Array.isArray(m)) throw bad('must be an object');
  const known = new Set<string>([...METADATA_STRINGS, 'tags']);
  const unknown = Object.keys(m).find((k) => !known.has(k));
  if (unknown !== undefined) throw bad(`has an unknown field '${unknown}'`);
  const out: { [key: string]: Json } = {};
  for (const k of METADATA_STRINGS) {
    const v = (m as unknown as Record<string, unknown>)[k];
    if (v === undefined) continue;
    if (typeof v !== 'string') throw bad(`${k} must be a string`);
    out[k] = v;
  }
  if (typeof out.name !== 'string') throw bad('name is required');
  if (m.tags !== undefined) {
    if (!Array.isArray(m.tags) || m.tags.some((t) => typeof t !== 'string')) throw bad('tags must be an array of strings');
    out.tags = [...m.tags];
  }
  return out;
}

const artifact = (path: string, bytes: Uint8Array) => ({ path, hash: hex(sha256(bytes)), size: bytes.length });
const padded = (n: number) => 512 + Math.ceil(n / 512) * 512;

function tarEntry(name: string, data: Uint8Array): Uint8Array {
  const header = new Uint8Array(512);
  const put = (off: number, text: string) => header.set(utf8(text), off);
  put(0, name);
  put(100, '0000644\0');
  put(108, '0000000\0');
  put(116, '0000000\0');
  put(124, `${data.length.toString(8).padStart(11, '0')}\0`);
  put(136, '00000000000\0');
  put(148, '        ');
  put(156, '0');
  put(257, 'ustar\0');
  put(263, '00');
  put(148, `${header.reduce((n, b) => n + b, 0).toString(8).padStart(6, '0')}\0 `);
  return concat(header, data, new Uint8Array((512 - (data.length % 512)) % 512));
}

// The entries of a tar this module wrote; a repeated name is refused.
function readTar(tar: Uint8Array): Map<string, Uint8Array> {
  const out = new Map<string, Uint8Array>();
  const text = (a: number, b: number) => new TextDecoder().decode(tar.subarray(a, b)).replace(/\0.*$/s, '');
  for (let off = 0; off + 512 <= tar.length && tar[off] !== 0; ) {
    const name = text(off, off + 100);
    const size = parseInt(text(off + 124, off + 136).trim(), 8);
    if (out.has(name)) throw new Error(`the archive has more than one entry at '${name}'`);
    out.set(name, tar.subarray(off + 512, off + 512 + size));
    off += padded(size);
  }
  return out;
}

/**
 * Checks an uncompressed archive as the node will: one manifest, its signature by `publicKey`,
 * exactly the files it declares, each with the declared size and SHA-256.
 */
export async function checkPackage(tar: Uint8Array, publicKey: Uint8Array): Promise<void> {
  const entries = readTar(tar);
  const raw = entries.get('manifest.json');
  if (!raw) throw new Error('manifest.json not found in bundle');
  const manifest = JSON.parse(new TextDecoder().decode(raw));
  await verifyManifestSignature(manifest);
  if (manifest.signature.publicKey !== b64url(publicKey)) throw new Error('the manifest is not signed by the publisher key');
  const declared = [manifest.compute, ...(manifest.pipelines ?? [])];
  const names = ['manifest.json', ...declared.map((a) => a.path)];
  if (names.length !== entries.size || names.some((n) => !entries.has(n))) {
    throw new Error('the archive does not hold exactly the files its manifest declares');
  }
  for (const a of declared) {
    const bytes = entries.get(a.path);
    if (!bytes || bytes.length !== a.size || hex(sha256(bytes)) !== a.hash) {
      throw new Error(`bundle artifact '${a.path}' failed its integrity check`);
    }
  }
}

async function gzip(bytes: Uint8Array): Promise<Uint8Array> {
  if (typeof CompressionStream === 'undefined') throw new Error('this runtime cannot gzip packages (no CompressionStream)');
  const stream = new Blob([buf(bytes)]).stream().pipeThrough(new CompressionStream('gzip'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

async function signerOf(key: PackKey, seed: Uint8Array | undefined, publicKey: Uint8Array) {
  if (seed) {
    const derived = fromB64url(await recoveryPublicKey(seed));
    if (hex(derived) !== hex(publicKey)) throw new Error("the publisher key's publicKey is not its seed's");
    return { signing: await privateKey(seed, false), publicKey: derived };
  }
  const k = (key as WebCryptoPublisherKey).privateKey;
  if (k?.algorithm?.name !== 'Ed25519' || k.type !== 'private' || !k.usages.includes('sign')) {
    throw new Error('the publisher key is not an Ed25519 signing key');
  }
  return { signing: k, publicKey };
}

/**
 * A signed compute-only `.mpk`: `manifest.json`, `compute.wasm` and the pipelines, as a tar.gz.
 * Refuses what core's node refuses at install, with core's messages, and checks its own output
 * before returning. It cannot check what the node compiles: that the component declares
 * compute methods and that the pipelines fit them.
 */
export async function packComputePackage(input: ComputePackageInput, key: PackKey): Promise<Uint8Array> {
  // Everything up to the first await copies and validates, so the caller's buffers cannot change under us.
  const seed = 'seed' in key && key.seed ? Uint8Array.from(key.seed) : undefined;
  const publicKey = Uint8Array.from(key.publicKey);
  const job = prepare(input);

  await requireEd25519();
  const { signing, publicKey: pub } = await signerOf(key, seed, publicKey);
  const manifest: { [key: string]: Json } = { ...job.manifest, signerId: signerIdOf({ publicKey: pub }) };
  const signature = await crypto.subtle.sign('Ed25519', signing, buf(await signingPayload(manifest)));
  manifest.signature = { algorithm: 'ed25519', publicKey: b64url(pub), signature: b64url(new Uint8Array(signature)) };

  // The archived manifest is the canonical form of the signed document, never a second serialization.
  const manifestBytes = utf8(jcs(manifest));
  if (manifestBytes.length > MAX_MANIFEST_BYTES) {
    throw new Error(`bundle manifest.json exceeds the ${MAX_MANIFEST_BYTES} byte cap`);
  }
  const files: [string, Uint8Array][] = [['manifest.json', manifestBytes], ['compute.wasm', job.compute], ...job.files];
  checkArchiveSize(files.map(([, b]) => b.length));
  const tar = concat(...files.map(([n, b]) => tarEntry(n, b)), new Uint8Array(1024));
  await checkPackage(tar, pub);
  const mpk = await gzip(tar);
  if (mpk.length > MAX_MPK_BYTES) throw new Error(`a package is limited to ${MAX_MPK_BYTES >> 20} MiB`);
  return mpk;
}

function checkArchiveSize(sizes: number[]): void {
  if (sizes.reduce((n, s) => n + padded(s), 1024) > MAX_ARCHIVE_BYTES) {
    throw new Error(`bundle archive exceeds the ${MAX_ARCHIVE_BYTES} byte decompressed cap`);
  }
}

// Validates the input and returns copies of what it archives, with the unsigned manifest.
function prepare(input: ComputePackageInput) {
  checkPathComponent(input.package, 'package');
  checkPathComponent(input.version, 'appVersion');
  if (!SEMVER.test(input.version)) throw new Error(`appVersion '${input.version}' is not a semantic version`);
  if (!input.compute?.length) throw new Error('the package ships no WASM');
  if (input.pipelines.length > MAX_PIPELINES_PER_PACKAGE) {
    throw new Error(`bundle declares ${input.pipelines.length} pipelines, at most ${MAX_PIPELINES_PER_PACKAGE} are allowed`);
  }
  checkArchiveSize([input.compute.length]);
  const compute = Uint8Array.from(input.compute);
  if (COMPONENT_HEADER.some((b, i) => compute[i] !== b)) {
    throw new Error('compute: not a WASM component (an SDK 0.12 core module is refused by the node)');
  }
  const files = input.pipelines.map((p, i): [string, Uint8Array] => {
    checkPipelinePath(p.path, `pipelines[${i}].path`);
    return [p.path, utf8(p.source)];
  });
  const big = files.find(([, b]) => b.length > MAX_PIPELINE_BYTES);
  if (big) throw new Error(`pipeline '${big[0]}' exceeds the ${MAX_PIPELINE_BYTES} byte cap`);
  const lower = files.map(([n]) => n.toLowerCase());
  if (new Set(lower).size !== files.length) throw new Error('two pipelines have the same path');
  const metadata = input.metadata === undefined ? undefined : normalizeMetadata(input.metadata);
  const manifest: { [key: string]: Json } = {
    version: '1.0',
    package: input.package,
    appVersion: input.version,
    minRuntimeVersion: MIN_RUNTIME_VERSION,
    ...(metadata ? { metadata } : {}),
    compute: artifact('compute.wasm', compute),
    ...(files.length ? { pipelines: files.map(([n, b]) => artifact(n, b)) } : {}),
  };
  return { manifest, compute, files };
}
