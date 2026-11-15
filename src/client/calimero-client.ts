/**
 * Client for Calimero 1.0, the confidential cloud: one app = one replicated log on
 * attested nodes; this client verifies the node, seals every request to it,
 * signs people in with passkeys and machines with API keys.
 *
 * ```ts
 * const cloud = new CalimeroClient({ baseUrl, policy: { release: { minReleaseVersion: '2.3.109' } } });
 * await cloud.signIn(browserPasskey());
 * const { result } = await cloud.call(appId, 'set', { key: 'a', value: '1' });
 * ```
 */

import {
  DEFAULT_RETRY,
  NodeTransport,
  directoryTransport,
  refusal,
  retryDelayMs,
  routePath,
  type CalimeroSession,
  type NodeEvent,
  type NodeVerificationState,
  type RetryOptions,
  type HttpMethod,
} from '../runtime/transport.js';
import { type NodePolicy, type NodeVerification } from '../runtime/attestation.js';
import { DEFAULT_DIRECTORY } from '../runtime/directory.js';
import { CalimeroError, LoginRequired, NotFound, OriginNotRegistered, UnavailableError, WrongRecoveryCode } from '../runtime/errors.js';
import { b64url, fromB64url, type PasskeyProvider } from '../runtime/passkey.js';
import { createRouteClient, type RouteClientOptions } from '../routes/route-client.js';
import type { MethodOptions } from '../routes/method.js';
import type { PackageCapabilities } from './package-capabilities.js';
import { createRoutes, type Routes } from '../generated/methods.js';
import { passkeyConfirm, type PasskeyConfirmOptions } from '../runtime/passkey.js';

import { exportFile, exportFileTo, type ExportedFile, type FileSink } from './files.js';
import { signRecovery } from './recovery-key.js';
import {
  CONSENT_STORAGE_KEY,
  confirmationCode,
  pageQuery,
  pendingApprovalsFrom,
  withoutConsentReturn,
  type AppsReport,
  type ConsentRequest,
  type ConsentResult,
  type ConsentStarted,
  type OpenConsentOptions,
  type PageOptions,
  type ResumeConsentOptions,
  type PendingApprovals,
  type ReportedApp,
} from './approvals.js';
import type { ApiAnswer, ApiSchema } from '../routes/types.js';

export {
  AppError,
  ArgsTooLarge,
  BadOrigin,
  CalimeroError,
  ConfirmCancelled,
  ConfirmPromptBlocked,
  ConfirmRequired,
  IdempotencyMismatch,
  LoginRequired,
  ConsentStale,
  FeatureNotActive,
  NotFound,
  OriginNotRegistered,
  OriginTaken,
  PasskeyUnknownHere,
  QuotaExceeded,
  RecoveryNotReady,
  ReleaseDeadlineError,
  RuntimeTooOld,
  SignUpRefused,
  TrapError,
  UnavailableError,
  WrongNode,
  WrongRecoveryCode,
  errorFrom,
} from '../runtime/errors.js';
export type { Committed, SignUpRefusal } from '../runtime/errors.js';

export type { CalimeroSession, NodeEvent };

/** The largest file sent as one request, and the part size a session is asked for. */
export const FILE_PART_BYTES = 16 * 1024 * 1024;
const PART_RESENDS = 1; // times a part is sent again before the node is asked where the session is
const MAX_FAILURES = 8; // failed sends in a row, with no part accepted between them, before giving up
const MAX_JUMPS = 16; // PART_OUT_OF_ORDER answers followed in one session before giving up
const CALL_RETRIES = 8; // times beginning and completing a session are sent again while the node is unavailable

export type CallResult = ApiAnswer<'intents'>;

export type ViewResult = ApiAnswer<'views'>;

/** An installed package and its releases. */
export type PackageInfo = ApiSchema<'Package'>;

/** One app instance as `listApps` answers it. */

export type AppInfo = ApiSchema<'AppEntry'>;

/**
 * An organization-wide upgrade run, as `client.orgs.upgrade`, `client.orgs.upgradeStatus` and
 * `client.orgs.upgradeRetry` answer it (core's `UpgradeRun`).
 */
export type UpgradeRun = ApiSchema<'UpgradeRun'>;

/** One of an app's blobs: `blobId` base58 (what apps print), `id` hex. */
export type BlobInfo = ApiAnswer<'blob_info'>;

/** A sealed file: `id` and `sha256` hex, `blobId` base58 (what apps print), `index` the log index that recorded it. */
export type FileInfo = ApiAnswer<'file_info'>;

/** What {@link CalimeroClient.uploadFile} takes; a `Blob` is read part by part, never whole. */
export type FileData = Blob | Uint8Array | ArrayBuffer;

/** One device present in an app (`publishPresence`); `state` hex, `null` once retracted. */
export type PresenceEntry = ApiAnswer<'presence_get'>['present'][number];

/**
 * `GET /orgs/{org}`: the organization as the caller sees it. Besides its
 * members, teams and metadata, who may approve its releases, the package
 * signing keys it publishes, and whether a passed release deadline has
 * stopped new work on its own set.
 */
export type OrgInfo = ApiSchema<'Org'>;

/**
 * How an organization holds its apps, derived from its app teams (core
 * `OrgMode`): `legacy` has none (its apps use organization access),
 * `standalone` has only open ones (everyone has every app), `shared` has a
 * closed one (each app lists who is in it).
 */
export type OrgMode = 'legacy' | 'standalone' | 'shared';

