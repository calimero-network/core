/**
 * The runtime every route of a Calimero node is sent through: the part of
 * this client that is not a route.
 *
 * A route layer (the one generated from the node's OpenAPI document:
 * `createRouteClient` and `createRoutes` in `../api`, which {@link CalimeroClient}
 * exposes) knows a method, a path and what the route takes and answers.
 * Everything else is here, behind one call, {@link Transport.request}:
 *
 * - the sealed channel to each node, opened only after the node's attestation
 *   passed the caller's policy (`../sealed`, `./attestation`);
 * - the credential (an API key or a session), and the session's refresh when a
 *   node answers `401` to it;
 * - moving to another node of the cluster on `421 WRONG_NODE`, and failing
 *   over when one stays unavailable to a call or a view (D70 retries);
 * - the `412 CONFIRM_REQUIRED` step-up: the passkey's assertion over the
 *   node's challenge, sent with the same body again;
 * - error mapping (a refusal is a typed {@link CalimeroError}) and
 *   server-sent event decoding.
 *
 * Paths are relative to the node (`apps/{app}/intents`, a leading `/` is
 * dropped); {@link routePath} fills a path template with encoded segments.
 */

import { SealedTransportError, createSealedFetch } from '../sealed/sealed.js';
import { hex } from '../crypto/internal.js';
import { Unanswered, verifyNode, type NodePolicy, type NodeVerification } from './attestation.js';
import { orderNodes, readDirectory } from './directory.js';
import {
  CalimeroError,
  ConfirmRequired,
  LoginRequired,
  UnavailableError,
  WrongNode,
  errorFrom,
} from './errors.js';
import { b64url, type PasskeyProvider, type PasskeyUserVerification } from './passkey.js';
import { passkeyConfirm, type ConfirmChallenge } from './passkey.js';
import type { components } from '../generated/api.js';

/** A signed-in session: what sign-up, sign-in and a refresh answer. */
export type CalimeroSession = components['schemas']['Session'];

/**
 * Where a node stands with the client's check: `refused` failed verification, `unreachable`
 * gave no answer (retried on next use).
 */
export type NodeVerificationState =
  | { status: 'checking'; url: string }
  | ({ status: 'verified' } & NodeVerification)
  | { status: 'refused' | 'unreachable'; url: string; reason: string; checkedAt: number };

/** One server-sent event: `data` parsed as JSON when it is. */
export interface NodeEvent {
  id: string;
  kind: string;
  data: unknown;
}

/**
 * How a call or a view is sent again while the node is unavailable (D70): with
 * the same key, after a backoff that starts at `baseDelayMs` and doubles up to
 * `maxDelayMs`, no jitter, at most `retries` times. The defaults are the Rust
 * client's: 6 retries, 200 ms doubling, at most 2 s (about 9 s in all, past a
 * leader election).
 */
export interface RetryOptions {
  /** Retries after the first attempt. Default 6. */
  retries?: number;
  /** The first backoff; each retry doubles it. Default 200 ms. */
  baseDelayMs?: number;
  /** The longest backoff. Default 2 s. */
  maxDelayMs?: number;
}

export const DEFAULT_RETRY = { retries: 6, baseDelayMs: 200, maxDelayMs: 2000 } as const;

/** The backoff before retry `retry` (0 for the first): `baseDelayMs` doubling, at most `maxDelayMs`. */
export function retryDelayMs(retry: number, options: { baseDelayMs: number; maxDelayMs: number }): number {
  return Math.min(options.baseDelayMs * 2 ** retry, options.maxDelayMs);
}

/** The HTTP methods the node's routes use. */
export type HttpMethod = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';

/** A query parameter's value; `undefined` and `null` leave it out, an array repeats it. */
export type QueryValue = string | number | boolean | null | undefined;

