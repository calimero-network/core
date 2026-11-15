/**
 * A stand-in cloud node for client tests: a real local HTTP server speaking mock
 * attestation, the sealed handshake and sealed, streamed answers, as core's cloud
 * node does. Synthetic data only.
 */

import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';

import { concat, hex } from '../../src/crypto/internal.js';
import { SymmetricState, dh, x25519KeyPair, type X25519KeyPair } from '../../src/sealed/noise.js';
import { SEALED_CONTENT_TYPE, transportKeyBinding, type RequestHead } from '../../src/sealed/sealed.js';
import { MOCK_QUOTE_HEADER } from '../../src/runtime/attestation.js';

const PROLOGUE = new TextEncoder().encode('calimero/sealed-http/v2');
const utf8 = new TextEncoder();

/** The bundle hash the stand-in's record holds for any app it is asked to attest. */
export const APP_BUNDLE_HASH = new Uint8Array(32).fill(0x11);

/** What the stand-in answers one sealed request with. */
export interface Reply {
  status: number;
  /** A JSON body. */
  json?: unknown;
  /** An event stream: SSE frames written as they come, kept open until the client goes. */
  events?: string[];
  /** Drop the connection after reading the request, answering nothing (`status` is ignored). */
  drop?: boolean;
  /** Raw bytes in place of `json`, served as `contentType`. */
  bytes?: Uint8Array;
  contentType?: string;
}

export class StandInNode {
  readonly requests: Array<{ head: RequestHead; body: Record<string, unknown> }> = [];
  /** Event streams the node still has open. */
  openStreams = 0;
  answer: (head: RequestHead, body: Record<string, unknown>) => Reply = () => ({ status: 200, json: {} });
  private sessions = new Map<string, { requestKey: CryptoKey; responseKey: CryptoKey }>();
  private server!: Server;
  private transport!: X25519KeyPair;
  url = '';

  async start(): Promise<void> {
    this.transport = await x25519KeyPair();
    this.server = createServer((req, res) => {
      this.route(req, res).catch((error) => {
        res.statusCode = 500;
        res.end(String(error));
      });
    });
    await new Promise<void>((ok) => this.server.listen(0, '127.0.0.1', ok));
    const { port } = this.server.address() as { port: number };
    this.url = `http://127.0.0.1:${port}`;
  }

  /** A new transport key, as after a node restart: sessions are gone and the old key is refused. */
  async rotateKey(): Promise<void> {
    this.transport = await x25519KeyPair();
    this.sessions.clear();
  }

  stop(): Promise<void> {
    this.server.closeAllConnections();
    return new Promise((ok) => this.server.close(() => ok()));
  }

  private async route(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const url = new URL(req.url ?? '/', this.url);
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(chunk as Buffer);
    const body = new Uint8Array(Buffer.concat(chunks));
    if (req.method === 'GET' && url.pathname === '/attestation') return this.attest(url, res);
    if (req.method === 'POST' && url.pathname === '/sealed/v2/handshake') return this.handshake(body, res);
    if (req.method === 'POST' && url.pathname === '/sealed/v2') return this.exchange(body, req, res);
    res.statusCode = 404;
    res.end();
  }

  /** A mock quote binding the asked nonce and the transport key, as a `--dev` node sends; asked for an `app`, it binds its own record's hash. */
  private async attest(url: URL, res: ServerResponse): Promise<void> {
    const nonce = Buffer.from(url.searchParams.get('nonce') ?? '', 'hex');
    const appHash = url.searchParams.has('app') ? APP_BUNDLE_HASH : new Uint8Array(32);
    const suffix = await transportKeyBinding(appHash, this.transport.publicKey);
    const quote = Buffer.concat([Buffer.from(MOCK_QUOTE_HEADER), nonce, Buffer.from(suffix)]).toString('base64');
    res.setHeader('content-type', 'application/json');
    res.end(JSON.stringify({ quote, transportKey: hex(this.transport.publicKey), appHash: hex(appHash), mock: true }));
  }

  private async handshake(body: Uint8Array, res: ServerResponse): Promise<void> {
    if (hex(body.slice(1, 33)) !== hex(this.transport.publicKey)) {
      res.statusCode = 409;
      res.end(JSON.stringify({ error: { code: 'stale_transport_key', message: 'the transport key changed' } }));
      return;
    }
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
    res.setHeader('content-type', SEALED_CONTENT_TYPE);
    res.end(concat(new Uint8Array([2]), message2));
  }

  private async exchange(envelope: Uint8Array, req: IncomingMessage, res: ServerResponse): Promise<void> {
    const header = envelope.slice(0, 25);
    const keys = this.sessions.get(hex(header.slice(1, 17)));
    if (!keys) throw new Error('unknown session');
    const requestId = header.slice(17, 25);
    const iv = (index: number) => {
      const bytes = new Uint8Array(12);
      bytes.set(requestId);
      new DataView(bytes.buffer).setUint32(8, index, false);
      return bytes;
    };
    const inner = new Uint8Array(
      await crypto.subtle.decrypt({ name: 'AES-GCM', iv: iv(0), additionalData: header }, keys.requestKey, envelope.slice(25)),
    );
    const len = new DataView(inner.buffer).getUint32(0, false);
    const head = JSON.parse(new TextDecoder().decode(inner.slice(4, 4 + len))) as RequestHead;
    const text = new TextDecoder().decode(inner.slice(4 + len));
    let body: Record<string, unknown> = {};
    try {
      body = text ? (JSON.parse(text) as Record<string, unknown>) : {};
    } catch {
      body = { raw: text }; // a byte upload (a blob, a bundle)
    }
    this.requests.push({ head, body });
    const reply = this.answer(head, body);
    if (reply.drop) {
      req.socket.destroy();
      return;
    }

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
    res.setHeader('content-type', SEALED_CONTENT_TYPE);
    const contentType = reply.events ? 'text/event-stream' : (reply.contentType ?? 'application/json');
    res.write(await frame(0, utf8.encode(JSON.stringify({ status: reply.status, headers: [['content-type', contentType]] }))));
    if (reply.events) {
      this.openStreams += 1;
      res.on('close', () => {
        this.openStreams -= 1;
      });
      for (const event of reply.events) res.write(await frame(1, utf8.encode(event)));
      return; // open until the client goes
    }
    res.write(await frame(1, reply.bytes ?? utf8.encode(JSON.stringify(reply.json ?? {}))));
    res.end(await frame(2, new Uint8Array()));
    void req;
  }
}
