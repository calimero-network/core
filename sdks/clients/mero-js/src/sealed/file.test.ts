import { describe, expect, it } from 'vitest';

import { fromHex, fromHexUnsized, hex } from '../crypto/internal.js';
import { SEALED_FILE_VECTORS as V } from './fixtures/sealed-file-vectors.js';
import { x25519KeyPair } from './noise.js';
import { SEGMENT_BYTES, SegmentReader, fileId, unwrapExport } from './file.js';

const sha = async (data: Uint8Array) => new Uint8Array(await crypto.subtle.digest('SHA-256', data));
const bytes = (value: string) => fromHexUnsized(value, 'vector');
const [three, full] = V.vectors;

async function read(vector: (typeof V.vectors)[number], segment: Uint8Array, list?: Uint8Array[]) {
  const reader = await SegmentReader.create(bytes(vector.dataKey), list ?? [await sha(segment)]);
  const out: Uint8Array[] = [];
  await reader.push(segment, (chunk) => void out.push(chunk));
  return { reader, out };
}

describe('constants and ids match the Rust reference', () => {
  it('has its segment size', () => expect(SEGMENT_BYTES).toBe(V.segmentBytes));

  it('derives file ids over one to three segments, order included', async () => {
    for (const { segments, id } of V.fileIds) {
      expect(hex(await fileId(segments.map((s) => fromHex(s, 'hash', 32))))).toBe(id);
    }
  });

  it('derives the id of every vector from its segment list', async () => {
    for (const v of V.vectors) expect(hex(await fileId(v.segments.map((s) => fromHex(s, 'h', 32))))).toBe(v.id);
  });
});

describe('unwrapExport', () => {
  const recipient = (v: (typeof V.vectors)[number]) => x25519KeyPair(bytes(v.recipientSecret));
  const open = async (v: (typeof V.vectors)[number], wrap: { appId: string; fileId: string }) =>
    unwrapExport(await recipient(v), bytes(v.ephemeral), wrap, bytes(v.wrapped));

  it('opens the Rust wrap to the data key', async () => {
    for (const v of V.vectors) {
      expect(hex(await open(v, { appId: v.app, fileId: v.id }))).toBe(v.dataKey);
      expect(hex((await recipient(v)).publicKey)).toBe(v.recipientPublic);
    }
  });

  it('does not open for another app, another file id or another key', async () => {
    await expect(open(three, { appId: 'other', fileId: three.id })).rejects.toThrow('does not open');
    await expect(open(three, { appId: three.app, fileId: full.id })).rejects.toThrow('does not open');
    const stranger = await x25519KeyPair();
    await expect(
      unwrapExport(stranger, bytes(three.ephemeral), { appId: three.app, fileId: three.id }, bytes(three.wrapped)),
    ).rejects.toThrow();
  });

  it('does not open when ids differ only in where they split', async () => {
    await expect(open(three, { appId: `${three.app}\0${three.id}`, fileId: '' })).rejects.toThrow('does not open');
  });

  it('refuses a truncated, tampered or low-order wrap', async () => {
    const wrap = { appId: three.app, fileId: three.id };
    const k = await recipient(three);
    const wrapped = bytes(three.wrapped);
    await expect(unwrapExport(k, bytes(three.ephemeral), wrap, wrapped.slice(0, 5))).rejects.toThrow('truncated');
    const flipped = wrapped.slice();
    flipped[flipped.length - 1] ^= 1;
    await expect(unwrapExport(k, bytes(three.ephemeral), wrap, flipped)).rejects.toThrow('does not open');
    await expect(unwrapExport(k, new Uint8Array(32), wrap, wrapped)).rejects.toThrow();
  });
});

describe('SegmentReader', () => {
  it('opens each Rust vector to its plaintext, size and hash', async () => {
    for (const v of V.vectors) {
      const { reader, out } = await read(v, bytes(v.segment0));
      const last = await reader.finishExpecting(v.size, v.plaintextSha256);
      const plain = Uint8Array.from({ length: v.plaintextLen }, (_, i) => i % 251);
      const joined = new Uint8Array([...out.flatMap((c) => [...c]), ...last]);
      expect(joined, v.name).toEqual(plain);
    }
  });

  it('holds the last chunk back even when it is full', async () => {
    const { out } = await read(full, bytes(full.segment0));
    expect(out).toEqual([]);
  });

  it('refuses a segment that is not the one listed, before opening any of it', async () => {
    const segment = bytes(three.segment0);
    const reader = await SegmentReader.create(bytes(three.dataKey), [await sha(segment)]);
    const tampered = segment.slice();
    tampered[tampered.length - 1] ^= 1;
    const out: Uint8Array[] = [];
    await expect(reader.push(tampered, (c) => void out.push(c))).rejects.toThrow('is not the one the file lists');
    expect(out).toEqual([]);
  });

  it('refuses a short non-last segment and a segment past the list', async () => {
    const segment = bytes(three.segment0);
    const list = [await sha(segment), await sha(segment)];
    const reader = await SegmentReader.create(bytes(three.dataKey), list);
    await expect(reader.push(segment, () => {})).rejects.toThrow('not a whole segment');
    const single = await SegmentReader.create(bytes(three.dataKey), [list[0]]);
    await single.push(segment, () => {});
    await expect(single.push(segment, () => {})).rejects.toThrow('no segment 1');
  });

  it('refuses a missing segment', async () => {
    const reader = await SegmentReader.create(bytes(three.dataKey), [new Uint8Array(32), new Uint8Array(32)]);
    await expect(reader.finishExpecting(0, '')).rejects.toThrow('missing segments');
  });

  it('refuses a wrong size or plaintext hash', async () => {
    const attempt = async (size: number, sha256: string) => {
      const { reader } = await read(three, bytes(three.segment0));
      return reader.finishExpecting(size, sha256);
    };
    await attempt(three.size, three.plaintextSha256);
    await expect(attempt(three.size + 1, three.plaintextSha256)).rejects.toThrow('not the one that was sealed');
    await expect(attempt(three.size, '00'.repeat(32))).rejects.toThrow('not the one that was sealed');
  });

  it('refuses a file cut after a chunk (the last chunk was not sealed as last)', async () => {
    const sealed = 65536 + 16;
    for (const keep of [2 * sealed, sealed]) {
      const cut = bytes(three.segment0).slice(0, keep);
      const { reader } = await read(three, cut);
      await expect(reader.finishExpecting(0, '')).rejects.toThrow('does not open');
    }
  });

  it('refuses a stream shorter than one tag', async () => {
    const { reader } = await read(three, new Uint8Array(0));
    await expect(reader.finishExpecting(0, '')).rejects.toThrow('truncated');
  });

  it('refuses a dropped middle chunk', async () => {
    const sealed = 65536 + 16;
    const s = bytes(three.segment0);
    const dropped = new Uint8Array([...s.slice(0, sealed), ...s.slice(2 * sealed)]);
    const { reader } = await read(three, dropped);
    await expect(reader.finishExpecting(0, '')).rejects.toThrow('does not open');
  });
});
