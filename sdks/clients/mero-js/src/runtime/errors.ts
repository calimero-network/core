/**
 * What the cloud node's refusals mean to a caller (spec D64, D70).
 *
 * The node answers an error as `{ error: CODE, message, details }` with an HTTP
 * status. {@link errorFrom} is the one place the client turns that into an
 * error a caller can switch on; every call, view and event stream goes through
 * it. It matches the status and the code together, never the status alone: the
 * node sends other 409s (`CONFLICT`, `NOT_INSTALLED`) that are not an
 * idempotency mismatch, and other 413s (`TOO_LARGE` from the sealed transport)
 * that are not oversized call arguments. Anything it does not recognise stays a
 * plain {@link CalimeroError} carrying both.
 *
 * Every typed error extends {@link CalimeroError}, so `instanceof CalimeroError`
 * and its `status` and `code` (the node's code) keep working for all of them.
 */

export class CalimeroError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
    readonly details: Record<string, unknown> = {},
  ) {
    super(`${status} ${code}: ${message}`);
    this.name = 'CalimeroError';
  }
}

/**
 * `401 UNAUTHENTICATED` that refreshing could not answer: sign in (again) and
 * repeat the request.
 *
 * A session token is signed by the node that issued it, and other nodes take
 * it only while they can vouch for that node's signing key: a `401` often
 * means only that the issuer was replaced or cannot be reached, not that the
 * user signed out. The client answers that itself when it holds a refresh
 * token: it refreshes once (any node takes the refresh) and repeats the
 * request. This error reaches a caller when there is no refresh token (an API
 * key, say), or the refresh was refused.
 */
export class LoginRequired extends CalimeroError {
  constructor(message: string, details: Record<string, unknown> = {}) {
    super(401, 'UNAUTHENTICATED', message, details);
    this.name = 'LoginRequired';
  }
}

/**
 * `401 PASSKEY_UNKNOWN_HERE`: a sign-in (or a confirmation) with a passkey no
 * account of this cluster has. Accounts and passkeys are each cluster's own,
 * while clusters may share one WebAuthn rp-id (every Calimero Cloud cluster
 * uses `calimero.network`), so the browser offers a passkey made on another
 * cluster here too; a passkey removed from its account answers the same.
 *
 * It is not a {@link LoginRequired}: the client does not refresh for it, and
 * signing in again with the same passkey fails the same way. Offer to sign up
 * on this cluster, or point the person to the app link for the cluster their
 * account is on; the passkey's name in their password manager names that
 * cluster (`alice · eu-shared-1`). The node does not say which cluster or
 * account the passkey belongs to: it only knows its own.
 */
export class PasskeyUnknownHere extends CalimeroError {
  constructor(message: string, details: Record<string, unknown> = {}) {
    super(
      401,
      'PASSKEY_UNKNOWN_HERE',
      message || 'this passkey belongs to another Calimero cluster or was removed from this one',
      details,
    );
    this.name = 'PasskeyUnknownHere';
  }
}

/**
 * `421 WRONG_NODE`: this node runs a mero-tee release the account's owners have
 * not approved, so it neither issues nor takes the account's sessions. Use
 * another node of the cluster; refreshing here would not help.
 *
 * The client moves to the next node it was given (`nodes`) and repeats the
 * request once by itself (a refresh: once per node). This error reaches a
 * caller when it knows no other node, when the next one refused too (for a
 * refresh, every node), or after a sign-in (which needs the passkey again:
 * repeat it, and it goes to the next node).
 */
export class WrongNode extends CalimeroError {
  constructor(message: string, details: Record<string, unknown> = {}) {
    super(421, 'WRONG_NODE', message, details);
    this.name = 'WrongNode';
  }
}

/**
 * `404 NOT_FOUND`: what the request names does not exist for this caller. For
 * an app this does not say whether it exists: an account the app does not
 * admit gets the same answer as a missing app.
 */
export class NotFound extends CalimeroError {
  constructor(message: string, details: Record<string, unknown> = {}) {
    super(404, 'NOT_FOUND', message, details);
    this.name = 'NotFound';
  }
}

/**
 * `412 CONFIRM_REQUIRED`: the call needs the account holder's passkey over a
 * challenge in `details`. The `*Confirmed` methods answer it themselves; it
 * reaches a caller only from a method that was given no passkey.
 */
