/**
 * Release and upgrade approvals as core's node reports them, and the consent
 * page an app sends its user to when one is theirs to make.
 *
 * An app never approves in its own page. A passkey assertion is blind (the
 * authenticator shows a site name, never what the challenge commits to), so
 * the page that runs the ceremony decides what the person believes they
 * approve. The node therefore refuses every approval route to a session
 * opened on an app's frontend, and serves its own **consent page** instead:
 * the app asks for a request (`POST /consent/requests`), sends the person to
 * the page's `url` on the node's own origin, and reads the decision when the
 * page sends them back (`returnTo?request_id=…&result=…`) or from
 * `GET /consent/{id}/result`. See core's
 * [consent page](https://calimero-network.github.io/core/protocol/consent-page/)
 * and [governance](https://calimero-network.github.io/core/protocol/governance/).
 */

import { compareReleaseVersions } from '../sealed/release.js';
import type { ApiAnswer, ApiSchema } from '../routes/types.js';
import type { operations } from '../generated/api.js';

// ---------------------------------------------------------------- governance

/**
 * Who holds one decision on a cluster (core's `Authority`), fixed at genesis
 * and attested by every replica. `owner`: the parties around each app, with a
 * passkey (for release approvals the app's publisher, for upgrades whoever
 * its package's manifest names); `operator`: the operator, for every app.
 */
export type Authority = 'owner' | 'operator';

/** Core's `Governance`: which side holds each decision on this cluster. */
export interface Governance {
  releases: Authority;
  appUpgrades: Authority;
  outbound: Authority;
  /** Who may install apps: any account (`open`, left out) or granted deployers only. */
  deployments?: 'open' | 'granted';
  /** Who grants deployers: the operator (left out) or the cluster's owner. */
  deployers?: 'operator' | 'owner';
  /** Who may create an account: anyone (`open`, left out), invitees or pre-authorized people only. */
  signUp?: 'open' | 'invite' | 'closed';
}

// ---------------------------------------------------------------- reports

/** Whose approvals a report covers: an organization's, or the account's own apps. */
export type ReportScope = { kind: 'account' } | { kind: 'org'; org: string };

/** A bundle as a report names it: its digest, and its release where the reader may see it. */
export interface ReportedBundle {
  bundle: string;
  package?: string;
  version?: string;
}

/** Who made an upgrade, by kind. */
export type ActorKind = 'owner' | 'member' | 'operator';

/** A replica's node whose runtime is older than a release needs, and what it runs: the version, or `{ before }` for a node on a release from before nodes named their runtime. */
export interface RuntimeShortNode {
  node: number;
  runtime: string | { before: string } | null;
}

/**
 * Why an app cannot move to the newest release it may see (core's
 * `latestBlocked`): `runtime`, a replica's node runs an older runtime than
 * the release's `minRuntimeVersion` (the upgrade would be refused `409
 * RUNTIME_TOO_OLD`); `authority`, the app's owner upgrades it and the
 * release would give its upgrades to the publisher.
 */
export interface LatestBlocked {
  version: string;
  reason: 'runtime' | 'authority';
  message: string;
  minRuntimeVersion?: string;
  nodes?: RuntimeShortNode[];
}

/** One app in a report. `name`, `team`, `availableVersions`, `latestReachable` and `latestBlocked` are in members' and accounts' reports only. */
export type ReportedApp = ApiAnswer<'org_apps'>['apps'][number];

/** `GET /auth/apps` or `GET /orgs/{org}/apps`: one page. */
export type AppsReport = ApiAnswer<'org_apps'>;

/** A page of a report: entries after the id `after`, at most `limit` (1 to {@link MAX_REPORT_PAGE}). */
export interface PageOptions {
  after?: string;
  limit?: number;
}

/** The most entries one page of a report answers (core's `MAX_REPORT_PAGE`). */
export const MAX_REPORT_PAGE = 100;

