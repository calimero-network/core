/**
 * The node's routes as a typed client, generated from core's OpenAPI document
 * (`openapi/core-openapi.json`, types in `src/generated/api.ts` by
 * `pnpm gen:api`), run by `openapi-fetch` over this package's runtime:
 *
 * - its `fetch` is {@link transportFetch}: every request goes through a
 *   {@link NodeTransport}, so it is sealed to an attested node, carries the
 *   transport's credential (refreshed on a `401`), moves on past a
 *   `421 WRONG_NODE`, and a route the document marks retry-safe retries a
 *   `503` (D70); the two
 *   routes a node serves in the clear (`/attestation`, `/health`) go
 *   unsealed, without the credential;
 * - the `412 CONFIRM_REQUIRED` step-up and the `428
 *   OPERATOR_SIGNATURE_REQUIRED` operator signature are middleware
 *   (`./step-up.ts`), for the whole client or one request.
 *
 * A node's refusal is the `error` of the result (`{ status, error, message,
 * details? }`), typed per route; {@link unwrap} throws it as the typed
 * {@link CalimeroError} instead. A transport failure (the node unreachable,
 * an attestation the policy refuses, an envelope past the sealed cap) rejects.
 * Streams (`GET /apps/{app}/events`, `/presence/stream`) are not routes of this
 * client: read them with `transport.events`.
 */

import createClient, { type Client, type Middleware } from 'openapi-fetch';

import type { paths } from '../generated/api.js';
import { errorFrom, type CalimeroError } from '../runtime/errors.js';
import { newIdempotencyKey, type HttpMethod, type NodeTransport, type QueryValue } from '../runtime/transport.js';
import { operatorSignature, stepUp, type OperatorProofCallback } from './step-up.js';
import { type ConfirmCallback } from '../runtime/passkey.js';
import { RETRY_SAFE_ROUTES } from '../generated/methods.js';

/** The typed client of every route of a Calimero node. */
export type RouteClient = Client<paths, 'application/json'>;

/** The node's API, path by path (the generated `paths`). */
export type { paths as ApiPaths } from '../generated/api.js';

/**
 * The base URL the generated client builds request URLs on. It names no
 * node: {@link transportFetch} sends the path and query to the node the
 * transport uses now.
 */
export const ROUTE_BASE_URL = 'https://calimero-node.invalid';

/** What {@link createRouteClient} sends through: a {@link NodeTransport}, or anything that answers like its `exchange`. */
export interface RouteTransport {
  exchange: NodeTransport['exchange'];
  /** `GET` of a route the node serves in the clear; without it those go sealed too. */
  inTheClear?: NodeTransport['inTheClear'];
}

/**
 * The routes a node serves outside the sealed channel (the document's
 * `x-calimero-sealed: false`): `GET /attestation` and `GET /health`.
 */
const IN_THE_CLEAR = new Set(['attestation', 'health']);

/** How {@link createRouteClient} answers the node's challenges for every request. */
export interface RouteClientOptions {
  /**
   * Answers every `412 CONFIRM_REQUIRED` (say, `passkeyConfirm(browserPasskey())`).
   * A request's own `stepUp` middleware answers first.
   */
  confirm?: ConfirmCallback;
  /** Answers every `428 OPERATOR_SIGNATURE_REQUIRED` with the operator key's signature. */
  operatorProof?: OperatorProofCallback;
}

/**
 * The routes a `503` is retried on (D70): those the document marks
 * `x-calimero-retry-safe` (a call, a view, a method by its path), each
 * carrying its idempotency key or reading. As path patterns, without the
 * leading `/`.
 */
const RETRIED = RETRY_SAFE_ROUTES.map((route) => ({
  method: route.method,
  pattern: new RegExp(`^${route.path.replace(/^\//, '').replace(/\{[^}]+\}/g, '[^/]+')}$`),
}));

/** The route templates that must carry an `idempotencyKey` to be retried. */
const NEEDS_KEY = new Set(RETRY_SAFE_ROUTES.filter((r) => r.requires === 'idempotencyKey').map((r) => `${r.method} ${r.path}`));