/** One app team, as `GET /orgs/{org}/app-teams` lists it (`client.orgs.appTeamList`). */
export type AppTeam = AppTeams['appTeams'][number];

/** `GET /orgs/{org}/app-teams`: the organization's mode and its app teams. A guest sees only the app teams they are in. */
export type AppTeams = ApiAnswer<'app_team_list'>;

/** One package's app team after `client.orgs.appTeamsScope`. */
export type ScopedAppTeam = ApiAnswer<'app_teams_scope'>['teams'][number];

/** `POST /orgs/{org}/app-teams`: the organization's mode after, and each package's team. */
export type ScopeAppsResult = ApiAnswer<'app_teams_scope'>;

/** A pending organization invite, as `GET /orgs/{org}/invites` lists it (never its token). */
export type OrgInvite = ApiAnswer<'invite_list'>['invites'][number];

/** `GET /orgs/{org}/teams/{team}` (`client.orgs.teamGet`). */
export type TeamInfo = ApiSchema<'TeamView'>;

/** When a recovery started and the earliest it may finish (Unix seconds). */
export interface RecoveryTimes {
  startedAt: number;
  readyAt: number;
}

/** One xcall that was never delivered (core's `DeadLetter`). */
export type DeadLetter = ApiSchema<'DeadLetter'>;

/**
 * An account or an organization, as a deployer, a deployment's owner or the
 * cluster's owner: `{ account: id }` or `{ org: id }`.
 */
export type Deployer = { account: string } | { org: string };

/** A deployer's key, as `GET /deployers` lists it and `DELETE /deployers/{key}` takes it. */
export function deployerKey(deployer: Deployer): string {
  return 'account' in deployer ? `account:${deployer.account}` : `org:${deployer.org}`;
}

/**
 * A package frontend's `/.well-known/calimero.json`: the package, its
 * releases, the deployments offering it and the cluster's API URLs.
 */
export interface FrontendManifest {
  package: string;
  versions: string[];
  deployments: Array<{ id: string; name: string; app: string; version: string }>;
  api: string[];
}

/**
 * Read the `/.well-known/calimero.json` of the frontend at `origin` (by
 * default the page's own). Served in the clear by the frontend host, like the
 * page itself: for choosing a deployment and the API URL to connect to, never
 * for trust (the client attests the node it then talks to).
 */
export async function frontendManifest(origin?: string, fetcher: typeof fetch = fetch): Promise<FrontendManifest> {
  const base = origin ?? (globalThis as { location?: { origin?: string } }).location?.origin;
  if (!base) throw new Error('no page origin to read the frontend manifest from; pass `origin`');
  const response = await fetcher(new URL('/.well-known/calimero.json', base).toString());
  if (!response.ok) throw await refusal(response);
  const manifest = (await response.json()) as FrontendManifest;
  return { ...manifest, deployments: manifest.deployments ?? [] };
}

/**
 * The deployment a frontend page was opened for: the node serves
 * `/d/{deployment}/` by redirecting to `/{version}/…?deployment={id}`, so the
 * page creates its instances from that deployment
 * (`client.apps.create({ name, deployment, package })`). `null` when the page was
 * not opened through a deployment. `href` defaults to `window.location.href`.
 */
export function pageDeployment(href?: string): string | null {
  const at = href ?? (globalThis as { location?: { href?: string } }).location?.href;
  if (!at) return null;
  try {
    return new URL(at).searchParams.get('deployment') || null;
  } catch {
    return null;
  }
}

/** A recovery of an account under way (core #4844, `docs/specs/account-recovery.md`). */
export interface AccountRecovery {
  accountId: string;
  /** The new passkey the recovery adds when it finishes. */
  credentialId: string;
  /** When it started (Unix seconds). */
  startedAt: number;
  /** The earliest it may finish (Unix seconds): 72 hours after the start. */
  readyAt: number;
}

/** One web origin registered for a package (`GET /package-origins/{package}`). */
export type PackageOrigin = ApiAnswer<'origins_list'>['origins'][number];

/**
 * Where a sign-up or sign-in says it runs. By default the page's origin
 * (`window.location.origin`) when it is not the origin of a node the client
 * talks to; nothing outside a browser. A string sends that origin; `null`
 * sends none (the cluster's own origins).
 */
export interface SignInOptions {
  origin?: string | null;
  /**
   * Closes an open passkey prompt when it aborts (a Cancel button): the
   * ceremony then rejects as a dismissed prompt does (`NotAllowedError`; in a
   * sign-up's confirmation, {@link ConfirmCancelled}). Given to the
   * {@link PasskeyProvider} as `signal`.
   */
  signal?: AbortSignal;
}

/**
 * {@link SignInOptions} for a sign-up, and the invite it is made with: an
 * organization invite's token (the new account joins the organization at
 * the invite's role) or a cluster sign-up invite's. A cluster whose policy
 * is `invite` or `closed` (`client.deployments.signUpPublic`) needs one.
 */
export interface SignUpOptions extends SignInOptions {
  invite?: string;
}

/**
 * `GET /sign-up`: who may create an account on the cluster (core's sign-up
 * policy, read without a session), so a page can say "invite only" before
 * any passkey prompt.
 */
export type SignUpPolicy = ApiAnswer<'sign_up_public'>;

/** {@link PageOptions} as a generated route's query, checked as {@link pageQuery} checks them. */
function pageParams(page: PageOptions): { after?: string; limit?: number } {
  pageQuery(page);
  return { after: page.after, limit: page.limit };
}