export class ConfirmRequired extends CalimeroError {
  constructor(message: string, details: Record<string, unknown> = {}) {
    super(412, 'CONFIRM_REQUIRED', message, details);
    this.name = 'ConfirmRequired';
  }
}

/** The user declined the passkey prompt a confirmation asked for. Nothing was sent. */
export class ConfirmCancelled extends Error {
  constructor(readonly cause?: unknown) {
    super('the passkey confirmation was cancelled');
    this.name = 'ConfirmCancelled';
  }
}

/**
 * The browser or a password manager refused to show the passkey prompt a
 * confirmation asked for: the page was not focused (an extension's popup
 * still held focus), or another prompt was still open. The user did not
 * decline; nothing was sent. Ask them to try again from a click, e.g. "Your
 * password manager or browser blocked the second prompt. Click to try again."
 * `cause` is the browser's error.
 */
export class ConfirmPromptBlocked extends Error {
  constructor(readonly cause?: unknown) {
    super('the passkey prompt was blocked by the browser or a password manager');
    this.name = 'ConfirmPromptBlocked';
  }
}

/** `413 ARGS_TOO_LARGE`: the call's arguments are over the node's limit; it did not run. */
export class ArgsTooLarge extends CalimeroError {
  constructor(message: string, details: Record<string, unknown> = {}) {
    super(413, 'ARGS_TOO_LARGE', message, details);
    this.name = 'ArgsTooLarge';
  }
}

/** `409 IDEMPOTENCY_MISMATCH`: the idempotency key was used before with different arguments. */
export class IdempotencyMismatch extends CalimeroError {
  constructor(message: string, details: Record<string, unknown> = {}) {
    super(409, 'IDEMPOTENCY_MISMATCH', message, details);
    this.name = 'IdempotencyMismatch';
  }
}

/**
 * `409 RELEASE_DEADLINE`: a mero-tee release's deadline has passed and the
 * set that governs this work (the app's publisher's, or its owner's) has not
 * approved it or a newer one, so new work (creating an app, installing a
 * release, upgrading an app) is refused. Apps already running keep running.
 * Approving `release` lifts it at once.
 */
export class ReleaseDeadlineError extends CalimeroError {
  /** The release to approve. */
  readonly release: string;
  /** Its deadline, RFC 3339 (`''` when the node did not say). */
  readonly after: string;

  constructor(message: string, details: Record<string, unknown> = {}) {
    super(409, 'RELEASE_DEADLINE', message, details);
    this.name = 'ReleaseDeadlineError';
    this.release = typeof details.release === 'string' ? details.release : '';
    this.after = typeof details.after === 'string' ? details.after : '';
  }
}

/**
 * `409 RUNTIME_TOO_OLD`: a node that would run the app (one of its replicas
 * for an upgrade, a node it would be placed on or moved to) runs an older
 * runtime than the release's signed `minRuntimeVersion`. Nothing ran. The
 * app's report names the newest release it can reach (`latestReachable`).
 */
export class RuntimeTooOld extends CalimeroError {
  /** The runtime the release needs. */
  readonly minRuntimeVersion: string;
  /** Each node that falls short, and what it runs (`{ before }`: older than that). */
  readonly nodes: { node: number; runtime: string | { before: string } | null }[];

  constructor(message: string, details: Record<string, unknown> = {}) {
    super(409, 'RUNTIME_TOO_OLD', message, details);
    this.name = 'RuntimeTooOld';
    this.minRuntimeVersion = typeof details.minRuntimeVersion === 'string' ? details.minRuntimeVersion : '';
    this.nodes = Array.isArray(details.nodes) ? (details.nodes as RuntimeTooOld['nodes']) : [];
  }
}

/**
 * `409 CONSENT_STALE`: what a consent page showed (the app's bundle, the
 * target release, the release's measurements) changed before the person
 * approved. Nothing was approved; ask for a new consent request so they see
 * the new facts.
 */
export class ConsentStale extends CalimeroError {
  constructor(message: string, details: Record<string, unknown> = {}) {
    super(409, 'CONSENT_STALE', message, details);
    this.name = 'ConsentStale';
  }
}

/**
 * `403 ORIGIN_NOT_REGISTERED`: a sign-up or sign-in started on a web origin
 * the cluster does not know: neither the cluster's own nor one a package's
 * manager registered (`packageOrigins.add`). `origin` is the page's origin
 * and `package` the package it was expected to be registered for, when known.
 */
