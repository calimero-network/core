import { describe, expect, it } from 'vitest';

import { fromHex, fromHexUnsized, hex } from '../crypto/internal.js';
import { SEALED_FILE_MULTI_SEGMENT_VECTORS as VECTORS } from './fixtures/sealed-file-multi-segment-vectors.js';
import { SEGMENT_BYTES, SegmentReader, fileId } from './file.js';

const CHUNK = 65536;
const aad = (last: boolean) => new Uint8Array([...new TextEncoder().encode('calimero.file.v1'), last ? 1 : 0]);
const sha = async (data: Uint8Array) => hex(new Uint8Array(await crypto.subtle.digest('SHA-256', data)));

/** The sealed stream the Rust writer makes: whole chunks, and `more` seals a full final chunk as not last. */
async function seal(key: Uint8Array, len: number, more: boolean): Promise<Uint8Array> {
  const plain = Uint8Array.from({ length: len }, (_, i) => i % 251);
  const aes = await crypto.subtle.importKey('raw', key, 'AES-GCM', false, ['encrypt']);
  const chunks = Math.max(1, Math.ceil(len / CHUNK) + (more && len % CHUNK === 0 ? 1 : 0));
  const out: Uint8Array[] = [];
  for (let i = 0; i < chunks; i += 1) {
    const iv = new Uint8Array(12);
    new DataView(iv.buffer).setBigUint64(4, BigInt(i), false);
    const piece = plain.subarray(i * CHUNK, Math.min(len, (i + 1) * CHUNK));
    out.push(new Uint8Array(await crypto.subtle.encrypt({ name: 'AES-GCM', iv, additionalData: aad(i === chunks - 1) }, aes, piece)));
  }
  return Uint8Array.from(out.flatMap((c) => [...c]));
}

describe('multi-segment files match the Rust reference', () => {
  for (const v of VECTORS) {
    it(v.name, async () => {
      const key = fromHexUnsized(v.key, 'key');
      const all = await seal(key, v.len, v.more);
      const segments: Uint8Array[] = [];
      for (let at = 0; at < all.length; at += SEGMENT_BYTES) segments.push(all.subarray(at, at + SEGMENT_BYTES));
      expect(await Promise.all(segments.map(sha))).toEqual(v.segments);
      expect(segments.at(-1)?.length).toBe(v.lastLen);
      expect(hex(await fileId(v.segments.map((h) => fromHex(h, 'hash', 32))))).toBe(v.id);

      const reader = await SegmentReader.create(key, v.segments.map((h) => fromHex(h, 'hash', 32)));
      const plain: number[] = [];
      for (const segment of segments) await reader.push(segment, (c) => void c.forEach((b) => plain.push(b)));
      (await reader.finishExpecting(v.size, v.sha256)).forEach((b) => plain.push(b));
      expect(plain).toHaveLength(v.len);
      expect(plain.every((b, i) => b === i % 251)).toBe(true);
    }, 60_000);
  }
});
