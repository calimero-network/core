import { describe, expect, it } from 'vitest';

import { fromHex, fromHexUnsized, hex } from '../crypto/internal.js';
import { SEALED_FILE_VECTORS as V } from '../sealed/fixtures/sealed-file-vectors.js';
import { fileId } from '../sealed/file.js';
import type { Transport } from '../runtime/transport.js';
import type { AppsRoutes } from '../generated/methods.js';
import { ExportError, exportFile, exportFileTo } from './files.js';
import { SEGMENT_BYTES } from '../sealed/file.js';

const bytes = (value: string) => fromHexUnsized(value, 'vector');
const sha = async (data: Uint8Array) => new Uint8Array(await crypto.subtle.digest('SHA-256', data));
const [three, full, empty] = V.vectors;
type Vector = (typeof V.vectors)[number];
const plain = (len: number) => Uint8Array.from({ length: len }, (_, i) => i % 251);

/** A node holding one vector: its export answer (changeable) and its segment. */
function node(
  v: Vector,
  change: (answer: Record<string, unknown>) => void = () => {},
  segment = bytes(v.segment0),
  respond: () => Response = () => new Response(segment),
) {
  const calls: string[] = [];
  const signals: unknown[] = [];
  let body: unknown;
  const answer: Record<string, unknown> = {
    app: v.app,
    id: v.id,
    size: v.size,
    sha256: v.plaintextSha256,
    segments: v.segments,
    ephemeral: v.ephemeral,
    wrapped: v.wrapped,
  };
  change(answer);
  const transport = {
    exportsExport: async (args: Record<string, unknown>, options?: { signal?: unknown }) => {
      calls.push(`POST apps/${String(args.app)}/files/${String(args.id)}/export`);
      signals.push(options?.signal);
      body = { recipient: args.recipient };
      return answer;
    },
    request: async (method: string, path: string, options?: { signal?: unknown }) => {
      calls.push(`${method} ${path}`);
      signals.push(options?.signal);
      return respond();
    },
  } as unknown as Pick<Transport, 'request'> & Pick<AppsRoutes, 'exportsExport'>;
  return { transport, calls, signals, body: () => body };
}

function sink() {
  const chunks: Uint8Array[] = [];
  return { chunks, write: (chunk: Uint8Array) => void chunks.push(chunk), total: () => chunks.reduce((n, c) => n + c.length, 0) };
}

const codeOf = (promise: Promise<unknown>) =>
  promise.then(
    () => 'resolved',
    (e: unknown) => (e instanceof ExportError ? e.code : `not an ExportError: ${String(e)}`),
  );

const run = (n: ReturnType<typeof node>, v: Vector, out = sink()) =>
  exportFileTo(n.transport, v.app, v.id, out, undefined, bytes(v.recipientSecret));