/** `?after=&limit=` for a report route; `limit` outside 1 to 100 is refused before sending. */
export function pageQuery(page: PageOptions = {}): string {
  const q = new URLSearchParams();
  if (page.after !== undefined) q.set('after', page.after);
  if (page.limit !== undefined) {
    if (!Number.isInteger(page.limit) || page.limit < 1 || page.limit > MAX_REPORT_PAGE) {
      throw new RangeError(`limit is 1 to ${MAX_REPORT_PAGE}`);
    }
    q.set('limit', String(page.limit));
  }
  const s = q.toString();
  return s ? `?${s}` : '';
}

// ---------------------------------------------------------------- deadlines

/** A release's end-of-new-work deadline. */
export interface ReleaseDeadline {
  version: string;
  /** RFC 3339. */
  after: string;
  /** Unix seconds. */
  afterSecs: number;
}

/** `GET /releases`: the cluster's release rules, public inside the sealed channel. */
export type ReleaseRules = ApiAnswer<'releases_public'>;

/** Semver order (core's `compare_versions`); text order for versions that are not semver. */
export function compareVersions(a: string, b: string): number {
  try {
    return compareReleaseVersions(a, b);
  } catch {
    return a < b ? -1 : a > b ? 1 : 0;
  }
}

/**
 * Whether a scope whose approved set is `approved` has lost new work at `now`
 * (Unix seconds), as core's `SystemState::stalled` judges it: a release's
 * deadline has passed and the newest approved release is older. Names the
 * newest such release, since approving it lifts every one. `null` approved
 * (unrestricted) is never stalled.
 */
export function stalledBy(
  approved: string[] | null,
  deadlines: ReleaseDeadline[],
  now: number = Date.now() / 1000,
): ReleaseDeadline | null {
  if (approved === null || approved.length === 0) return null;
  const newest = approved.reduce((a, b) => (compareVersions(a, b) >= 0 ? a : b));
  let stall: ReleaseDeadline | null = null;
  for (const d of deadlines) {
    if (d.afterSecs <= now && compareVersions(newest, d.version) < 0) {
      if (stall === null || compareVersions(d.version, stall.version) > 0) stall = d;
    }
  }
  return stall;
}

// ---------------------------------------------------------------- publishers

/**
 * The text a package signing key signs to bind it to `org` as its publisher
 * (core's `system::publisher_binding_message`). Take `cluster` and
 * `generation` from `client.public.publisherBinding`; sign the UTF-8
 * bytes with the key's Ed25519 secret and send
 * `client.orgs.publisher`.
 */
export function publisherBindingMessage(cluster: string, org: string, signerId: string, generation: number): string {
  return `calimero-publisher-binding/v1\n${cluster}\n${org}\n${signerId}\n${generation}`;
}

/** {@link publisherBindingMessage} as the bytes to sign. */
export function publisherBindingBytes(cluster: string, org: string, signerId: string, generation: number): Uint8Array {
  return new TextEncoder().encode(publisherBindingMessage(cluster, org, signerId, generation));
}

// ---------------------------------------------------------------- consent page

/** Whose approval a release or outbound request is for. */
export type ConsentScope = 'account' | { org: string };

/**
 * What the app asks its user to approve (`POST /consent/requests`), by
 * `kind`: the route's generated body. Unknown fields are refused. Build one
 * with the helpers below (`releaseConsent`, `createOrgConsent`, ...), each
 * typed as the kind it builds.
 *
 * - `release`, `upgrade`, `outbound`: a release approval (or withdrawal), an
 *   app upgrade, outbound prefixes.
 * - `createOrg`, `createInvite`, `acceptInvite` (core#5014): a new
 *   organization, an invite, joining with one.
 * - `leaveOrg`, `removeMember`, `setMemberRole`, `setMemberTeams`,
 *   `removeMemberTeams`, `revokeInvite`, `renameOrg`, `deleteOrg`,
 *   `setCapabilities`, `setAllApps` (core#5018): managing an organization.
 *
 * `returnTo` is where the page sends the person back: the package's frontend
 * origin, or one its manifest names.
 */
export type ConsentRequest = NonNullable<operations['consent_page_create']['requestBody']>['content']['application/json'];

/** A consent request's kind. */
export type ConsentKind = ConsentRequest['kind'];