/** Bytes as lower-case hex. */
function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

/**
 * A presence state as `POST /apps/{app}/presence` takes it: the bytes as
 * lower-case hex, or `null`, which retracts this device's entry
 * (`client.apps.presencePublish({ app, seq, state: presenceState(bytes) })`).
 */
export function presenceState(state: Uint8Array | null): string | null {
  return state === null ? null : toHex(state);
}

export { DEFAULT_RETRY, retryDelayMs };
export type { RetryOptions };

export interface CalimeroClientOptions {
  /** One node of the cluster: the one the client talks to first. */
  baseUrl: string;
  /**
   * Other nodes of the same cluster, in the order to move to them when a node
   * answers `421 WRONG_NODE` (its release may not serve this account), or stays
   * unavailable to a call or a view through all its retries. Each is attested
   * under the same `policy` before anything is sent to it. Without them a `421`
   * reaches the caller as {@link WrongNode}.
   */
  nodes?: string[];
  policy: NodePolicy;
  /** Fetch that carries attestation and envelopes. Defaults to global `fetch`. */
  fetch?: typeof fetch;
  /** How calls and views retry a 503 or a failed connection. */
  retry?: RetryOptions;
}

/** How {@link CalimeroClient.connect} reaches a Calimero Cloud cluster by name. */
export interface CalimeroConnectOptions {
  /** The cluster's name, e.g. `eu-shared-1`. */
  cluster: string;
  /** What every node must attest to before anything is sent to it. */
  policy: NodePolicy;
  /** The node directory to ask. Default {@link DEFAULT_DIRECTORY}. */
  directory?: string;
  /** Try the nodes in this zone (e.g. `europe-west4-a`) first. */
  zone?: string;
  /** Fetch for the directory, attestation and envelopes. Defaults to global `fetch`. */
  fetch?: typeof fetch;
  /** How calls and views retry a 503 or a failed connection. */
  retry?: RetryOptions;
  /** Randomness for the weighted node order, in [0, 1). Defaults to `Math.random`. */
  random?: () => number;
}

/** Bytes `start` to `end` of `data`, as a view or a slice: a `Blob` is never read whole. */
async function readSlice(data: FileData, start: number, end: number): Promise<Uint8Array> {
  if (data instanceof Blob) return new Uint8Array(await data.slice(start, end).arrayBuffer());
  return data instanceof ArrayBuffer ? new Uint8Array(data, start, end - start) : data.subarray(start, end);
}

/** The `next` an answer names, which cannot be past the last part. */
function nextPart(answer: unknown, parts: number): number {
  const next = (answer as { next?: unknown } | undefined)?.next;
  if (typeof next === 'number' && Number.isInteger(next) && next >= 0 && next <= parts) return next;
  throw new Error(`the node answered a bad next: ${JSON.stringify(answer)}`);
}

export class CalimeroClient {
  /**
   * The runtime every route of this client is sent through
   * (`@calimero-network/mero-js/runtime`): the attested, sealed channel to each
   * node, the credential and its refresh, the `412 CONFIRM_REQUIRED` step-up,
   * D70 retries and error mapping. A route layer of its own (one generated from
   * the node's OpenAPI document, say) sends through it and shares this
   * client's session.
   */
  readonly transport: NodeTransport;

  /**
   * The node's routes as methods, generated from core's OpenAPI document
   * (`src/generated/methods.ts`), one group per tag. Each takes one argument
   * object (path parameters, query and body fields merged) and answers the
   * route's payload; a refusal is thrown as the typed {@link CalimeroError}.
   * A step-up or an operator signature is answered by the client's `confirm`
   * / `operatorProof`, or the call's own (`{ confirm: passkeyConfirm(passkey) }`).
   *
   * `client.apps.call({ app, method, args })`, `client.orgs.get({ org })`,
   * `client.orgs.inviteCreate({ org, role }, { confirm })`.
   */
  readonly apps: Routes['apps'];
  /** Account, session, passkey and API-key routes: `client.auth.me()`. See {@link apps}. */
  readonly auth: Routes['auth'];
  /** The consent page's routes. See {@link apps}. */
  readonly consent: Routes['consent'];
  /** Operator routes (the ops token). See {@link apps}. */
  readonly ops: Routes['ops'];
  /** Organization, member, team and invite routes. See {@link apps}. */
  readonly orgs: Routes['orgs'];
  /** Routes anyone may read: attestation, health, releases, publisher bindings. See {@link apps}. */
  readonly public: Routes['public'];
  /** Package routes (`client.packages.install({ package, version })`). See {@link apps}. */
  readonly packages: Routes['packages'];
  /**
   * Deployment, deployer and sign-up routes (`client.deployments.create({ name, owner }, { confirm })`).
   * See {@link apps}.
   */
  readonly deployments: Routes['deployments'];

  /**
   * A client for the Calimero Cloud cluster named `cluster`: its node list read
   * from the public node directory, those in `zone` first, each group in
   * weighted-random order. The directory only picks which nodes to try: each is
   * attested under `policy` before anything is sent to it. Throws
   * {@link DirectoryError} when the directory names no usable node.
   *
   * The list is read again in the background after a failover, and at the
   * first request once the directory's `refreshAfterSeconds` has passed.
   */
  static async connect(options: CalimeroConnectOptions, challenges: RouteClientOptions = {}): Promise<CalimeroClient> {
    const transport = await directoryTransport({ ...options, directory: options.directory ?? DEFAULT_DIRECTORY });
    return new CalimeroClient(transport, challenges);
  }

