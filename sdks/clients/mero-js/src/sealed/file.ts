/**
 * Opening a sealed file, byte for byte as core's `calimero-sealed` crate
 * (`file.rs`, `wrap.rs`) writes it: 64 KiB AES-256-GCM STREAM chunks under a
 * data key, cut into segments, and that key wrapped to an X25519 key the
 * requester made for one export.
 */

import { sha256 as nobleSha256 } from '@noble/hashes/sha2.js';
import { concat, hex } from '../crypto/internal.js';
import { dh, type X25519KeyPair } from './noise.js';

const CHUNK_BYTES = 64 * 1024; // plaintext bytes in every chunk but the last
const TAG_BYTES = 16;
const SEALED_CHUNK_BYTES = CHUNK_BYTES + TAG_BYTES;
/** Stored bytes of every segment but the last: 256 whole sealed chunks. */
export const SEGMENT_BYTES = 256 * SEALED_CHUNK_BYTES;
const NONCE_BYTES = 12;
const enc = new TextEncoder();
const AAD_PREFIX = enc.encode('calimero.file.v1'); // then the last-chunk flag byte
const SEGMENT_DOMAIN = enc.encode('calimero.file.segments.v1\0'); // prefix of a file id's preimage
const EXPORT_WRAP_SALT = enc.encode('calimero.export-wrap.v1');
const DERIVE_LABEL = enc.encode('calimero.sealed.derive.v1\0'); // prefix of every wrap's HKDF info

/** Why an export was refused locally. A node's own refusal stays the transport's typed error. */
export type ExportErrorCode = 'BAD_ANSWER' | 'NOT_THE_FILE' | 'TAMPERED' | 'TOO_LARGE' | 'ABORTED';

/** An export that failed a check here, never one the node refused; messages carry no plaintext or keys. */
export class ExportError extends Error {
  constructor(
    readonly code: ExportErrorCode,
    message: string,
  ) {
    super(message);
    this.name = 'ExportError';
  }
}

const sha256 = async (data: Uint8Array): Promise<Uint8Array> =>
  new Uint8Array(await crypto.subtle.digest('SHA-256', data));

/** The file id: SHA-256 over the domain, the segment count (u32 big-endian) and the ordered segment hashes. */
export function fileId(segments: Uint8Array[]): Promise<Uint8Array> {
  const count = new Uint8Array(4);
  new DataView(count.buffer).setUint32(0, segments.length, false);
  return sha256(concat(SEGMENT_DOMAIN, count, ...segments));
}

/** What an export wrap is bound to: the app and the file (hex id). */
export interface ExportWrap {
  appId: string;
  fileId: string;
}

function prefixed(field: string): Uint8Array {
  const bytes = enc.encode(field);
  const length = new Uint8Array(4);
  new DataView(length.buffer).setUint32(0, bytes.length, false);
  return concat(length, bytes);
}

/** Open the data key `wrapped` (`nonce || AES-256-GCM`) that the node wrapped to `recipient` for `wrap`. */
export async function unwrapExport(
  recipient: X25519KeyPair,
  ephemeral: Uint8Array,
  wrap: ExportWrap,
  wrapped: Uint8Array,
): Promise<Uint8Array> {
  if (wrapped.length < NONCE_BYTES) throw new ExportError('TAMPERED', 'the wrapped key is truncated');
  const shared = await dh(recipient.privateKey, ephemeral);
  if (shared.every((b) => b === 0)) throw new ExportError('BAD_ANSWER', 'the ephemeral key is of low order');
  const info = concat(ephemeral, recipient.publicKey, prefixed(wrap.appId), prefixed(wrap.fileId));
  const ikm = await crypto.subtle.importKey('raw', shared, 'HKDF', false, ['deriveBits']);
  const keyBytes = await crypto.subtle.deriveBits(
    { name: 'HKDF', hash: 'SHA-256', salt: EXPORT_WRAP_SALT, info: concat(DERIVE_LABEL, info) },
    ikm,
    256,
  );
  const key = await crypto.subtle.importKey('raw', keyBytes, 'AES-GCM', false, ['decrypt']);
  let secret: ArrayBuffer;
  try {
    secret = await crypto.subtle.decrypt(
      { name: 'AES-GCM', iv: wrapped.subarray(0, NONCE_BYTES) },
      key,
      wrapped.subarray(NONCE_BYTES),
    );
  } catch {
    throw new ExportError('TAMPERED', 'the wrapped key does not open');
  }
  if (secret.byteLength !== 32) throw new ExportError('TAMPERED', 'the wrapped key has the wrong length');
  return new Uint8Array(secret);
}

