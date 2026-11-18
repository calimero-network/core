import { spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { gunzipSync } from 'node:zlib';
import { sha256 } from '@noble/hashes/sha2.js';
import { afterEach, describe, expect, it, vi } from 'vitest';

import * as root from '../index.js';

import { privateKey, buf } from '../client/recovery-key.js';
import { b64url } from '../runtime/passkey.js';
import {
  MAX_ARCHIVE_BYTES,
  MAX_MANIFEST_BYTES,
  MAX_MPK_BYTES,
  MAX_PIPELINE_BYTES,
  MAX_PIPELINES_PER_PACKAGE,
  canonicalizeManifest,
  checkPackage,
  exportPublisherKey,
  generatePublisherKey,
  importPublisherKey,
  isDevPublisherKey,
  packComputePackage,
  signerIdOf,
  signingPayload,
  verifyManifestSignature,
  webCryptoPublisherKey,
} from './pack.js';

const fx = (name: string) => readFileSync(new URL(`./fixtures/${name}`, import.meta.url));
const json = (name: string) => JSON.parse(fx(name).toString('utf8'));
const hex = (b: Uint8Array) => Buffer.from(b).toString('hex');
const SEED = Uint8Array.from({ length: 32 }, (_, i) => i);
const key = () => importSeed(Buffer.from(SEED).toString('hex'));

const compute = new Uint8Array(fx('compute.wasm'));
const pipelines = [
  { path: 'pipelines/double.yaml', source: fx('double.yaml').toString('utf8') },
  { path: 'pipelines/triple.yaml', source: fx('triple.yaml').toString('utf8') },
];
const base = { package: 'com.example.pipeline', version: '1.2.3', compute, pipelines };

/** The entries of a .mpk: gunzip, then walk the tar's 512-byte blocks. */
function entries(mpk: Uint8Array): Map<string, Buffer> {
  const tar = gunzipSync(mpk);
  const out = new Map<string, Buffer>();
  for (let off = 0; off + 512 <= tar.length && tar[off] !== 0; ) {
    const name = tar.subarray(off, off + 100).toString('utf8').replace(/\0.*$/, '');
    const size = parseInt(tar.subarray(off + 124, off + 136).toString('utf8').replace(/\0.*$/, '').trim(), 8);
    out.set(name, tar.subarray(off + 512, off + 512 + size));
    off += 512 + Math.ceil(size / 512) * 512;
  }
  return out;
}

const manifestOf = (mpk: Uint8Array) => JSON.parse(entries(mpk).get('manifest.json')!.toString('utf8'));

describe('conformance with core', () => {
  for (const [name, vectors, extra] of [
    ['a compute-only package', 'core-vectors.json', {}],
    ['a package with metadata', 'core-vectors-metadata.json', { metadata: json('core-vectors-metadata.json').unsignedManifest.metadata }],
  ] as const) {
    it(`signs ${name} exactly as core does`, async () => {
      const v = json(vectors);
      const k = await importSeed(v.seedHex);
      const mpk = await packComputePackage({ ...base, ...extra }, k);
      const manifest = manifestOf(mpk);
      expect(manifest).toEqual(v.signedManifest);
      const { signature: _s, ...signed } = manifest;
      expect(new TextDecoder().decode(canonicalizeManifest(signed))).toBe(v.canonicalUtf8);
      expect(hex(await signingPayload(signed))).toBe(v.payloadHex);
    });
  }

  it('derives the signer id core derives', async () => {
    expect(signerIdOf(await key())).toBe(json('core-vectors.json').signerId);
  });

  it('ships the compute component and each pipeline byte for byte', async () => {
    const e = entries(await packComputePackage(base, await key()));
    expect([...e.keys()]).toEqual(['manifest.json', 'compute.wasm', 'pipelines/double.yaml', 'pipelines/triple.yaml']);
    expect(Buffer.compare(e.get('compute.wasm')!, Buffer.from(compute))).toBe(0);
  });

  it('builds the archive core accepted (recorded verdict)', async () => {
    const recorded = entries(new Uint8Array(fx('js-built.mpk')));
    const built = entries(await packComputePackage(base, await key()));
    expect([...built.keys()]).toEqual([...recorded.keys()]);
    for (const [name, bytes] of recorded) expect(Buffer.compare(built.get(name)!, bytes), name).toBe(0);
    const verdicts = json('core-verdicts.json');
    expect(verdicts['js-built.mpk'].accepted).toBe(true);
    expect(verdicts['js-built.mpk'].detail.computeOnly).toBe(true);
    expect(verdicts['js-built.mpk'].detail.signerId).toBe(json('core-vectors.json').signerId);
  });

  // CORE_PACK_VERIFIER: the pack-vectors binary; runs core's real verifier on this build's output.
  it.skipIf(!process.env.CORE_PACK_VERIFIER)('is accepted by core live', async () => {
    const path = join(mkdtempSync(join(tmpdir(), 'mpk-')), 'x.mpk');
    writeFileSync(path, await packComputePackage(base, await key()));
    const r = spawnSync(process.env.CORE_PACK_VERIFIER!, ['verify', path], { encoding: 'utf8' });
    expect(JSON.parse(r.stdout).accepted, r.stdout).toBe(true);
  });
});

async function importSeed(seedHex: string) {
  const seed = Uint8Array.from(Buffer.from(seedHex, 'hex'));
  return importPublisherKey(new TextEncoder().encode(JSON.stringify({ private_key: Buffer.from(seed).toString('base64url') })));
}

describe('tampering', () => {
  it('fails the signature when a signed field changes', async () => {
    const manifest = manifestOf(await packComputePackage(base, await key()));
    expect(await verifyManifestSignature(manifest)).toBe(signerIdOfSync(manifest));
    for (const edit of [
      (m: any) => (m.appVersion = '9.9.9'),
      (m: any) => (m.pipelines[0].hash = 'f'.repeat(64)),
      (m: any) => (m.compute.size += 1),
      (m: any) => (m.extra = 1),
    ]) {
      const t = structuredClone(manifest);
      edit(t);
      await expect(verifyManifestSignature(t)).rejects.toThrow(/signature/);
    }
  });

  it('refuses a manifest whose signer id is not its key', async () => {
    const manifest = manifestOf(await packComputePackage(base, await key()));
    manifest.signerId = 'did:key:z6MknF3p5L5FDHJQ7FREUapuX4Wmp4MtF6WrHYaXS2B3eZQd';
    const payload = await signingPayload(manifest);
    const sig = await crypto.subtle.sign('Ed25519', await privateKey(SEED, false), buf(payload));
    manifest.signature.signature = b64url(new Uint8Array(sig));
    await expect(verifyManifestSignature(manifest)).rejects.toThrow(/signerId mismatch/);
  });

  it('ignores signature and underscore fields when canonicalizing, as core does', () => {
    const a = canonicalizeManifest({ b: 1, a: 'x', signature: { x: 1 }, _binary: 'y' });
    expect(new TextDecoder().decode(a)).toBe('{"a":"x","b":1}');
  });

  it('is refused by core when the manifest or an archive file is altered (recorded verdicts)', () => {
    const verdicts = json('core-verdicts.json');
    expect(verdicts['tampered-manifest.mpk'].accepted).toBe(false);
    expect(verdicts['tampered-manifest.mpk'].error).toMatch(/signature verification failed/);
    expect(verdicts['tampered-archive.mpk'].accepted).toBe(false);
    expect(verdicts['tampered-archive.mpk'].error).toMatch(/integrity check/);
  });
});

describe('limits', () => {
  const pack = (over: object, k?: Awaited<ReturnType<typeof key>>) => async () =>
    packComputePackage({ ...base, ...over } as never, k ?? (await key()));

  it('refuses a pipeline over the byte cap, and takes one at the cap', async () => {
    const at = { path: 'pipelines/big.yaml', source: 'a'.repeat(MAX_PIPELINE_BYTES) };
    await expect(pack({ pipelines: [at] })()).resolves.toBeInstanceOf(Uint8Array);
    const over = { path: 'pipelines/big.yaml', source: 'a'.repeat(MAX_PIPELINE_BYTES + 1) };
    await expect(pack({ pipelines: [over] })()).rejects.toThrow(
      `pipeline 'pipelines/big.yaml' exceeds the ${MAX_PIPELINE_BYTES} byte cap`,
    );
  });

  it('counts a pipeline in bytes, not characters', async () => {
    const source = '\u00e9'.repeat(MAX_PIPELINE_BYTES / 2 + 1);
    await expect(pack({ pipelines: [{ path: 'pipelines/u.yaml', source }] })()).rejects.toThrow(/byte cap/);
  });

  it('refuses more pipelines than a package may declare', async () => {
    const many = (n: number) => Array.from({ length: n }, (_, i) => ({ path: `pipelines/p${i}.yaml`, source: 'a: 1' }));
    await expect(pack({ pipelines: many(MAX_PIPELINES_PER_PACKAGE) })()).resolves.toBeInstanceOf(Uint8Array);
    await expect(pack({ pipelines: many(MAX_PIPELINES_PER_PACKAGE + 1) })()).rejects.toThrow(
      `bundle declares ${MAX_PIPELINES_PER_PACKAGE + 1} pipelines, at most ${MAX_PIPELINES_PER_PACKAGE} are allowed`,
    );
  });

  it.each([
    ['', /pipelines\[0\]\.path is empty/],
    ['/pipelines/a.yaml', /is an absolute path/],
    ['pipelines\\a.yaml', /contains backslash/],
    ['pipelines/../a.yaml', /path traversal component '\.\.'/],
    ['C:/a.yaml', /absolute Windows path/],
    ['pipelines/a\0.yaml', /null byte/],
    ['a.yaml', /must be pipelines\/<name>\.yaml/],
    ['pipelines/a.txt', /must be pipelines\/<name>\.yaml/],
    ['pipelines/sub/a.yaml', /must be pipelines\/<name>\.yaml/],
    ['pipelines/manifest.json.yaml/', /must be pipelines\/<name>\.yaml/],
  ])('refuses the pipeline path %j', async (path, message) => {
    await expect(pack({ pipelines: [{ path, source: 'a: 1' }] })()).rejects.toThrow(message);
  });

  it('refuses two pipelines at one path', async () => {
    const p = { path: 'pipelines/a.yaml', source: 'a: 1' };
    await expect(pack({ pipelines: [p, p] })()).rejects.toThrow(/same path/);
  });

  it('refuses a missing compute component and a core module', async () => {
    await expect(pack({ compute: undefined })()).rejects.toThrow('the package ships no WASM');
    await expect(pack({ compute: new Uint8Array() })()).rejects.toThrow('the package ships no WASM');
    await expect(pack({ compute: Uint8Array.from([0, 0x61, 0x73, 0x6d, 1, 0, 0, 0]) })()).rejects.toThrow(
      /not a WASM component/,
    );
  });

  it.each([
    ['package', 'a/b', /package contains directory separator/],
    ['package', '', /package is empty/],
    ['version', '1..2', /appVersion contains path traversal sequence/],
    ['version', '', /appVersion is empty/],
  ])('refuses a bad %s %j', async (field, value, message) => {
    await expect(pack({ [field]: value })()).rejects.toThrow(message);
  });

  it('refuses a manifest over the byte cap', async () => {
    const metadata = { name: 'n', guide: 'g'.repeat(MAX_MANIFEST_BYTES) };
    await expect(pack({ metadata })()).rejects.toThrow(`bundle manifest.json exceeds the ${MAX_MANIFEST_BYTES} byte cap`);
  });

  it('refuses a non-integer number or a lone surrogate in the signed document', () => {
    expect(() => canonicalizeManifest({ a: 1.5 })).toThrow(/integer/);
    expect(() => canonicalizeManifest({ a: '\ud800' })).toThrow(/surrogate/);
  });
});

describe('the publisher key', () => {
  it('is generated fresh and signs', async () => {
    const [a, b] = [await generatePublisherKey(), await generatePublisherKey()];
    expect(hex(a.seed)).not.toBe(hex(b.seed));
    expect(signerIdOf(a)).toMatch(/^did:key:z6Mk/);
    await expect(packComputePackage(base, a)).resolves.toBeInstanceOf(Uint8Array);
  });

  it('round-trips through its backup, in the shape mero-sign reads', async () => {
    const k = await generatePublisherKey();
    const bytes = exportPublisherKey(k);
    const parsed = JSON.parse(new TextDecoder().decode(bytes));
    expect(Object.keys(parsed).sort()).toEqual(['private_key', 'public_key', 'signer_id']);
    expect(parsed.private_key).toBe(Buffer.from(k.seed).toString('base64url'));
    const back = await importPublisherKey(bytes);
    expect(hex(back.seed)).toBe(hex(k.seed));
    expect(signerIdOf(back)).toBe(parsed.signer_id);
  });

  it.each([
    ['not json', 'nope'],
    ['a short seed', JSON.stringify({ private_key: 'AAAA' })],
    ['a signer id that is not the seed\'s', JSON.stringify({ private_key: Buffer.alloc(32).toString('base64url'), signer_id: 'did:key:zX' })],
    ['a public key that is not the seed\'s', JSON.stringify({ private_key: Buffer.alloc(32).toString('base64url'), public_key: Buffer.alloc(32, 1).toString('base64url') })],
  ])('refuses a backup with %s', async (_n, text) => {
    await expect(importPublisherKey(new TextEncoder().encode(text))).rejects.toThrow();
  });
});

function signerIdOfSync(manifest: { signerId: string }) {
  return manifest.signerId;
}

const tarOf = (mpk: Uint8Array) => new Uint8Array(gunzipSync(mpk));
const publicOf = async () => (await key()).publicKey;

describe('the signed manifest is the archived manifest', () => {
  it.each([
    ['a toJSON that lies', { name: 'n', toJSON: () => 'evil' }, /unknown field 'toJSON'/],
    ['a Date', new Date(0), /name is required/],
    ['an undefined name', { name: undefined }, /name is required/],
    ['a numeric name', { name: 1 }, /name must be a string/],
    ['tags that are not an array', { name: 'n', tags: 'a' }, /tags must be an array of strings/],
    ['a non-string tag', { name: 'n', tags: ['a', 2] }, /tags must be an array of strings/],
    ['an unknown field', { name: 'n', extra: 'x' }, /unknown field 'extra'/],
    ['a lone surrogate', { name: '\ud800' }, /lone surrogate/],
  ])('refuses metadata with %s', async (_n, metadata, message) => {
    await expect(packComputePackage({ ...base, metadata } as never, await key())).rejects.toThrow(message);
  });

  it('archives the canonical bytes it signed', async () => {
    const mpk = await packComputePackage({ ...base, metadata: { name: 'n', tags: ['b', 'a'] } }, await key());
    const text = entries(mpk).get('manifest.json')!.toString('utf8');
    const sorted = (v: any): any =>
      Array.isArray(v) ? v.map(sorted) : v && typeof v === 'object' ? Object.fromEntries(Object.keys(v).sort().map((k) => [k, sorted(v[k])])) : v;
    expect(text).toBe(JSON.stringify(sorted(JSON.parse(text))));
  });

  it('checks the archive it built: digests, file set, signer and duplicates', async () => {
    const tar = tarOf(await packComputePackage(base, await key()));
    await expect(checkPackage(tar, await publicOf())).resolves.toBeUndefined();
    const flip = (needle: string, to: string) => {
      const c = Buffer.from(tar);
      const i = c.indexOf(needle);
      Buffer.from(to).copy(c, i);
      return new Uint8Array(c);
    };
    await expect(checkPackage(flip('fixture compute', 'fixtuRe compute'), await publicOf())).rejects.toThrow(/integrity check/);
    await expect(checkPackage(tar, (await generatePublisherKey()).publicKey)).rejects.toThrow(/not signed by the publisher key/);
    await expect(checkPackage(tar.subarray(0, 1024 + 512 * 2), await publicOf())).rejects.toThrow(/exactly the files/);
  });

  it('refuses an archive with a second manifest.json', async () => {
    const tar = Buffer.from(tarOf(await packComputePackage(base, await key())));
    const len = 512 + Math.ceil(entries(await packComputePackage(base, await key())).get('manifest.json')!.length / 512) * 512;
    const doubled = new Uint8Array(Buffer.concat([tar.subarray(0, len), tar]));
    await expect(checkPackage(doubled, await publicOf())).rejects.toThrow(/more than one entry at 'manifest.json'/);
  });
});

describe('what the caller hands in', () => {
  it('is copied before any await, so a later write cannot split the hash from the archive', async () => {
    const mine = Uint8Array.from(compute);
    const sources = [{ path: 'pipelines/a.yaml', source: 'a: 1' }];
    const pending = packComputePackage({ ...base, compute: mine, pipelines: sources }, await key());
    mine[20] ^= 0xff;
    sources[0].source = 'b: 2';
    const e = entries(await pending);
    expect(Buffer.compare(e.get('compute.wasm')!, Buffer.from(compute))).toBe(0);
    expect(e.get('pipelines/a.yaml')!.toString()).toBe('a: 1');
  });

  it('refuses a key whose publicKey is not its seed\'s', async () => {
    const k = await key();
    const other = await generatePublisherKey();
    await expect(packComputePackage(base, { seed: k.seed, publicKey: other.publicKey })).rejects.toThrow(/not its seed's/);
  });
});

describe('size bounds', () => {
  it('refuses a compute component past the decompressed cap before copying it', async () => {
    const huge = new Uint8Array(MAX_ARCHIVE_BYTES + 1);
    await expect(packComputePackage({ ...base, compute: huge }, await key())).rejects.toThrow(
      `bundle archive exceeds the ${MAX_ARCHIVE_BYTES} byte decompressed cap`,
    );
  }, 60_000);

  it('refuses a package past what a node takes, with core\'s wording', async () => {
    const noise = new Uint8Array(MAX_MPK_BYTES + 1024 * 1024);
    let x = 0x9e3779b9;
    const words = new Uint32Array(noise.buffer);
    for (let i = 0; i < words.length; i++) {
      x ^= x << 13; x >>>= 0; x ^= x >>> 17; x ^= x << 5; x >>>= 0;
      words[i] = x;
    }
    noise.set([0, 0x61, 0x73, 0x6d, 0x0d, 0, 0x01, 0]);
    await expect(packComputePackage({ ...base, compute: noise }, await key())).rejects.toThrow(
      `a package is limited to ${MAX_MPK_BYTES >> 20} MiB`,
    );
  }, 60_000);
});

describe('a non-extractable key', () => {
  it('signs the same package as the seed does', async () => {
    const k = await key();
    const web = await webCryptoPublisherKey(k);
    expect(web.privateKey.extractable).toBe(false);
    const [a, b] = [await packComputePackage(base, k), await packComputePackage(base, web)];
    const [ea, eb] = [entries(a), entries(b)];
    for (const [name, bytes] of ea) expect(Buffer.compare(eb.get(name)!, bytes), name).toBe(0);
    await checkPackage(tarOf(b), k.publicKey);
  });

  it('is checked against the public key it is given', async () => {
    const web = await webCryptoPublisherKey(await key());
    await expect(packComputePackage(base, { ...web, publicKey: (await generatePublisherKey()).publicKey })).rejects.toThrow(
      /signature verification failed/,
    );
  });

  it('refuses a key that is not an Ed25519 signing key', async () => {
    const hmac = await crypto.subtle.generateKey({ name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
    await expect(packComputePackage(base, { privateKey: hmac, publicKey: new Uint8Array(32) })).rejects.toThrow(
      /not an Ed25519 signing key/,
    );
  });
});

describe('a runtime without Ed25519', () => {
  afterEach(() => vi.unstubAllGlobals());
  const message = /this runtime cannot sign Ed25519 packages/;

  it('says so when there is no WebCrypto', async () => {
    const k = await key();
    vi.stubGlobal('crypto', undefined);
    await expect(packComputePackage(base, k)).rejects.toThrow(message);
    await expect(generatePublisherKey()).rejects.toThrow(message);
  });

  it('says so when WebCrypto lacks Ed25519', async () => {
    const k = await key();
    vi.stubGlobal('crypto', { subtle: { importKey: () => Promise.reject(new Error('NotSupportedError')) } });
    await expect(packComputePackage(base, k)).rejects.toThrow(message);
    await expect(verifyManifestSignature({})).rejects.toThrow(message);
  });
});

describe('the package root', () => {
  it('does not export the loose manifest verifier, and does export the dev key check', () => {
    expect((root as Record<string, unknown>).verifyManifestSignature).toBeUndefined();
    expect(root.isDevPublisherKey).toBeTypeOf('function');
    expect(root.packComputePackage).toBeTypeOf('function');
  });
});

describe('a backup is strict', () => {
  const backup = (seed: string, extra: object = {}) => new TextEncoder().encode(JSON.stringify({ private_key: seed, ...extra }));
  const good = Buffer.from(SEED).toString('base64url');

  it.each([
    ['padding', `${good}=`],
    ['standard base64', Buffer.from(SEED).toString('base64').replace(/=+$/, '').replace(/-/g, '+') + '+'],
    ['a 42 character seed', good.slice(0, 42)],
    ['non-canonical trailing bits', `${good.slice(0, 42)}${'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_'[('ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_'.indexOf(good[42]) + 1) % 64]}`],
    ['whitespace', ` ${good}`],
  ])('refuses %s without echoing the seed', async (_n, seed) => {
    const error = await importPublisherKey(backup(seed)).catch((e) => e);
    expect(error).toBeInstanceOf(Error);
    expect(error.message).toMatch(/unpadded base64url 32-byte seed/);
    expect(error.message).not.toContain(seed.trim());
  });

  it('ignores other fields, as mero-sign does', async () => {
    const k = await importPublisherKey(backup(good, { note: 'x' }));
    expect(hex(k.seed)).toBe(hex(SEED));
  });
});

describe('names', () => {
  const pack = async (over: object) => packComputePackage({ ...base, ...over } as never, await key());

  it.each(['pipelines/a b.yaml', 'pipelines/é.yaml', 'pipelines/a:b.yaml', 'pipelines/a*.yaml'])('refuses the file name %j', async (path) => {
    await expect(pack({ pipelines: [{ path, source: 'a: 1' }] })).rejects.toThrow(/must be pipelines\/<name>\.yaml/);
  });

  it('refuses two names that differ only in case', async () => {
    const p = (path: string) => ({ path, source: 'a: 1' });
    await expect(pack({ pipelines: [p('pipelines/A.yaml'), p('pipelines/a.yaml')] })).rejects.toThrow(/same path/);
  });

  it.each(['v1.0.0', '1.0', '01.0.0', '1.0.0.0', '1.0.0-', 'latest'])('refuses the version %j', async (version) => {
    await expect(pack({ version })).rejects.toThrow(/not a semantic version/);
  });

  it.each(['1.0.0', '0.0.1-rc.1', '2.3.4+build.5', '1.0.0-alpha.1+b'])('takes the version %j', async (version) => {
    await expect(pack({ version })).resolves.toBeInstanceOf(Uint8Array);
  });
});

describe('the development key', () => {
  it('is recognised by its signer id and its published seed', async () => {
    const dev = await importSeed(Buffer.from(sha256(new TextEncoder().encode('calimero-dev-signing-key-v1'))).toString('hex'));
    expect(isDevPublisherKey(dev)).toBe(true);
    expect(signerIdOf(dev)).toBe('did:key:z6MknF3p5L5FDHJQ7FREUapuX4Wmp4MtF6WrHYaXS2B3eZQd');
    expect(isDevPublisherKey(await key())).toBe(false);
    await expect(packComputePackage(base, dev)).resolves.toBeInstanceOf(Uint8Array);
  });
});