export class OriginNotRegistered extends CalimeroError {
  readonly origin: string;
  readonly package: string;
  /** What the node said, if anything. */
  readonly nodeMessage: string;

  constructor(message: string, details: Record<string, unknown> = {}) {
    const origin = typeof details.origin === 'string' ? details.origin : '';
    const pkg = typeof details.package === 'string' ? details.package : '';
    const site = origin ? `this site (${origin})` : 'this site';
    super(403, 'ORIGIN_NOT_REGISTERED', `${site} is not registered for ${pkg || 'any package'}${message ? ` (${message})` : ''}`, details);
    this.name = 'OriginNotRegistered';
    this.origin = origin;
    this.package = pkg;
    this.nodeMessage = message;
  }
}

/**
 * `400 BAD_ORIGIN` or `400 BAD_RP_ID`: the node does not accept the origin a
 * package's manager tried to register (it must be a bare web origin,
 * `scheme://host[:port]`), or the relying-party id given for it (which must
 * fit the origin's host). `code` says which. A served origin (`served: true`)
 * must also be `https://host` with no port, and not a name under the
 * cluster's apps domain or its rp-id.
 */
export class BadOrigin extends CalimeroError {
  constructor(code: 'BAD_ORIGIN' | 'BAD_RP_ID', message: string, details: Record<string, unknown> = {}) {
    super(400, code, message, details);
    this.name = 'BadOrigin';
  }
}

/**
 * `409 ORIGIN_TAKEN`: the origin is registered already (for another package,
 * or for this one with the other `served`: remove it first), or it is one of
 * the cluster's own (a node's API host, a cluster origin, a frontend it
 * serves).
 */
export class OriginTaken extends CalimeroError {
  constructor(message: string, details: Record<string, unknown> = {}) {
    super(409, 'ORIGIN_TAKEN', message, details);
    this.name = 'OriginTaken';
  }
}

/**
 * `400 NOT_SERVING`: a served origin (a custom domain, `served: true`) was
 * asked of a cluster that serves no frontends (merod without
 * `--apps-domain`). Register the origin unserved, or use a cluster that
 * serves them.
 */
export class NotServing extends CalimeroError {
  constructor(message: string, details: Record<string, unknown> = {}) {
    super(400, 'NOT_SERVING', message, details);
    this.name = 'NotServing';
  }
}

/**
 * `409 CLUSTER_NOT_UPGRADED`: the change needs every node of the cluster
 * (replicas, voters and learners) on the node's release or later, and one
 * is not yet; for instance adding a served origin. Retry once the cluster is
 * upgraded.
 */
export class ClusterNotUpgraded extends CalimeroError {
  constructor(message: string, details: Record<string, unknown> = {}) {
    super(409, 'CLUSTER_NOT_UPGRADED', message, details);
    this.name = 'ClusterNotUpgraded';
  }
}

/**
 * `409 FEATURE_NOT_ACTIVE`: the command (or a field of it) needs a cluster
 * version not every node of the cluster supports yet, during a rollout from
 * an older release; nothing was done. `feature` names it, `needs` is the core
 * release it needs and `active` the cluster's version now (`''` when not every
 * node said which it supports). It works once the old nodes leave the cluster:
 * retry then, or tell the user the feature is not available yet.
 */
export class FeatureNotActive extends CalimeroError {
  readonly feature: string;
  readonly needs: string;
  readonly active: string;

  constructor(message: string, details: Record<string, unknown> = {}) {
    super(409, 'FEATURE_NOT_ACTIVE', message, details);
    this.name = 'FeatureNotActive';
    this.feature = typeof details.feature === 'string' ? details.feature : '';
    this.needs = typeof details.needs === 'string' ? details.needs : '';
    this.active = typeof details.active === 'string' ? details.active : '';
  }
}

/**
 * `409 RECOVERY_NOT_READY`: a recovery (an account's or an organization's)
 * may finish only once its 72-hour delay has passed. `readyAt` is when it may
 * (Unix seconds), when the node said.
 */
export class RecoveryNotReady extends CalimeroError {
  readonly readyAt: number | undefined;

  constructor(message: string, details: Record<string, unknown> = {}) {
    super(409, 'RECOVERY_NOT_READY', message, details);
    this.name = 'RecoveryNotReady';
    this.readyAt = typeof details.readyAt === 'number' ? details.readyAt : undefined;
  }
}

