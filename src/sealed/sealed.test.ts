import { describe, expect, it, vi } from 'vitest';

import { concat, fromHex, fromHexUnsized, hex } from '../crypto/internal.js';
import { SymmetricState, dh, initiate, x25519KeyPair, type X25519KeyPair } from './noise.js';
import {
  HANDSHAKE_PATH,
  LEGACY_HANDSHAKE_PATH,
  SEALED_CONTENT_TYPE,
  SEALED_PATH,
  SealedTransportError,
  StaleTransportKeyError,
  createSealedFetch,
  openResponse,
  sealRequest,
  sessionFrom,
  transportKeyBinding,
  type RequestHead,
} from './sealed.js';

// Core's published vectors (`crates/server/src/sealed/tests.rs` and
// `crates/tee-attestation/src/generate.rs`), repeated verbatim: the node and
// this client are separate implementations of one wire format, and the node's
// half of the handshake is snow's.
const TRANSPORT_PUBLIC_VECTOR = '0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20';
const HANDSHAKE_REQUEST_VECTOR =
  '020faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f207b0d47d93427f831116078' +
  '1c7c733fd89f88970aef490d8aa0ee19a4cb8a1b1461fba335b4f3e9be8a34a61900e5b4be';
const HANDSHAKE_RESPONSE_VECTOR =
  '02ff2ee45601ec1b67310c7790404585ae697331eee1c1f8cf2419731c1fff3e6b7d6339b7296bdc4778248f' +
  '57126a10ee5b7fb39b0cb104dcbfcaaaf44d0e6017379638a4';
const SEALED_REQUEST_VECTOR =
  '025555555555555555555555555555555500000000000000019fa4301264a48bdf6811cfb1d7c3dad713ede5' +
  'ea1820608358ff2779cee1605efc3d97879f24d6cf302a7877beda552ca6714ef9d5496f978fdcdde522435c' +
  'f663e4be5f60170e8eab74161a913608fe268e5352fcd3090d62e2a12c92b06b51885d43c4d83dcfe544';
const SEALED_RESPONSE_VECTOR =
  '0000004f4873ec5a589a1e3585360440100bc00d20522fe35bac2221dcfe80341ce3069481d4794b801776f4' +
  'a76188b81887eb83cfccd2981229af48a3820df924261b3ff12d4b29340ca07c392ed4e0ee4bed0000001cfa' +
  '20a7633655179808da19e2fcbc81bc219262fc4f5406582834f549000000111426897864d7a92eec9261fe29' +
  '93415592';
const TRANSPORT_BINDING_VECTOR = '30274595433e8afc5d4035e30a2b599d93caf0d470867527f13dc6af92fa12a8';

const PROLOGUE = new TextEncoder().encode('calimero/sealed-http/v2');
const transportKey = fromHex(TRANSPORT_PUBLIC_VECTOR, 'transport', 32);
const vectorHead: RequestHead = {
  method: 'POST',
  path: '/jsonrpc',
  headers: [['content-type', 'application/json']],
};

/** The vector session: the handshake with core's fixed client ephemeral key. */
async function vectorSession() {
  const noise = await initiate(transportKey, PROLOGUE, new Uint8Array(32).fill(0x33));
  const session = await sessionFrom(noise, fromHexUnsized(HANDSHAKE_RESPONSE_VECTOR, 'response'));
  return { noise, session };
}

function streamOf(bytes: Uint8Array): Response {
  return new Response(bytes, { headers: { 'content-type': SEALED_CONTENT_TYPE } });
}

