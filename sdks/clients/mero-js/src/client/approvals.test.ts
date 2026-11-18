/**
 * Approvals as core's node reports them (`GET /auth/apps`, `/orgs/{org}/apps`,
 * `/releases`), the consent page an app sends its user to, publisher binding
 * and the operator's report and deadline routes, through the real sealed
 * channel to a stand-in node. Synthetic data only.
 */

import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';

import { StandInNode } from '../../tests/support/stand-in-node.js';
import {
  CalimeroClient,
  CalimeroError,
  ConsentStale,
  ORG_CAPABILITIES,
  ReleaseDeadlineError,
  RuntimeTooOld,
  confirmationCode,
  consentReturn,
  pageQuery,
  pendingApprovalsFrom,
  publisherBindingBytes,
  publisherBindingMessage,
  releaseConsent,
  stalledBy,
  upgradeConsent,
  verifyNodeUrl,
  verifyReleaseUrl,
  withdrawConsent,
  type AppsReport,
  type ReleaseRules,
} from './index.js';
import type { PasskeyProvider } from '../runtime/passkey.js';
import { passkeyConfirm } from '../runtime/passkey.js';

const node = new StandInNode();
beforeAll(() => node.start());
afterAll(() => node.stop());
beforeEach(() => {
  node.requests.length = 0;
  node.answer = () => ({ status: 200, json: {} });
});

const client = () => new CalimeroClient({ baseUrl: node.url, policy: { allowMock: true } });
const sent = () => node.requests.map((r) => [r.head.method, r.head.path]);
const last = () => node.requests[node.requests.length - 1];
const passkey: PasskeyProvider = {
  create: async () => ({ attestationObject: 'ao', clientDataJson: 'cd' }),
  get: async () => ({ credentialId: 'cred', authenticatorData: 'ad', clientDataJson: 'cd', signature: 'sig' }),
};
const CHALLENGE = { challengeId: 'c-1', challenge: 'AAEC', rpId: 'localhost' };
const CONFIRM = { challengeId: 'c-1', assertion: expect.objectContaining({ signature: 'sig' }) };
const OWNERS = { releases: 'owner', appUpgrades: 'owner', outbound: 'owner' } as const;

const app = (id: string, version: string, availableVersions: string[]) => ({
  app: id,
  group: 2,
  scope: { kind: 'account' as const },
  bundle: `h-${id}`,
  package: { package: 'com.example.kv', version, service: null },
  publisher: { signer: 'did:key:z6Mk', org: null },
  upgradesHeldBy: 'owner' as const,
  releasesApprovedBy: 'owner' as const,
  lastUpgrade: null,
  lastWriteAt: null,
  approvedReleases: ['2.3.109'],
  replicas: [{ node: 1, release: '2.3.109', approved: true }],
  name: `app ${id}`,
  availableVersions,
});

const REPORT: AppsReport = {
  scope: { kind: 'account' },
  governance: OWNERS,
  approvedReleases: ['2.3.109'],
  clusterReleases: [
    { release: '2.3.109', nodes: 2, approved: true },
    { release: '2.3.120', nodes: 1, approved: false },
    { release: '2.3.130', nodes: 1, approved: false },
  ],
  apps: [app('a-1', '1.0.0', ['1.0.1', '1.1.0']), app('a-2', '2.0.0', [])],
  next: null,
};
const at = (iso: string) => Date.parse(iso) / 1000;
const RULES: ReleaseRules = {
  governance: OWNERS,
  deadlines: [{ version: '2.3.120', after: '2027-03-12T00:00:00Z', afterSecs: at('2027-03-12T00:00:00Z') }],
};