/** The consent request of kind `K`. */
type ConsentRequestOf<K extends ConsentKind> = Extract<ConsentRequest, { kind: K }>;

/**
 * What a `createInvite` consent request asks for: `org`, `role`, and
 * optionally `teams` to join, `ttlSecs` it lasts, `uses` it allows and
 * `allApps`, every app of the organization.
 */
export type InviteConsentSubject = ConsentRequestOf<'createInvite'>['subject'];

/** A role an invite gives (core's `Role`, short of `owner`). */
export type InviteRole = InviteConsentSubject['role'];

/** A member's role in an organization (core's `Role`). */
export type OrgRole = ApiSchema<'OrgRole'>;

/** A member's role in a team (core's `TeamRole`). */
export type TeamRole = ApiSchema<'TeamRole'>;

/**
 * One team row a `setMemberTeams` consent request sets: `team`, and the
 * member's `role` and `capabilities` bits (`ORG_CAPABILITIES`) there,
 * each the team's default when left out.
 */
export type MemberTeamConsent = ConsentRequestOf<'setMemberTeams'>['subject']['teams'][number];

/**
 * Whose capability bits (`ORG_CAPABILITIES`) a `setCapabilities`
 * consent request sets: with `account`, that member's (organization-wide, or
 * in `team`); without, a default (the organization's, or `team`'s).
 */
export type CapabilitiesConsentSubject = ConsentRequestOf<'setCapabilities'>['subject'];

/** The node's answer to a consent request: send the person to `url`. */
export type ConsentStarted = ApiAnswer<'consent_page_create'>;

/** How {@link CalimeroClient.openConsentPage} sends the person to the page. */
export interface OpenConsentOptions {
  /** Sends the browser to the page's `url`; by default `window.location.assign`. */
  navigate?: (url: string) => void;
  /**
   * Runs after the node answers and before navigating, with the answer: show
   * the person `started.code` here so they can check the page shows the same.
   * Answer `false` and the page is not opened (the request stays on the node
   * until it expires).
   */
  confirm?: (started: ConsentStarted) => boolean | Promise<boolean>;
}

const CROCKFORD = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';

/**
 * The confirmation code the node's consent page shows for `requestId`: the
 * first 30 bits of SHA-256 of its UTF-8 bytes, big-endian, as six Crockford
 * base32 characters written `XXX-XXX` (core's consent page). Compare it with
 * {@link ConsentStarted.code} to check the node's answer.
 */
export async function confirmationCode(requestId: string): Promise<string> {
  const h = new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(requestId)));
  const word = ((h[0] << 24) | (h[1] << 16) | (h[2] << 8) | h[3]) >>> 0;
  let code = '';
  for (let i = 0; i < 6; i++) {
    if (i === 3) code += '-';
    code += CROCKFORD[(word >>> (27 - 5 * i)) & 31];
  }
  return code;
}

/**
 * What an approval made, by kind (`outcome` on {@link ConsentResult}, once
 * approved): `createOrg`, `createInvite`, `acceptInvite`, `renameOrg` and
 * `deleteOrg` make something; the other kinds nothing. An invite's `token` is
 * a credential: keep it out of logs.
 */
export interface ConsentOutcomes {
  release: never;
  upgrade: never;
  outbound: never;
  createOrg: Required<Pick<GeneratedOutcome, 'orgId'>>;
  createInvite: Required<Pick<GeneratedOutcome, 'token'>>;
  acceptInvite: Required<Pick<GeneratedOutcome, 'org'>>;
  leaveOrg: never;
  removeMember: never;
  setMemberRole: never;
  setMemberTeams: never;
  removeMemberTeams: never;
  revokeInvite: never;
  renameOrg: Required<Pick<GeneratedOutcome, 'orgId' | 'name'>>;
  /** The apps deleted with the organization. */
  deleteOrg: Required<Pick<GeneratedOutcome, 'apps'>>;
  setCapabilities: never;
  setAllApps: never;
}

/** The route's generated answer, whose one `outcome` has every kind's fields optional. */
type GeneratedResult = ApiAnswer<'consent_page_result'>;
type GeneratedOutcome = NonNullable<GeneratedResult['outcome']>;

