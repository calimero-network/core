/**
 * The node's refusals, as an app shows them.
 *
 * mero-js turns every refusal into a typed error ({@link CalimeroError} and its
 * subclasses, `errorFrom`). Hooks keep that error as it is in `error`, so an
 * app can still `instanceof AppError` and read `appCode` / `fields`, and add
 * {@link describeError}'s reading of it in `failure`: one `kind` to switch on,
 * a sentence to show, and whether trying again can help.
 */

import {
  AppError,
  ArgsTooLarge,
  CalimeroError,
  ConfirmCancelled,
  ConfirmRequired,
  ConsentStale,
  DirectoryError,
  IdempotencyMismatch,
  LoginRequired,
  NotFound,
  ReleaseDeadlineError,
  SealedTransportError,
  TrapError,
  UnavailableError,
  WrongNode,
} from '@calimero-network/mero-js';

export type FailureKind =
  /** The app refused the call (`422 APP_ERROR`): see `appCode` / `fields`. */
  | 'app'
  /** The run trapped (`422 TRAP`): out of fuel, out of memory, a panic. */
  | 'trap'
  /** The session is gone and could not be refreshed: sign in again. */
  | 'login'
  /**
   * The request needs the passkey (`412 CONFIRM_REQUIRED`) and none answered
   * it: a call made without `confirm: true`, or a client method called with no
   * passkey.
   */
  | 'confirm'
  /** The person dismissed the passkey prompt. Nothing was sent. */
  | 'cancelled'
  /** `421 WRONG_NODE` / `NOT_LEADER` the client could not move past. */
  | 'wrong-node'
  /** `503` or no answer, after the client's retries ran out. */
  | 'unavailable'
  /** `404`: missing, or not visible to this account. */
  | 'not-found'
  /** `403`: this session may not do that (an app frontend cannot approve, say). */
  | 'forbidden'
  /** `409 RELEASE_DEADLINE`: approve the named release to lift it. */
  | 'release-deadline'
  /** `409 CONSENT_STALE`: ask for a new consent request. */
  | 'consent-stale'
  /** `428 OPERATOR_SIGNATURE_REQUIRED`. */
  | 'operator-signature'
  /** `413`: the arguments or the body are over the node's limit. */
  | 'too-large'
  /** `409 IDEMPOTENCY_MISMATCH`: the key was used with other arguments. */
  | 'idempotency'
  /** `429`: the node is at its limit (event streams, say). */
  | 'rate-limited'
  /** The node's attestation or the sealed channel failed. */
  | 'attestation'
  /** The node directory named no node of the cluster (`DirectoryError`). */
  | 'directory'
  /** Anything else. */
  | 'unknown';

export interface Failure {
  kind: FailureKind;
  /** A sentence an app can show as it is. */
  message: string;
  /** Whether the same request may succeed later without anything changing. */
  retryable: boolean;
  /** The node's HTTP status, when it answered. */
  status?: number;
  /** The node's error code (`APP_ERROR`, `WRONG_NODE`, ...), when it answered. */
  code?: string;
  /** For `kind: 'app'`: the app's declared error (`Taken`, `Forbidden`, ...). */
  appCode?: string;
  /** For `kind: 'app'`: the error's named fields. */
  fields?: Record<string, unknown>;
  /** For `kind: 'unavailable'`: whether the call may have committed. */
  committed?: false | 'unknown';
  /** For `kind: 'release-deadline'`: the mero-tee release to approve. */
  release?: string;
  /** The error itself. */
  error: unknown;
}