/** What a request carries beyond its method and path. */
export interface RequestOptions {
  /** Query parameters, appended to the path (encoded here). */
  query?: Record<string, QueryValue | QueryValue[]>;
  /** A JSON body. */
  body?: unknown;
  /** A raw body instead of JSON (a bundle, a package, a blob). */
  bytes?: Uint8Array;
  /** The raw body's content type. Default `application/wasm`. */
  contentType?: string;
  /**
   * Answers a `412 CONFIRM_REQUIRED`: the passkey asserts over the node's
   * challenge and the same JSON body is sent again with `confirm`. Without
   * one the `412` reaches the caller as {@link ConfirmRequired}.
   */
  passkey?: PasskeyProvider;
  /** The user verification the step-up asks for when the node names none. Default `required`. */
  userVerification?: PasskeyUserVerification;
  /**
   * D70 retries for a call or a view: `true` sends it again while the node
   * answers `503` or cannot be reached, then fails over to the next node;
   * `'once'` sends it exactly once (a confirmed attempt, whose challenge is
   * single-use). Only for a JSON body that carries its own idempotency key,
   * or a read.
   */
  retry?: boolean | 'once';
  /** Aborts the request (and a stream), and the step-up's passkey prompt. */
  signal?: AbortSignal;
  /** Aborts only the step-up's passkey prompt, not the requests around it. Default `signal`. */
  promptSignal?: AbortSignal;
}

/**
 * What a route layer calls. Every method sends through the attested, sealed
 * channel of the node in use, with the credential held now. A refusal (any
 * status other than 2xx) is thrown as a typed {@link CalimeroError}
 * ({@link errorFrom}); a transport failure as the error that caused it.
 */
export interface Transport {
  /** The response to `method path`, status 2xx; its body is not read. */
  request(method: HttpMethod, path: string, options?: RequestOptions): Promise<Response>;
  /** {@link request}'s JSON body (`{}` for an empty one). */
  json<T = Record<string, unknown>>(method: HttpMethod, path: string, options?: RequestOptions): Promise<T>;
  /** A server-sent event stream on `GET path`, until the stream ends or `signal` aborts. */
  events(path: string, options?: Pick<RequestOptions, 'query' | 'signal'>): AsyncGenerator<NodeEvent>;
}

/** How {@link NodeTransport} reaches a cluster's nodes. */
export interface NodeTransportOptions {
  /** One node of the cluster: the one the transport talks to first. */
  baseUrl: string;
  /** Other nodes of the same cluster, in the order to move to them. */
  nodes?: string[];
  /** What every node must attest to before anything is sent to it. */
  policy: NodePolicy;
  /** Fetch that carries attestation and envelopes. Defaults to global `fetch`. */
  fetch?: typeof fetch;
  /** How calls and views retry a 503 or a failed connection. */
  retry?: RetryOptions;
}

/** Where a transport whose nodes came from the directory re-reads its node list. */
export interface DirectorySource {
  directory: string;
  cluster: string;
  zone: string | undefined;
  random: () => number;
  /** When the listing is due to be read again (ms since the epoch). */
  due: number;
  /** The re-read under way, if any. */
  reading?: Promise<void>;
}

/**
 * The node refuses an envelope past its 64 MiB sealed cap with `413` in the
 * clear, before opening it, so the refusal arrives as the transport's. To the
 * caller it is the API's `TOO_LARGE`, as the Rust client reports it.
 */
function sealedError(error: unknown): unknown {
  if (error instanceof SealedTransportError && error.status === 413) {
    return errorFrom(413, {
      error: 'TOO_LARGE',
      message: error.message.replace(/^Sealed request refused: /, ''),
    });
  }
  return error;
}

/** Read a refused response's body and map it ({@link errorFrom}). */
export async function refusal(response: Response): Promise<CalimeroError> {
  const text = await response.text().catch(() => '');
  let body: unknown = {};
  try {
    body = text ? JSON.parse(text) : {};
  } catch {
    body = { message: text };
  }
  return errorFrom(response.status, body, response.headers.get('retry-after'));
}

/** Server-sent event frames from a response body: `{ id, kind, data }` each, `data` parsed as JSON when it is. */
export async function* sseFrames(body: ReadableStream<Uint8Array>): AsyncGenerator<NodeEvent> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  for (;;) {
    const { value, done } = await reader.read();
    if (done) return;
    buffer += decoder.decode(value, { stream: true });
    let cut: number;
    while ((cut = buffer.indexOf('\n\n')) >= 0) {
      const frame = buffer.slice(0, cut);
      buffer = buffer.slice(cut + 2);
      let id = '';
      let kind = '';
      let data = '';
      for (const line of frame.split('\n')) {
        if (line.startsWith('id:')) id = line.slice(3).trim();
        else if (line.startsWith('event:')) kind = line.slice(6).trim();
        else if (line.startsWith('data:')) data += line.slice(5).trim();
      }
      if (!kind && !data) continue;
      let parsed: unknown = data;
      try {
        parsed = JSON.parse(data);
      } catch {
        /* plain text */
      }
      yield { id, kind, data: parsed };
    }
  }
}