describe('pendingApprovalsFrom', () => {
  it('lists unapproved cluster releases, newest first, and apps with newer releases', () => {
    const pending = pendingApprovalsFrom(REPORT, RULES, at('2027-01-01T00:00:00Z'));
    expect(pending.releases).toEqual([
      {
        version: '2.3.130',
        nodes: 1,
        actionable: true,
        releaseNotesUrl: 'https://github.com/calimero-network/mero-tee/releases/tag/mero-tee-v2.3.130',
        verifyUrl: 'https://mero-tee.vercel.app/mero-tee?release_tag=mero-tee-v2.3.130',
        deadline: null,
      },
      expect.objectContaining({ version: '2.3.120', deadline: RULES.deadlines[0] }),
    ]);
    expect(pending.upgrades).toEqual([
      {
        app: 'a-1',
        name: 'app a-1',
        package: 'com.example.kv',
        from: '1.0.0',
        to: '1.1.0',
        reachable: true,
        blocked: null,
        available: ['1.0.1', '1.1.0'],
        actionable: true,
      },
    ]);
    expect(pending.stalledBy).toBeNull();
    expect(pending.actionable).toBe(true);
  });

  it('names the release a passed deadline stops new work on, until it is approved', () => {
    const after = at('2027-04-01T00:00:00Z');
    expect(pendingApprovalsFrom(REPORT, RULES, after).stalledBy).toEqual(RULES.deadlines[0]);
    expect(stalledBy(['2.3.109', '2.3.120'], RULES.deadlines, after)).toBeNull();
    expect(stalledBy(['2.3.130'], RULES.deadlines, after)).toBeNull();
    // Unrestricted (no approvals ever recorded) is never stopped.
    expect(stalledBy(null, RULES.deadlines, after)).toBeNull();
  });

  it('offers the newest release an app can reach, and says why the latest is not it', () => {
    const blocked = {
      version: '1.1.0',
      reason: 'runtime' as const,
      message: 'com.example.kv@1.1.0 needs runtime 1.0.0 or newer, and node 4 runs 0.9.0',
      minRuntimeVersion: '1.0.0',
      nodes: [{ node: 4, runtime: '0.9.0' }],
    };
    const behind = { ...app('a-1', '1.0.0', ['1.0.1', '1.1.0']), latestReachable: '1.0.1', latestBlocked: blocked };
    const stuck = { ...app('a-2', '1.0.0', ['1.1.0']), latestReachable: null, latestBlocked: { ...blocked } };
    const pending = pendingApprovalsFrom({ ...REPORT, apps: [behind, stuck] }, RULES, at('2027-01-01T00:00:00Z'));
    expect(pending.upgrades).toEqual([
      expect.objectContaining({ app: 'a-1', to: '1.0.1', reachable: true, blocked }),
      expect.objectContaining({ app: 'a-2', to: '1.1.0', reachable: false, blocked }),
    ]);
  });

  it('marks nothing actionable where the operator holds the decisions', () => {
    const operator = { releases: 'operator', appUpgrades: 'operator', outbound: 'owner' } as const;
    const pending = pendingApprovalsFrom({ ...REPORT, governance: operator }, RULES);
    expect(pending.releases.every((r) => !r.actionable)).toBe(true);
    expect(pending.upgrades.every((u) => !u.actionable)).toBe(true);
    expect(pending.actionable).toBe(false);
  });
});

describe('helpers', () => {
  it('builds the publisher binding message core verifies', () => {
    const msg = publisherBindingMessage('prod-eu', 'org-1', 'did:key:z6Mk', 0);
    expect(msg).toBe('calimero-publisher-binding/v1\nprod-eu\norg-1\ndid:key:z6Mk\n0');
    expect(new TextDecoder().decode(publisherBindingBytes('prod-eu', 'org-1', 'did:key:z6Mk', 0))).toBe(msg);
  });

  it('reads the consent page redirect, and nothing else', () => {
    expect(consentReturn('https://kv.apps.example/settings?request_id=3.abc&result=approved')).toEqual({
      requestId: '3.abc',
      result: 'approved',
    });
    expect(consentReturn('https://kv.apps.example/settings?request_id=3.abc&result=maybe')).toBeNull();
    expect(consentReturn('https://kv.apps.example/settings')).toBeNull();
  });

  it("computes the consent page's confirmation code as core does", async () => {
    expect(await confirmationCode('1.AAAAAAAAAAAAAAAAAAAAAA')).toBe('R02-9P2');
    expect(await confirmationCode('2.q7Xb3kR0fLzM8nW1yVt5cA')).toBe('54E-429');
  });

  it('pages reports and refuses a limit core would', () => {
    expect(pageQuery()).toBe('');
    expect(pageQuery({ after: 'a-1', limit: 50 })).toBe('?after=a-1&limit=50');
    expect(() => pageQuery({ limit: 101 })).toThrow(RangeError);
    expect(() => pageQuery({ limit: 0 })).toThrow(RangeError);
  });

  it('has the APPROVE_RELEASES capability on bit 10', () => {
    expect(ORG_CAPABILITIES.APPROVE_RELEASES).toBe(1 << 10);
  });
});