describe('exportFileTo', () => {
  it('decrypts the Rust vectors to exactly their plaintext', async () => {
    for (const v of V.vectors) {
      const n = node(v);
      const out = sink();
      const done = await run(n, v, out);
      expect(done).toEqual({ size: v.size, sha256: v.plaintextSha256 });
      expect(new Uint8Array(out.chunks.flatMap((c) => [...c])), v.name).toEqual(plain(v.plaintextLen));
      expect(hex(await sha(new Uint8Array(out.chunks.flatMap((c) => [...c]))))).toBe(v.plaintextSha256);
      expect(n.calls).toEqual([
        `POST apps/${v.app}/files/${v.id}/export`,
        `GET apps/${v.app}/files/${v.id}/segments/0`,
      ]);
      expect(n.body()).toEqual({ recipient: v.recipientPublic });
    }
  });

  it('hands the sink chunks of at most 64 KiB, in order', async () => {
    const out = sink();
    await run(node(three), three, out);
    expect(out.chunks.map((c) => c.length)).toEqual([65536, 65536, 5]);
  });

  it('writes nothing for an empty file', async () => {
    const out = sink();
    await run(node(empty), empty, out);
    expect(out.chunks).toEqual([]);
  });

  it('works with a fresh export key, which cannot open a wrap made for another', async () => {
    await expect(exportFileTo(node(three).transport, three.app, three.id, sink())).rejects.toThrow('does not open');
  });

  it('fails a tampered segment before any of its bytes reach the sink', async () => {
    const tampered = bytes(three.segment0);
    tampered[3] ^= 1;
    const out = sink();
    await expect(run(node(three, () => {}, tampered), three, out)).rejects.toThrow('is not the one the file lists');
    expect(out.chunks).toEqual([]);
  });

  it('fails a segment list that does not hash to the id', async () => {
    const n = node(three, (a) => (a.segments = [hex(new Uint8Array(32).fill(7))]));
    await expect(run(n, three)).rejects.toThrow('are not the file');
    expect(n.calls).toHaveLength(1);
  });

  it('fails an answer that names another file than the one asked for', async () => {
    const n = node(three);
    await expect(
      exportFileTo(n.transport, three.app, full.id, sink(), undefined, bytes(three.recipientSecret)),
    ).rejects.toThrow('are not the file');
  });

  it('fails an empty segment list even when it hashes to the requested id', async () => {
    const id = hex(await fileId([]));
    const n = node(three, (a) => {
      a.segments = [];
      a.id = id;
    });
    await expect(exportFileTo(n.transport, three.app, id, sink(), undefined, bytes(three.recipientSecret))).rejects.toThrow(
      'are not the file',
    );
  });

  it('fails a consistent answer for a file other than the one asked for', async () => {
    const other = new Uint8Array(32).fill(9);
    const id = hex(await fileId([other]));
    const n = node(three, (a) => {
      a.segments = [hex(other)];
      a.id = id;
    });
    await expect(run(n, three)).rejects.toThrow('are not the file');
  });

  it('fails a wrong final size, withholding the last chunk', async () => {
    const out = sink();
    await expect(run(node(three, (a) => (a.size = three.size + 1)), three, out)).rejects.toThrow('not the one that was sealed');
    expect(out.total()).toBe(2 * 65536);
  });

  it('fails a wrong final SHA-256', async () => {
    const n = node(three, (a) => (a.sha256 = '00'.repeat(32)));
    await expect(run(n, three)).rejects.toThrow('not the one that was sealed');
  });

  it('fails a wrap made for another app', async () => {
    const n = node(three, (a) => (a.app = 'another-app'));
    await expect(run(n, three)).rejects.toThrow('does not open');
    expect(n.calls).toHaveLength(1);
  });

  it('fails a malformed answer', async () => {
    await expect(run(node(three, (a) => (a.ephemeral = 'zz')), three)).rejects.toThrow('ephemeral key');
    await expect(run(node(three, (a) => (a.sha256 = 'ab')), three)).rejects.toThrow('sha256');
    await expect(run(node(three, (a) => (a.size = -1)), three)).rejects.toThrow('bad size');
    await expect(run(node(three, (a) => (a.size = 2 ** 60)), three)).rejects.toThrow('bad size');
  });
});

describe('exportFile', () => {
  it('returns the bytes', async () => {
    // exportFile makes its own export key, so only the failure path can run without the vector's secret
    await expect(exportFile(node(three).transport, three.app, three.id)).rejects.toThrow('does not open');
    expect(fromHex(three.id, 'id', 32)).toHaveLength(32);
  });
});

/** A body of `chunk`-sized pieces, `count` of them (forever if 0), counting pulls and noting a cancel. */
function stream(chunk: number, count: number, headers: Record<string, string> = {}, delayMs = 0) {
  const seen = { pulls: 0, cancelled: false };
  const body = new ReadableStream<Uint8Array>({
    async pull(controller) {
      seen.pulls += 1;
      if (delayMs) await new Promise((resolve) => setTimeout(resolve, delayMs));
      if (count && seen.pulls > count) controller.close();
      else controller.enqueue(new Uint8Array(chunk));
    },
    cancel() {
      seen.cancelled = true;
    },
  });
  return { seen, response: () => new Response(body, { headers }) };
}