/**
 * A `fetch` that sends `openapi-fetch`'s request through `transport`: the
 * path and query after {@link ROUTE_BASE_URL}, the body as it was serialised,
 * and the request's signal. Headers other than `content-type` are not sent:
 * the transport sets the credential. A call or a view is retried as D70 says
 * (its confirmed attempt, carrying `confirm`, exactly once).
 */
export function transportFetch(transport: RouteTransport): (request: Request) => Promise<Response> {
  return async (request) => {
    if (!request.url.startsWith(`${ROUTE_BASE_URL}/`)) {
      throw new Error(`not a route of the generated client: ${request.url}`);
    }
    const url = new URL(request.url);
    const path = url.pathname.replace(/^\/+/, '');
    const query: Record<string, QueryValue[]> = {};
    for (const [name, value] of url.searchParams) (query[name] ??= []).push(value);
    const method = request.method as HttpMethod;
    if (method === 'GET' && IN_THE_CLEAR.has(path) && transport.inTheClear) {
      return transport.inTheClear(path, { query, signal: request.signal });
    }
    const contentType = request.headers.get('content-type') ?? undefined;
    const bytes = method === 'GET' ? undefined : new Uint8Array(await request.arrayBuffer());
    const body = bytes && bytes.length > 0 ? bytes : undefined;
    let retry: boolean | 'once' = false;
    if (RETRIED.some((route) => route.method === method && route.pattern.test(path))) {
      const json = body ? (JSON.parse(new TextDecoder().decode(body)) as { confirm?: unknown }) : {};
      retry = json.confirm === undefined ? true : 'once';
    }
    return transport.exchange(method, path, {
      query,
      ...(body ? { bytes: body, contentType: contentType ?? 'application/octet-stream' } : {}),
      retry,
      signal: request.signal,
    });
  };
}

/**
 * Gives a request to a route whose retries require an `idempotencyKey`
 * (`x-calimero-retry-requires`: a call, a method by its path) one when it has
 * none, made before it is first sent, so its D70 retries and its step-up's
 * confirmed retry carry the same key.
 */
export const idempotencyKeys: Middleware = {
  async onRequest({ request, schemaPath }) {
    if (!NEEDS_KEY.has(`${request.method} ${schemaPath}`)) return undefined;
    const text = await request.clone().text();
    const body = text ? (JSON.parse(text) as Record<string, unknown>) : {};
    if (body.idempotencyKey !== undefined) return undefined;
    return new Request(request, { body: JSON.stringify({ ...body, idempotencyKey: newIdempotencyKey() }) });
  },
};

/**
 * The typed client of the node's routes, sent through `transport` (a
 * `CalimeroClient`'s is `client.transport`; its session is this client's).
 *
 * ```ts
 * const api = createRouteClient(client.transport);
 * const { data, error } = await api.POST('/apps/{app}/intents', {
 *   params: { path: { app } },
 *   body: { method: 'set', args: { key: 'a', value: 'b' } },
 *   middleware: [stepUp(passkeyConfirm(passkey))],
 * });
 * ```
 */
export function createRouteClient(transport: RouteTransport, options: RouteClientOptions = {}): RouteClient {
  const api = createClient<paths, 'application/json'>({ baseUrl: ROUTE_BASE_URL, fetch: transportFetch(transport) });
  api.use(idempotencyKeys);
  if (options.confirm) api.use(stepUp(options.confirm));
  if (options.operatorProof) api.use(operatorSignature(options.operatorProof));
  return api;
}

/** What every method of {@link RouteClient} answers. */
export interface RouteResult<T> {
  data?: T;
  error?: unknown;
  response: Response;
}

/**
 * The `data` of a result, or its refusal thrown as the typed
 * {@link CalimeroError} ({@link errorFrom}, the `Retry-After` header
 * included). An empty answer is `{}`.
 */
export async function unwrap<T>(result: Promise<RouteResult<T>> | RouteResult<T>): Promise<NonNullable<T>> {
  const { data, error, response } = await result;
  if (!response.ok) throw refusalOf(response, error);
  return (data ?? {}) as NonNullable<T>;
}

/** A refused result as the typed {@link CalimeroError}. */
export function refusalOf(response: Response, error: unknown): CalimeroError {
  const body = typeof error === 'string' ? (error ? { message: error } : {}) : error;
  return errorFrom(response.status, body, response.headers.get('retry-after'));
}

export type { Middleware };