/**
 * A key for a call the caller gave none, made once so every retry shares it:
 * `node:c-` and 16 random bytes in base64url without padding, as the Rust
 * client mints it. It only has to outlive this client's own retries, so it
 * takes the node's short-lived class: a key starting `node:` is kept 10
 * minutes, a caller's own key 24 hours.
 * `getRandomValues`, not `randomUUID`, which a page outside a secure context lacks.
 */
export function newIdempotencyKey(): string {
  return `node:c-${b64url(crypto.getRandomValues(new Uint8Array(16)))}`;
}

/**
 * `template` (`apps/{app}/blobs/{id}`) with each `{name}` replaced by
 * `params[name]`, encoded as one path segment so an id cannot reach another
 * route. A name `params` lacks throws.
 */
export function routePath(template: string, params: Record<string, string | number> = {}): string {
  return template.replace(/\{([^}]+)\}/g, (_, name: string) => {
    const value = params[name];
    if (value === undefined) throw new Error(`no value for path parameter ${name} of ${template}`);
    return encodeURIComponent(String(value));
  });
}

/** `query` as a search string (`?a=1&b=2`), or `''` when it sets nothing. */
export function queryString(query: RequestOptions['query']): string {
  if (!query) return '';
  const search = new URLSearchParams();
  for (const [name, value] of Object.entries(query)) {
    for (const one of Array.isArray(value) ? value : [value]) {
      if (one !== undefined && one !== null) search.append(name, String(one));
    }
  }
  const text = search.toString();
  return text ? `?${text}` : '';
}

/**
 * The ceremonies that mint a session, which {@link NodeTransport} never
 * repeats by itself: a sign-in or sign-up is started again by its caller (with
 * the passkey), and a refresh moves through the nodes in
 * {@link NodeTransport.refresh}. A node answers them `421 WRONG_NODE` before
 * it spends the challenge or the refresh token (core #4774).
 */
function ceremony(path: string): boolean {
  return path === 'auth/refresh' || path.startsWith('auth/passkey/') || RECOVERY_CEREMONIES.has(path);
}

/** The signed-out halves of an account recovery: a `401` there is the code's, not a session's. */
const RECOVERY_CEREMONIES = new Set(['auth/recovery/begin', 'auth/recovery/start', 'auth/recovery/finish']);

/** The route path a request names: no leading `/`. */
function relative(path: string): string {
  return path.replace(/^\/+/, '');
}

/**
 * The {@link Transport} to one Calimero cluster: its nodes, each attested
 * under one policy before its first request, the credential, and the session's
 * refresh. {@link CalimeroClient} is built on one (`client.transport`); a
 * generated route layer takes one too.
 */
export class NodeTransport implements Transport {
  /** Every node this transport may use, `baseUrl` first (or as the directory last ordered them). */
  private nodeUrls: string[];
  /** The one in use: an index into {@link nodeUrls}. */
  private at = 0;
  /** A sealed fetch per node, each attesting its own node on first use. */
  private readonly channels = new Map<string, typeof fetch>();
  /** The latest attestation check of each node, and the attempt it came from. */
  private readonly checks = new Map<string, NodeVerificationState>();
  private readonly attempts = new Map<string, Promise<unknown>>();
  /** The transport key each node's channel was attested to. */
  private readonly keys = new Map<string, Uint8Array>();
  private readonly policy: NodePolicy;
  /** The unsealed fetch, for the one route served in the clear (`/attestation`). */
  readonly raw: typeof fetch;
  private auth: string | undefined;
  private current: CalimeroSession | undefined;
  /** The refresh under way, which every request that needs one shares. */
  private refreshing: Promise<CalimeroSession> | undefined;
  /** The backoff between retries, for a caller that retries a request the routes do not. */
  readonly retry: Required<RetryOptions>;
  /** Rejections of the underlying `fetch` itself: the node could not be reached or dropped the connection. */
  private readonly networkFailures = new WeakSet<object>();
  /** The directory the node list came from, if it came from one. */
  private source: DirectorySource | undefined;