describe('sealed transport wire format', () => {
  it('opens the handshake exactly as the node expects', async () => {
    const { noise } = await vectorSession();
    expect(hex(concat(new Uint8Array([2]), transportKey, noise.message1))).toBe(HANDSHAKE_REQUEST_VECTOR);
  });

  it('seals a request under the session exactly as the node expects', async () => {
    const { session } = await vectorSession();
    const { envelope } = await sealRequest(session, vectorHead, new TextEncoder().encode('{}'));
    expect(hex(envelope)).toBe(SEALED_REQUEST_VECTOR);
  });

  it('opens the frames of a response the node sealed', async () => {
    const { session } = await vectorSession();
    const { header, requestId } = await sealRequest(session, vectorHead, new Uint8Array());
    const response = await openResponse(
      streamOf(fromHexUnsized(SEALED_RESPONSE_VECTOR, 'response')),
      session.responseKey,
      header,
      requestId,
    );
    expect(response.status).toBe(200);
    expect(response.headers.get('content-type')).toBe('application/json');
    expect(await response.text()).toBe('{"ok":true}');
  });

  it('refuses a handshake reply from anyone but the attested node', async () => {
    const noise = await initiate(transportKey, PROLOGUE, new Uint8Array(32).fill(0x33));
    const forged = fromHexUnsized(HANDSHAKE_RESPONSE_VECTOR, 'response');
    forged[forged.length - 1] ^= 1;
    await expect(sessionFrom(noise, forged)).rejects.toThrow(/did not come from the attested node/);
  });

  it('refuses a response frame that anyone but the node sealed', async () => {
    const { session } = await vectorSession();
    const { header, requestId } = await sealRequest(session, vectorHead, new Uint8Array());
    const forged = fromHexUnsized(SEALED_RESPONSE_VECTOR, 'response');
    forged[forged.length - 1] ^= 1;
    const response = await openResponse(streamOf(forged), session.responseKey, header, requestId);
    await expect(response.text()).rejects.toThrow(/did not open/);
  });

  it('notices a response cut short before its end frame', async () => {
    const { session } = await vectorSession();
    const { header, requestId } = await sealRequest(session, vectorHead, new Uint8Array());
    const whole = fromHexUnsized(SEALED_RESPONSE_VECTOR, 'response');
    const endFrame = 4 + 1 + 16;
    const response = await openResponse(
      streamOf(whole.slice(0, whole.length - endFrame)),
      session.responseKey,
      header,
      requestId,
    );
    await expect(response.text()).rejects.toThrow(/cut short/);
  });

  it('refuses a frame larger than the node ever sends, before buffering it', async () => {
    const { session } = await vectorSession();
    const { header, requestId } = await sealRequest(session, vectorHead, new Uint8Array());
    let pulled = 0;
    const endless = new ReadableStream<Uint8Array>({
      pull(controller) {
        pulled += 1;
        controller.enqueue(pulled === 1 ? new Uint8Array([0xff, 0xff, 0xff, 0xff]) : new Uint8Array(1024));
      },
    });
    const response = new Response(endless, { headers: { 'content-type': SEALED_CONTENT_TYPE } });
    await expect(openResponse(response, session.responseKey, header, requestId)).rejects.toThrow(
      /larger than the node sends/,
    );
    expect(pulled).toBeLessThan(3);
  });

  it('computes the transport binding the node puts in its quote', async () => {
    const binding = await transportKeyBinding(new Uint8Array(32).fill(0x11), new Uint8Array(32).fill(0x22));
    expect(hex(binding)).toBe(TRANSPORT_BINDING_VECTOR);
  });
});

interface Answer {
  status: number;
  headers: Array<[string, string]>;
  /** Body pieces, each sent as its own frame, `delayMs` apart. */
  chunks: string[];
  delayMs?: number;
}

/**
 * A stand-in node: answers the Noise handshake with its transport secret and
 * seals its answers frame by frame, as core does.
 */
class StandInNode {
  static readonly baseUrl = 'https://tee-node.example/node';
  handshakes = 0;
  requests: Array<{ head: RequestHead; body: string }> = [];
  private sessions = new Map<string, { requestKey: CryptoKey; responseKey: CryptoKey }>();

  private constructor(
    private transport: X25519KeyPair,
    private answer: (head: RequestHead, body: string) => Answer,
  ) {}

  static async start(answer: (head: RequestHead, body: string) => Answer): Promise<StandInNode> {
    return new StandInNode(await x25519KeyPair(), answer);
  }

  get transportKey(): Uint8Array {
    return this.transport.publicKey;
  }

  async restart(): Promise<void> {
    this.transport = await x25519KeyPair();
    this.sessions.clear();
  }

  forgetSessions(): void {
    this.sessions.clear();
  }

  fetch = vi.fn(async (url: string, init: RequestInit): Promise<Response> => {
    const body = init.body as Uint8Array;
    if (url === `${StandInNode.baseUrl}${HANDSHAKE_PATH}`) return this.handshake(body);
    if (url === `${StandInNode.baseUrl}${LEGACY_HANDSHAKE_PATH}`) return this.handshake(body);
    if (url === `${StandInNode.baseUrl}${SEALED_PATH}`) return this.exchange(body);
    throw new Error(`unexpected ${url}`);
  });

