/**
 * Sealed transport: requests encrypted end to end to a TEE node's attested key.
 *
 * A node speaks plain HTTP and any TLS in front of it ends outside the enclave,
 * at whatever proxy or relay the operator runs, so everything crossing that hop
 * (the bearer token, JSON-RPC arguments, results) is readable there. Sealing
 * encrypts it so that only the attested TD can read it, and only the TD can
 * produce a response the client accepts.
 *
 * The client opens a session with a Noise NK handshake to the X25519 transport
 * key the node's quote commits to (`fetchAttestedKey`, `GET /attestation`). The
 * transport key only authenticates the node: the session keys come from both
 * sides' ephemeral keys, so once a session is gone (an hour at most) what was
 * sent in it cannot be opened, not even with the transport key. Each request
 * is sealed under the session; each response streams back in sealed frames, so
 * server-sent events are sealed too. WebSockets cannot be, and the node
 * refuses one inside the envelope with a 501.
 *
 * The node's side, and the wire format this implements byte for byte, are in
 * core's `crates/sealed` (`calimero-sealed`):
 *
 * ```text
 * handshake   Noise_NK_25519_AESGCM_SHA256, prologue "calimero/sealed-http/v2"
 *   request   POST /sealed/handshake      0x02 || transport_pk || Noise message 1
 *             (nodes before it served only /sealed/v2/handshake; see LEGACY_HANDSHAKE_PATH)
 *   response  0x02 || Noise message 2 (payload = session_id(16) || lifetime secs, u32 BE)
 *   keys      (req key, resp key) = Split()
 * exchange    POST /sealed/v2
 *   header    0x02 || session_id(16) || request_id (u64 BE; from 1, each used once)
 *   request   header || AES-256-GCM(req key, nonce = request_id || 0u32, aad = header, inner)
 *   response  frames, each u32 BE length || AES-256-GCM(resp key,
 *             nonce = request_id || frame index (u32 BE, from 0), aad = header, kind || data)
 *             kind 0 = head (JSON), 1 = body bytes, 2 = end; no end frame = cut short
 *   inner     u32 BE head length || head (JSON) || body
 * ```
 *
 * `CalimeroClient` seals every request this way; {@link createSealedFetch} is
 * the same channel as a plain `fetch`.
 */

import { concat } from '../crypto/internal.js';
import { initiate } from './noise.js';
import type { DcapCollateral } from './verify.js';

/** Where a session is opened, relative to the node's base URL. */
export const HANDSHAKE_PATH = '/sealed/handshake';
/**
 * Where nodes released before {@link HANDSHAKE_PATH} existed open a session (newer
 * nodes still serve it as a deprecated alias). The handshake is tried here only
 * when the node answers 404 or 405 at {@link HANDSHAKE_PATH}.
 */
export const LEGACY_HANDSHAKE_PATH = '/sealed/v2/handshake';
/** Where sealed requests are posted, relative to the node's base URL. */
export const SEALED_PATH = '/sealed/v2';
/** Content type of every sealed body. */
export const SEALED_CONTENT_TYPE = 'application/vnd.calimero.sealed';

const VERSION = 2;
const PROLOGUE = new TextEncoder().encode('calimero/sealed-http/v2');
const SESSION_ID_LEN = 16;
const HEADER_LEN = 1 + SESSION_ID_LEN + 8;
const FRAME_HEAD = 0;
const FRAME_DATA = 1;
const FRAME_END = 2;
/**
 * The largest frame the node sends: a kind byte, at most 64 KiB of data, a tag.
 * Anything larger is refused before it is buffered, so a proxy cannot make the
 * client hold an unbounded frame it has yet to authenticate.
 */
const MAX_FRAME_LEN = 1 + 64 * 1024 + 16;
/** Far more than a handshake reply (69 bytes) ever is. */
const MAX_HANDSHAKE_REPLY = 1024;
/** Open a new session this long before the node would drop the current one. */
const RENEW_BEFORE_MS = 60_000;
const TRANSPORT_BINDING_DOMAIN = 'calimero.tee-attest.transport-key.v1';
const NULL_BODY_STATUSES = [101, 204, 205, 304];

const utf8 = new TextEncoder();

/** The node no longer holds the transport key this client attested (it restarted). */
export class StaleTransportKeyError extends Error {
  name = 'StaleTransportKeyError';

