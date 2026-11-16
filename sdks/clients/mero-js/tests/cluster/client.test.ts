import { readFileSync } from 'node:fs';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import {
  BadOrigin,
  CalimeroClient,
  CalimeroError,
  ConfirmRequired,
  LoginRequired,
  NotFound,
  OriginNotRegistered,
} from '../../src/client/index.js';
import { KV_STORE_WASM, startCluster, type LocalCluster } from './cluster.js';
import { publisher, signedMpk, variant } from './mpk.js';
import { SoftPasskey } from './soft-passkey.js';
import { passkeyConfirm } from '../../src/runtime/passkey.js';
import { presenceState } from '../../src/client/calimero-client.js';

const ORIGIN = 'http://localhost:3000';
let cluster: LocalCluster;

const client = (i: number, allowMock = true) => new CalimeroClient({ baseUrl: cluster.urls[i], policy: { allowMock } });

async function person(i: number, name: string): Promise<[CalimeroClient, SoftPasskey]> {
  const c = client(i);
  const pk = new SoftPasskey('localhost', ORIGIN);
  await c.signUp(pk, name);
  return [c, pk];
}

beforeAll(async () => {
  cluster = await startCluster(3);
});

afterAll(() => cluster?.stop());

describe('Calimero 1.0 cluster', () => {
  it('refuses a mock quote unless allowed', async () => {
    await expect(client(0, false).auth.loginBegin()).rejects.toThrow(/mock quote refused/);
  });

  it('signs up, deploys kv-store and calls it through the sealed channel', async () => {
    const [alice, pk] = await person(0, 'alice');
    const me = await alice.auth.me();
    expect(me.via).toBe('session');

    // The same passkey signs in on another node.
    const again = client(1);
    const s = await again.signIn(pk);
    expect(s.accountId).toBe(me.accountId);

    const bundle = await (await alice.packages.bundlePut({ body: new Uint8Array(readFileSync(KV_STORE_WASM)), contentType: 'application/wasm' })).hash;
    const app = await (await alice.apps.create({ name: 'kv', bundle })).appId;

    const r = await alice.apps.call({ app, method: 'set', args: { key: 'a', value: '1' }, idempotencyKey: 'k1' });
    expect(r.index).toBeGreaterThan(0);
    expect(r.events).toHaveLength(1);
    const replay = await alice.apps.call({ app, method: 'set', args: { key: 'a', value: 'other' }, idempotencyKey: 'k1' });
    expect(replay.index).toBe(r.index);

    expect((await again.apps.view({ app, method: 'get', args: { key: 'a' } })).result).toBe('1');
    expect((await again.apps.view({ app, method: 'get', args: { key: 'a' }, consistency: { stale: true, minIndex: r.index }.stale ? 'stale' : undefined, minIndex: { stale: true, minIndex: r.index }.minIndex })).result).toBe('1');

    // Events replay from the start.
    const ac = new AbortController();
    const first = await alice.events(app, 0, ac.signal).next();
    ac.abort();
    expect(first.value?.kind).toBeTruthy();

    // A confirm method needs a fresh passkey assertion over the exact call.
    await expect(alice.apps.call({ app, method: 'clear', args: {} })).rejects.toMatchObject({ code: 'CONFIRM_REQUIRED' });
    await alice.apps.call({ app, method: 'clear', args: {} }, { confirm: passkeyConfirm(pk) });
    expect((await alice.apps.view({ app, method: 'len', args: {} })).result).toBe(0);

    // Others cannot reach an owner-only app.
    const [bob] = await person(2, 'bob');
    await expect(bob.apps.call({ app, method: 'set', args: { key: 'x', value: 'y' } })).rejects.toMatchObject({ code: 'NOT_FOUND' });

    // A read-only API key reads, never writes.
    const { key } = await alice.auth.apikeyCreate({ readOnly: true, apps: [app] }, { confirm: passkeyConfirm(pk) });
    const agent = client(2);
    agent.useApiKey(key);
    await agent.apps.view({ app, method: 'len', args: {} });
    const denied = await agent.apps.call({ app, method: 'set', args: { key: 'x', value: 'y' } }).catch((e) => e);
    expect(denied).toBeInstanceOf(CalimeroError);
    expect((denied as CalimeroError).code).toBe('FORBIDDEN');

    // Refresh rotates the session.
    const fresh = await alice.refresh();
    expect(fresh.sessionToken).not.toBe(s.sessionToken);
  });

  it('organizations gate apps and removal cuts access off', async () => {
    const [alice, alicePk] = await person(0, 'alice-org');
    const [bob] = await person(1, 'bob-org');
    const bobId = (await bob.auth.me()).accountId as string;
    const org = await (await alice.orgs.create({ name: 'Acme' })).orgId;
    const team = await (await alice.orgs.teamCreate({ org, name: 'eng', join: 'invite' })).teamId;
    const token = await (await alice.orgs.inviteCreate({ org, role: 'member', teams: [team] }, { confirm: passkeyConfirm(alicePk) })).token;
    await bob.orgs.inviteRedeem({ token });

    const bundle = await (await alice.packages.bundlePut({ body: new Uint8Array(readFileSync(KV_STORE_WASM)), contentType: 'application/wasm' })).hash;
    const app = await (await alice.apps.create({ name: 'team-kv', bundle, org })).appId;
    await bob.apps.call({ app, method: 'set', args: { key: 'from', value: 'bob' } });

    // Removing a member is approved with a passkey; without one the node asks for it.
    await expect(alice.orgs.removeMember({ org, account: bobId })).rejects.toMatchObject({ code: 'CONFIRM_REQUIRED' });
    await alice.orgs.removeMember({ org, account: bobId }, { confirm: passkeyConfirm(alicePk) });
    await new Promise((r) => setTimeout(r, 300));
    await expect(bob.apps.call({ app, method: 'set', args: { key: 'x', value: 'y' } })).rejects.toMatchObject({ code: 'NOT_FOUND' });
  });

  it('stores an app\'s blobs and serves them only to that app', async () => {
    const [alice] = await person(0, 'alice-blobs');
    const bundle = await (await alice.packages.bundlePut({ body: new Uint8Array(readFileSync(KV_STORE_WASM)), contentType: 'application/wasm' })).hash;
    const app = await (await alice.apps.create({ name: 'files', bundle })).appId;
    const other = await (await alice.apps.create({ name: 'other', bundle })).appId;
    const data = new TextEncoder().encode('%PDF-1.7 '.repeat(500));
    const up = await alice.apps.blobUpload({ app, body: data });
    expect(up.size).toBe(data.length);
    expect(await alice.downloadBlob(app, up.blobId)).toEqual(data);
    expect(await alice.downloadBlob(app, up.id)).toEqual(data);
    await expect(alice.downloadBlob(other, up.blobId)).rejects.toMatchObject({ code: 'NOT_FOUND' });
  });

  it('installs signed packages, refuses unsigned ones, upgrades by version, uninstalls and deletes', async () => {
    const [alice, alicePk] = await person(0, 'alice-packages');
    const kv = new Uint8Array(readFileSync(KV_STORE_WASM));
    const key = publisher();

    // An archive with no signature is refused.
    await expect(alice.packages.upload({ body: new Uint8Array([1, 2, 3]), contentType: 'application/octet-stream' })).rejects.toMatchObject({ code: 'BAD_PACKAGE' });

    // Its own bytes: other tests' apps run kv-store uploaded raw, and a bundle is
    // its hash, so 1.0.0 would count as running there and could not be removed.
    const installed = await alice.packages.upload({ body: signedMpk('com.mero-js.kv', '1.0.0', variant(kv, 'v100'), key), contentType: 'application/octet-stream' });
    expect(installed).toMatchObject({ package: 'com.mero-js.kv', version: '1.0.0', signerId: key.signerId });
    // Another key cannot release the same package.
    await expect(alice.packages.upload({ body: signedMpk('com.mero-js.kv', '1.1.0', kv, publisher()), contentType: 'application/octet-stream' })).rejects.toMatchObject({
      status: 403,
    });

    const app = await (await alice.apps.create({ name: 'pkg-kv', package: 'com.mero-js.kv', version: '1.0.0' })).appId;
    await alice.apps.call({ app, method: 'set', args: { key: 'a', value: '1' } });
    expect((await (await alice.apps.list({ package: 'com.mero-js.kv' })).apps)[0]).toMatchObject({ appId: app, version: '1.0.0' });

    await alice.packages.upload({ body: signedMpk('com.mero-js.kv', '1.0.1', variant(kv, 'v101'), key), contentType: 'application/octet-stream' });
    // The owner approves an upgrade with their passkey.
    await expect(alice.apps.upgrade({ app, version: '1.0.1' })).rejects.toMatchObject({ code: 'CONFIRM_REQUIRED' });
    const up = await alice.apps.upgrade({ app, version: '1.0.1' }, { confirm: passkeyConfirm(alicePk) });
    expect(up.status).toBe('ok');
    expect((await alice.apps.view({ app, method: 'get', args: { key: 'a' } })).result).toBe('1');
    const info = await alice.packages.get({ package: 'com.mero-js.kv' });
    expect(info.latest).toBe('1.0.1');
    expect(info.versions.map((v) => v.version)).toEqual(['1.0.0', '1.0.1']);

    await expect(alice.packages.versionUninstall({ package: 'com.mero-js.kv', version: '1.0.1' })).rejects.toThrow(/still runs/);
    await alice.packages.versionUninstall({ package: 'com.mero-js.kv', version: '1.0.0' });
    await alice.apps.delete({ app });
    await expect(alice.apps.get({ app })).rejects.toMatchObject({ status: 404 });
    await alice.packages.uninstall({ package: 'com.mero-js.kv' });
    expect((await alice.packages.list()).packages.map((p) => p.package)).not.toContain('com.mero-js.kv');
  });

  it('covers the rest of the API: passkeys, API keys, access, orgs and teams', async () => {
    const [alice, pk] = await person(0, 'alice-api');
    const [bob] = await person(1, 'bob-api');
    const bobId = (await bob.auth.me()).accountId as string;

    // A second passkey is another device, approved by the one the account has; it can be removed again.
    const second = new SoftPasskey('localhost', ORIGIN);
    // The new passkey cannot approve its own addition.
    await expect(alice.addPasskey(second, second)).rejects.toBeInstanceOf(CalimeroError);
    const cred = await alice.addPasskey(second, pk);
    const viaSecond = client(2);
    await viaSecond.signIn(second);
    expect((await viaSecond.auth.me()).accountId).toBe((await alice.auth.me()).accountId);
    await expect(alice.auth.passkeyRemove({ cred })).rejects.toMatchObject({ code: 'CONFIRM_REQUIRED' });
    await alice.auth.passkeyRemove({ cred }, { confirm: passkeyConfirm(pk) });
    await expect(client(2).signIn(second)).rejects.toBeInstanceOf(CalimeroError);

    await expect(alice.auth.apikeyCreate({ label: 'ci' })).rejects.toMatchObject({ code: 'CONFIRM_REQUIRED' });
    const { keyId } = await alice.auth.apikeyCreate({ label: 'ci' }, { confirm: passkeyConfirm(pk) });
    expect((await (await alice.auth.apikeyList()).keys).map((k) => k.keyId)).toContain(keyId);
    // A node from before answers the revocation at once; this one asks for the passkey (412) first.
    await alice.auth.apikeyRevoke({ key: keyId }, { confirm: passkeyConfirm(pk) });
    expect((await (await alice.auth.apikeyList()).keys).map((k) => k.keyId)).not.toContain(keyId);

    const bundle = await (await alice.packages.bundlePut({ body: new Uint8Array(readFileSync(KV_STORE_WASM)), contentType: 'application/wasm' })).hash;
    const app = await (await alice.apps.create({ name: 'shared', bundle })).appId;
    await expect(bob.apps.call({ app, method: 'set', args: { key: 'x', value: 'y' } })).rejects.toMatchObject({ code: 'NOT_FOUND' });
    await alice.apps.access({ app, access: { kind: 'accounts', accounts: [bobId] } });
    await new Promise((r) => setTimeout(r, 300));
    await bob.apps.call({ app, method: 'set', args: { key: 'x', value: 'y' } });

    const org = await (await alice.orgs.create({ name: 'Initech' })).orgId;
    await alice.orgs.setMember({ org, account: bobId, role: 'member' }, { confirm: passkeyConfirm(pk) });
    const team = await (await alice.orgs.teamCreate({ org, name: 'ops', join: 'invite' })).teamId;
    await alice.orgs.teamSetMember({ org, team, account: bobId, role: 'readOnly' }, { confirm: passkeyConfirm(pk) });
    expect(await alice.orgs.teamGet({ org, team })).toMatchObject({ teamId: team });
    await alice.orgs.teamUpdate({ org, team, name: 'platform', metadata: { colour: 'blue' } });
    expect(await alice.orgs.teamGet({ org, team })).toMatchObject({ name: 'platform' });
    await alice.orgs.capabilities({ org, capabilities: 0, account: bobId }, { confirm: passkeyConfirm(pk) });
    await alice.orgs.teamRemoveMember({ org, team, account: bobId }, { confirm: passkeyConfirm(pk) });
    // `ops` holds no apps: it goes without a passkey.
    await alice.orgs.teamDelete({ org, team });
    await expect(alice.orgs.teamGet({ org, team })).rejects.toMatchObject({ status: 404 });
    // A team whose sub-team holds an app goes with alice's passkey, and the node's ask names the app
    // (core#4891). A node from before deletes it at once, so the 412 is checked only where it is asked.
    const eng = await (await alice.orgs.teamCreate({ org, name: 'eng', join: 'invite' })).teamId;
    const backend = await (await alice.orgs.teamCreate({ org, name: 'backend', join: 'invite', parent: eng })).teamId;
    const teamApp = await (await alice.apps.create({ name: 'eng-kv', bundle, org, access: { kind: 'team', team: backend } })).appId;
    const asked = await alice.orgs.teamDelete({ org, team: eng }).then(() => undefined, (e: unknown) => e);
    if (asked !== undefined) {
      expect(asked).toMatchObject({ status: 412, code: 'CONFIRM_REQUIRED', details: { apps: [teamApp] } });
      expect(await alice.apps.get({ app: teamApp })).toBeTruthy();
      await alice.orgs.teamDelete({ org, team: eng }, { confirm: passkeyConfirm(pk) });
    }
    await new Promise((r) => setTimeout(r, 300));
    await expect(alice.orgs.teamGet({ org, team: eng })).rejects.toMatchObject({ status: 404 });
    await expect(alice.apps.get({ app: teamApp })).rejects.toMatchObject({ status: 404 });
    const token = await (await alice.orgs.inviteCreate({ org, role: 'guest', teams: [], ttlSecs: 60, uses: 1 }, { confirm: passkeyConfirm(pk) })).token;
    expect(token).toBeTruthy();
    expect(await alice.orgs.get({ org })).toMatchObject({ orgId: org });
  });

  it('sets aliases, deletes an org with the owner\'s passkey and signs out everywhere', async () => {
    const [alice, pk] = await person(0, 'alice-aliases');
    const bundle = await (await alice.packages.bundlePut({ body: new Uint8Array(readFileSync(KV_STORE_WASM)), contentType: 'application/wasm' })).hash;
    const app = await (await alice.apps.create({ name: 'kv-alias', bundle })).appId;
    await alice.auth.aliasSet({ alias: 'notes', app });
    expect(await (await alice.auth.aliasList()).aliases).toEqual({ notes: app });
    await alice.auth.aliasDelete({ alias: 'notes' });
    expect(await (await alice.auth.aliasList()).aliases).toEqual({});

    const org = await (await alice.orgs.create({ name: 'doomed' })).orgId;
    await expect(alice.orgs.delete({ org })).rejects.toMatchObject({ code: 'CONFIRM_REQUIRED' });
    await alice.orgs.delete({ org }, { confirm: passkeyConfirm(pk) });
    await expect(alice.orgs.get({ org })).rejects.toMatchObject({ status: 404 });

    const laptop = client(1);
    await laptop.signIn(pk);
    await alice.signOutEverywhere();
    await expect(laptop.auth.me()).rejects.toMatchObject({ status: 401 });
  });

  it("runs the console's passkey actions: org upgrade runs, outbound and release approvals", async () => {
    const [alice, pk] = await person(0, 'alice-console');
    const [bob] = await person(1, 'bob-console');
    const bobId = (await bob.auth.me()).accountId as string;
    // Signed in again elsewhere, as the console does with a fresh tab.
    const tab = client(2);
    expect((await tab.signIn(pk)).accountId).toBe((await alice.auth.me()).accountId);

    const org = await (await tab.orgs.create({ name: 'Console Co' })).orgId;
    const kv = new Uint8Array(readFileSync(KV_STORE_WASM));
    const key = publisher();
    await tab.packages.upload({ body: signedMpk('com.mero-js.console', '1.0.0', variant(kv, 'c100'), key), contentType: 'application/octet-stream' });
    await tab.packages.upload({ body: signedMpk('com.mero-js.console', '1.0.1', variant(kv, 'c101'), key), contentType: 'application/octet-stream' });
    // A dev cluster has no registry: the install reaches the node and is refused there.
    await expect(tab.packages.install({ package: 'com.mero-js.console', version: '1.0.1' })).rejects.toMatchObject({ code: 'NO_REGISTRY' });

    const app = await (await tab.apps.create({ name: 'console-kv', package: 'com.mero-js.console', version: '1.0.0', org })).appId;
    await tab.apps.call({ app, method: 'set', args: { key: 'a', value: '1' } });

    // One passkey approval for the whole run, then follow it.
    await expect(tab.orgs.upgrade({ org, package: 'com.mero-js.console', version: '1.0.1' })).rejects.toMatchObject({
      code: 'CONFIRM_REQUIRED',
    });
    let run = await tab.orgs.upgrade({ org, package: 'com.mero-js.console', version: '1.0.1' }, { confirm: passkeyConfirm(pk) });
    expect(run).toMatchObject({ package: 'com.mero-js.console', version: '1.0.1', apps: [app] });
    for (let i = 0; i < 50 && !run.finished; i++) {
      await new Promise((r) => setTimeout(r, 200));
      run = await tab.orgs.upgradeStatus({ org, run: run.runId });
    }
    expect(run).toMatchObject({ finished: true, done: [app], failed: {} });
    expect((await (await tab.apps.list({ package: 'com.mero-js.console' })).apps)[0]).toMatchObject({ appId: app, version: '1.0.1' });
    expect((await tab.apps.view({ app, method: 'get', args: { key: 'a' } })).result).toBe('1');
    // Nothing failed, so a retry has nothing to run.
    const retried = await tab.orgs.upgradeRetry({ org, run: run.runId }, { confirm: passkeyConfirm(pk) });
    expect(retried.apps).toEqual([]);
    expect(retried.runId).not.toBe(run.runId);

    // Outbound: the release declares no prefixes; approving and revoking them is the owner's.
    await tab.orgs.outboundApproval({ org, package: 'com.mero-js.console' }, { confirm: passkeyConfirm(pk) });
    await tab.orgs.outboundApproval({ org, package: 'com.mero-js.console', prefixes: [], replace: true }, { confirm: passkeyConfirm(pk) });
    // Releases: approve one for the org.
    await tab.orgs.releaseApproval({ org, version: '2.3.200', approved: true }, { confirm: passkeyConfirm(pk) });

    // The account's own apps are its personal organization's, made at sign-up
    // (core #5015, 1.0.0-rc.20); a merod before it has none, and its account
    // routes are no longer in the client.
    const mine = (await tab.auth.me()).orgs.find((o) => o.personal)?.orgId;
    if (mine) {
      await tab.orgs.outboundApproval({ org: mine, package: 'com.mero-js.console' }, { confirm: passkeyConfirm(pk) });
      await tab.orgs.releaseApproval({ org: mine, version: '2.3.200', approved: true }, { confirm: passkeyConfirm(pk) });
      await tab.orgs.releaseApproval({ org: mine, version: '2.3.200', approved: false }, { confirm: passkeyConfirm(pk) });
    }

    const { key: apiKey, keyId } = await tab.auth.apikeyCreate({ label: 'console', apps: [app], readOnly: true }, { confirm: passkeyConfirm(pk) });
    expect(apiKey).toBeTruthy();
    await tab.auth.apikeyRevoke({ key: keyId }, { confirm: passkeyConfirm(pk) });
    expect((await (await tab.auth.apikeyList()).keys).map((k) => k.keyId)).not.toContain(keyId);
    const token = await (await tab.orgs.inviteCreate({ org, role: 'member' }, { confirm: passkeyConfirm(pk) })).token;
    await bob.orgs.inviteRedeem({ token });
    expect(bobId).toBeTruthy();
  });

  it("dates the account's passkeys, lists its sessions and ends one, on a node that has them (core #4894)", async () => {
    const [laptop, pk] = await person(0, 'alice-sessions');
    const me = await laptop.auth.me();
    const mine = me.passkeys?.find((p) => p.id === me.passkeyId);
    expect(mine).toBeTruthy();
    const phone = client(1);
    await phone.signIn(pk);

    let sessions;
    try {
      sessions = await (await laptop.auth.sessionsList()).sessions;
    } catch (error) {
      // merod 1.0.0-rc.14 and earlier: no `/auth/sessions`, no passkey times.
      if (!(error instanceof CalimeroError && error.status === 404)) throw error;
      expect(mine?.addedAt ?? null).toBeNull();
      return;
    }
    expect(mine?.addedAt).toEqual(expect.any(Number));
    expect(mine?.lastSignInAt).toBeGreaterThanOrEqual(mine?.addedAt as number);

    expect(sessions).toHaveLength(2);
    const own = sessions.find((s) => s.current);
    const other = sessions.find((s) => !s.current);
    expect(own && other).toBeTruthy();
    for (const s of sessions) {
      expect(s.passkey).toBe(me.passkeyId);
      expect(s.app).toBeNull();
      expect(s.lastActiveAt).toEqual(expect.any(Number));
    }
    expect((await (await phone.auth.sessionsList()).sessions).find((s) => s.current)?.session).toBe(other?.session);

    // The laptop ends the phone's session: the phone must sign in again.
    await laptop.auth.sessionRevoke({ session: other?.session as string });
    await expect(phone.auth.me()).rejects.toBeInstanceOf(LoginRequired);
    expect((await (await laptop.auth.sessionsList()).sessions).map((s) => s.session)).toEqual([own?.session]);
    await expect(laptop.auth.sessionRevoke({ session: other?.session as string })).rejects.toBeInstanceOf(NotFound);
    expect((await laptop.auth.me()).accountId).toBe(me.accountId);
  });

  it("shows an organization's admins their members' lockout risk, on a node that has it (core #4902)", async () => {
    const [alice, alicePk] = await person(0, 'alice-security');
    const [bob] = await person(1, 'bob-security');
    const org = await (await alice.orgs.create({ name: 'Lockout Co' })).orgId;
    await bob.orgs.inviteRedeem({ token: await (await alice.orgs.inviteCreate({ org, role: 'member' }, { confirm: passkeyConfirm(alicePk) })).token });
    const bobId = (await bob.auth.me()).accountId as string;
    let members;
    try {
      members = await (await alice.orgs.members({ org, detail: 'security' })).members;
    } catch (error) {
      // merod before core #4902: no `GET /orgs/{org}/members` (a bare `404`).
      if (!(error instanceof CalimeroError && (error.status === 404 || error.status === 405)) || error instanceof NotFound) {
        throw error;
      }
      return;
    }
    expect(members).toHaveLength(2);
    const b = members.find((m) => m.account === bobId);
    expect(b).toMatchObject({ role: 'member', passkeys: 1, recoveryCode: false, sessions: 1 });
    expect(b?.lastActiveDay).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    for (const m of members) {
      expect(Object.keys(m).sort()).toEqual(
        ['account', 'lastActiveDay', 'name', 'passkeys', 'recoveryCode', 'role', 'sessions', 'teams'],
      );
    }
    // A member is refused; removed, bob is gone from the listing.
    await expect(bob.orgs.members({ org, detail: 'security' })).rejects.toMatchObject({ status: 403 });
    await alice.orgs.removeMember({ org, account: bobId }, { confirm: passkeyConfirm(alicePk) });
    expect((await (await alice.orgs.members({ org, detail: 'security' })).members).map((m) => m.account)).not.toContain(bobId);
  });

  it("lists the account's sign-ins and approvals, on a node that has them (core #4901)", async () => {
    const [laptop, pk] = await person(0, 'alice-activity');
    const me = await laptop.auth.me();
    let activity;
    try {
      activity = await (await laptop.auth.activityList({})).activity;
    } catch (error) {
      // merod before core #4901: no `/auth/activity` (a bare `404`).
      if (!(error instanceof CalimeroError && error.status === 404) || error instanceof NotFound) throw error;
      return;
    }
    expect(activity).toEqual([
      expect.objectContaining({ kind: 'signIn', passkey: me.passkeyId, app: null, origin: null }),
    ]);
    const signedUp = activity[0]?.at as number;

    await laptop.auth.apikeyCreate({ label: 'ci' }, { confirm: passkeyConfirm(pk) });
    activity = await (await laptop.auth.activityList({})).activity;
    expect(activity.map((a) => [a.kind, a.method, a.passkey])).toEqual([
      ['approval', '__create_api_key', me.passkeyId],
      ['signIn', null, me.passkeyId],
    ]);
    // `after` keeps only what came later.
    expect((await (await laptop.auth.activityList({ after: signedUp })).activity).every((a) => a.at > signedUp)).toBe(true);

    // Each account reads only its own.
    const [bob] = await person(1, 'bob-activity');
    const bobs = await (await bob.auth.activityList({})).activity;
    expect(bobs).toHaveLength(1);
    expect(bobs[0]?.passkey).not.toBe(me.passkeyId);
  });

  it("covers the rest of 1.0's routes: metadata, usage, blobs, presence, transfer", async () => {
    const [alice, pk] = await person(0, 'alice-rest');
    const [bob] = await person(1, 'bob-rest');
    const bobId = (await bob.auth.me()).accountId as string;
    const me = await alice.auth.me();

    await alice.auth.passkeyLabel({ cred: me.passkeyId as string, label: 'laptop' });
    expect(await alice.auth.usage()).toBeTruthy();
    const doc = await alice.public.attestation({ nonce: '00'.repeat(32) });
    expect(doc.mock).toBe(true);

    const org = await (await alice.orgs.create({ name: 'Rest Co' })).orgId;
    await alice.orgs.setMember({ org, account: bobId, role: 'member' }, { confirm: passkeyConfirm(pk) });
    await alice.orgs.metadata({ org, metadata: { colour: 'teal' } });
    await bob.orgs.memberMetadata({ org, account: bobId, metadata: { title: 'engineer' } });
    await alice.orgs.memberMetadata({ org, account: bobId, metadata: null });

    const bundle = await (await alice.packages.bundlePut({ body: new Uint8Array(readFileSync(KV_STORE_WASM)), contentType: 'application/wasm' })).hash;
    const app = await (await alice.apps.create({ name: 'rest-kv', bundle, org })).appId;
    await alice.auth.aliasSet({ alias: 'rest', app });
    expect(await (await alice.auth.aliasGet({ alias: 'rest' })).appId).toBe(app);
    await alice.apps.metadata({ app, metadata: { icon: 'k' } });
    await alice.apps.call({ app, method: 'set', args: { key: 'a', value: '1' } });
    expect(await alice.apps.storage({ app })).toHaveProperty('keys');
    expect(await alice.apps.abi({ app })).toHaveProperty('bundle');

    const up = await alice.apps.blobUpload({ app, body: new TextEncoder().encode('hello blob') });
    expect((await (await alice.apps.blobList({ app })).blobs).map((b) => b.id)).toContain(up.id);
    expect(await alice.apps.blobInfo({ app, id: up.blobId })).toMatchObject({ id: up.id, size: 10 });
    await alice.apps.blobDelete({ app, id: up.blobId });
    await expect(alice.apps.blobInfo({ app, id: up.blobId })).rejects.toMatchObject({ status: 404 });

    await alice.apps.presencePublish({ app, seq: 1, state: presenceState(new Uint8Array([1, 2, 3])) });
    expect(await (await alice.apps.presenceGet({ app })).present).toContainEqual(expect.objectContaining({ state: '010203' }));
    const ac = new AbortController();
    const first = await alice.presenceStream(app, ac.signal).next();
    ac.abort();
    expect(first.value?.kind).toBe('snapshot');
    await alice.apps.presencePublish({ app, seq: 2, state: presenceState(null) });

    await alice.apps.transfer({ app, to: bobId }, { confirm: passkeyConfirm(pk) });
    expect(await bob.apps.get({ app })).toMatchObject({ owner: bobId });
  });

  // Core's `feat/org-all-apps`: a node without the route answers 404, and the
  // case is skipped.
  it('gives a member every app added later, by invite and by an admin', async (ctx) => {
    const [alice, alicePk] = await person(0, 'alice-allapps');
    const [bob] = await person(1, 'bob-allapps');
    const [carol, carolPk] = await person(2, 'carol-allapps');
    const org = await (await alice.orgs.create({ name: 'All Apps Co' })).orgId;
    await carol.orgs.inviteRedeem({ token: await (await alice.orgs.inviteCreate({ org, role: 'member' }, { confirm: passkeyConfirm(alicePk) })).token });
    const carolId = (await carol.auth.me()).accountId as string;
    const set = await alice.orgs.allApps({ org, account: carolId, allApps: true }, { confirm: passkeyConfirm(alicePk) }).catch((e: unknown) => e);
    if (set instanceof CalimeroError && (set.status === 404 || set.status === 405) && !(set instanceof NotFound)) {
      return ctx.skip();
    }
    expect(set).toEqual({});
    await bob.orgs.inviteRedeem({ token: await (await alice.orgs.inviteCreate({ org, role: 'member', teams: [], allApps: true }, { confirm: passkeyConfirm(alicePk) })).token });
    const bobId = (await bob.auth.me()).accountId as string;
    expect([...((await alice.orgs.get({ org })).allApps ?? [])].sort()).toEqual([bobId, carolId].sort());
    // A member below admin gives nobody every app, and mints no such invite.
    await expect(carol.orgs.allApps({ org, account: carolId, allApps: false }, { confirm: passkeyConfirm(carolPk) })).rejects.toMatchObject({ status: 403 });
    await expect(carol.orgs.inviteCreate({ org, role: 'member', teams: [], allApps: true }, { confirm: passkeyConfirm(carolPk) })).rejects.toMatchObject({ status: 403 });
    await alice.orgs.allApps({ org, account: carolId, allApps: false }, { confirm: passkeyConfirm(alicePk) });
    expect((await alice.orgs.get({ org })).allApps).toEqual([bobId]);
  });

  // Core's `feat/move-app-to-org`: a node without the route answers the move
  // 404, and the case is skipped.
  it('moves an app with its people into another organization, its data kept', async (ctx) => {
    const [alice, pk] = await person(0, 'alice-move');
    const [bob] = await person(1, 'bob-move');
    const bobId = (await bob.auth.me()).accountId as string;
    const bundle = await (await alice.packages.bundlePut({ body: new Uint8Array(readFileSync(KV_STORE_WASM)), contentType: 'application/wasm' })).hash;
    const src = await (await alice.orgs.create({ name: 'move-src' })).orgId;
    const dst = await (await alice.orgs.create({ name: 'move-dst' })).orgId;
    await alice.orgs.setMember({ org: src, account: bobId, role: 'member' }, { confirm: passkeyConfirm(pk) });
    const app = await (await alice.apps.create({ name: 'moving', bundle, org: src })).appId;
    await alice.apps.call({ app, method: 'set', args: { key: 'a', value: '1' } });
    const moved = await alice.apps.move({ app, toOrg: dst, bringMembers: true }, { confirm: passkeyConfirm(pk) }).catch((e: unknown) => e);
    if (moved instanceof CalimeroError && (moved.status === 404 || moved.status === 405)) return ctx.skip();
    expect(moved).toMatchObject({ appId: app, from: src, org: dst, added: [bobId], access: { kind: 'org' } });
    expect(await alice.apps.get({ app })).toMatchObject({ org: dst });
    expect((await bob.apps.view({ app, method: 'get', args: { key: 'a' } })).result).toBe('1');
    // Back again, now without a passkey: the node asks for one.
    await expect(alice.apps.move({ app, toOrg: src })).rejects.toBeInstanceOf(ConfirmRequired);
  });

  // Core's `claude/package-origins`: a node without the routes answers the
  // listing of an installed package 404, and the case is skipped.
  it("registers a package's own origin, signs in from it, and removes it", async (ctx) => {
    const [alice, alicePk] = await person(0, 'alice-origins');
    const pkg = 'com.mero-js.origins';
    const kv = new Uint8Array(readFileSync(KV_STORE_WASM));
    await alice.packages.upload({ body: signedMpk(pkg, '1.0.0', variant(kv, 'origins'), publisher()), contentType: 'application/octet-stream' });
    const listed = await alice.packages
      .originsList({ package: pkg })
      .then((r) => r.origins)
      .catch((e: unknown) => e);
    if (listed instanceof CalimeroError && (listed.status === 404 || listed.status === 405)) return ctx.skip();
    expect(listed).toEqual([]);

    const site = 'https://chat.mero-js.test';
    // Adding one is a step-up; without a passkey the node asks for it.
    await expect(alice.packages.originsAdd({ package: pkg, origin: site })).rejects.toBeInstanceOf(ConfirmRequired);
    const added = await alice.packages.originsAdd({ package: pkg, origin: site }, { confirm: passkeyConfirm(alicePk) });
    expect(added).toMatchObject({ package: pkg, origin: site, rpId: 'chat.mero-js.test' });
    expect((await (await alice.packages.originsList({ package: pkg })).origins).map((o) => o.origin)).toEqual([site]);
    await expect(alice.packages.originsAdd({ package: pkg, origin: 'not an origin' }, { confirm: passkeyConfirm(alicePk) })).rejects.toBeInstanceOf(BadOrigin);

    // A person signs up on the developer's site: the passkey is that site's,
    // the session bound to the package.
    const carol = client(1);
    const carolPk = new SoftPasskey('chat.mero-js.test', site);
    await carol.signUp(carolPk, 'carol', { origin: site });
    const me = await carol.auth.me();
    expect(me.origin).toBe(site);
    expect(me.app).toBeTruthy();
    const again = client(2);
    await again.signIn(carolPk, { origin: site });

    // A site nobody registered is refused before any passkey prompt.
    const stranger = await client(0)
      .signIn(new SoftPasskey('stranger.mero-js.test', 'https://stranger.mero-js.test'), {
        origin: 'https://stranger.mero-js.test',
      })
      .catch((e: unknown) => e);
    expect(stranger).toBeInstanceOf(OriginNotRegistered);
    expect((stranger as OriginNotRegistered).origin).toBe('https://stranger.mero-js.test');

    // Removing the origin ends the sessions opened there.
    expect(await alice.packages.originsRemove({ package: pkg, origin: site }, { confirm: passkeyConfirm(alicePk) })).toMatchObject({ package: pkg, origin: site, removed: true });
    await new Promise((r) => setTimeout(r, 300));
    await expect(carol.auth.me()).rejects.toBeInstanceOf(LoginRequired);
    await expect(alice.packages.originsRemove({ package: pkg, origin: site }, { confirm: passkeyConfirm(alicePk) })).rejects.toBeInstanceOf(NotFound);
    expect(await (await alice.packages.originsList({ package: pkg })).origins).toEqual([]);
  });

  it('keeps serving after a node dies', async () => {
    const [alice, pk] = await person(1, 'alice-failover');
    const bundle = await (await alice.packages.bundlePut({ body: new Uint8Array(readFileSync(KV_STORE_WASM)), contentType: 'application/wasm' })).hash;
    const app = await (await alice.apps.create({ name: 'kv-ha', bundle })).appId;
    await alice.apps.call({ app, method: 'set', args: { key: 'k', value: 'v' } });
    cluster.stop(0);
    const other = client(2);
    await other.signIn(pk);
    let ok = false;
    for (let i = 0; i < 40 && !ok; i++) {
      try {
        await other.apps.call({ app, method: 'set', args: { key: 'k2', value: 'v2' }, idempotencyKey: 'after-failover' });
        ok = true;
      } catch {
        await new Promise((r) => setTimeout(r, 250));
      }
    }
    expect(ok).toBe(true);
    expect((await other.apps.view({ app, method: 'get', args: { key: 'k' } })).result).toBe('v');
  });
});