describe('over the sealed channel', () => {
  it('pendingApprovals reads every page of the report and the release rules', async () => {
    node.answer = (head) => {
      if (head.path === '/auth/apps') return { status: 200, json: { ...REPORT, apps: [REPORT.apps[0]], next: 'a-1' } };
      if (head.path === '/auth/apps?after=a-1') return { status: 200, json: { ...REPORT, apps: [REPORT.apps[1]], next: null } };
      if (head.path === '/releases') return { status: 200, json: RULES };
      return { status: 404, json: { error: 'NOT_FOUND', message: head.path } };
    };
    const pending = await client().pendingApprovals();
    expect(sent()).toEqual(
      expect.arrayContaining([
        ['GET', '/auth/apps'],
        ['GET', '/auth/apps?after=a-1'],
        ['GET', '/releases'],
      ]),
    );
    expect(pending.upgrades.map((u) => u.app)).toEqual(['a-1']);
    expect(pending.releases.map((r) => r.version)).toEqual(['2.3.130', '2.3.120']);
  });

  it("reads an organization's report and an organization's release fields", async () => {
    const c = client();
    node.answer = () => ({ status: 200, json: { ...REPORT, scope: { kind: 'org', org: 'org-1' } } });
    await c.orgs.apps({ org: 'org-1', limit: 10 });
    expect(last().head.path).toBe('/orgs/org-1/apps?limit=10');
    node.answer = () => ({
      status: 200,
      json: { orgId: 'org-1', releaseApprovers: ['acc-1'], publishes: ['did:key:z6Mk'], newWorkStopped: null },
    });
    const org = await c.orgs.get({ org: 'org-1' });
    expect(org.releaseApprovers).toEqual(['acc-1']);
    expect(org.publishes).toEqual(['did:key:z6Mk']);
    expect(org.newWorkStopped).toBeNull();
  });

  it('asks the node for a consent page and reads the decision', async () => {
    const c = client();
    const started = { requestId: '1.xyz', url: 'https://node-1.example/consent/1.xyz', expiresAt: 9e9 };
    node.answer = () => ({ status: 200, json: started });
    const opened: string[] = [];
    const request = upgradeConsent('a-1', '1.1.0', 'https://kv.apps.example/settings');
    expect(await c.openConsentPage(request, (url) => opened.push(url))).toEqual(started);
    expect(opened).toEqual([started.url]);
    expect(last().head.path).toBe('/consent/requests');
    expect(last().body).toEqual({ kind: 'upgrade', subject: { app: 'a-1', toVersion: '1.1.0' }, returnTo: 'https://kv.apps.example/settings' });

    await c.consent.pageCreate({ body: releaseConsent({ kind: 'org', org: 'org-1' }, '2.3.130', 'https://kv.apps.example/') });
    expect(last().body).toEqual({
      kind: 'release',
      scope: { org: 'org-1' },
      subject: { version: '2.3.130' },
      returnTo: 'https://kv.apps.example/',
    });
    await c.consent.pageCreate({ body: releaseConsent({ kind: 'account' }, '2.3.130', 'https://kv.apps.example/') });
    expect(last().body).toMatchObject({ scope: 'account' });

    // Withdrawing goes through the page too; an approval sends no `withdraw` at all.
    expect(node.requests[node.requests.length - 2].body).toEqual(
      expect.objectContaining({ subject: { version: '2.3.130' } }),
    );
    await c.consent.pageCreate({ body: withdrawConsent({ kind: 'org', org: 'org-1' }, '2.3.109', 'https://kv.apps.example/') });
    expect(last().body).toEqual({
      kind: 'release',
      scope: { org: 'org-1' },
      subject: { version: '2.3.109', withdraw: true },
      returnTo: 'https://kv.apps.example/',
    });
    expect(releaseConsent({ kind: 'account' }, '2.3.109', 'https://x/', { withdraw: true })).toEqual(
      withdrawConsent({ kind: 'account' }, '2.3.109', 'https://x/'),
    );

    let polls = 0;
    node.answer = () => {
      polls += 1;
      return {
        status: 200,
        json: { requestId: '1.xyz', kind: 'upgrade', status: polls < 2 ? 'pending' : 'approved', decidedAt: polls < 2 ? null : 5 },
      };
    };
    const result = await c.waitForConsent('1.xyz', { intervalMs: 1 });
    expect(result).toEqual({ requestId: '1.xyz', kind: 'upgrade', status: 'approved', decidedAt: 5 });
    expect(last().head.path).toBe('/consent/1.xyz/result');
  });

  it("shows the page's code before navigating, and refuses a code that is not the request's", async () => {
    const c = client();
    const request = upgradeConsent('a-1', '1.1.0', 'https://kv.apps.example/settings');
    const started = {
      requestId: '1.AAAAAAAAAAAAAAAAAAAAAA',
      url: 'https://node-1.example/consent/1.AAAAAAAAAAAAAAAAAAAAAA',
      expiresAt: 9e9,
      code: 'R02-9P2',
    };
    node.answer = () => ({ status: 200, json: started });

    // `confirm` sees the answer, code included, before the page opens.
    const opened: string[] = [];
    const shown: string[] = [];
    const confirm = async (s: { code?: string; url: string }) => {
      shown.push(s.code ?? '');
      expect(opened).toEqual([]);
      return true;
    };
    expect(await c.openConsentPage(request, { navigate: (url) => opened.push(url), confirm })).toEqual(started);
    expect(shown).toEqual(['R02-9P2']);
    expect(opened).toEqual([started.url]);

    // Declined: answered, not opened.
    opened.length = 0;
    expect(await c.openConsentPage(request, { navigate: (url) => opened.push(url), confirm: () => false })).toEqual(started);
    expect(opened).toEqual([]);

    // A code that is not the request's: refused, not opened, `confirm` not asked.
    node.answer = () => ({ status: 200, json: { ...started, code: '54E-429' } });
    let asked = false;
    const refused = await c
      .openConsentPage(request, {
        navigate: (url) => opened.push(url),
        confirm: () => (asked = true),
      })
      .catch((e: unknown) => e);
    expect(refused).toBeInstanceOf(CalimeroError);
    expect(refused).toMatchObject({
      code: 'CONSENT_CODE_MISMATCH',
      details: { requestId: started.requestId, code: '54E-429', expected: 'R02-9P2' },
    });
    expect(asked).toBe(false);
    expect(opened).toEqual([]);

    // An older node sends no code: `confirm` still runs, and the page opens.
    const { code: _code, ...older } = started;
    node.answer = () => ({ status: 200, json: older });
    shown.length = 0;
    expect(await c.openConsentPage(request, { navigate: (url) => opened.push(url), confirm })).toEqual(older);
    expect(shown).toEqual(['']);
    expect(opened).toEqual([started.url]);
  });

  it('types the deadline and stale-consent refusals', async () => {
    const c = client();
    node.answer = () => ({
      status: 409,
      json: {
        error: 'RELEASE_DEADLINE',
        message: 'release deadline: approve mero-tee release 2.3.120',
        details: { release: '2.3.120', after: '2027-03-12T00:00:00Z', afterSecs: 1804809600 },
      },
    });
    const refused = await c.apps.upgrade({ app: 'a-1', version: '1.1.0' }).catch((e: unknown) => e);
    expect(refused).toBeInstanceOf(ReleaseDeadlineError);
    expect(refused).toMatchObject({ status: 409, release: '2.3.120', after: '2027-03-12T00:00:00Z' });

    node.answer = () => ({
      status: 409,
      json: {
        error: 'RUNTIME_TOO_OLD',
        message: 'com.example.kv@1.1.0 needs runtime 1.0.0 or newer, and node 4 runs 0.9.0',
        details: { minRuntimeVersion: '1.0.0', nodes: [{ node: 4, runtime: '0.9.0' }] },
      },
    });
    const tooOld = await c.apps.upgrade({ app: 'a-1', version: '1.1.0' }).catch((e: unknown) => e);
    expect(tooOld).toBeInstanceOf(RuntimeTooOld);
    expect(tooOld).toMatchObject({ status: 409, minRuntimeVersion: '1.0.0', nodes: [{ node: 4, runtime: '0.9.0' }] });

    node.answer = () => ({ status: 409, json: { error: 'CONSENT_STALE', message: 'the target changed' } });
    expect(await c.consent.pageCreate({ body: upgradeConsent('a-1', '1.1.0', 'https://kv.apps.example/') }).catch((e: unknown) => e)).toBeInstanceOf(
      ConsentStale,
    );
  });

  it('binds a publisher key and upgrades its published instances with a passkey', async () => {
    const c = client();
    node.answer = () => ({ status: 200, json: { signerId: 'did:key:z6Mk', cluster: 'prod-eu', generation: 2, bound: false } });
    const info = await c.public.publisherBinding({ signer: 'did:key:z6Mk' });
    expect(last().head.path).toBe('/publishers/did%3Akey%3Az6Mk');
    expect(info.generation).toBe(2);

    const confirming = (json: unknown) => {
      node.answer = (_, body) =>
        body.confirm
          ? { status: 200, json }
          : { status: 412, json: { error: 'CONFIRM_REQUIRED', message: 'approve', details: CHALLENGE } };
    };
    // core 92ac297a1: the binding answers the generation it was bound at, not `{ signer, approval }`.
    confirming({ generation: 2 });
    const bound = await c.orgs.publisher({ org: 'org-1', signerId: 'did:key:z6Mk', publicKey: 'pk', signature: 'sig' }, { confirm: passkeyConfirm(passkey) });
    expect(bound.generation).toBe(2);
    expect(last().head.path).toBe('/orgs/org-1/publishers');
    expect(last().body).toEqual({ signerId: 'did:key:z6Mk', publicKey: 'pk', signature: 'sig', confirm: CONFIRM });

    confirming({ runId: 'r-1' });
    await c.orgs.publisherUpgrade({ org: 'org-1', package: 'com.example.kv', version: '1.1.0' }, { confirm: passkeyConfirm(passkey) });
    expect(last().head.path).toBe('/orgs/org-1/packages/com.example.kv/upgrade');
    expect(last().body).toEqual({ version: '1.1.0', confirm: CONFIRM });
  });

  it("calls the operator's reports and deadline routes", async () => {
    const c = client();
    c.useApiKey('ops-token');
    node.answer = () => ({ status: 200, json: { fresh: true, governance: OWNERS, apps: [], orgs: [], next: null } });
    await c.ops.apps({ limit: 100 });
    await c.ops.orgs({ after: 'org-1' });
    await c.ops.releaseDeadline({ version: '2.3.120', after: new Date('2027-03-12T00:00:00Z').toISOString() });
    expect(last().body).toEqual({ after: '2027-03-12T00:00:00.000Z' });
    await c.ops.releaseDeadlineLift({ version: '2.3.120' });
    expect(sent()).toEqual([
      ['GET', '/ops/apps?limit=100'],
      ['GET', '/ops/orgs?after=org-1'],
      ['POST', '/ops/releases/2.3.120/deadline'],
      ['DELETE', '/ops/releases/2.3.120/deadline'],
    ]);
    expect(last().head.headers).toContainEqual(['authorization', 'Bearer ops-token']);
  });
});

describe('the public verifier links', () => {
  it('names the release for a release', () => {
    expect(verifyReleaseUrl('2.3.130')).toBe('https://mero-tee.vercel.app/mero-tee?release_tag=mero-tee-v2.3.130');
  });

  it('names the node and its release for a node, the link mdma builds', () => {
    expect(verifyNodeUrl('https://node-1.eu.example/', '2.3.130')).toBe(
      'https://mero-tee.vercel.app/mero-tee?node_url=https%3A%2F%2Fnode-1.eu.example&release_tag=mero-tee-v2.3.130',
    );
  });

  it('encodes a node URL that carries a path and a query', () => {
    const link = new URL(verifyNodeUrl('https://h.example/p?a=1&b=2', '1.0.0'));
    expect(link.searchParams.get('node_url')).toBe('https://h.example/p?a=1&b=2');
  });
});