  constructor() {
    super('The node no longer holds the attested transport key; attest again for the current one');
  }
}

/** The node refused the envelope itself, before anything inside it ran. */
export class SealedTransportError extends Error {
  name = 'SealedTransportError';

  constructor(
    public status: number,
    public code: string | undefined,
    message: string,
    /** How long the node asked to wait before retrying (`Retry-After`), if it did. */
    public retryAfterMs?: number,
  ) {
    super(message);
  }
}

/**
 * What the attest endpoint puts in report data bytes `32..64` when asked to
 * bind its transport key. `inner` is what would have been there otherwise: the
 * expected application hash, or 32 zero bytes when none was asked for.
 * Mirrors core's `attest_transport_binding`.
 */
export async function transportKeyBinding(
  inner: Uint8Array,
  transportKey: Uint8Array,
): Promise<Uint8Array> {
  const digest = await crypto.subtle.digest(
    'SHA-256',
    concat(utf8.encode(TRANSPORT_BINDING_DOMAIN), inner, transportKey),
  );
  return new Uint8Array(digest);
}

export interface VerifyTransportQuote {
  /**
   * Verify `quoteB64` — never by asking the node being attested — and check
   * that its report data is `nonce || reportDataSuffix` (both hex). Resolve
   * true only when the quote is genuine, its measurements are ones you accept,
   * and the report data matches. {@link createQuoteVerifier} does all of this
   * in the page. `collateral` is what the node sent with the quote, when the
   * verifier asked for it.
   */
  (args: {
    quoteB64: string;
    nonce: string;
    reportDataSuffix: string;
    collateral?: DcapCollateral;
  }): Promise<boolean>;
  /**
   * Set by the verifiers this package builds. A Calimero 1.0 node sends no
   * collateral with its quote; the verifier fetches it itself.
   */
  readonly includeCollateral?: boolean;
}

export interface SealedFetchOptions {
  /** The node's base URL. */
  baseUrl: string;
  /**
   * The node's attested transport key, or a function that attests and returns
   * it — typically `() => fetchAttestedKey(baseUrl, policy)`. Given a
   * function, a restarted node is handled: the key is attested again, through
   * your verifier, and the request retried once. Given bytes, a restart
   * surfaces as {@link StaleTransportKeyError}.
   */
  transportPublicKey: Uint8Array | (() => Promise<Uint8Array>);
  /** Fetch that carries the sealed envelope. Defaults to global `fetch`. */
  fetch?: typeof fetch;
}

/**
 * A `fetch` that seals every request to the node's attested transport key.
 *
 * A request for any URL outside `baseUrl` is refused rather than sent in the
 * clear: falling back to plaintext would silently undo the point of sealing.
 */