  constructor(options: NodeTransportOptions) {
    const trim = (url: string) => url.replace(/\/+$/, '');
    this.nodeUrls = [...new Set([options.baseUrl, ...(options.nodes ?? [])].map(trim))];
    this.policy = options.policy;
    this.retry = { ...DEFAULT_RETRY, ...options.retry };
    const given = options.fetch ?? ((input, init) => fetch(input, init));
    // `fetch` rejects with a `TypeError` when it gets no answer: the connection
    // was refused, reset or dropped before the response. The sealed fetch passes
    // that rejection on unchanged, so it is recognised here, at its source, and
    // not by its type alone (a bad URL is a `TypeError` too, and is not retried).
    const raw: typeof fetch = async (input, init) => {
      try {
        return await given(input, init);
      } catch (error) {
        if (error instanceof TypeError) this.networkFailures.add(error);
        throw error;
      }
    };
    this.raw = raw;
  }

  /** Whether `error` is a host that gave no answer: a `fetch` that failed, or an attestation input left {@link Unanswered}. */
  isNetworkFailure(error: unknown): boolean {
    return (error instanceof TypeError && this.networkFailures.has(error)) || error instanceof Unanswered;
  }

  /** Re-read the node list from `source` when it is due, and after a failover. */
  useDirectory(source: DirectorySource): void {
    this.source = source;
  }

  /** The node requests go to now: `baseUrl`, until a `421 WRONG_NODE` or a failover moved on. */
  get nodeUrl(): string {
    return this.nodeUrls[this.at];
  }

  /** Every node this transport may use, in the order it moves through them. */
  get nodes(): string[] {
    return [...this.nodeUrls];
  }

  /** Authenticate every request with an API key. */
  useApiKey(key: string): void {
    this.auth = key;
    this.current = undefined;
  }

  /** Authenticate every request with `session`, refreshed when a node answers `401` to it. */
  useSession(session: CalimeroSession): void {
    this.auth = session.sessionToken;
    this.current = session;
  }

  /** Drop the credential: no request after this carries it. */
  signOut(): void {
    this.auth = undefined;
    this.current = undefined;
  }

  get session(): CalimeroSession | undefined {
    return this.current;
  }

  /** @internal The policy nodes are attested under, for the client's own verification (not part of the public API). */
  get nodePolicy(): NodePolicy | undefined {
    return this.policy;
  }

  /** The sealed channel to `url`, attested under the policy before its first request. */
  private channel(url: string): typeof fetch {
    let channel = this.channels.get(url);
    if (!channel) {
      // A channel that has been dropped records nothing: only the one in use speaks for the node.
      const live = () => this.channels.get(url) === sealed;
      let first: Promise<Uint8Array> | undefined;
      const sealed: typeof fetch = createSealedFetch({
        baseUrl: url,
        transportPublicKey: () => {
          const key = first ?? this.attest(url, live);
          first = undefined;
          return key;
        },
        fetch: this.raw,
      });
      this.channels.set(url, sealed);
      first = this.attest(url, live);
      channel = sealed;
    }
    return channel;
  }

  /** Attest node `url` for the channel `live` tells is in use, keeping what the check verified or why it failed. */
  private attest(url: string, live: () => boolean): Promise<Uint8Array> {
    if (!live()) return verifyNode(url, this.policy, this.raw).then(({ key }) => key);
    this.checks.set(url, { status: 'checking', url });
    const attempt: Promise<Uint8Array> = verifyNode(url, this.policy, this.raw).then(
      ({ key, verification }) => {
        if (live()) {
          this.checks.set(url, { status: 'verified', ...verification });
          this.keys.set(url, key);
        }
        return key;
      },
      (error: unknown) => {
        const reason = error instanceof Error ? error.message : String(error);
        const unreachable = this.isNetworkFailure(error);
        if (live()) {
          this.checks.set(url, { status: unreachable ? 'unreachable' : 'refused', url, reason, checkedAt: Date.now() });
          // No answer says nothing about the node: the next use attests on a new channel.
          if (unreachable) this.channels.delete(url);
        }
        throw error;
      },
    );
    // A failure surfaces on the request that needed the key, not as an unhandled rejection.
    attempt.catch(() => undefined);
    this.attempts.set(url, attempt);
    return attempt;
  }