/** `GET /consent/{id}/result` for a request of kind `K`: `requestId`, `status`, and `decidedAt` (Unix seconds; `null` while pending). */
export interface ConsentResultOf<K extends ConsentKind> extends Omit<GeneratedResult, 'kind' | 'outcome'> {
  kind: K;
  /** What the approval made, once approved, for the kinds that make something. */
  outcome?: ConsentOutcomes[K];
}

/**
 * `GET /consent/{id}/result`, narrowed by `kind`:
 * `if (r.kind === 'createOrg') r.outcome?.orgId`. Read `outcome` only from
 * this sealed answer, never from the page's redirect, which carries none.
 *
 * The generated answer, with `outcome` typed by kind.
 */
export type ConsentResult = { [K in ConsentKind]: ConsentResultOf<K> }[ConsentKind];

/**
 * What the consent page put on `returnTo` when it sent the person back, or
 * `null` when `href` carries none. A hint only (anyone can type a URL): confirm
 * with `client.consent.pageResult` before acting on it.
 */
export function consentReturn(href: string | URL): { requestId: string; result: 'approved' | 'declined' } | null {
  const url = typeof href === 'string' ? new URL(href) : href;
  const requestId = url.searchParams.get('request_id');
  const result = url.searchParams.get('result');
  if (!requestId || (result !== 'approved' && result !== 'declined')) return null;
  return { requestId, result };
}

/** `href` without the consent page's `request_id` and `result`. */
export function withoutConsentReturn(href: string | URL): string {
  const url = new URL(String(href));
  url.searchParams.delete('request_id');
  url.searchParams.delete('result');
  return url.toString();
}

/** The `sessionStorage` key {@link CalimeroClient.openConsentPage} keeps its request id under, for {@link CalimeroClient.resumeConsent}. */
export const CONSENT_STORAGE_KEY = 'calimero.consent';

/** How {@link CalimeroClient.resumeConsent} reads the page's return. */
export interface ResumeConsentOptions {
  /**
   * The request this page started; by default the one
   * {@link CalimeroClient.openConsentPage} stored in this tab.
   */
  requestId?: string;
  /** The URL the page sent the person back to; by default `location.href`, whose parameters are then removed. */
  href?: string;
}

// ---------------------------------------------------------------- pending

/** The public verifier for mero-tee releases (`attestation-verifier`). */
export const RELEASE_VERIFIER = 'https://mero-tee.vercel.app';

/** A mero-tee release's notes on GitHub. */
export function releaseNotesUrl(version: string): string {
  return `https://github.com/calimero-network/mero-tee/releases/tag/mero-tee-v${encodeURIComponent(version)}`;
}

function verifierUrl(query: Record<string, string>): string {
  return `${RELEASE_VERIFIER}/mero-tee?${new URLSearchParams(query)}`;
}

/** The public verifier's page for a mero-tee release. */
export function verifyReleaseUrl(version: string): string {
  return verifierUrl({ release_tag: `mero-tee-v${version}` });
}

/** The public verifier's page for a node: it fetches a fresh quote from `nodeUrl` and checks it against `release`. */
export function verifyNodeUrl(nodeUrl: string, release: string): string {
  return verifierUrl({ node_url: nodeUrl.replace(/\/+$/, ''), release_tag: `mero-tee-v${release}` });
}

/** A mero-tee release the cluster's nodes run that the scope has not approved. */
export interface PendingRelease {
  version: string;
  /** How many of the cluster's nodes run it. */
  nodes: number;
  /** Whether the scope's side holds release approvals (else the operator does). */
  actionable: boolean;
  releaseNotesUrl: string;
  verifyUrl: string;
  /** Its deadline, when it carries one. */
  deadline: ReleaseDeadline | null;
}