/**
 * Joins a file's segments in order, each checked against the list before it is
 * opened; holds at most one segment and one chunk. Output is authentic but may
 * be a prefix of the file until {@link finishExpecting} returns.
 */
export class SegmentReader {
  private pending: Uint8Array = new Uint8Array();
  private index = 0;
  private next = 0;
  private size = 0;
  private readonly hash = nobleSha256.create();

  private constructor(
    private readonly key: CryptoKey,
    private readonly segments: string[],
  ) {}

  static async create(dataKey: Uint8Array, segments: Uint8Array[]): Promise<SegmentReader> {
    const key = await crypto.subtle.importKey('raw', dataKey, 'AES-GCM', false, ['decrypt']);
    return new SegmentReader(key, segments.map(hex));
  }

  /** Take the next segment; `emit` gets the plaintext of every chunk it completes. */
  async push(segment: Uint8Array, emit: (plaintext: Uint8Array) => Promise<void> | void): Promise<void> {
    const want = this.segments[this.next];
    if (want === undefined) throw new ExportError('TAMPERED', `the file has no segment ${this.next}`);
    if (this.next + 1 < this.segments.length && segment.length !== SEGMENT_BYTES) {
      throw new ExportError('TAMPERED', `segment ${this.next} is not a whole segment`);
    }
    if (hex(await sha256(segment)) !== want) {
      throw new ExportError('TAMPERED', `segment ${this.next} is not the one the file lists`);
    }
    this.next += 1;
    // Bytes past a full chunk prove it was not the last, so one chunk is always held back.
    const data = concat(this.pending, segment);
    let at = 0;
    while (data.length - at > SEALED_CHUNK_BYTES) {
      await emit(await this.open(data.subarray(at, at + SEALED_CHUNK_BYTES), false));
      at += SEALED_CHUNK_BYTES;
    }
    this.pending = data.slice(at);
  }

  /** Open the last chunk and check the whole plaintext's size and SHA-256: GCM does not commit to its key. */
  async finishExpecting(size: number, sha256Hex: string): Promise<Uint8Array> {
    if (this.next !== this.segments.length) throw new ExportError('TAMPERED', 'the file is missing segments');
    if (this.pending.length < TAG_BYTES) throw new ExportError('TAMPERED', 'the file is truncated');
    const last = await this.open(this.pending, true);
    if (this.size !== size || hex(this.hash.digest()) !== sha256Hex) {
      throw new ExportError('TAMPERED', 'the file is not the one that was sealed');
    }
    return last;
  }

  private async open(chunk: Uint8Array, last: boolean): Promise<Uint8Array> {
    const iv = new Uint8Array(NONCE_BYTES);
    new DataView(iv.buffer).setBigUint64(4, BigInt(this.index), false);
    let plain: Uint8Array;
    try {
      plain = new Uint8Array(
        await crypto.subtle.decrypt(
          { name: 'AES-GCM', iv, additionalData: concat(AAD_PREFIX, new Uint8Array([last ? 1 : 0])) },
          this.key,
          chunk,
        ),
      );
    } catch {
      throw new ExportError(
        'TAMPERED',
        `chunk ${this.index} does not open: the file is altered, truncated or under another key`,
      );
    }
    this.hash.update(plain);
    this.size += plain.length;
    this.index += 1;
    return plain;
  }
}