export function createSealedFetch(options: SealedFetchOptions): typeof fetch {
  const base = new URL(options.baseUrl);
  const basePath = base.pathname.replace(/\/+$/, '');
  const nodeUrl = `${base.origin}${basePath}`;
  const baseFetch = options.fetch ?? ((input, init) => fetch(input, init));
  const attest =
    typeof options.transportPublicKey === 'function' ? options.transportPublicKey : undefined;
  let transportKey: Promise<Uint8Array> = attest
    ? Promise.resolve().then(attest)
    : Promise.resolve(options.transportPublicKey as Uint8Array);
  // Attesting starts now, before any request awaits it. A node that fails to
  // attest must surface on the request that needed the key, not earlier as an
  // unhandled rejection; the failure is still thrown to every awaiter.
  transportKey.catch(() => undefined);
  let session: Promise<Session> | undefined;

  const openSession = async (): Promise<Session> => {
    try {
      return await handshake(baseFetch, nodeUrl, await checkedKey(transportKey));
    } catch (error) {
      if (!(error instanceof StaleTransportKeyError) || !attest) throw error;
      transportKey = Promise.resolve().then(attest);
      return handshake(baseFetch, nodeUrl, await checkedKey(transportKey));
    }
  };
  /** The session to use now, and the promise it came from, to name it if it turns out stale. */
  const currentSession = async (): Promise<[Session, Promise<Session>]> => {
    const pending = session ?? renew(undefined);
    const open = await pending;
    if (Date.now() < open.renewAt) return [open, pending];
    const renewed = renew(pending);
    return [await renewed, renewed];
  };
  // Replace the session only if it is still the one found stale, so concurrent
  // requests that all find it stale open one new session between them.
  const renew = (stale: Promise<Session> | undefined): Promise<Session> => {
    if (session === stale) {
      const opening = openSession();
      session = opening;
      opening.catch(() => {
        if (session === opening) session = undefined;
      });
    }
    return session as Promise<Session>;
  };

  return async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const request = new Request(input, init);
    const url = new URL(request.url);
    if (url.origin !== base.origin || !url.pathname.startsWith(`${basePath}/`)) {
      throw new Error(`Refusing to send ${url.href} outside the sealed node at ${options.baseUrl}`);
    }
    const headers: Array<[string, string]> = [];
    request.headers.forEach((value, name) => headers.push([name, value]));
    const head = { method: request.method, path: url.pathname.slice(basePath.length) + url.search, headers };
    const body = new Uint8Array(await request.arrayBuffer());
    // The caller's own signal, not `request.signal`: a Request's signal follows
    // the one it was built from only through a weak reference (undici), so once
    // the Request is garbage collected (it is, while a stream stays open) an
    // abort no longer reaches the fetch, and an event stream never closes.
    const signal = init?.signal ?? (input instanceof Request ? input.signal : undefined);

    const send = async (open: Session): Promise<Response> => {
      const { requestId, header, envelope } = await sealRequest(open, head, body);
      const response = await baseFetch(`${nodeUrl}${SEALED_PATH}`, {
        method: 'POST',
        headers: { 'content-type': SEALED_CONTENT_TYPE },
        body: envelope,
        signal,
      });
      if (response.headers.get('content-type') !== SEALED_CONTENT_TYPE) {
        throw await envelopeRefusal(response);
      }
      return openResponse(response, open.responseKey, header, requestId);
    };

    const [open, pending] = await currentSession();
    try {
      return await send(open);
    } catch (error) {
      // The node refused the envelope before running anything in it, so the
      // request can be sent again safely under a new session.
      if (!(error instanceof SealedTransportError) || error.code !== 'unknown_session') throw error;
      return send(await renew(pending));
    }
  };
}

async function checkedKey(key: Promise<Uint8Array>): Promise<Uint8Array> {
  const bytes = await key;
  if (bytes.length !== 32) throw new Error('transportPublicKey must be 32 bytes');
  return bytes;
}

export interface Session {
  requestKey: CryptoKey;
  responseKey: CryptoKey;
  nextRequestId: number;
  renewAt: number;
  header(requestId: number): Uint8Array;
}

async function handshake(
  baseFetch: typeof fetch,
  nodeUrl: string,
  transportKey: Uint8Array,
): Promise<Session> {
  try {
    return await handshakeOnce(baseFetch, nodeUrl, transportKey);
  } catch (error) {
    // The node limits how fast sessions open. A refused handshake opened
    // nothing, so trying once more after the pause it asks for is safe.
    if (!(error instanceof SealedTransportError) || error.code !== 'busy') throw error;
    await new Promise((resolve) => setTimeout(resolve, error.retryAfterMs ?? 1000));
    return handshakeOnce(baseFetch, nodeUrl, transportKey);
  }
}

async function handshakeOnce(
  baseFetch: typeof fetch,
  nodeUrl: string,
  transportKey: Uint8Array,
): Promise<Session> {
  const noise = await initiate(transportKey, PROLOGUE);
  const response = await postHandshake(
    baseFetch,
    nodeUrl,
    concat(new Uint8Array([VERSION]), transportKey, noise.message1),
  );
  if (response.headers.get('content-type') !== SEALED_CONTENT_TYPE) {
    throw await envelopeRefusal(response);
  }
  const reply = await readAtMost(response, MAX_HANDSHAKE_REPLY);
  return sessionFrom(noise, reply);
}

/**
 * Post handshake message 1 to {@link HANDSHAKE_PATH}; a node from before the
 * path gets the same message once at {@link LEGACY_HANDSHAKE_PATH}. Such a node
 * routes nothing there (404, 405) or, sealing everything as Cloud nodes do,
 * refuses it as an unsealed request (403 `sealed_required`). That node never
 * read the first message, so sending it again opens nothing twice. Any other
 * answer is returned as it is.
 */