/**
 * The recovery code is not one the cluster holds: mistyped, replaced by a
 * newer one, or spent by a recovery that started (whether it finished or was
 * cancelled). The node answers it `401`; a recovery is signed out, so it is
 * this error, not {@link LoginRequired}.
 */
export class WrongRecoveryCode extends CalimeroError {
  constructor(message: string, details: Record<string, unknown> = {}) {
    super(401, 'UNAUTHENTICATED', message || 'unknown or used recovery code', details);
    this.name = 'WrongRecoveryCode';
  }
}

/** Why a sign-up was refused ({@link SignUpRefused.reason}), read from what the node said. */
export type SignUpRefusal = 'invite-only' | 'closed' | 'unknown-invite' | 'expired-invite' | 'org-gone' | 'other';

/**
 * `403 SIGN_UP_REFUSED`: the cluster's sign-up policy does not take this
 * sign-up (core `SystemState::admits`), answered by `register/begin` before a
 * passkey is made and by `register/finish` before its challenge is spent.
 * `reason` says why, so a page can say "invite only" or "this invite has
 * expired" instead of the node's sentence:
 *
 * - `invite-only`: the cluster takes new accounts with an invite only.
 * - `closed`: only a cluster sign-up invite (the operator's or the owner's) admits.
 * - `unknown-invite`: the invite is not one the cluster knows (mistyped, revoked,
 *   or used up: a spent invite is gone).
 * - `expired-invite`: past its expiry, or no use left.
 * - `org-gone`: the organization the invite was for was deleted.
 */
export class SignUpRefused extends CalimeroError {
  readonly reason: SignUpRefusal;

  constructor(message: string, details: Record<string, unknown> = {}) {
    super(403, 'SIGN_UP_REFUSED', message, details);
    this.name = 'SignUpRefused';
    this.reason = signUpRefusal(message);
  }
}

function signUpRefusal(message: string): SignUpRefusal {
  if (/invite only/.test(message)) return 'invite-only';
  if (/pre-authorized|sign-up invite/.test(message)) return 'closed';
  if (/unknown invite/.test(message)) return 'unknown-invite';
  if (/invite has expired/.test(message)) return 'expired-invite';
  if (/organization is gone/.test(message)) return 'org-gone';
  return 'other';
}

/**
 * `429 QUOTA_EXCEEDED`: the caller holds as many of something (origins,
 * served origins per package or per cluster, blobs, ...) as it may.
 */
export class QuotaExceeded extends CalimeroError {
  constructor(message: string, details: Record<string, unknown> = {}) {
    super(429, 'QUOTA_EXCEEDED', message, details);
    this.name = 'QuotaExceeded';
  }
}

/**
 * `422 APP_ERROR`: the app refused the call. `appCode` is the app's declared
 * error (`Taken`, `Forbidden`, ...) and `fields` its named fields. The call was
 * logged, so its idempotency key is spent.
 */
export class AppError extends CalimeroError {
  readonly appCode: string;
  readonly fields: Record<string, unknown>;

  constructor(message: string, details: Record<string, unknown> = {}) {
    super(422, 'APP_ERROR', message, details);
    this.name = 'AppError';
    const { appCode, fields } = appErrorOf(details);
    this.appCode = appCode;
    this.fields = fields;
  }
}

/** `422 TRAP`: the run trapped (`OutOfFuel`, `OutOfMemory`, ...). The call was logged. */
export class TrapError extends CalimeroError {
  readonly kind: string;

  constructor(message: string, details: Record<string, unknown> = {}) {
    super(422, 'TRAP', message, details);
    this.name = 'TrapError';
    this.kind = typeof details.kind === 'string' ? details.kind : '';
  }
}

/**
 * Whether a call the node answered 503 may have committed: `false` when the
 * node says it did not (`TIMEOUT`, `MIGRATING`), `"unknown"` otherwise (a call
 * that raced a leader change). Never a plain boolean: `"unknown"` must not read
 * as "no".
 */
export type Committed = false | 'unknown';

/**
 * Any `503`, or a connection that failed, after the client's retries with the
 * same idempotency key ran out. `reason` is the node's code in lower case
 * (`timeout`, `migrating`, `unavailable`, ...), or `network` when the node gave
 * no answer at all (code `NETWORK`, `cause` the `fetch` failure); such a call
 * may have committed, so its `committed` is `'unknown'`.
 */