  /**
   * A client of the nodes `options` names, or one sending through an existing
   * `transport`. `challenges` answers the node's challenges for every
   * generated method a call's own options leave open (`confirm` a
   * `412 CONFIRM_REQUIRED`, `operatorProof` a `428 OPERATOR_SIGNATURE_REQUIRED`).
   */
  constructor(options: CalimeroClientOptions | NodeTransport, challenges: RouteClientOptions = {}) {
    this.transport = options instanceof NodeTransport ? options : new NodeTransport(options);
    const routes = createRoutes(createRouteClient(this.transport, challenges));
    this.apps = routes.apps;
    this.auth = routes.auth;
    this.consent = routes.consent;
    this.ops = routes.ops;
    this.orgs = routes.orgs;
    this.public = routes.public;
    this.packages = routes.packages;
    this.deployments = routes.deployments;
  }

  /** A generated method's answer, as the type this class has always answered. */
  private as<T>(result: Promise<unknown>): Promise<T> {
    return result as Promise<T>;
  }

  /** The options that answer a `412 CONFIRM_REQUIRED` with `passkey`, when one is given. */
  private confirmWith(passkey: PasskeyProvider | undefined, options: PasskeyConfirmOptions = {}): MethodOptions | undefined {
    return passkey ? { confirm: passkeyConfirm(passkey, options) } : undefined;
  }

  /** The node requests go to now: `baseUrl`, until a `421 WRONG_NODE` or a failover moved the client on. */
  get nodeUrl(): string {
    return this.transport.nodeUrl;
  }

  /** Every node this client may use, in the order it moves through them. */
  get nodes(): string[] {
    return this.transport.nodes;
  }

  /** What the check of node `url` (from {@link nodes}; default: in use) established, or `undefined` before it first ran. */
  verification(url?: string): NodeVerificationState | undefined {
    return this.transport.verification(url);
  }

  /** Wait for the check requests wait for, starting it if needed, and answer its outcome (not thrown) without a request. */
  verify(url?: string): Promise<NodeVerificationState> {
    return this.transport.verify(url);
  }

  /** Authenticate every request with an API key. */
  useApiKey(key: string): void {
    this.transport.useApiKey(key);
  }

  useSession(session: CalimeroSession): void {
    this.transport.useSession(session);
  }

  get session(): CalimeroSession | undefined {
    return this.transport.session;
  }

  /** A JSON request through the transport, to an untyped path. */
  private json<T>(method: HttpMethod, path: string, body?: unknown): Promise<T> {
    return this.transport.json<T>(method, path, { body });
  }

  /** Attest `app` on node `url` (from {@link nodes}; default: in use): the record's `appHash` is the one the node bound. */
  attestApp(app: string, url?: string): Promise<NodeVerification> {
    return this.transport.attestApp(app, url);
  }

  /** The manifest facts and ABI of a package version (of `service`, for a multi-service release), for a signed-in caller. */
  packageCapabilities(pkg: string, version: string, options: { service?: string } = {}): Promise<PackageCapabilities> {
    return this.transport.json<PackageCapabilities>(
      'GET',
      routePath('packages/{package}/{version}/capabilities', { package: pkg, version }),
      { query: { service: options.service } },
    );
  }

  // ---------------------------------------------------------------- identity

  /**
   * Create an account with a new passkey, and sign in. The node then asks the
   * new passkey to sign the account it creates (`412 CONFIRM_REQUIRED`), so a
   * sign-up prompts twice: create, then sign. On a page whose origin is not
   * the cluster's ({@link SignInOptions}), the node makes the passkey for that
   * origin's relying party (the confirmation's `rpId` is that origin's too)
   * and binds the session to the package the origin is registered for; an
   * unregistered origin is refused with {@link OriginNotRegistered}.
   *
   * `invite` ({@link SignUpOptions}) goes on both halves and on the retry,
   * as the node judges it on each before it spends anything; a sign-up the
   * cluster's policy refuses, or an unknown, expired or used invite, is
   * {@link SignUpRefused} (from `begin`, before any passkey is made).
   */
  async signUp(passkey: PasskeyProvider, name: string, options: SignUpOptions = {}): Promise<CalimeroSession> {
    const origin = this.pageOrigin(options);
    const invite = options.invite ? { invite: options.invite } : {};
    const begin = await this.begin<{ challengeId: string; options: Record<string, unknown> }>(
      this.auth.registerBegin({ ...origin === undefined ? { name, ...invite } : { name, origin, ...invite } }),
      origin,
    );
    const signal = options.signal;
    const registration = await (signal ? passkey.create(begin.options, { signal }) : passkey.create(begin.options));
    // The node checks the sign-up's confirmation with the user present, not
    // verified (core `api.rs`: `rp(node)`, `require_uv: false`), as it made
    // the passkey with `preferred`: asking `required` here would refuse a
    // security key without a PIN after it has made the passkey, leaving a
    // passkey with no account. The document says so
    // (`x-calimero-user-verification: preferred`), and the generated method
    // asks the passkey for it.
    const session = await this.auth.registerFinish({ challengeId: begin.challengeId, registration, ...invite }, this.confirmWith(passkey, { signal }));
    this.useSession(session);
    return session;
  }