  /** What the client's check of node `url` (a URL from `nodes`; default: the one in use) established; `undefined` before it is first used. */
  verification(url: string = this.nodeUrl): NodeVerificationState | undefined {
    return this.checks.get(url);
  }

  /**
   * Attest `app` on node `url` (a URL from `nodes`; default: the one in use): the record's
   * `appHash` is the one the node bound. Throws on refusal, or when the quote's key is not the channel's.
   */
  async attestApp(app: string, url: string = this.nodeUrl): Promise<NodeVerification> {
    const pinned = this.policy.app === app ? this.policy.appHash : undefined;
    const policy = { ...this.policy, app, appHash: pinned };
    await this.settled(url);
    let attested = await verifyNode(url, policy, this.raw);
    if (!this.sameKey(url, attested.key)) {
      // The node may have restarted: the channel's own check, on a new channel, decides which key requests use.
      this.channels.delete(url);
      await this.settled(url);
      attested = await verifyNode(url, policy, this.raw);
      if (!this.sameKey(url, attested.key)) {
        throw new Error('The node attested the app under another transport key than its channel');
      }
    }
    return attested.verification;
  }

  /** The settled check of node `url`; throws the reason when it failed. */
  private async settled(url: string): Promise<void> {
    const state = await this.verify(url);
    if (state.status === 'refused' || state.status === 'unreachable') throw new Error(state.reason);
  }

  private sameKey(url: string, key: Uint8Array): boolean {
    const channelKey = this.keys.get(url);
    return channelKey !== undefined && hex(channelKey) === hex(key);
  }

  /**
   * Start the check of node `url` if none runs and answer its settled outcome, without a request or a repeat.
   * `checkedAt` is when that check ran; a failure is answered, not thrown.
   */
  async verify(url: string = this.nodeUrl): Promise<NodeVerificationState> {
    this.channel(url);
    for (;;) {
      const attempt = this.attempts.get(url);
      await attempt?.catch(() => undefined);
      if (this.attempts.get(url) === attempt) return this.checks.get(url) as NodeVerificationState;
    }
  }

  /**
   * Move on from node `from` (the one that answered `421 WRONG_NODE`) to the
   * next one. False when there is no other. A request that saw the same node
   * refuse after another request moved on already does not move again.
   */
  private moveOn(from: string): boolean {
    if (this.nodeUrls.length < 2) return false;
    if (this.nodeUrl === from) this.at = (this.at + 1) % this.nodeUrls.length;
    return true;
  }

  /**
   * Fail over from node `from`, where a request's retries ran out, to the next
   * node it has not `tried`. False when it has tried them all. A request whose
   * node another request moved away from already goes to that node, if untried.
   */
  private failOver(from: string, tried: Set<string>): boolean {
    if (this.nodeUrl !== from && !tried.has(this.nodeUrl)) return true;
    const count = this.nodeUrls.length;
    for (let step = 1; step < count; step++) {
      const next = (this.at + step) % count;
      if (!tried.has(this.nodeUrls[next])) {
        this.at = next;
        this.reread();
        return true;
      }
    }
    return false;
  }

  /**
   * Read the node list again from the directory, in the background: the node
   * in use stays first, then the directory's order; a node no longer listed is
   * dropped. A failed read is ignored. One read at a time.
   */
  private reread(): void {
    const source = this.source;
    if (!source || source.reading) return;
    source.reading = readDirectory(source.directory, source.cluster, this.raw)
      .then((listing) => {
        const current = this.nodeUrl;
        const listed = orderNodes(listing.nodes, source.zone, source.random);
        this.nodeUrls = [current, ...listed.filter((url) => url !== current)];
        this.at = 0;
        source.due = Date.now() + listing.refreshAfterSeconds * 1000;
      })
      .catch(() => undefined)
      .finally(() => {
        source.reading = undefined;
      });
  }