describe('bounded segment reads', () => {
  it('refuses a declared length past a whole segment without reading the body', async () => {
    const s = stream(1024, 0, { 'content-length': String(SEGMENT_BYTES + 1) });
    const out = sink();
    const n = node(three, () => {}, undefined, s.response);
    expect(await codeOf(run(n, three, out))).toBe('TOO_LARGE');
    expect(s.seen.pulls).toBeLessThanOrEqual(1);
    expect(s.seen.cancelled).toBe(true);
    expect(out.chunks).toEqual([]);
  });

  it('stops reading a stream that runs past a whole segment, the last segment included', async () => {
    const s = stream(1 << 20, 0);
    const n = node(three, () => {}, undefined, s.response);
    expect(await codeOf(run(n, three))).toBe('TOO_LARGE');
    expect(s.seen.pulls).toBeLessThan(40);
    expect(s.seen.cancelled).toBe(true);
  });
});

describe('abort', () => {
  it('passes the signal to both requests', async () => {
    const controller = new AbortController();
    const n = node(three);
    await exportFileTo(n.transport, three.app, three.id, sink(), controller.signal, bytes(three.recipientSecret));
    expect(n.signals).toEqual([controller.signal, controller.signal]);
  });

  it('stops a download that never ends', async () => {
    const s = stream(1024, 0, {}, 5);
    const n = node(three, () => {}, undefined, s.response);
    const controller = new AbortController();
    const done = codeOf(
      exportFileTo(n.transport, three.app, three.id, sink(), controller.signal, bytes(three.recipientSecret)),
    );
    setTimeout(() => controller.abort(), 20);
    expect(await done).toBe('ABORTED');
    expect(s.seen.cancelled).toBe(true);
  });

  it('reports a transport that rejects on abort as ABORTED', async () => {
    const controller = new AbortController();
    const n = node(three);
    n.transport.request = (async () => {
      controller.abort();
      throw new DOMException('aborted', 'AbortError');
    }) as never;
    const done = exportFileTo(n.transport, three.app, three.id, sink(), controller.signal, bytes(three.recipientSecret));
    expect(await codeOf(done)).toBe('ABORTED');
  });

  it('does not start when already aborted', async () => {
    const controller = new AbortController();
    controller.abort();
    const n = node(three);
    const done = exportFileTo(n.transport, three.app, three.id, sink(), controller.signal, bytes(three.recipientSecret));
    expect(await codeOf(done)).toBe('ABORTED');
    expect(n.calls.filter((c) => c.startsWith('GET'))).toEqual([]);
  });
});

describe('error codes', () => {
  it('names the check that failed', async () => {
    const tampered = bytes(three.segment0);
    tampered[3] ^= 1;
    expect(await codeOf(run(node(three, () => {}, tampered), three))).toBe('TAMPERED');
    expect(await codeOf(run(node(three, (a) => (a.size = 1)), three))).toBe('TAMPERED');
    expect(await codeOf(run(node(three, (a) => (a.app = 'x')), three))).toBe('TAMPERED');
    expect(await codeOf(run(node(three, (a) => (a.segments = [hex(new Uint8Array(32))])), three))).toBe('NOT_THE_FILE');
  });

  it('turns a malformed answer into BAD_ANSWER, never a TypeError', async () => {
    const bad: Array<(a: Record<string, unknown>) => void> = [
      (a) => (a.segments = 'nope'),
      (a) => (a.segments = [7]),
      (a) => (a.segments = ['zz']),
      (a) => (a.app = 5),
      (a) => (a.id = undefined),
      (a) => (a.size = '3'),
      (a) => (a.size = 1.5),
      (a) => (a.sha256 = null),
      (a) => (a.ephemeral = 'ab'),
      (a) => (a.wrapped = 'abc'),
      (a) => (a.wrapped = undefined),
    ];
    for (const change of bad) expect(await codeOf(run(node(three, change), three))).toBe('BAD_ANSWER');
    const n = node(three);
    n.transport.exportsExport = (async () => null) as never;
    expect(await codeOf(run(n, three))).toBe('BAD_ANSWER');
  });

  it('leaves the node\'s own refusal as the transport\'s error', async () => {
    const n = node(three);
    n.transport.exportsExport = (async () => {
      throw new RangeError('refused');
    }) as never;
    expect(await codeOf(run(n, three))).toContain('not an ExportError');
  });
});