/** An app with newer releases of its package. */
export interface PendingUpgrade {
  app: string;
  name: string | null;
  package: string;
  from: string;
  /**
   * The newest it can move to now (the report's `latestReachable`); when
   * none can be (`reachable: false`), or from a node that does not say, the
   * newest it may see.
   */
  to: string;
  /** Whether an upgrade to `to` would be taken now; `false`: `blocked` says why none is. */
  reachable: boolean;
  /** Why the newest release it may see is not reachable, when it is not. */
  blocked: LatestBlocked | null;
  /** Every newer release it may move to, oldest first. */
  available: string[];
  /** Whether owners hold app upgrades on this cluster; the node checks the package's own `upgrades` when asked. */
  actionable: boolean;
}

/** What {@link CalimeroClient.pendingApprovals} answers. */
export interface PendingApprovals {
  scope: ReportScope;
  governance: Governance;
  approvedReleases: string[] | null;
  releases: PendingRelease[];
  upgrades: PendingUpgrade[];
  /** A passed deadline has stopped new work for the scope: approve `stalledBy.version` (or newer) to lift it. */
  stalledBy: ReleaseDeadline | null;
  /** Whether anything here is the user's to approve. */
  actionable: boolean;
}

/**
 * The pending list from a scope's report pages (every page's apps
 * concatenated into `report.apps`) and the cluster's release rules.
 */
export function pendingApprovalsFrom(report: AppsReport, rules: ReleaseRules, now: number = Date.now() / 1000): PendingApprovals {
  const deadlineOf = (v: string) => rules.deadlines.find((d) => d.version === v) ?? null;
  const releasesActionable = report.governance.releases === 'owner';
  const upgradesActionable = report.governance.appUpgrades === 'owner';
  const releases: PendingRelease[] = report.clusterReleases
    .filter((r) => !r.approved)
    .sort((a, b) => compareVersions(b.release, a.release))
    .map((r) => ({
      version: r.release,
      nodes: r.nodes,
      actionable: releasesActionable,
      releaseNotesUrl: releaseNotesUrl(r.release),
      verifyUrl: verifyReleaseUrl(r.release),
      deadline: deadlineOf(r.release),
    }));
  const upgrades: PendingUpgrade[] = [];
  for (const a of report.apps) {
    const available = a.availableVersions ?? [];
    if (a.package === null || available.length === 0) continue;
    const latest = available[available.length - 1];
    const reachable = a.latestReachable === undefined ? latest : a.latestReachable;
    upgrades.push({
      app: a.app,
      name: a.name ?? null,
      package: a.package.package,
      from: a.package.version,
      to: reachable ?? latest,
      reachable: reachable !== null,
      blocked: a.latestBlocked ?? null,
      available,
      actionable: upgradesActionable,
    });
  }
  return {
    scope: report.scope,
    governance: report.governance,
    approvedReleases: report.approvedReleases,
    releases,
    upgrades,
    stalledBy: stalledBy(report.approvedReleases, rules.deadlines, now),
    actionable: releases.some((r) => r.actionable) || upgrades.some((u) => u.actionable),
  };
}

/**
 * The consent request that approves `version` for `scope`, or with
 * `{ withdraw: true }` withdraws its approval ({@link withdrawConsent}).
 */
export function releaseConsent(
  scope: ReportScope,
  version: string,
  returnTo: string,
  options: { withdraw?: boolean } = {},
): ConsentRequest & { kind: 'release' } {
  return {
    kind: 'release',
    scope: scope.kind === 'org' ? { org: scope.org } : 'account',
    subject: options.withdraw ? { version, withdraw: true } : { version },
    returnTo,
  };
}

/**
 * The consent request that withdraws `version`'s approval for `scope`: nodes
 * on it are then sent none of the scope's apps' data (nothing already there
 * moves). The page fetches no measurements for it. Withdrawing the scope's
 * last approved release is refused on the page (`400 REJECTED`, the request
 * stays pending), and the page warns before.
 */
export function withdrawConsent(
  scope: ReportScope,
  version: string,
  returnTo: string,
): ConsentRequest & { kind: 'release' } {
  return releaseConsent(scope, version, returnTo, { withdraw: true });
}

/** The consent request that upgrades `app` to `toVersion`. */
export function upgradeConsent(
  app: string,
  toVersion: string,
  returnTo: string,
): ConsentRequest & { kind: 'upgrade' } {
  return { kind: 'upgrade', subject: { app, toVersion }, returnTo };
}

