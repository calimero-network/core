/**
 * What every generated method (`src/generated/methods.ts`, by
 * `scripts/gen-methods.mjs`) runs: one flattened argument object split back
 * into the route's path parameters, query and body, sent through the
 * generated `openapi-fetch` client, and its answer unwrapped (a refusal thrown
 * as the typed `CalimeroError`). The route types themselves all come from the
 * generated `operations`; nothing here names a route.
 */

import type { Middleware } from 'openapi-fetch';

import type { RouteClient } from './route-client.js';
import { unwrap } from './route-client.js';
import { operatorSignature, stepUp, type OperatorProofCallback } from './step-up.js';
import { type ConfirmCallback } from '../runtime/passkey.js';

/** What a generated method takes beside its arguments. */
export interface MethodOptions {
  /**
   * Answers this request's `412 CONFIRM_REQUIRED` (say,
   * `passkeyConfirm(passkey)`), before the client's own `confirm`.
   */
  confirm?: ConfirmCallback;
  /** Answers this request's `428 OPERATOR_SIGNATURE_REQUIRED`, before the client's own `operatorProof`. */
  operatorProof?: OperatorProofCallback;
  /** Aborts the request (and a step-up's prompt, when `confirm` reads it). */
  signal?: AbortSignal;
}

/** A raw request body: the bytes as they are, sent with `contentType`. */
export interface RawBody {
  body: Uint8Array | ArrayBuffer | Blob | string;
  /** The body's media type. Default: the one the route documents. */
  contentType?: string;
}

/** How a generated method's arguments map onto its route (written by the generator). */
export interface RouteShape {
  method: 'GET' | 'POST' | 'PUT' | 'DELETE' | 'PATCH';
  path: string;
  /** Names of the path parameters. */
  pathParams: readonly string[];
  /** Names of the query parameters. */
  query: readonly string[];
  /** The query is `args.query` (a query parameter is named like a path parameter). */
  queryWhole?: boolean;
  /**
   * `fields`: the arguments other than the path and query are the JSON body;
   * `whole`: the JSON body is `args.body`; `raw`: `args.body` is sent as it is,
   * with `args.contentType` (or `media`); `none`: no body.
   */
  body: 'fields' | 'whole' | 'raw' | 'none';
  /** Whether the route takes no body at all as well (`fields`: an empty one is left out). */
  optionalBody: boolean;
  /** The raw body's documented media type. */
  media?: string;
  /** How the answer is read. */
  reply: 'json' | 'bytes' | 'text';
  /** The route answers `412 CONFIRM_REQUIRED` first (`x-calimero-step-up`). */
  stepUp?: boolean;
  /** The user verification its step-up asks the passkey for (`x-calimero-user-verification`). */
  userVerification?: 'required' | 'preferred' | 'discouraged';
  /** The route answers `428 OPERATOR_SIGNATURE_REQUIRED` first (`x-calimero-operator`). */
  operator?: boolean;
}

/** Send one generated route: split `args`, send it, answer its payload or throw its refusal. */
export async function route(
  api: RouteClient,
  shape: RouteShape,
  args: Record<string, unknown> | undefined,
  options: MethodOptions = {},
): Promise<unknown> {
  const given = args ?? {};
  const path: Record<string, unknown> = {};
  const query: Record<string, unknown> = {};
  const rest: Record<string, unknown> = {};
  for (const [name, value] of Object.entries(given)) {
    if (shape.pathParams.includes(name)) path[name] = value;
    else if (shape.queryWhole && name === 'query') Object.assign(query, value);
    else if (!shape.queryWhole && shape.query.includes(name)) query[name] = value;
    else rest[name] = value;
  }
  for (const name of shape.pathParams) {
    if (path[name] === undefined) throw new TypeError(`${shape.method} ${shape.path}: no value for ${name}`);
  }
  const init: Record<string, unknown> = { params: { path, query } };
  if (shape.body === 'fields') {
    const empty = Object.values(rest).every((value) => value === undefined);
    if (!(empty && shape.optionalBody)) init.body = rest;
  } else if (shape.body === 'whole') {
    if (rest.body !== undefined || !shape.optionalBody) init.body = rest.body;
  } else if (shape.body === 'raw') {
    init.body = rest.body;
    init.bodySerializer = (body: unknown) => body;
    init.headers = { 'Content-Type': (rest.contentType as string | undefined) ?? shape.media ?? 'application/octet-stream' };
  }
  if (shape.reply === 'bytes') init.parseAs = 'blob';
  else if (shape.reply === 'text') init.parseAs = 'text';
  const middleware: Middleware[] = [];
  if (options.confirm) {
    const confirm = options.confirm;
    // The route's own user verification, where the node's challenge names none
    // (a sign-up's confirmation takes `preferred`, every other step-up `required`).
    const answer: ConfirmCallback = shape.userVerification
      ? (challenge, context) => confirm({ userVerification: shape.userVerification, ...challenge }, context)
      : confirm;
    middleware.push(stepUp(answer));
  }
  if (options.operatorProof) middleware.push(operatorSignature(options.operatorProof));
  if (middleware.length) init.middleware = middleware;
  if (options.signal) init.signal = options.signal;
  // The shape was written from the same document as `api`'s types; the
  // generated method's own signature carries them.
  const send = api.request as unknown as (method: string, path: string, init: unknown) => ReturnType<RouteClient['GET']>;
  return unwrap(send(shape.method.toLowerCase(), shape.path, init));
}