  /**
   * Sign in with an existing passkey. On a page whose origin is not the
   * cluster's ({@link SignInOptions}), the assertion is made for that
   * origin's relying-party id, as the node answers it, and the session is
   * bound to the origin's package; an unregistered origin is refused with
   * {@link OriginNotRegistered}. A passkey this cluster does not know (made
   * on another cluster, or removed) is refused with {@link PasskeyUnknownHere}.
   */
  async signIn(passkey: PasskeyProvider, options: SignInOptions = {}): Promise<CalimeroSession> {
    const origin = this.pageOrigin(options);
    const begin = await this.begin<{
      challengeId: string;
      options: { challenge: string; rpId?: string; rp?: { id?: string } };
    }>(this.auth.loginBegin({ ...origin === undefined ? {} : { origin } }), origin);
    const rpId = begin.options.rpId ?? begin.options.rp?.id ?? '';
    const challenge = fromB64url(begin.options.challenge);
    const assertion = await (options.signal
      ? passkey.get(challenge, rpId, undefined, { signal: options.signal })
      : passkey.get(challenge, rpId));
    const session = await this.auth.loginFinish({ challengeId: begin.challengeId, assertion });
    this.useSession(session);
    return session;
  }

  /**
   * The origin a sign-up or sign-in sends: the one given, or the page's own
   * when it is not a node's origin; `undefined` for none.
   */
  private pageOrigin(options: SignInOptions): string | undefined {
    if (options.origin !== undefined) return options.origin ?? undefined;
    const page = (globalThis as { location?: { origin?: string } }).location?.origin;
    if (!page || page === 'null') return undefined;
    const nodes = new Set(
      this.transport.nodes.map((url) => {
        try {
          return new URL(url).origin;
        } catch {
          return url;
        }
      }),
    );
    return nodes.has(page) ? undefined : page;
  }

  /** POST a ceremony's `begin`, naming the page's `origin` in an {@link OriginNotRegistered} the node leaves it out of. */
  private async begin<T>(result: Promise<unknown>, origin: string | undefined): Promise<T> {
    try {
      return await this.as<T>(result);
    } catch (error) {
      if (error instanceof OriginNotRegistered && !error.origin && origin !== undefined) {
        throw new OriginNotRegistered(error.nodeMessage, { ...error.details, origin });
      }
      throw error;
    }
  }

  /**
   * Swap the refresh token for a new session, on whichever node the client
   * uses: any node of the cluster takes it. A refresh token is used once, so
   * calls made while one refresh is under way share it rather than sending the
   * same token again (which would revoke the session).
   */
  refresh(): Promise<CalimeroSession> {
    return this.transport.refresh();
  }

  /**
   * End this session. The client is signed out whether or not the node
   * answers: the revoke (`POST /auth/logout`) is attempted, and its failure
   * (the node unreachable, say) is thrown after the session is dropped, so a
   * caller can say the server may still hold it, but no request after this
   * one carries its token.
   */
  async signOut(): Promise<void> {
    await this.signingOut(this.auth.logout());
  }

  /**
   * End every session of the signed-in account, on every device, this one
   * too. Like {@link signOut}, this client is signed out even when the
   * request fails, and the failure is thrown.
   */
  async signOutEverywhere(): Promise<void> {
    await this.signingOut(this.auth.logoutAll());
  }

  private async signingOut(result: Promise<unknown>): Promise<void> {
    try {
      await result;
    } finally {
      this.transport.signOut();
    }
  }

  /**
   * Add another passkey (another device, `passkey`) to the signed-in account;
   * answers its credential id. The node asks for the approval of a passkey
   * the account already has (`412 CONFIRM_REQUIRED`, never the new one),
   * which `approver` gives.
   */
  async addPasskey(passkey: PasskeyProvider, approver: PasskeyProvider): Promise<string> {
    const begin = await this.as<{ challengeId: string; options: Record<string, unknown> }>(this.auth.passkeyAddBegin());
    const registration = await passkey.create(begin.options);
    const { credentialId } = await this.auth.passkeyAddFinish({ challengeId: begin.challengeId, registration }, this.confirmWith(approver));
    return credentialId;
  }

  // ---------------------------------------------------------------- account recovery
  //
  // core #4844 (`docs/specs/account-recovery.md`): a recovery code, made with
  // a passkey and shown once, lets someone who lost every passkey register a
  // new one; it is added, and the account signed in, 72 hours later unless a
  // passkey the account still has cancels it first.

  /**
   * Signed out: start recovering the account `code` names. `passkey` makes a
   * new passkey for it (the browser's create prompt, user verified); the
   * node spends the code and keeps the passkey as pending. Nothing is added
   * and no session opens: finish with {@link finishAccountRecovery} at
   * `readyAt`. A code the cluster does not hold (mistyped, replaced or
   * spent) is {@link WrongRecoveryCode}; an account with a recovery already
   * under way `409 RECOVERY_PENDING`.
   */
  async startAccountRecovery(code: string, passkey: PasskeyProvider): Promise<AccountRecovery> {
    try {
      const begin = await this.as<{ challengeId: string; options: Record<string, unknown> }>(this.auth.accountRecoveryBegin({ code }));
      const registration = await passkey.create(begin.options);
      return await this.auth.accountRecoveryStart({ challengeId: begin.challengeId, code, registration });
    } catch (error) {
      if (error instanceof LoginRequired) throw new WrongRecoveryCode(error.message.replace(/^401 UNAUTHENTICATED: /, ''), error.details);
      throw error;
    }
  }