async function postHandshake(
  baseFetch: typeof fetch,
  nodeUrl: string,
  body: Uint8Array,
): Promise<Response> {
  const post = (path: string) =>
    baseFetch(`${nodeUrl}${path}`, {
      method: 'POST',
      headers: { 'content-type': SEALED_CONTENT_TYPE },
      body,
    });
  const response = await post(HANDSHAKE_PATH);
  if (!(await predatesHandshakePath(response))) return response;
  await response.body?.cancel();
  return post(LEGACY_HANDSHAKE_PATH);
}

/** Whether a handshake answer says the node predates {@link HANDSHAKE_PATH} (see `postHandshake`). */
async function predatesHandshakePath(response: Response): Promise<boolean> {
  if (response.status === 404 || response.status === 405) return true;
  if (response.status !== 403) return false;
  try {
    const body = (await response.clone().json()) as { error?: { code?: string } };
    return body?.error?.code === 'sealed_required';
  } catch {
    return false;
  }
}

/** Finish the handshake with the node's reply. Exported for the published vectors. */
export async function sessionFrom(
  noise: Awaited<ReturnType<typeof initiate>>,
  reply: Uint8Array,
): Promise<Session> {
  if (reply[0] !== VERSION) throw new Error('The handshake reply is malformed');
  let finished: Awaited<ReturnType<(typeof noise)['finish']>>;
  try {
    finished = await noise.finish(reply.slice(1));
  } catch {
    throw new Error('The handshake reply did not open: it did not come from the attested node');
  }
  const { payload, split } = finished;
  if (payload.length !== SESSION_ID_LEN + 4) throw new Error('The handshake reply is malformed');
  const sessionId = payload.slice(0, SESSION_ID_LEN);
  const lifetimeSecs = new DataView(payload.buffer, payload.byteOffset).getUint32(SESSION_ID_LEN, false);
  const [requestKey, responseKey] = await Promise.all([
    crypto.subtle.importKey('raw', split[0], 'AES-GCM', false, ['encrypt']),
    crypto.subtle.importKey('raw', split[1], 'AES-GCM', false, ['decrypt']),
  ]);
  split[0].fill(0);
  split[1].fill(0);
  return {
    requestKey,
    responseKey,
    nextRequestId: 1,
    renewAt: Date.now() + lifetimeSecs * 1000 - RENEW_BEFORE_MS,
    header(requestId) {
      const header = new Uint8Array(HEADER_LEN);
      header[0] = VERSION;
      header.set(sessionId, 1);
      new DataView(header.buffer).setBigUint64(1 + SESSION_ID_LEN, BigInt(requestId), false);
      return header;
    },
  };
}

/** Seal one request under the session's next request id. */
export async function sealRequest(
  session: Session,
  head: RequestHead,
  body: Uint8Array,
): Promise<{ requestId: number; header: Uint8Array; envelope: Uint8Array }> {
  const requestId = session.nextRequestId;
  session.nextRequestId += 1;
  const header = session.header(requestId);
  const sealed = await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv: nonce(requestId, 0), additionalData: header },
    session.requestKey,
    joinInner(head, body),
  );
  return { requestId, header, envelope: concat(header, new Uint8Array(sealed)) };
}

/** `request_id || index`, as the node numbers them. */
function nonce(requestId: number, index: number): Uint8Array {
  const bytes = new Uint8Array(12);
  const view = new DataView(bytes.buffer);
  view.setBigUint64(0, BigInt(requestId), false);
  view.setUint32(8, index, false);
  return bytes;
}

export interface RequestHead {
  method: string;
  path: string;
  headers: Array<[string, string]>;
}

export interface ResponseHead {
  status: number;
  headers: Array<[string, string]>;
}

function joinInner(head: RequestHead, body: Uint8Array): Uint8Array {
  const encoded = utf8.encode(JSON.stringify(head));
  const len = new Uint8Array(4);
  new DataView(len.buffer).setUint32(0, encoded.length, false);
  return concat(len, encoded, body);
}

/**
 * Open a sealed response as it streams: wait for its head, then hand back a
 * `Response` whose body opens each frame as it arrives. A frame that does not
 * open, arrives out of order, or a stream that stops before its end frame
 * errors the body rather than passing anything unauthenticated on.
 */