export class UnavailableError extends CalimeroError {
  readonly reason: string;
  readonly committed: Committed;
  /** Seconds the node asked to wait, when it said. */
  readonly retryAfter: number | undefined;
  /** What failed, when the node gave no answer. */
  readonly cause: unknown;

  constructor(code: string, message: string, details: Record<string, unknown> = {}, retryAfter?: number, cause?: unknown) {
    super(503, code, message, details);
    this.name = 'UnavailableError';
    this.reason = code.toLowerCase();
    this.committed = code === 'TIMEOUT' || code === 'MIGRATING' ? false : 'unknown';
    this.retryAfter = retryAfter;
    this.cause = cause;
  }
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
}

/**
 * The app's code and named fields. The node puts them in `details` as
 * `{ code, fields }`; an older node nests the app's error under `details.error`.
 * Positional payloads (`"0"`, `"1"`) are never read as fields (D63).
 */
function appErrorOf(details: Record<string, unknown>): { appCode: string; fields: Record<string, unknown> } {
  const source = typeof details.code === 'string' ? details : record(details.error);
  const appCode = typeof source.code === 'string' ? source.code : '';
  return { appCode, fields: { ...record(source.fields) } };
}

function retryAfterOf(details: Record<string, unknown>, header?: string | null): number | undefined {
  for (const candidate of [details.retryAfter, header == null ? undefined : Number(header)]) {
    if (typeof candidate === 'number' && Number.isFinite(candidate) && candidate >= 0) return candidate;
  }
  return undefined;
}

/**
 * The error for a refusal the node answered with `status` and `body` (its JSON
 * `{ error, message, details }`, or whatever it sent). The one place statuses
 * become typed errors.
 */
export function errorFrom(status: number, body: unknown, retryAfterHeader?: string | null): CalimeroError {
  const value = record(body);
  const code = typeof value.error === 'string' ? value.error : 'ERROR';
  const message = typeof value.message === 'string' ? value.message : '';
  const details = record(value.details);
  if (status === 503) return new UnavailableError(code, message, details, retryAfterOf(details, retryAfterHeader));
  switch (`${status} ${code}`) {
    case '401 UNAUTHENTICATED':
      return new LoginRequired(message, details);
    case '401 PASSKEY_UNKNOWN_HERE':
      return new PasskeyUnknownHere(message, details);
    case '421 WRONG_NODE':
      return new WrongNode(message, details);
    case '404 NOT_FOUND':
      return new NotFound(message, details);
    case '409 IDEMPOTENCY_MISMATCH':
      return new IdempotencyMismatch(message, details);
    case '409 RELEASE_DEADLINE':
      return new ReleaseDeadlineError(message, details);
    case '409 RUNTIME_TOO_OLD':
      return new RuntimeTooOld(message, details);
    case '409 CONSENT_STALE':
      return new ConsentStale(message, details);
    case '403 ORIGIN_NOT_REGISTERED':
      return new OriginNotRegistered(message, details);
    case '400 BAD_ORIGIN':
    case '400 BAD_RP_ID':
      return new BadOrigin(code as 'BAD_ORIGIN' | 'BAD_RP_ID', message, details);
    case '409 ORIGIN_TAKEN':
      return new OriginTaken(message, details);
    case '400 NOT_SERVING':
      return new NotServing(message, details);
    case '409 CLUSTER_NOT_UPGRADED':
      return new ClusterNotUpgraded(message, details);
    case '409 FEATURE_NOT_ACTIVE':
      return new FeatureNotActive(message, details);
    case '429 QUOTA_EXCEEDED':
      return new QuotaExceeded(message, details);
    case '409 RECOVERY_NOT_READY':
      return new RecoveryNotReady(message, details);
    case '403 SIGN_UP_REFUSED':
      return new SignUpRefused(message, details);
    case '412 CONFIRM_REQUIRED':
      return new ConfirmRequired(message, details);
    case '413 ARGS_TOO_LARGE':
      return new ArgsTooLarge(message, details);
    case '422 APP_ERROR':
      return new AppError(message, details);
    case '422 TRAP':
      return new TrapError(message, details);
    default:
      return new CalimeroError(status, code, message, details);
  }
}