  /**
   * Signed out, once the delay passed: finish the recovery with the passkey
   * it registered (`passkey` signs the node's sign-in challenge). The passkey
   * joins the account, every other session of the account ends, and this
   * client is signed in. `accountId` (from the start) lets the node refuse
   * early and offer only that passkey; before `readyAt` it is
   * {@link RecoveryNotReady}, with no recovery under way {@link NotFound}.
   */
  async finishAccountRecovery(passkey: PasskeyProvider, accountId?: string): Promise<CalimeroSession> {
    const session = await this.auth.accountRecoveryFinish({ ...accountId ? { accountId } : {} }, this.confirmWith(passkey));
    this.useSession(session);
    return session;
  }

  // ---------------------------------------------------------------- apps

  /** One of `app`'s blobs, by id (base58 or hex). */
  async downloadBlob(app: string, id: string): Promise<Uint8Array> {
    return new Uint8Array(await (await this.apps.blobDownload({ app, id })).arrayBuffer());
  }

  private get exportClient() {
    return { exportsExport: this.apps.exportsExport, request: this.transport.request.bind(this.transport) };
  }

  /**
   * Write the plaintext of sealed file `id` of component app `app` to `sink`, checked against its id,
   * size and hash. On rejection, discard what `sink` received: it is authentic but may be a prefix of
   * the file. A failed check is an `ExportError`; `signal` aborts the download.
   */
  exportFileTo(app: string, id: string, sink: FileSink, signal?: AbortSignal): Promise<ExportedFile> {
    return exportFileTo(this.exportClient, app, id, sink, signal);
  }

  /** The plaintext of sealed file `id` of component app `app`, in memory; use {@link exportFileTo} for a large one. */
  exportFile(app: string, id: string, signal?: AbortSignal): Promise<Uint8Array> {
    return exportFile(this.exportClient, app, id, signal);
  }

  /**
   * Download a component app's blob with a grant one of its runs handed out
   * (`BlobRef::grant`, up to 24 h). No session needed: the grant is the
   * permission. A bad or expired grant answers `NotFound`, as a missing blob.
   */
  async downloadBlobGranted(app: string, id: string, grant: string): Promise<Uint8Array> {
    return new Uint8Array(await (await this.apps.blobDownload({ app, id, grant })).arrayBuffer());
  }

  /**
   * Upload a sealed file for component app `app`; answers `{ id, blobId, size, sha256, index }`.
   * Up to {@link FILE_PART_BYTES} it is one request, sent once (the node mints a new
   * file for each, so one that may have reached it is never repeated); a larger file
   * is a session of parts, each sent again after a dropped connection or a `503`.
   * A session the node lost is begun again once; any other failure aborts it.
   * `413 TOO_LARGE` and `429 QUOTA_EXCEEDED` are the node's refusals.
   */
  async uploadFile(app: string, data: FileData): Promise<FileInfo> {
    const size = data instanceof Blob ? data.size : data.byteLength;
    if (size <= FILE_PART_BYTES) return this.apps.filesUpload({ app, body: await readSlice(data, 0, size) });
    try {
      return await this.uploadSession(app, data, size);
    } catch (error) {
      if (!(error instanceof CalimeroError) || error.code !== 'UPLOAD_UNKNOWN') throw error;
      return this.uploadSession(app, data, size);
    }
  }

  /** Begin a session, send its parts and complete it, aborting it if that fails for good. */
  private async uploadSession(app: string, data: FileData, size: number): Promise<FileInfo> {
    const begin = await this.untilAvailable(() => this.apps.uploadBegin({ app, size }));
    const { uploadId: upload, partBytes, parts } = begin;
    if (!upload || !partBytes || partBytes <= 0 || parts !== Math.ceil(size / partBytes)) {
      throw new Error(`the node began an upload it cannot take: ${JSON.stringify(begin)}`);
    }
    try {
      await this.sendParts(app, upload, data, size, partBytes, parts);
      return await this.untilAvailable(() => this.apps.uploadComplete({ app, upload }));
    } catch (error) {
      if (!(error instanceof CalimeroError && error.code === 'UPLOAD_UNKNOWN')) await this.apps.uploadAbort({ app, upload }).catch(() => undefined);
      throw error;
    }
  }

  /** Send every part, in order, from wherever the node says the session is. */
  private async sendParts(app: string, upload: string, data: FileData, size: number, partBytes: number, parts: number): Promise<void> {
    let [next, failures, jumps] = [0, 0, 0];
    while (next < parts) {
      try {
        const body = await readSlice(data, next * partBytes, Math.min((next + 1) * partBytes, size));
        next = nextPart(await this.apps.uploadPart({ app, upload, k: String(next), body }), parts);
        failures = 0;
        continue;
      } catch (error) {
        if (error instanceof CalimeroError && error.code === 'PART_OUT_OF_ORDER') {
          jumps += 1;
          if (jumps > MAX_JUMPS) throw error;
          next = nextPart(error.details, parts);
          continue;
        }
        if (!this.isTransient(error) || ++failures > MAX_FAILURES) throw error;
        await new Promise((resolve) => setTimeout(resolve, retryDelayMs(failures - 1, this.transport.retry)));
        if (failures > PART_RESENDS) {
          try {
            const at = nextPart(await this.apps.uploadStatus({ app, upload }), parts);
            if (at > next) failures = 0;
            next = at;
          } catch (asked) {
            if (!this.isTransient(asked)) throw asked;
          }
        }
      }
    }
  }