  private async handshake(body: Uint8Array): Promise<Response> {
    if (hex(body.slice(1, 33)) !== hex(this.transport.publicKey)) {
      return refusal(409, 'stale_transport_key');
    }
    this.handshakes += 1;
    const message1 = body.slice(33);
    const state = await SymmetricState.initialize();
    await state.mixHash(PROLOGUE);
    await state.mixHash(this.transport.publicKey);
    const remote = message1.slice(0, 32);
    await state.mixHash(remote);
    await state.mixKey(await dh(this.transport.privateKey, remote));
    await state.decryptAndHash(message1.slice(32));
    const ephemeral = await x25519KeyPair();
    await state.mixHash(ephemeral.publicKey);
    await state.mixKey(await dh(ephemeral.privateKey, remote));
    const sessionId = crypto.getRandomValues(new Uint8Array(16));
    const payload = concat(sessionId, new Uint8Array([0, 0, 0x0e, 0x10]));
    const message2 = concat(ephemeral.publicKey, await state.encryptAndHash(payload));
    const [requestKey, responseKey] = await state.split();
    this.sessions.set(hex(sessionId), {
      requestKey: await crypto.subtle.importKey('raw', requestKey, 'AES-GCM', false, ['decrypt']),
      responseKey: await crypto.subtle.importKey('raw', responseKey, 'AES-GCM', false, ['encrypt']),
    });
    return new Response(concat(new Uint8Array([2]), message2), {
      headers: { 'content-type': SEALED_CONTENT_TYPE },
    });
  }

  private async exchange(envelope: Uint8Array): Promise<Response> {
    const header = envelope.slice(0, 25);
    const keys = this.sessions.get(hex(header.slice(1, 17)));
    if (!keys) return refusal(409, 'unknown_session');
    const requestId = header.slice(17, 25);
    const iv = (index: number) => concat(requestId, new Uint8Array([0, 0, 0, index]));
    const inner = new Uint8Array(
      await crypto.subtle.decrypt(
        { name: 'AES-GCM', iv: iv(0), additionalData: header },
        keys.requestKey,
        envelope.slice(25),
      ),
    );
    const len = new DataView(inner.buffer).getUint32(0, false);
    const head = JSON.parse(new TextDecoder().decode(inner.slice(4, 4 + len))) as RequestHead;
    const body = new TextDecoder().decode(inner.slice(4 + len));
    this.requests.push({ head, body });
    const reply = this.answer(head, body);

    let index = 0;
    const frame = async (kind: number, data: Uint8Array) => {
      const sealed = new Uint8Array(
        await crypto.subtle.encrypt(
          { name: 'AES-GCM', iv: iv(index++), additionalData: header },
          keys.responseKey,
          concat(new Uint8Array([kind]), data),
        ),
      );
      const length = new Uint8Array(4);
      new DataView(length.buffer).setUint32(0, sealed.length, false);
      return concat(length, sealed);
    };
    const encode = (text: string) => new TextEncoder().encode(text);
    const stream = new ReadableStream<Uint8Array>({
      async start(controller) {
        controller.enqueue(await frame(0, encode(JSON.stringify({ status: reply.status, headers: reply.headers }))));
        for (const chunk of reply.chunks) {
          if (reply.delayMs) await new Promise((resolve) => setTimeout(resolve, reply.delayMs));
          controller.enqueue(await frame(1, encode(chunk)));
        }
        controller.enqueue(await frame(2, new Uint8Array()));
        controller.close();
      },
    });
    return new Response(stream, { headers: { 'content-type': SEALED_CONTENT_TYPE } });
  }
}