  /**
   * Send one request to the node in use, with the credential held now, and
   * answer the response, whatever its status. {@link send} judges it.
   */
  private async once(method: string, path: string, options: RequestOptions): Promise<Response> {
    const headers: Record<string, string> = {};
    if (this.auth) headers.authorization = `Bearer ${this.auth}`;
    let payload: BodyInit | undefined;
    if (options.bytes) {
      headers['content-type'] = options.contentType ?? 'application/wasm';
      payload = options.bytes as unknown as BodyInit;
    } else if (options.body !== undefined) {
      headers['content-type'] = 'application/json';
      payload = JSON.stringify(options.body);
    }
    if (this.source && Date.now() >= this.source.due) this.reread();
    const url = this.nodeUrl;
    try {
      return await this.channel(url)(`${url}/${path}${queryString(options.query)}`, {
        method,
        headers,
        body: payload,
        signal: options.signal,
      });
    } catch (error) {
      throw sealedError(error);
    }
  }

  /**
   * {@link once}, answering what a lost issuing node or a node on the wrong
   * release does to a session: a successful response, or the refusal as a
   * typed error ({@link errorFrom}).
   *
   * - `401 UNAUTHENTICATED` to a request that carried a session: the node that
   *   signed the session may be gone, replaced, or out of reach of the node
   *   asked, which any node of the cluster answers with a refresh. The
   *   transport refreshes once, shared by every request that needs it at the
   *   same time ({@link refresh}), and repeats the request once. A refresh that
   *   is refused surfaces as {@link LoginRequired}.
   * - `421 WRONG_NODE`: this node's release may not serve the account. The
   *   transport moves to the next node it knows and repeats the request once;
   *   with none, the {@link WrongNode} reaches the caller.
   *
   * Repeating is safe even for a request that is not idempotent: the node
   * answers `401` and `421` when it judges the credential, before it reads what
   * the request asks, so the first attempt ran nothing. The exceptions are the
   * ceremonies that mint a session ({@link ceremony}), never repeated here.
   */
  private async send(method: string, path: string, options: RequestOptions): Promise<Response> {
    const response = await this.answer(method, path, options);
    if (!response.ok) throw await refusal(response);
    return response;
  }

  /**
   * {@link send}'s answer, a refusal included: the response after the
   * session's refresh and the move past a `421 WRONG_NODE`, whatever its
   * status. The generated route layer reads a refusal itself.
   */
  private async answer(method: string, path: string, options: RequestOptions): Promise<Response> {
    let refreshed = false;
    let moved = false;
    for (;;) {
      const [token, at] = [this.auth, this.nodeUrl];
      const response = await this.once(method, path, options);
      if (response.ok || (response.status !== 401 && response.status !== 421)) return response;
      const error = await refusal(response.clone());
      if (error instanceof WrongNode && !moved) {
        moved = true;
        if (this.moveOn(at) && !ceremony(path)) continue;
      } else if (error instanceof LoginRequired && !refreshed && this.mayRefresh(token, path)) {
        refreshed = true;
        await this.renew(token as string);
        continue;
      }
      return response;
    }
  }

  /** Whether a `401` to a request sent with `token` is one a refresh may answer. */
  private mayRefresh(token: string | undefined, path: string): boolean {
    return token !== undefined && this.current !== undefined && !ceremony(path);
  }

  /**
   * A fresh session in place of the one `token` belonged to: by the refresh
   * another request already made, or by one this request starts.
   */
  private async renew(token: string): Promise<void> {
    if (this.auth !== token) return;
    await this.refresh();
  }

  private async read<T>(method: string, path: string, options: RequestOptions): Promise<T> {
    const response = await this.send(method, path, options);
    const text = await response.text();
    return (text ? JSON.parse(text) : {}) as T;
  }