  /** `send` again, after a backoff, while the node is unavailable or cannot be reached. */
  private async untilAvailable<T>(send: () => Promise<T>): Promise<T> {
    for (let retry = 0; ; retry++) {
      try {
        return await send();
      } catch (error) {
        if (!this.isTransient(error) || retry >= CALL_RETRIES) throw error;
        await new Promise((resolve) => setTimeout(resolve, retryDelayMs(retry, this.transport.retry)));
      }
    }
  }

  /** Whether the same request may succeed if sent again: the node answered `503`, or gave no answer. */
  private isTransient(error: unknown): boolean {
    return error instanceof UnavailableError || this.transport.isNetworkFailure(error);
  }

  // ---------------------------------------------------------------- packages

  // ---------------------------------------------------------------- app instances

  /** Committed events from index `from`, then live, until `signal` aborts. */
  events(app: string, from = 0, signal?: AbortSignal): AsyncGenerator<NodeEvent> {
    return this.transport.events(routePath('apps/{app}/events', { app }), { query: { from }, signal });
  }

  // ---------------------------------------------------------------- presence

  /**
   * Presence as a stream until `signal` aborts: a `snapshot` event (`data` the
   * entries present), then a `presence` event (`data` one entry) per change.
   */
  presenceStream(app: string, signal?: AbortSignal): AsyncGenerator<NodeEvent> {
    return this.transport.events(routePath('apps/{app}/presence/stream', { app }), { signal });
  }

  // ---------------------------------------------------------------- aliases

  // ---------------------------------------------------------------- organizations

  /**
   * Start handing `org` to the signed-in account with the recovery seed,
   * which signs here and never leaves this process. Finish with
   * `client.orgs.recoveryFinish` once `readyAt` has passed; until then the owner may
   * cancel. A wrong seed (or no organization, or none set) is a `403`.
   */
  async startRecovery(org: string, seed: Uint8Array): Promise<RecoveryTimes> {
    const account = (await this.auth.me()).accountId as string;
    const nonce = b64url(crypto.getRandomValues(new Uint8Array(16)));
    const signature = await signRecovery(seed, org, account, nonce);
    return this.orgs.recoveryStart({ org, nonce, signature });
  }

  // ---------------------------------------------------------------- approvals

  /** The report of a scope's apps (`GET /orgs/{org}/apps`, or with `org` `null` the account's own, `GET /auth/apps`), every page, with the apps of all of them in `apps`. */
  async appsReportAll(org: string | null): Promise<AppsReport> {
    const report = (page: PageOptions = {}) =>
      org === null ? this.auth.accountApps(pageParams(page)) : this.orgs.apps({ org, ...pageParams(page) });
    const first = await report();
    const apps: ReportedApp[] = [...first.apps];
    let next = first.next;
    while (next !== null) {
      const page = await report({ after: next });
      apps.push(...page.apps);
      next = page.next;
    }
    return { ...first, apps, next: null };
  }

  /**
   * What waits for a decision in a scope (`org`, or the account's own apps
   * with `null`): mero-tee releases the cluster's nodes run that the scope has
   * not approved, apps with newer releases of their package, and whether a
   * passed deadline stops new work. Read over the sealed channel; act on an
   * item by sending the user to the consent page (`client.consent.pageCreate`,
   * `releaseConsent`, `upgradeConsent`).
   */
  async pendingApprovals(org: string | null = null): Promise<PendingApprovals> {
    const [report, rules] = await Promise.all([this.appsReportAll(org), this.public.releasesPublic()]);
    return pendingApprovalsFrom(report, rules);
  }

  /**
   * `client.consent.pageCreate`, then send the browser to the consent page
   * (`navigate`, by default `window.location.assign`). Pass `navigate` itself
   * or {@link OpenConsentOptions}, whose `confirm` runs first with the node's
   * answer so the app can show the person its `code`; when it answers `false`
   * the page is not opened. When the node sends a `code` that is not
   * {@link confirmationCode} of `requestId`, it throws `CalimeroError`
   * (`CONSENT_CODE_MISMATCH`) and does not navigate. Answers the request
   * once `navigate` returns (or `confirm` declines): with a popup
   * (`url => window.open(url)`), poll it with {@link waitForConsent}.
   * Before navigating it keeps `requestId` in `sessionStorage` (where there
   * is one), for {@link resumeConsent} when the page sends the person back.
   */
  async openConsentPage(
    request: ConsentRequest,
    navigateOrOptions: ((url: string) => void) | OpenConsentOptions = {},
  ): Promise<ConsentStarted> {
    const options = typeof navigateOrOptions === 'function' ? { navigate: navigateOrOptions } : navigateOrOptions;
    const navigate =
      options.navigate ??
      ((url: string) => {
        const w = (globalThis as { window?: { location: { assign(url: string): void } } }).window;
        if (!w) throw new Error('no window to open the consent page in; pass `navigate`');
        w.location.assign(url);
      });
    const started = await this.consent.pageCreate({ body: request });
    if (started.code !== undefined) {
      const expected = await confirmationCode(started.requestId);
      if (started.code !== expected) {
        throw new CalimeroError(200, 'CONSENT_CODE_MISMATCH', 'the consent page code does not match the request', {
          requestId: started.requestId,
          code: started.code,
          expected,
        });
      }
    }
    if (options.confirm && !(await options.confirm(started))) return started;
    try {
      consentStorage()?.setItem(CONSENT_STORAGE_KEY, started.requestId);
    } catch {
      // No storage here: pass `requestId` to `resumeConsent`.
    }
    navigate(started.url);
    return started;
  }

