// Compiled by tsconfig.consumer.json as an external NodeNext consumer of the built
// dist/. The @ts-expect-error lines fail if unresolvable specifiers widen the API to any.
import {
  CalimeroClient,
  ConfirmRequired,
  browserPasskey,
  releaseConsent,
  upgradeConsent,
  withdrawConsent,
} from '@calimero-network/mero-js';
import type { CalimeroClientOptions, CalimeroSession, NodePolicy, UpgradeRun } from '@calimero-network/mero-js';

const policy: NodePolicy = { release: { minReleaseVersion: '2.3.109' } };
const options: CalimeroClientOptions = { baseUrl: 'https://node.example', policy };

export const client = new CalimeroClient(options);
export const passkey = browserPasskey();

// @ts-expect-error baseUrl is required
new CalimeroClient({ policy });

// @ts-expect-error minReleaseVersion is a string
export const badPolicy: NodePolicy = { release: { minReleaseVersion: 2 } };

export const accountOf = (s: CalimeroSession): string => s.accountId;

export function finished(run: UpgradeRun): boolean {
  return run.finished;
}

// @ts-expect-error a run's failures are keyed by app id, not a list
export const failedList: UpgradeRun['failed'] = ['a-1'];

export function needsPasskey(e: unknown): boolean {
  return e instanceof ConfirmRequired && e.status === 412;
}

// The runtime entry point: a route layer of its own sends through a client's transport.
import { NodeTransport, routePath } from '@calimero-network/mero-js/runtime';
import type { RequestOptions, Transport } from '@calimero-network/mero-js/runtime';

export const transport: Transport = client.transport;
export const own = new NodeTransport(options);
export const sameClient = new CalimeroClient(own);
export const getApp = (app: string): Promise<{ appId: string }> => transport.json('GET', routePath('apps/{app}', { app }));

// @ts-expect-error a method is one of the node's
export const badMethod = (): Promise<Response> => transport.request('FETCH', 'apps');

export const stepUp: RequestOptions = { body: {}, passkey, retry: 'once' };

// The generated methods on a transport of your own.
import { createRouteClient, createRoutes, passkeyConfirm } from '@calimero-network/mero-js';
import type { NodeEvent, Routes } from '@calimero-network/mero-js';

export const ownRoutes: Routes = createRoutes(createRouteClient(client.transport, { confirm: passkeyConfirm(passkey) }));
export const me = async (): Promise<string> => (await ownRoutes.auth.me()).accountId;
export const kindOf = (event: NodeEvent): string => event.kind;
export const ownPolicy: NodePolicy = policy;

// The generated methods: one typed argument object, the typed payload.
export const orgName = async (org: string): Promise<string> => (await client.orgs.get({ org })).name;
export const inviteToken = async (org: string): Promise<string> =>
  (await client.orgs.inviteCreate({ org, role: 'member' }, { confirm: passkeyConfirm(passkey) })).token;
export const called = (app: string) => client.apps.call({ app, method: 'set', args: { k: 'v' } });

// @ts-expect-error a path parameter is required
export const noOrg = () => client.orgs.get({});

// @ts-expect-error not an argument of the route
export const extraArg = () => client.orgs.get({ org: 'o', bogus: 1 });

// @ts-expect-error an invite's role is one the node knows
export const badRole = () => client.orgs.inviteCreate({ org: 'o', role: 'emperor' });

// The consent page's organization kinds: requests and outcomes typed by kind.
import { createOrgConsent } from '@calimero-network/mero-js';
import type { ConsentResult } from '@calimero-network/mero-js';

export const newOrg = () => client.openConsentPage(createOrgConsent('Acme', 'https://kv.apps.example/'));
export const orgOf = (r: ConsentResult): string | undefined => (r.kind === 'createOrg' ? r.outcome?.orgId : undefined);
export const resumed = (): Promise<ConsentResult | null> => client.resumeConsent();
// @ts-expect-error an invite's role is guest, member or admin
export const badInvite = () => client.openConsentPage({ kind: 'createInvite', subject: { org: 'o', role: 'owner' }, returnTo: 'https://x/' });

// Each consent helper builds the request kind its name says, and that request is
// what the node's `POST /consent/requests` route takes, as apps send it. A helper
// typed as the whole `ConsentRequest` union would carry kinds the generated route
// body may not list yet.
export const openUpgrade = (c: CalimeroClient, appId: string, version: string, returnTo: string) =>
  c.consent.pageCreate({ body: upgradeConsent(appId, version, returnTo) });
export const openRelease = (c: CalimeroClient, version: string, returnTo: string) =>
  c.consent.pageCreate({ body: releaseConsent({ kind: 'account' }, version, returnTo) });
export const openWithdraw = (c: CalimeroClient, version: string, returnTo: string) =>
  c.consent.pageCreate({ body: withdrawConsent({ kind: 'account' }, version, returnTo) });

// The organization management kinds (core#5018): each builder's request is what
// the generated route body takes, and outcomes are typed by kind.
import {
  deleteOrgConsent,
  leaveOrgConsent,
  removeMemberConsent,
  removeMemberTeamsConsent,
  renameOrgConsent,
  revokeInviteConsent,
  setAllAppsConsent,
  setCapabilitiesConsent,
  setMemberRoleConsent,
  setMemberTeamsConsent,
} from '@calimero-network/mero-js';

const back = 'https://kv.apps.example/';
export const orgKinds = (c: CalimeroClient) => [
  c.consent.pageCreate({ body: leaveOrgConsent('o', back) }),
  c.consent.pageCreate({ body: removeMemberConsent('o', 'a', back) }),
  c.consent.pageCreate({ body: setMemberRoleConsent('o', 'a', 'owner', back) }),
  c.consent.pageCreate({ body: setMemberTeamsConsent('o', 'a', [{ team: 't', role: 'lead', capabilities: 1 }], back) }),
  c.consent.pageCreate({ body: removeMemberTeamsConsent('o', 'a', ['t'], back) }),
  c.consent.pageCreate({ body: revokeInviteConsent('o', 'k', back) }),
  c.consent.pageCreate({ body: renameOrgConsent('o', 'n', back) }),
  c.consent.pageCreate({ body: deleteOrgConsent('o', back) }),
  c.consent.pageCreate({ body: setCapabilitiesConsent({ org: 'o', team: 't', capabilities: 3 }, back) }),
  c.consent.pageCreate({ body: setAllAppsConsent('o', 'a', true, back) }),
];
export const renamedTo = (r: ConsentResult): string | undefined => (r.kind === 'renameOrg' ? r.outcome?.name : undefined);
export const deletedApps = (r: ConsentResult): string[] | undefined => (r.kind === 'deleteOrg' ? r.outcome?.apps : undefined);
// @ts-expect-error removeMember makes nothing: its outcome has no orgId
export const noOutcome = (r: ConsentResult) => (r.kind === 'removeMember' ? r.outcome?.orgId : undefined);
// @ts-expect-error a member's organization role is guest, member, admin or owner
export const badOrgRole = () => setMemberRoleConsent('o', 'a', 'lead', back);
// @ts-expect-error a team role is lead, member or readOnly
export const badTeamRole = () => setMemberTeamsConsent('o', 'a', [{ team: 't', role: 'admin' }], back);
// @ts-expect-error capabilities are required
export const noCaps = () => setCapabilitiesConsent({ org: 'o', team: 't' }, back);