  /**
   * {@link read} for a call or a view (D70): while the node answers 503 or
   * cannot be reached, the same request, with the same idempotency key, is sent
   * again after {@link retryDelayMs} (or the pause the node asked for, if
   * longer), at most `retries` times. Every other answer is final.
   *
   * When a node's retries run out, the request fails over to the next node the
   * transport knows and its retries start again there, with the same key: the
   * key is kept in the app's replicated log, so the cluster answers it once
   * whichever node it reaches. Each node is tried once per request. The last
   * failure surfaces as {@link UnavailableError}: the node's 503, or reason
   * `network` for a connection that failed, whose `committed` is `'unknown'`.
   *
   * `once` sends the request exactly once, to the node in use (a confirmed
   * attempt, whose challenge is single-use).
   */
  private async retried<T>(method: string, path: string, options: RequestOptions, once: boolean): Promise<T> {
    const response = await this.answerRetried(method, path, options, once);
    if (!response.ok) throw await refusal(response);
    const text = await response.text();
    return (text ? JSON.parse(text) : {}) as T;
  }

  /**
   * {@link retried}'s answer: the last response, a refusal included (a `503`
   * whose retries ran out on every node too). A connection that failed on the
   * last node rejects with {@link UnavailableError} (reason `network`).
   */
  private async answerRetried(method: string, path: string, options: RequestOptions, once: boolean): Promise<Response> {
    const retries = once ? 0 : this.retry.retries;
    const tried = new Set<string>();
    for (;;) {
      let unavailable: UnavailableError | undefined;
      let last: Response | undefined;
      let node = this.nodeUrl;
      for (let attempt = 0; ; attempt++) {
        node = this.nodeUrl;
        tried.add(node);
        try {
          const response = await this.answer(method, path, options);
          if (response.status !== 503) return response;
          last = response;
          unavailable = (await refusal(response.clone())) as UnavailableError;
        } catch (error) {
          if (error instanceof TypeError && this.networkFailures.has(error)) {
            last = undefined;
            unavailable = new UnavailableError('NETWORK', `the node could not be reached: ${error.message}`, {}, undefined, error);
          } else throw error;
        }
        if (attempt >= retries) break;
        const backoff = retryDelayMs(attempt, this.retry);
        await new Promise((resolve) => setTimeout(resolve, Math.max(backoff, (unavailable.retryAfter ?? 0) * 1000)));
      }
      if (once || !this.failOver(node, tried)) {
        if (last) return last;
        throw unavailable;
      }
    }
  }

  /**
   * One request to a route, answered whatever its status: the runtime's part
   * (sealing, the session and its refresh, `421 WRONG_NODE`, and with
   * `options.retry` the D70 retries and failover) and nothing of the route's.
   * A refusal is the node's response, for the caller to read; a connection
   * that failed rejects as `fetch` did (with `retry`, as
   * {@link UnavailableError} once the retries ran out). The generated route
   * layer (`createRouteClient`) sends through this.
   */
  exchange(method: HttpMethod, path: string, options: RequestOptions = {}): Promise<Response> {
    const relativePath = relative(path);
    if (!options.retry) return this.answer(method, relativePath, options);
    return this.answerRetried(method, relativePath, options, options.retry === 'once');
  }

  /**
   * One JSON request, retried as `options.retry` says. The confirmed attempt
   * of a step-up (`body.confirm` set) goes out exactly once.
   */
  private attempt<T>(method: string, path: string, options: RequestOptions): Promise<T> {
    if (!options.retry) return this.read<T>(method, path, options);
    const confirming = (options.body as { confirm?: unknown } | undefined)?.confirm !== undefined;
    return this.retried<T>(method, path, options, options.retry === 'once' || confirming);
  }

  /**
   * {@link attempt}, answering a `412 CONFIRM_REQUIRED` with `passkey`'s
   * assertion over the node's challenge: the same body again, with `confirm`.
   * Without a passkey the `412` reaches the caller as {@link ConfirmRequired}.
   */
  private async confirmed<T>(method: string, path: string, options: RequestOptions): Promise<T> {
    const { passkey } = options;
    try {
      return await this.attempt<T>(method, path, options);
    } catch (error) {
      if (!(error instanceof ConfirmRequired) || !passkey) throw error;
      const challenge = error.details as ConfirmChallenge;
      const assertion = await passkeyConfirm(passkey, {
        userVerification: options.userVerification,
        signal: options.promptSignal ?? options.signal,
      })(challenge, { request: new Request(`${this.nodeUrl}/${path}`, { method }) });
      const body = { ...(options.body as Record<string, unknown>), confirm: { challengeId: challenge.challengeId, assertion } };
      return this.attempt<T>(method, path, { ...options, body });
    }
  }