  /**
   * On the page the consent page sent the person back to
   * (`returnTo?request_id=…&result=…`): the decision on the request this page
   * started, read from the node (`GET /consent/{id}/result`), never from the
   * URL. `null` when the URL carries no `request_id`. The request is
   * `options.requestId`, or the one {@link openConsentPage} stored in this
   * tab; with the default `href` (`location.href`) the parameters are then
   * removed with `history.replaceState`, and the stored id once decided.
   *
   * Throws `CalimeroError`:
   * - `CONSENT_NOT_STARTED`: the URL names a request, and this page started none;
   * - `CONSENT_REQUEST_MISMATCH`: the URL names another request than this page's;
   * - `CONSENT_NOT_DECIDED`: this page's request is still pending although the
   *   page sent the person back, a sign that another request was approved on a
   *   page that was not the node's. Do not act on it;
   * - `CONSENT_REPLACED`: the node no longer knows this page's request (`404`):
   *   an account has one undecided request per node, and a newer one replaced
   *   it, possibly on a page that was not the node's. Do not act on it either.
   *
   * The URL's `result` is never trusted: the answer's `status` is the node's.
   * An approval's `outcome` (`orgId`, an invite's `token`, the `org` joined) is
   * in the answer, typed by `kind`.
   */
  async resumeConsent(options: ResumeConsentOptions = {}): Promise<ConsentResult | null> {
    const g = globalThis as {
      location?: { href: string };
      history?: { state: unknown; replaceState(state: unknown, unused: string, url?: string): void };
    };
    const href = options.href ?? g.location?.href;
    if (href === undefined) throw new Error('no page to read the consent return from; pass `href`');
    const url = new URL(href);
    const returned = url.searchParams.get('request_id');
    if (!returned) return null;
    if (options.href === undefined && g.history) g.history.replaceState(g.history.state, '', withoutConsentReturn(url));

    let ownId = options.requestId;
    if (ownId === undefined) {
      try {
        ownId = consentStorage()?.getItem(CONSENT_STORAGE_KEY) ?? undefined;
      } catch {
        ownId = undefined;
      }
    }
    if (!ownId) {
      throw new CalimeroError(200, 'CONSENT_NOT_STARTED', 'this page started no consent request', { returned });
    }
    if (returned !== ownId) {
      throw new CalimeroError(200, 'CONSENT_REQUEST_MISMATCH', 'the consent page returned another request than this page started', {
        requestId: ownId,
        returned,
      });
    }
    let result: ConsentResult;
    try {
      result = (await this.consent.pageResult({ request: ownId })) as ConsentResult;
    } catch (e) {
      // One undecided request per account per node: a newer one replaced
      // this page's, which then answers 404, perhaps swapped on a forged page.
      if (e instanceof NotFound) {
        forgetConsent(ownId);
        throw new CalimeroError(404, 'CONSENT_REPLACED', "this page's consent request was replaced by a newer one", {
          requestId: ownId,
        });
      }
      throw e;
    }
    if (result.requestId !== ownId) {
      throw new CalimeroError(200, 'CONSENT_REQUEST_MISMATCH', 'the node answered another request', { requestId: ownId, returned: result.requestId });
    }
    if (result.status === 'pending') {
      throw new CalimeroError(200, 'CONSENT_NOT_DECIDED', 'the consent page returned, but this request is still pending', {
        requestId: ownId,
      });
    }
    forgetConsent(ownId);
    return result;
  }

  /**
   * Poll `client.consent.pageResult` until the request is decided, every
   * `intervalMs` (2 s), until `signal` aborts or `expiresAt` (Unix seconds)
   * passes, when it answers the last `pending` result.
   */
  async waitForConsent(
    requestId: string,
    options: { intervalMs?: number; signal?: AbortSignal; expiresAt?: number } = {},
  ): Promise<ConsentResult> {
    const interval = options.intervalMs ?? 2000;
    for (;;) {
      const result = (await this.consent.pageResult({ request: requestId })) as ConsentResult;
      if (result.status !== 'pending') return result;
      if (options.signal?.aborted) return result;
      if (options.expiresAt !== undefined && Date.now() / 1000 >= options.expiresAt) return result;
      await new Promise((resolve) => setTimeout(resolve, interval));
    }
  }

  // ---------------------------------------------------------------- operator

  // ---------------------------------------------------------------- app teams

  // ---------------------------------------------------------------- package origins

  // ---------------------------------------------------------------- deployers

  // ---------------------------------------------------------------- deployments

}

/** This page's `sessionStorage`, where there is one (a sandboxed frame or a turned-off store throws). */
function consentStorage(): Pick<Storage, 'getItem' | 'setItem' | 'removeItem'> | undefined {
  try {
    return (globalThis as { sessionStorage?: Storage }).sessionStorage;
  } catch {
    return undefined;
  }
}

/** Drop the stored consent request id, when it is `requestId`. */
function forgetConsent(requestId: string): void {
  try {
    const store = consentStorage();
    if (store?.getItem(CONSENT_STORAGE_KEY) === requestId) store.removeItem(CONSENT_STORAGE_KEY);
  } catch {
    // nothing to clear
  }
}