function refusal(status: number, code: string): Response {
  return new Response(JSON.stringify({ error: { code, message: code } }), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

const ok = (body = '{}'): Answer => ({ status: 200, headers: [['content-type', 'application/json']], chunks: [body] });

describe('createSealedFetch', () => {
  const baseUrl = StandInNode.baseUrl;

  it('delivers the whole request to the node and returns its answer as a Response', async () => {
    const node = await StandInNode.start(() => ({
      status: 201,
      headers: [['x-answer', 'yes']],
      chunks: ['{"result":', '42}'],
    }));
    const sealedFetch = createSealedFetch({ baseUrl, transportPublicKey: node.transportKey, fetch: node.fetch as never });

    const response = await sealedFetch(`${baseUrl}/jsonrpc?x=1`, {
      method: 'POST',
      headers: { authorization: 'Bearer secret-token', 'content-type': 'application/json' },
      body: '{"method":"secret-call"}',
    });

    expect(node.requests[0].head.method).toBe('POST');
    expect(node.requests[0].head.path).toBe('/jsonrpc?x=1');
    expect(node.requests[0].head.headers).toContainEqual(['authorization', 'Bearer secret-token']);
    expect(node.requests[0].body).toBe('{"method":"secret-call"}');
    expect(response.status).toBe(201);
    expect(response.headers.get('x-answer')).toBe('yes');
    expect(await response.json()).toEqual({ result: 42 });

    const wire = node.fetch.mock.calls.map(([, init]) => new TextDecoder('latin1').decode(init.body as Uint8Array)).join('');
    expect(wire).not.toContain('secret-token');
    expect(wire).not.toContain('secret-call');
    expect(wire).not.toContain('jsonrpc');
  });

  it('opens one session for many requests, concurrent ones included', async () => {
    const node = await StandInNode.start(() => ok());
    const sealedFetch = createSealedFetch({ baseUrl, transportPublicKey: node.transportKey, fetch: node.fetch as never });
    await Promise.all([1, 2, 3].map(() => sealedFetch(`${baseUrl}/admin-api/health`)));
    await sealedFetch(`${baseUrl}/admin-api/health`);
    expect(node.handshakes).toBe(1);
    expect(node.requests).toHaveLength(4);
  });

  it('opens a new session when the node dropped the old one, and retries', async () => {
    const node = await StandInNode.start(() => ok('"again"'));
    const sealedFetch = createSealedFetch({ baseUrl, transportPublicKey: node.transportKey, fetch: node.fetch as never });
    await sealedFetch(`${baseUrl}/admin-api/health`);
    node.forgetSessions();
    const response = await sealedFetch(`${baseUrl}/admin-api/health`);
    expect(await response.json()).toBe('again');
    expect(node.handshakes).toBe(2);
  });

  it('attests again, through the caller, when the node restarted', async () => {
    const node = await StandInNode.start(() => ok());
    const attest = vi.fn(async () => node.transportKey);
    const sealedFetch = createSealedFetch({ baseUrl, transportPublicKey: attest, fetch: node.fetch as never });
    await sealedFetch(`${baseUrl}/admin-api/health`);
    await node.restart();
    const response = await sealedFetch(`${baseUrl}/admin-api/health`);
    expect(response.status).toBe(200);
    expect(attest).toHaveBeenCalledTimes(2);
  });

  it('tells the caller to attest again when given a fixed key and the node restarted', async () => {
    const node = await StandInNode.start(() => ok());
    const sealedFetch = createSealedFetch({ baseUrl, transportPublicKey: node.transportKey, fetch: node.fetch as never });
    await node.restart();
    await expect(sealedFetch(`${baseUrl}/admin-api/health`)).rejects.toBeInstanceOf(StaleTransportKeyError);
  });

  it('refuses a handshake reply too long to be one', async () => {
    const carrier = vi.fn().mockResolvedValue(
      new Response(new Uint8Array(4096), { headers: { 'content-type': SEALED_CONTENT_TYPE } }),
    );
    const sealedFetch = createSealedFetch({ baseUrl, transportPublicKey: transportKey, fetch: carrier });
    await expect(sealedFetch(`${baseUrl}/admin-api/health`)).rejects.toThrow(/too long/);
  });

  it('waits and retries once when the node is too busy to open a session', async () => {
    const node = await StandInNode.start(() => ok());
    let refused = 0;
    const carrier = vi.fn(async (url: string, init: RequestInit) => {
      if (url.endsWith(HANDSHAKE_PATH) && refused === 0) {
        refused += 1;
        const busy = refusal(503, 'busy');
        busy.headers.set('retry-after', '0.01');
        return busy;
      }
      return node.fetch(url, init);
    });
    const sealedFetch = createSealedFetch({ baseUrl, transportPublicKey: node.transportKey, fetch: carrier as never });
    const response = await sealedFetch(`${baseUrl}/admin-api/health`);
    expect(response.status).toBe(200);
    expect(node.handshakes).toBe(1);
  });

  it('opens the session at /sealed/handshake', async () => {
    const node = await StandInNode.start(() => ok());
    const sealedFetch = createSealedFetch({ baseUrl, transportPublicKey: node.transportKey, fetch: node.fetch as never });
    expect((await sealedFetch(`${baseUrl}/admin-api/health`)).status).toBe(200);
    const urls = node.fetch.mock.calls.map(([url]) => url);
    expect(HANDSHAKE_PATH).toBe('/sealed/handshake');
    expect(urls).toEqual([`${baseUrl}/sealed/handshake`, `${baseUrl}${SEALED_PATH}`]);
  });

  it.each([404, 405])(
    'opens the session at /sealed/v2/handshake when the node answers %i at /sealed/handshake',
    async (status) => {
      // A node released before /sealed/handshake routes only the v2 path.
      const node = await StandInNode.start(() => ok());
      const carrier = vi.fn(async (url: string, init: RequestInit) => {
        if (url === `${baseUrl}${HANDSHAKE_PATH}`) return new Response('Not Found', { status });
        return node.fetch(url, init);
      });
      const sealedFetch = createSealedFetch({ baseUrl, transportPublicKey: node.transportKey, fetch: carrier as never });
      expect((await sealedFetch(`${baseUrl}/admin-api/health`)).status).toBe(200);
      expect(carrier.mock.calls.map(([url]) => url)).toEqual([
        `${baseUrl}/sealed/handshake`,
        `${baseUrl}/sealed/v2/handshake`,
        `${baseUrl}${SEALED_PATH}`,
      ]);
      expect(node.handshakes).toBe(1);
    },
  );

  it('opens the session at /sealed/v2/handshake when a sealed-only node answers 403 sealed_required', async () => {
    // An older node that seals everything refuses an unknown path as unsealed.
    const node = await StandInNode.start(() => ok());
    const carrier = vi.fn(async (url: string, init: RequestInit) => {
      if (url === `${baseUrl}${HANDSHAKE_PATH}`) return refusal(403, 'sealed_required');
      return node.fetch(url, init);
    });
    const sealedFetch = createSealedFetch({ baseUrl, transportPublicKey: node.transportKey, fetch: carrier as never });
    expect((await sealedFetch(`${baseUrl}/admin-api/health`)).status).toBe(200);
    expect(carrier.mock.calls.map(([url]) => url)).toEqual([
      `${baseUrl}/sealed/handshake`,
      `${baseUrl}/sealed/v2/handshake`,
      `${baseUrl}${SEALED_PATH}`,
    ]);
  });

  it('reports a 404 from the old handshake path too, without trying again', async () => {
    const carrier = vi.fn(async () => new Response('Not Found', { status: 404 }));
    const sealedFetch = createSealedFetch({ baseUrl, transportPublicKey: transportKey, fetch: carrier });
    const error = await sealedFetch(`${baseUrl}/admin-api/health`).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(SealedTransportError);
    expect((error as SealedTransportError).status).toBe(404);
    expect(carrier).toHaveBeenCalledTimes(2);
  });

  it.each([
    [400, 'malformed'],
    [403, 'forbidden'],
    [409, 'stale_transport_key'],
    [500, 'internal'],
  ])('does not try the old handshake path when the node answers %i', async (status, code) => {
    const carrier = vi.fn(async () => refusal(status, code));
    const sealedFetch = createSealedFetch({ baseUrl, transportPublicKey: transportKey, fetch: carrier });
    await expect(sealedFetch(`${baseUrl}/admin-api/health`)).rejects.toBeInstanceOf(Error);
    expect(carrier).toHaveBeenCalledTimes(1);
    expect(carrier).toHaveBeenCalledWith(`${baseUrl}${HANDSHAKE_PATH}`, expect.anything());
  });

  it('passes a node refusal on as a SealedTransportError', async () => {
    const carrier = vi.fn().mockResolvedValue(refusal(400, 'malformed'));
    const sealedFetch = createSealedFetch({ baseUrl, transportPublicKey: transportKey, fetch: carrier });
    const error = await sealedFetch(`${baseUrl}/admin-api/health`).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(SealedTransportError);
    expect((error as SealedTransportError).code).toBe('malformed');
  });

  it('hands over a streamed body frame by frame, before the stream ends', async () => {
    const node = await StandInNode.start(() => ({
      status: 200,
      headers: [['content-type', 'text/event-stream']],
      chunks: ['data: one\n\n', 'data: two\n\n'],
      delayMs: 50,
    }));
    const sealedFetch = createSealedFetch({ baseUrl, transportPublicKey: node.transportKey, fetch: node.fetch as never });
    const response = await sealedFetch(`${baseUrl}/sse`, { headers: { accept: 'text/event-stream' } });
    expect(response.headers.get('content-type')).toBe('text/event-stream');
    const reader = response.body!.getReader();
    const first = await reader.read();
    expect(new TextDecoder().decode(first.value)).toBe('data: one\n\n');
    const second = await reader.read();
    expect(new TextDecoder().decode(second.value)).toBe('data: two\n\n');
    expect((await reader.read()).done).toBe(true);
  });

  it('never sends a request outside the node in the clear', async () => {
    const carrier = vi.fn();
    const sealedFetch = createSealedFetch({ baseUrl, transportPublicKey: transportKey, fetch: carrier });
    await expect(sealedFetch('https://elsewhere.example/node/jsonrpc')).rejects.toThrow(/Refusing/);
    await expect(sealedFetch('https://tee-node.example/other/jsonrpc')).rejects.toThrow(/Refusing/);
    expect(carrier).not.toHaveBeenCalled();
  });
});