function messageOf(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

/** Read any error a hook or the client threw. */
export function describeError(error: unknown): Failure {
  if (error instanceof AppError) {
    return {
      kind: 'app',
      message: error.appCode ? `The app refused: ${error.appCode}` : 'The app refused the call.',
      retryable: false,
      status: error.status,
      code: error.code,
      appCode: error.appCode,
      fields: error.fields,
      error,
    };
  }
  if (error instanceof TrapError) {
    return { kind: 'trap', message: `The app stopped: ${error.kind || 'trap'}.`, retryable: false, status: 422, code: error.code, error };
  }
  if (error instanceof LoginRequired) {
    return { kind: 'login', message: 'Your session ended. Sign in again.', retryable: false, status: 401, code: error.code, error };
  }
  if (error instanceof ConfirmRequired) {
    return { kind: 'confirm', message: 'This needs your passkey.', retryable: false, status: 412, code: error.code, error };
  }
  if (error instanceof ConfirmCancelled) {
    return { kind: 'cancelled', message: 'The passkey prompt was dismissed.', retryable: true, error };
  }
  if (error instanceof WrongNode) {
    return { kind: 'wrong-node', message: 'This node cannot serve your account. Try another node.', retryable: true, status: 421, code: error.code, error };
  }
  if (error instanceof UnavailableError) {
    return {
      kind: 'unavailable',
      message:
        error.committed === 'unknown'
          ? 'The node did not answer. The change may or may not have gone through.'
          : 'The node is busy. Try again in a moment.',
      retryable: true,
      status: 503,
      code: error.code,
      committed: error.committed,
      error,
    };
  }
  if (error instanceof NotFound) {
    return { kind: 'not-found', message: 'Not found.', retryable: false, status: 404, code: error.code, error };
  }
  if (error instanceof ReleaseDeadlineError) {
    return {
      kind: 'release-deadline',
      message: error.release
        ? `New work is paused until release ${error.release} is approved.`
        : 'New work is paused until a newer release is approved.',
      retryable: false,
      status: 409,
      code: error.code,
      release: error.release,
      error,
    };
  }
  if (error instanceof ConsentStale) {
    return { kind: 'consent-stale', message: 'What you were asked to approve changed. Ask again.', retryable: false, status: 409, code: error.code, error };
  }
  if (error instanceof ArgsTooLarge) {
    return { kind: 'too-large', message: 'That is too large to send.', retryable: false, status: 413, code: error.code, error };
  }
  if (error instanceof IdempotencyMismatch) {
    return { kind: 'idempotency', message: 'That request key was already used for something else.', retryable: false, status: 409, code: error.code, error };
  }
  if (error instanceof CalimeroError) {
    const base = { status: error.status, code: error.code, error };
    if (error.status === 421) return { ...base, kind: 'wrong-node', message: 'This node cannot take that request. Try again.', retryable: true };
    if (error.status === 403) return { ...base, kind: 'forbidden', message: 'You are not allowed to do that here.', retryable: false };
    if (error.status === 413) return { ...base, kind: 'too-large', message: 'That is too large to send.', retryable: false };
    if (error.status === 428) return { ...base, kind: 'operator-signature', message: 'This needs the operator to sign off.', retryable: false };
    if (error.status === 429) return { ...base, kind: 'rate-limited', message: 'The node is at its limit. Try again later.', retryable: true };
    return { ...base, kind: 'unknown', message: messageOf(error), retryable: false };
  }
  if (error instanceof DirectoryError) {
    const message =
      error.reason === 'not-found'
        ? `There is no cluster named ${error.cluster}.`
        : error.reason === 'empty'
          ? `Cluster ${error.cluster} has no node taking traffic. Try again later.`
          : `Could not look up cluster ${error.cluster}. Try again.`;
    return {
      kind: 'directory',
      message,
      retryable: error.reason !== 'not-found',
      status: error.status,
      code: error.reason,
      error,
    };
  }
  if (error instanceof SealedTransportError) {
    return { kind: 'attestation', message: 'Could not open a verified channel to the node.', retryable: true, error };
  }
  return { kind: 'unknown', message: messageOf(error), retryable: false, error };
}

/** Whether an error means the session is gone for good (sign in again). */
export function isLoginRequired(error: unknown): boolean {
  return error instanceof LoginRequired;
}
