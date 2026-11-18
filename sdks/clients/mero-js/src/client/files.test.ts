/**
 * uploadFile, listFiles and getFile against a stand-in node that keeps the
 * files API's session state and fails on cue. A part's bytes are one letter
 * per part, so the node can tell which slice of the file it was sent.
 */

import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

import { StandInNode } from '../../tests/support/stand-in-node.js';
import { CalimeroClient, CalimeroError, FILE_PART_BYTES, QuotaExceeded, UnavailableError } from './index.js';

const PART = FILE_PART_BYTES / 2;
const THREE_PARTS = FILE_PART_BYTES + 1; // sessions of two 8 MiB parts and a one-byte last
const BUSY = { status: 503, json: { error: 'FILES_UNAVAILABLE', message: 'busy' } };
const GONE = { status: 404, json: { error: 'UPLOAD_UNKNOWN', message: 'gone' } };

const node = new StandInNode();
beforeAll(() => node.start());
afterAll(() => node.stop());

const client = () =>
  new CalimeroClient({
    baseUrl: node.url,
    policy: { allowMock: true },
    retry: { retries: 0, baseDelayMs: 1, maxDelayMs: 2 },
  });

/** `size` bytes, each `partBytes`-long stretch one letter: a, b, c... */
function bytes(size: number, partBytes = PART): Uint8Array {
  const out = new Uint8Array(size);
  for (let i = 0; i < size; i += partBytes) out.fill(97 + i / partBytes, i, Math.min(i + partBytes, size));
  return out;
}

type Fault = { when: string; reply: { status: number; json?: unknown } | 'forget' | 'cut' | 'cut-after-store' };

/** The node's side of the files API: `log` is what it was asked, `faults` fire once each. */
class Files {
  log: string[] = [];
  faults: Fault[] = [];
  nextOnBegin = 0;
  private session: string | undefined;
  private begun = 0;
  private parts = 0;
  private next = 0;
  private held: string[] = [];

