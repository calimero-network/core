/**
 * Downloading a sealed file, as core's `Client::export_file_to` does: the node
 * answers only ciphertext and the file's data key wrapped to a key made here
 * for this call, and nothing it says is believed until it checks out.
 */

import { concat, fromHex, fromHexUnsized, hex } from '../crypto/internal.js';
import { ExportError, SEGMENT_BYTES, SegmentReader, fileId, unwrapExport } from '../sealed/file.js';
import { x25519KeyPair } from '../sealed/noise.js';
import { routePath, type Transport } from '../runtime/transport.js';
import type { AppsRoutes } from '../generated/methods.js';

export { ExportError, type ExportErrorCode } from '../sealed/file.js';

/** Where an export writes the plaintext, in order. A `WritableStreamDefaultWriter` fits. */
export interface FileSink {
  write(chunk: Uint8Array): Promise<void> | void;
}

/** What an export wrote, as proven against the file's mark. */
export interface ExportedFile {
  size: number;
  /** SHA-256 of the plaintext, hex. */
  sha256: string;
}

type ExportTransport = Pick<Transport, 'request'> & Pick<AppsRoutes, 'exportsExport'>;

function hexField(value: unknown, label: string, bytes?: number): Uint8Array {
  if (typeof value !== 'string') throw new ExportError('BAD_ANSWER', `the node answered a bad ${label}`);
  try {
    return bytes === undefined ? fromHexUnsized(value, label) : fromHex(value, label, bytes);
  } catch {
    throw new ExportError('BAD_ANSWER', `the node answered a bad ${label}`);
  }
}

/** The answer's fields, checked for shape; whether they name the requested file is checked after. */
function checked(answer: unknown) {
  const a = answer as Record<string, unknown> | null;
  if (typeof a !== 'object' || a === null) throw new ExportError('BAD_ANSWER', 'the node answered a bad export');
  if (typeof a.app !== 'string') throw new ExportError('BAD_ANSWER', 'the node answered a bad app');
  if (typeof a.size !== 'number' || !Number.isSafeInteger(a.size) || a.size < 0) {
    throw new ExportError('BAD_ANSWER', 'the node answered a bad size');
  }
  if (!Array.isArray(a.segments)) throw new ExportError('BAD_ANSWER', 'the node answered a bad segment list');
  return {
    app: a.app,
    size: a.size,
    named: hexField(a.id, 'file id', 32),
    segments: a.segments.map((h) => hexField(h, 'segment hash', 32)),
    sha256: hex(hexField(a.sha256, 'sha256', 32)),
    ephemeral: hexField(a.ephemeral, 'ephemeral key', 32),
    wrapped: hexField(a.wrapped, 'wrapped key'),
  };
}

const aborted = () => new ExportError('ABORTED', 'the export was aborted');

/** A segment's bytes, refusing one longer than a whole segment before buffering it, and stopping on `signal`. */
async function readSegment(response: Response, signal?: AbortSignal): Promise<Uint8Array> {
  const declared = Number(response.headers.get('content-length'));
  if (Number.isFinite(declared) && declared > SEGMENT_BYTES) {
    await response.body?.cancel();
    throw new ExportError('TOO_LARGE', 'a segment is longer than a whole segment');
  }
  if (!response.body) return new Uint8Array();
  const reader = response.body.getReader();
  const stop = () => void reader.cancel();
  signal?.addEventListener('abort', stop);
  const parts: Uint8Array[] = [];
  let total = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      total += value.length;
      if (total > SEGMENT_BYTES) {
        await reader.cancel();
        throw new ExportError('TOO_LARGE', 'a segment is longer than a whole segment');
      }
      parts.push(value);
    }
  } finally {
    signal?.removeEventListener('abort', stop);
  }
  if (signal?.aborted) throw aborted();
  return concat(...parts);
}

/**
 * Write the plaintext of sealed file `id` (64 hex) of app `app` to `sink`, one
 * segment at a time. Each segment is checked against its hash before it is
 * opened, and the segment list, size and plaintext hash against the file's id
 * and mark at the end. On rejection, discard what `sink` received: it is
 * authentic but may be a prefix of the file. A failed check is an
 * {@link ExportError}; a refusal by the node is the transport's own error.
 * `recipientSecret` pins the export key, for the published vectors only.
 */
export async function exportFileTo(
  transport: ExportTransport,
  app: string,
  id: string,
  sink: FileSink,
  signal?: AbortSignal,
  recipientSecret?: Uint8Array,
): Promise<ExportedFile> {
  try {
    return await exportChecked(transport, app, id, sink, signal, recipientSecret);
  } catch (error) {
    if (signal?.aborted && !(error instanceof ExportError)) throw aborted();
    throw error;
  }
}

async function exportChecked(
  transport: ExportTransport,
  app: string,
  id: string,
  sink: FileSink,
  signal?: AbortSignal,
  recipientSecret?: Uint8Array,
): Promise<ExportedFile> {
  const recipient = await x25519KeyPair(recipientSecret);
  const answer = checked(await transport.exportsExport({ app, id, recipient: hex(recipient.publicKey) }, { signal }));
  const { segments, named } = answer;
  if (segments.length === 0 || hex(await fileId(segments)) !== hex(named) || hex(named) !== id) {
    throw new ExportError('NOT_THE_FILE', `the node answered segments that are not the file ${id}`);
  }
  const dataKey = await unwrapExport(recipient, answer.ephemeral, { appId: answer.app, fileId: id }, answer.wrapped);
  const reader = await SegmentReader.create(dataKey, segments);
  for (let k = 0; k < segments.length; k += 1) {
    if (signal?.aborted) throw aborted();
    // The generated segment method buffers the whole body, so this stays on the streaming request.
    const response = await transport.request('GET', routePath('apps/{app}/files/{id}/segments/{k}', { app, id, k }), {
      signal,
    });
    await reader.push(await readSegment(response, signal), (chunk) => sink.write(chunk));
  }
  const last = await reader.finishExpecting(answer.size, answer.sha256);
  if (last.length > 0) await sink.write(last);
  return { size: answer.size, sha256: answer.sha256 };
}

/** The plaintext of sealed file `id` of app `app`, in memory. Prefer {@link exportFileTo} for a large file. */
export async function exportFile(
  transport: ExportTransport,
  app: string,
  id: string,
  signal?: AbortSignal,
): Promise<Uint8Array> {
  const chunks: Uint8Array[] = [];
  await exportFileTo(transport, app, id, { write: (chunk) => void chunks.push(chunk) }, signal);
  return concat(...chunks);
}