/**
 * The consent request that creates an organization named `name`; its outcome names the new `orgId`.
 * `capabilities` (`ORG_CAPABILITIES` bits) is what members get unless set for them, in place of the default.
 */
export function createOrgConsent(
  name: string,
  returnTo: string,
  capabilities?: number,
): ConsentRequest & { kind: 'createOrg' } {
  return { kind: 'createOrg', subject: capabilities === undefined ? { name } : { name, capabilities }, returnTo };
}

/** The consent request that creates an invite to `subject.org`; its outcome carries the invite's `token`. */
export function createInviteConsent(subject: InviteConsentSubject, returnTo: string): ConsentRequest & { kind: 'createInvite' } {
  return { kind: 'createInvite', subject, returnTo };
}

/** The consent request that joins the organization `token` invites to; its outcome names the `org`. */
export function acceptInviteConsent(token: string, returnTo: string): ConsentRequest & { kind: 'acceptInvite' } {
  return { kind: 'acceptInvite', subject: { token }, returnTo };
}

/** The consent request that has the person leave `org`. */
export function leaveOrgConsent(org: string, returnTo: string): ConsentRequest & { kind: 'leaveOrg' } {
  return { kind: 'leaveOrg', subject: { org }, returnTo };
}

/** The consent request that removes `account` from `org`. */
export function removeMemberConsent(org: string, account: string, returnTo: string): ConsentRequest & { kind: 'removeMember' } {
  return { kind: 'removeMember', subject: { org, account }, returnTo };
}

/** The consent request that gives the member `account` of `org` the role `role`. */
export function setMemberRoleConsent(
  org: string,
  account: string,
  role: OrgRole,
  returnTo: string,
): ConsentRequest & { kind: 'setMemberRole' } {
  return { kind: 'setMemberRole', subject: { org, account, role }, returnTo };
}

/** The consent request that sets the member `account`'s rows in teams of `org`. */
export function setMemberTeamsConsent(
  org: string,
  account: string,
  teams: MemberTeamConsent[],
  returnTo: string,
): ConsentRequest & { kind: 'setMemberTeams' } {
  return { kind: 'setMemberTeams', subject: { org, account, teams }, returnTo };
}

/** The consent request that removes the member `account` from `teams` of `org`. */
export function removeMemberTeamsConsent(
  org: string,
  account: string,
  teams: string[],
  returnTo: string,
): ConsentRequest & { kind: 'removeMemberTeams' } {
  return { kind: 'removeMemberTeams', subject: { org, account, teams }, returnTo };
}

/** The consent request that revokes the invite `inviteKey` of `org`. */
export function revokeInviteConsent(org: string, inviteKey: string, returnTo: string): ConsentRequest & { kind: 'revokeInvite' } {
  return { kind: 'revokeInvite', subject: { org, inviteKey }, returnTo };
}

/** The consent request that renames `org` to `name`; its outcome is `{ orgId, name }`. */
export function renameOrgConsent(org: string, name: string, returnTo: string): ConsentRequest & { kind: 'renameOrg' } {
  return { kind: 'renameOrg', subject: { org, name }, returnTo };
}

/** The consent request that deletes `org`; its outcome lists the `apps` deleted with it. */
export function deleteOrgConsent(org: string, returnTo: string): ConsentRequest & { kind: 'deleteOrg' } {
  return { kind: 'deleteOrg', subject: { org }, returnTo };
}

/** The consent request that sets capability bits in `subject.org` ({@link CapabilitiesConsentSubject}). */
export function setCapabilitiesConsent(
  subject: CapabilitiesConsentSubject,
  returnTo: string,
): ConsentRequest & { kind: 'setCapabilities' } {
  return { kind: 'setCapabilities', subject, returnTo };
}

/** The consent request that gives the member `account` of `org` every app (`allApps: true`), or takes it back. */
export function setAllAppsConsent(
  org: string,
  account: string,
  allApps: boolean,
  returnTo: string,
): ConsentRequest & { kind: 'setAllApps' } {
  return { kind: 'setAllApps', subject: { org, account, allApps }, returnTo };
}