export async function openResponse(
  response: Response,
  responseKey: CryptoKey,
  header: Uint8Array,
  requestId: number,
): Promise<Response> {
  if (!response.body) throw new Error('The sealed response has no body');
  const frames = frameReader(response.body.getReader());
  let index = 0;
  const next = async (): Promise<{ kind: number; data: Uint8Array } | null> => {
    const sealed = await frames.next();
    if (!sealed) return null;
    let plain: Uint8Array;
    try {
      plain = new Uint8Array(
        await crypto.subtle.decrypt(
          { name: 'AES-GCM', iv: nonce(requestId, index), additionalData: header },
          responseKey,
          sealed,
        ),
      );
    } catch {
      throw new Error('A sealed response frame did not open: it was not sealed by the attested node');
    }
    index += 1;
    return { kind: plain[0], data: plain.slice(1) };
  };

  const first = await next();
  if (!first || first.kind !== FRAME_HEAD) throw new Error('The sealed response is malformed');
  const head = JSON.parse(new TextDecoder().decode(first.data)) as ResponseHead;
  if (NULL_BODY_STATUSES.includes(head.status)) {
    await frames.cancel();
    return new Response(null, { status: head.status, headers: head.headers });
  }

  const body = new ReadableStream<Uint8Array>({
    async pull(controller) {
      try {
        const frame = await next();
        if (!frame) {
          controller.error(new Error('The sealed response was cut short'));
        } else if (frame.kind === FRAME_DATA) {
          controller.enqueue(frame.data);
        } else if (frame.kind === FRAME_END) {
          controller.close();
          await frames.cancel();
        } else {
          controller.error(new Error('The sealed response is malformed'));
        }
      } catch (error) {
        controller.error(error);
      }
    },
    cancel() {
      return frames.cancel();
    },
  });
  return new Response(body, { status: head.status, headers: head.headers });
}

/** Split a byte stream into length-prefixed frames. */
function frameReader(reader: ReadableStreamDefaultReader<Uint8Array>): {
  next(): Promise<Uint8Array | null>;
  cancel(): Promise<void>;
} {
  let buffer = new Uint8Array();
  const fill = async (length: number): Promise<boolean> => {
    while (buffer.length < length) {
      const { done, value } = await reader.read();
      if (done) return false;
      buffer = concat(buffer, value);
    }
    return true;
  };
  return {
    async next() {
      if (!(await fill(4))) return null;
      const length = new DataView(buffer.buffer, buffer.byteOffset).getUint32(0, false);
      if (length > MAX_FRAME_LEN) {
        await reader.cancel();
        throw new Error('The sealed response is malformed: a frame is larger than the node sends');
      }
      if (!(await fill(4 + length))) return null;
      const frame = buffer.slice(4, 4 + length);
      buffer = buffer.slice(4 + length);
      return frame;
    },
    cancel: () => reader.cancel(),
  };
}

/** Read a body, refusing one longer than `limit` rather than buffering it. */
async function readAtMost(response: Response, limit: number): Promise<Uint8Array> {
  if (!response.body) return new Uint8Array();
  const reader = response.body.getReader();
  let bytes = new Uint8Array();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) return bytes;
    bytes = concat(bytes, value);
    if (bytes.length > limit) {
      await reader.cancel();
      throw new Error('The handshake reply is malformed: it is too long');
    }
  }
}

async function envelopeRefusal(response: Response): Promise<Error> {
  let code: string | undefined;
  let message = `Sealed request refused: HTTP ${response.status}`;
  try {
    const body = (await response.json()) as { error?: { code?: string; message?: string } };
    code = body?.error?.code;
    if (body?.error?.message) message = `Sealed request refused: ${body.error.message}`;
  } catch {
    // Not the node's JSON refusal — a proxy's page, most likely. The status is
    // all that can honestly be reported.
  }
  if (code === 'stale_transport_key') return new StaleTransportKeyError();
  const retryAfter = Number(response.headers.get('retry-after'));
  const retryAfterMs =
    Number.isFinite(retryAfter) && retryAfter > 0 ? Math.min(retryAfter, 5) * 1000 : undefined;
  return new SealedTransportError(response.status, code, message, retryAfterMs);
}