  request(method: HttpMethod, path: string, options: RequestOptions = {}): Promise<Response> {
    return this.send(method, relative(path), options);
  }

  json<T = Record<string, unknown>>(method: HttpMethod, path: string, options: RequestOptions = {}): Promise<T> {
    return this.confirmed<T>(method, relative(path), options);
  }

  async *events(path: string, options: Pick<RequestOptions, 'query' | 'signal'> = {}): AsyncGenerator<NodeEvent> {
    const response = await this.send('GET', relative(path), options);
    if (!response.body) throw new CalimeroError(response.status, 'EVENTS', 'the event stream has no body');
    yield* sseFrames(response.body);
  }

  /**
   * `GET path` in the clear, from the node in use: only for the route a node
   * serves unsealed (`attestation`), and never with the credential.
   */
  async unsealed(path: string, options: Pick<RequestOptions, 'query'> = {}): Promise<Response> {
    const response = await this.inTheClear(path, options);
    if (!response.ok) throw await refusal(response);
    return response;
  }

  /**
   * {@link unsealed}'s answer, a refusal included: `GET path` in the clear,
   * without the credential, for the routes a node serves unsealed
   * (`attestation`, `health`).
   */
  inTheClear(path: string, options: Pick<RequestOptions, 'query' | 'signal'> = {}): Promise<Response> {
    return this.raw(`${this.nodeUrl}/${relative(path)}${queryString(options.query)}`, { signal: options.signal });
  }

  /**
   * Swap the refresh token for a new session, on whichever node the transport
   * uses: any node of the cluster takes it. A refresh token is used once, so
   * requests made while one refresh is under way share it rather than sending
   * the same token again (which would revoke the session).
   */
  refresh(): Promise<CalimeroSession> {
    if (!this.refreshing) {
      const pending = this.exchangeRefresh();
      this.refreshing = pending;
      const clear = () => {
        if (this.refreshing === pending) this.refreshing = undefined;
      };
      pending.then(clear, clear);
    }
    return this.refreshing;
  }

  private async exchangeRefresh(): Promise<CalimeroSession> {
    const current = this.current;
    if (!current) throw new Error('no session to refresh');
    let session: CalimeroSession | undefined;
    // A node that may not serve the account answers `421 WRONG_NODE` before it
    // spends the refresh token (core #4774), so the same token goes to the next
    // node (`send` moved there already): once per node, at most.
    for (let tried = 1; session === undefined; tried++) {
      try {
        session = await this.json<CalimeroSession>('POST', 'auth/refresh', { body: { refreshToken: current.refreshToken } });
      } catch (error) {
        if (!(error instanceof WrongNode) || tried >= this.nodeUrls.length) throw error;
      }
    }
    // A sign-in or another session taken while this one was refreshing wins.
    if (this.current === current) this.useSession(session);
    return session;
  }
}

/**
 * A {@link NodeTransport} to the cluster named `cluster`, its node list read
 * from the node directory (`directory`), those in `zone` first, each group in
 * weighted-random order, and read again in the background after a failover
 * and once the listing's `refreshAfterSeconds` passed.
 */
export async function directoryTransport(options: {
  cluster: string;
  policy: NodePolicy;
  directory: string;
  zone?: string;
  fetch?: typeof fetch;
  retry?: RetryOptions;
  random?: () => number;
}): Promise<NodeTransport> {
  const random = options.random ?? Math.random;
  const fetcher = options.fetch ?? ((input, init) => fetch(input, init));
  const listing = await readDirectory(options.directory, options.cluster, fetcher);
  const [baseUrl, ...nodes] = orderNodes(listing.nodes, options.zone, random);
  const transport = new NodeTransport({ baseUrl, nodes, policy: options.policy, fetch: options.fetch, retry: options.retry });
  transport.useDirectory({
    directory: options.directory,
    cluster: options.cluster,
    zone: options.zone,
    random,
    due: Date.now() + listing.refreshAfterSeconds * 1000,
  });
  return transport;
}