  constructor() {
    node.answer = (head, body) => this.step(head.method, head.path.replace('/apps/app/files', '').replace(/^\//, ''), body);
  }

  private step(method: string, tail: string, body: Record<string, unknown>) {
    const shown = `${method} ${tail || 'files'}`;
    this.log.push(shown);
    const at = this.faults.findIndex((f) => shown.endsWith(f.when));
    let cutAfter = false;
    if (at >= 0) {
      const { reply } = this.faults.splice(at, 1)[0];
      if (reply === 'cut') return { status: 0, drop: true };
      if (reply === 'forget') this.session = undefined;
      else if (reply === 'cut-after-store') cutAfter = true;
      else return reply;
    }
    const answer = this.route(method, tail, body);
    return cutAfter ? { status: 0, drop: true } : answer;
  }

  private route(method: string, tail: string, body: Record<string, unknown>) {
    const [, id, what, k] = tail.split('/');
    if (method === 'POST' && tail === '') return { status: 200, json: file(this.seen(body.raw as string)) };
    if (method === 'POST' && tail === 'uploads') {
      this.begun += 1;
      this.session = `u${this.begun}`;
      this.parts = Math.ceil((body.size as number) / PART);
      this.next = this.nextOnBegin;
      this.held = [];
      return { status: 200, json: { uploadId: this.session, partBytes: PART, parts: this.parts, expiresAt: 0 } };
    }
    if (id !== this.session) return GONE;
    if (method === 'PUT' && what === 'parts') return this.put(Number(k), body.raw as string);
    if (method === 'GET') return { status: 200, json: { uploadId: id, partBytes: PART, parts: this.parts, expiresAt: 0, size: 0, next: this.next } };
    if (method === 'POST' && what === 'complete') {
      expect(this.next, 'complete before every part').toBe(this.parts);
      return { status: 200, json: file(this.held.join(',')) };
    }
    if (method === 'DELETE') {
      this.session = undefined;
      return { status: 200, json: {} };
    }
    throw new Error(`unexpected request ${method} ${tail}`);
  }

  /** What the node tells a part by: its length and its letter. */
  private seen = (raw: string) => `${raw.length}${raw[0]}`;

  private put(k: number, raw: string) {
    const seen = this.seen(raw);
    const ahead = { status: 409, json: { error: 'PART_OUT_OF_ORDER', message: 'ahead', details: { next: this.next } } };
    if (k > this.next) return ahead;
    if (k < this.next) {
      if (this.held[k] === undefined) return ahead;
      if (this.held[k] === seen) return { status: 200, json: { next: k + 1 } };
      return { status: 409, json: { error: 'PART_CONFLICT', message: 'differs' } };
    }
    this.held[k] = seen;
    this.next += 1;
    return { status: 200, json: { next: this.next } };
  }
}

/** The file object; `sha256` carries what the node saw, for the assertions. */
const file = (seen: string) => ({ id: 'f1', blobId: 'b1', size: 0, sha256: seen, index: 7 });

let files: Files;
beforeEach(() => {
  node.requests.length = 0;
  files = new Files();
});

describe('uploadFile: a file that fits one request', () => {
  it.each([
    ['a Uint8Array', (b: Uint8Array) => b],
    ['an ArrayBuffer', (b: Uint8Array) => b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength)],
    ['a Blob', (b: Uint8Array) => new Blob([b])],
  ])('sends %s as one raw request', async (_name, wrap) => {
    const done = await client().uploadFile('app', wrap(bytes(1000)));
    expect(files.log).toEqual(['POST files']);
    expect(node.requests[0].head.headers).toContainEqual(['content-type', 'application/octet-stream']);
    expect(done).toEqual({ id: 'f1', blobId: 'b1', size: 0, sha256: '1000a', index: 7 });
  });

  it('sends a file of exactly one part as one request', async () => {
    await client().uploadFile('app', bytes(FILE_PART_BYTES));
    expect(files.log).toEqual(['POST files']);
  });

  it('does not send a 503 on the single request again: the node may have committed it', async () => {
    files.faults.push({ when: 'POST files', reply: BUSY });
    const error = await client()
      .uploadFile('app', bytes(10))
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(UnavailableError);
    expect(files.log).toEqual(['POST files']);
  });

  it('does not send a dropped single request again', async () => {
    files.faults.push({ when: 'POST files', reply: 'cut-after-store' });
    await expect(client().uploadFile('app', bytes(10))).rejects.toBeInstanceOf(TypeError);
    expect(files.log).toEqual(['POST files']);
  });

  it('stops on a 413', async () => {
    files.faults.push({ when: 'POST files', reply: { status: 413, json: { error: 'TOO_LARGE', message: 'over' } } });
    await expect(client().uploadFile('app', bytes(10))).rejects.toMatchObject({ status: 413, code: 'TOO_LARGE' });
    expect(files.log).toEqual(['POST files']);
  });

  it('stops on a 429', async () => {
    files.faults.push({ when: 'POST files', reply: { status: 429, json: { error: 'QUOTA_EXCEEDED', message: 'full' } } });
    await expect(client().uploadFile('app', bytes(10))).rejects.toBeInstanceOf(QuotaExceeded);
    expect(files.log).toEqual(['POST files']);
  });
});

describe('uploadFile: a session', () => {
  const log = (...rest: string[]) => ['POST uploads', ...rest];
  const put = (k: number, u = 'u1') => `PUT uploads/${u}/parts/${k}`;
  const complete = (u = 'u1') => `POST uploads/${u}/complete`;
  const upload = () => client().uploadFile('app', bytes(THREE_PARTS));
  const SEEN = `${PART}a,${PART}b,1c`;

  it('sends three parts in order and completes, reading a Blob one slice at a time', async () => {
    const blob = new Blob([bytes(THREE_PARTS)]);
    const slices: Array<[number, number | undefined]> = [];
    const slice = blob.slice.bind(blob);
    blob.slice = (start, end, type) => (slices.push([start as number, end]), slice(start, end, type));
    const whole = vi.spyOn(blob, 'arrayBuffer');
    const done = await client().uploadFile('app', blob);
    expect(files.log).toEqual(log(put(0), put(1), put(2), complete()));
    expect(node.requests[0].body).toEqual({ size: THREE_PARTS });
    expect(done.sha256).toBe(SEEN);
    expect(slices).toEqual([[0, PART], [PART, 2 * PART], [2 * PART, THREE_PARTS]]);
    expect(whole).not.toHaveBeenCalled();
  });

  it('sends a part again after a dropped connection', async () => {
    files.faults.push({ when: 'parts/1', reply: 'cut' });
    expect((await upload()).sha256).toBe(SEEN);
    expect(files.log).toEqual(log(put(0), put(1), put(1), put(2), complete()));
  });

  it('asks the node for next after a resend fails too, and goes on from it', async () => {
    files.faults.push({ when: 'parts/1', reply: 'cut-after-store' }, { when: 'parts/1', reply: 'cut' });
    expect((await upload()).sha256).toBe(SEEN);
    expect(files.log).toEqual(log(put(0), put(1), put(1), 'GET uploads/u1', put(2), complete()));
  });

  it('sends a part again after a 503', async () => {
    files.faults.push({ when: 'parts/1', reply: BUSY });
    expect((await upload()).sha256).toBe(SEEN);
    expect(files.log).toEqual(log(put(0), put(1), put(1), put(2), complete()));
  });

  it('jumps to the next the node names on PART_OUT_OF_ORDER', async () => {
    files.nextOnBegin = 2;
    await upload();
    expect(files.log).toEqual(log(put(0), put(2), complete()));
  });

  it('begins again once when the session is unknown', async () => {
    files.faults.push({ when: 'parts/1', reply: 'forget' });
    expect((await upload()).sha256).toBe(SEEN);
    expect(files.log).toEqual(log(put(0), put(1), 'POST uploads', put(0, 'u2'), put(1, 'u2'), put(2, 'u2'), complete('u2')));
  });

  it('gives up when the session is unknown a second time', async () => {
    files.faults.push({ when: 'parts/0', reply: 'forget' }, { when: 'parts/0', reply: 'forget' });
    await expect(upload()).rejects.toMatchObject({ status: 404, code: 'UPLOAD_UNKNOWN' });
    expect(files.log.filter((l) => l === 'POST uploads')).toHaveLength(2);
    expect(files.log.some((l) => l.startsWith('DELETE'))).toBe(false);
  });

  it('sends complete again on a 503', async () => {
    files.faults.push({ when: '/complete', reply: BUSY }, { when: '/complete', reply: BUSY });
    expect((await upload()).sha256).toBe(SEEN);
    expect(files.log).toEqual(log(put(0), put(1), put(2), complete(), complete(), complete()));
  });

  it('stops on a 413 when beginning', async () => {
    files.faults.push({ when: 'POST uploads', reply: { status: 413, json: { error: 'TOO_LARGE', message: 'over' } } });
    await expect(upload()).rejects.toMatchObject({ status: 413, code: 'TOO_LARGE' });
    expect(files.log).toEqual(['POST uploads']);
  });

  it('stops on a 429 and aborts the session', async () => {
    files.faults.push({ when: 'parts/1', reply: { status: 429, json: { error: 'QUOTA_EXCEEDED', message: 'full' } } });
    const error = await upload().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(QuotaExceeded);
    expect(files.log).toEqual(log(put(0), put(1), 'DELETE uploads/u1'));
  });

  it('aborts the session when a part is refused for good', async () => {
    files.faults.push({ when: 'parts/0', reply: { status: 400, json: { error: 'BAD_PART', message: 'length' } } });
    await expect(upload()).rejects.toBeInstanceOf(CalimeroError);
    expect(files.log).toEqual(log(put(0), 'DELETE uploads/u1'));
  });

  it('gives up after too many failures in a row and aborts', async () => {
    for (let i = 0; i < 9; i++) files.faults.push({ when: 'parts/0', reply: BUSY });
    await expect(upload()).rejects.toBeInstanceOf(UnavailableError);
    expect(files.log.filter((l) => l === put(0))).toHaveLength(9);
    expect(files.log.at(-1)).toBe('DELETE uploads/u1');
  });
});

describe('the generated files routes', () => {
  it('lists the files of an app', async () => {
    node.answer = () => ({ status: 200, json: { files: [file('x')] } });
    expect(await client().apps.filesList({ app: 'app' })).toEqual({ files: [file('x')] });
    expect(node.requests.map((r) => [r.head.method, r.head.path])).toEqual([['GET', '/apps/app/files']]);
  });

  it('describes one file, its id escaped as a path segment', async () => {
    node.answer = () => ({ status: 200, json: file('x') });
    expect(await client().apps.fileInfo({ app: 'app', id: 'a/b' })).toEqual(file('x'));
    expect(node.requests[0].head.path).toBe('/apps/app/files/a%2Fb');
  });
});
