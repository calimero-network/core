import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { AppError, errorFrom, upgradeConsent, type NodeEvent } from '@calimero-network/mero-js';
import {
  useAccount,
  useApiKeys,
  useApp,
  useAppAbi,
  useBlobInfo,
  useBlobs,
  useCreateOrg,
  useDeleteApp,
  useDeleteBlob,
  useDeleteOrg,
  useDeleteTeam,
  usePackages,
  useSetAppMetadata,
  useSetMemberMetadata,
  useSetOrgMetadata,
  useTeam,
  useUpdateTeam,
  useUpgradeApp,
  useApps,
  useConsent,
  useCreateApp,
  useCreateInvite,
  useCreateTeam,
  useEphemeral,
  useExecute,
  useInviteRedemption,
  useOrg,
  usePasskeys,
  useRemoveMember,
  useRemoveTeamMember,
  useSetCapabilities,
  useSetMember,
  useSetTeamMember,
  useSubscription,
  useUploadBlob,
  useView,
} from '../hooks';
import { useMero } from '../context';
import { memorySessionStore } from '../storage';
import { expectConfirmedBy, fakeClient, passkey, SESSION, wrapper } from './fake';

describe('calls and views', () => {
  it('useApp reads the app and binds call / view to it', async () => {
    const client = fakeClient();
    const { result } = renderHook(() => useApp(), { wrapper: wrapper(client) });
    await waitFor(() => expect(result.current.app).toEqual({ appId: 'app-1', name: 'kv' }));
    await result.current.call('set', { key: 'a' });
    expect(client.apps.call).toHaveBeenCalledWith({ app: 'app-1', method: 'set', args: { key: 'a' }, idempotencyKey: undefined }, undefined);
    await result.current.view('get', { key: 'a' }, { stale: true });
    expect(client.apps.view).toHaveBeenCalledWith({
      app: 'app-1',
      method: 'get',
      args: { key: 'a' },
      consistency: 'stale',
      minIndex: undefined,
    });
  });

  it('useExecute resolves the result and passes an idempotency key', async () => {
    const client = fakeClient();
    const { result } = renderHook(() => useExecute('app-9'), { wrapper: wrapper(client) });
    let value: unknown;
    await act(async () => {
      value = await result.current.execute('set', { key: 'a' }, { idempotencyKey: 'k1' });
    });
    expect(value).toBe('ok');
    expect(client.apps.call).toHaveBeenCalledWith({ app: 'app-9', method: 'set', args: { key: 'a' }, idempotencyKey: 'k1' }, undefined);
  });

  it('useExecute answers a confirm method with the passkey', async () => {
    const client = fakeClient();
    const { result } = renderHook(() => useExecute(), { wrapper: wrapper(client) });
    await act(async () => {
      await result.current.execute('clear', {}, { confirm: true });
    });
    const [args, options] = client.apps.call.mock.calls[0] as unknown as [unknown, unknown];
    expect(args).toEqual({ app: 'app-1', method: 'clear', args: {}, idempotencyKey: undefined });
    await expectConfirmedBy(options, passkey);
  });

  it('useExecute surfaces the app error, typed', async () => {
    const refusal = errorFrom(422, { error: 'APP_ERROR', details: { code: 'Taken', fields: { key: 'a' } } });
    const client = fakeClient({}, { apps: { call: vi.fn(async () => Promise.reject(refusal)) } });
    const { result } = renderHook(() => useExecute(), { wrapper: wrapper(client) });
    await act(async () => {
      expect(await result.current.execute('set')).toBeNull();
    });
    expect(result.current.error).toBeInstanceOf(AppError);
    expect(result.current.failure).toMatchObject({ kind: 'app', appCode: 'Taken', fields: { key: 'a' } });
  });

  it('useExecute rethrows with throwOnError', async () => {
    const client = fakeClient({}, { apps: { call: vi.fn(async () => Promise.reject(errorFrom(404, { error: 'NOT_FOUND' }))) } });
    const { result } = renderHook(() => useExecute(undefined, { throwOnError: true }), { wrapper: wrapper(client) });
    await act(async () => {
      await expect(result.current.execute('x')).rejects.toMatchObject({ status: 404 });
    });
  });

  it('a refused refresh during a call signs the page out', async () => {
    const client = fakeClient({}, { apps: { call: vi.fn(async () => Promise.reject(errorFrom(401, { error: 'UNAUTHENTICATED' }))) } });
    const { result } = renderHook(() => ({ exec: useExecute(), mero: useMero() }), { wrapper: wrapper(client) });
    await act(async () => {
      await result.current.exec.execute('x');
    });
    expect(result.current.mero.isAuthenticated).toBe(false);
  });

  it('useView reads, and reads again when the arguments change', async () => {
    const client = fakeClient();
    const { result, rerender } = renderHook(({ key }) => useView<{ value: number }>('get', { key }), {
      wrapper: wrapper(client),
      initialProps: { key: 'a' },
    });
    await waitFor(() => expect(result.current.data).toEqual({ value: 1 }));
    expect(result.current.index).toBe(7);
    rerender({ key: 'b' });
    await waitFor(() =>
      expect(client.apps.view).toHaveBeenLastCalledWith({
        app: 'app-1',
        method: 'get',
        args: { key: 'b' },
        consistency: undefined,
        minIndex: undefined,
      }),
    );
  });

  it('nothing reads while signed out', async () => {
    const client = fakeClient();
    const { result } = renderHook(() => useView('get'), { wrapper: wrapper(client, { store: memorySessionStore() }) });
    await new Promise((r) => setTimeout(r, 10));
    expect(client.apps.view).not.toHaveBeenCalled();
    expect(result.current.data).toBeUndefined();
  });
});

describe('useSubscription', () => {
  it('delivers events and aborts the stream on unmount', async () => {
    let seen: AbortSignal | undefined;
    const client = fakeClient({
      events: vi.fn(async function* (_app: string, _from: number, signal: AbortSignal): AsyncGenerator<NodeEvent> {
        seen = signal;
        yield { id: '3:0', kind: 'Added', data: { k: 'a' } };
        await new Promise<void>((resolve) => signal.addEventListener('abort', () => resolve()));
      }),
    });
    const got: unknown[] = [];
    const { unmount } = renderHook(() => useSubscription('app-1', (e) => got.push(e)), { wrapper: wrapper(client) });
    await waitFor(() => expect(got).toHaveLength(1));
    expect(got[0]).toEqual({ appId: 'app-1', id: '3:0', index: 3, seq: 0, kind: 'Added', data: { k: 'a' } });
    expect((client.events.mock.calls[0] as unknown[])[1]).toBe(Number.MAX_SAFE_INTEGER);
    unmount();
    expect(seen?.aborted).toBe(true);
  });

  it('reopens from the last event and skips what it already delivered', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      let round = 0;
      const client = fakeClient({
        events: vi.fn(async function* (_app: string, _from: number, signal: AbortSignal): AsyncGenerator<NodeEvent> {
          round += 1;
          if (round === 1) {
            yield { id: '5:0', kind: 'A', data: 1 };
            yield { id: '5:1', kind: 'B', data: 2 };
            return; // the node closed the stream
          }
          yield { id: '5:0', kind: 'A', data: 1 };
          yield { id: '5:1', kind: 'B', data: 2 };
          yield { id: '6:0', kind: 'C', data: 3 };
          await new Promise<void>((resolve) => signal.addEventListener('abort', () => resolve()));
        }),
      });
      const kinds: string[] = [];
      const { unmount } = renderHook(() => useSubscription(['app-1'], (e) => kinds.push(e.kind), { from: 0 }), {
        wrapper: wrapper(client),
      });
      await waitFor(() => expect(kinds).toEqual(['A', 'B']));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1100);
      });
      await waitFor(() => expect(kinds).toEqual(['A', 'B', 'C']));
      expect((client.events.mock.calls[0] as unknown[])[1]).toBe(0);
      expect((client.events.mock.calls[1] as unknown[])[1]).toBe(5);
      unmount();
    } finally {
      vi.useRealTimers();
    }
  });

  it('stops and signs out when the session is gone', async () => {
    const client = fakeClient({
      events: vi.fn(async function* (): AsyncGenerator<NodeEvent> {
        throw errorFrom(401, { error: 'UNAUTHENTICATED' });
      }),
    });
    const { result } = renderHook(
      () => {
        useSubscription('app-1', () => undefined);
        return useMero();
      },
      { wrapper: wrapper(client) },
    );
    await waitFor(() => expect(result.current.isAuthenticated).toBe(false));
  });
});

describe('passkey step-ups', () => {
  it('approves membership changes with the passkey', async () => {
    const client = fakeClient();
    const { result } = renderHook(
      () => ({
        set: useSetMember(),
        remove: useRemoveMember(),
        caps: useSetCapabilities(),
        teamSet: useSetTeamMember(),
        teamRemove: useRemoveTeamMember(),
      }),
      { wrapper: wrapper(client) },
    );
    await act(async () => {
      await result.current.set.setMember('org-1', 'acct-2', 'admin');
      await result.current.remove.removeMember('org-1', 'acct-3');
      await result.current.caps.setCapabilities('org-1', 5, { account: 'acct-2' });
      await result.current.teamSet.setTeamMember('org-1', 'team-1', 'acct-2', { role: 'lead' });
      await result.current.teamRemove.removeTeamMember('org-1', 'team-1', 'acct-2');
    });
    const steps: Array<[{ mock: { calls: unknown[][] } }, unknown]> = [
      [client.orgs.setMember, { org: 'org-1', account: 'acct-2', role: 'admin' }],
      [client.orgs.removeMember, { org: 'org-1', account: 'acct-3' }],
      [client.orgs.capabilities, { org: 'org-1', capabilities: 5, account: 'acct-2' }],
      [client.orgs.teamSetMember, { org: 'org-1', team: 'team-1', account: 'acct-2', role: 'lead' }],
      [client.orgs.teamRemoveMember, { org: 'org-1', team: 'team-1', account: 'acct-2' }],
    ];
    for (const [spy, args] of steps) {
      expect(spy.mock.calls).toHaveLength(1);
      expect(spy.mock.calls[0][0]).toEqual(args);
      await expectConfirmedBy(spy.mock.calls[0][1], passkey);
    }
  });

  it('approves adding and removing a passkey with one the account has', async () => {
    const client = fakeClient();
    const other = { create: vi.fn(), get: vi.fn() };
    const { result } = renderHook(() => usePasskeys(), { wrapper: wrapper(client) });
    await act(async () => {
      expect(await result.current.add()).toBe('cred-2');
      await result.current.add({ passkey: other });
      await result.current.remove('cred-1');
    });
    expect(client.addPasskey).toHaveBeenNthCalledWith(1, passkey, passkey);
    expect(client.addPasskey).toHaveBeenNthCalledWith(2, other, passkey);
    expect(client.auth.passkeyRemove.mock.calls[0][0]).toEqual({ cred: 'cred-1' });
    await expectConfirmedBy(client.auth.passkeyRemove.mock.calls[0][1], passkey);
  });

  it('lists, creates (with the passkey) and revokes API keys', async () => {
    const client = fakeClient();
    const { result } = renderHook(() => useApiKeys(), { wrapper: wrapper(client) });
    await waitFor(() => expect(result.current.keys).toEqual([{ keyId: 'k-1' }]));
    let made: { keyId: string; key: string } | null = null;
    await act(async () => {
      made = await result.current.create({ label: 'ci', readOnly: true });
      await result.current.revoke('k-1');
    });
    expect(made).toEqual({ keyId: 'k-2', key: 'ak_secret' });
    expect(client.auth.apikeyCreate.mock.calls[0][0]).toEqual({ label: 'ci', readOnly: true });
    await expectConfirmedBy(client.auth.apikeyCreate.mock.calls[0][1], passkey);
    expect(client.auth.apikeyRevoke.mock.calls[0][0]).toEqual({ key: 'k-1' });
    await expectConfirmedBy(client.auth.apikeyRevoke.mock.calls[0][1], passkey);
  });
});

describe('apps, orgs, teams, invites', () => {
  it('lists and creates apps', async () => {
    const client = fakeClient({}, { apps: { list: vi.fn(async () => ({ apps: [{ appId: 'a' }] })) } });
    const { result } = renderHook(() => ({ list: useApps({ package: 'com.x' }), create: useCreateApp() }), {
      wrapper: wrapper(client),
    });
    await waitFor(() => expect(result.current.list.data).toEqual([{ appId: 'a' }]));
    expect(client.apps.list).toHaveBeenCalledWith({ package: 'com.x', bundle: undefined });
    let id: string | null = null;
    await act(async () => {
      id = await result.current.create.create({ name: 'kv', bundle: { package: 'com.x' }, org: 'org-1' });
    });
    expect(id).toBe('app-new');
    expect(client.apps.create).toHaveBeenCalledWith({ name: 'kv', package: 'com.x', org: 'org-1', access: undefined });
  });

  it('reads an org and creates a team', async () => {
    const client = fakeClient();
    const { result } = renderHook(() => ({ org: useOrg('org-1'), team: useCreateTeam() }), { wrapper: wrapper(client) });
    await waitFor(() => expect(result.current.org.data?.newWorkStopped).toBeNull());
    await act(async () => {
      await result.current.team.create({ orgId: 'org-1', name: 'design' });
    });
    expect(client.orgs.teamCreate).toHaveBeenCalledWith({ org: 'org-1', name: 'design', join: 'invite', parent: undefined });
  });

  it('mints an invite link that carries the token in the fragment', async () => {
    const client = fakeClient();
    const { result } = renderHook(() => useCreateInvite(), { wrapper: wrapper(client) });
    let made: { token: string; link: string } | null = null;
    await act(async () => {
      made = await result.current.create({ orgId: 'org-1', teams: ['t'], linkBase: 'https://app.test/join' });
    });
    expect(client.orgs.inviteCreate.mock.calls[0][0]).toEqual({
      org: 'org-1',
      role: 'member',
      teams: ['t'],
      ttlSecs: undefined,
      uses: undefined,
    });
    await expectConfirmedBy(client.orgs.inviteCreate.mock.calls[0][1], passkey);
    expect(made).toEqual({ token: 'tok-1', link: 'https://app.test/join#invite=tok-1' });
  });

  describe('useInviteRedemption', () => {
    beforeEach(() => {
      sessionStorage.clear();
      window.history.replaceState(null, '', '/page?x=1#invite=tok-9');
    });

    it('waits for sign-in, then redeems and cleans the address bar', async () => {
      const client = fakeClient();
      const { result } = renderHook(() => ({ inv: useInviteRedemption(), mero: useMero() }), {
        wrapper: wrapper(client, { store: memorySessionStore() }),
      });
      expect(result.current.inv.token).toBe('tok-9');
      await waitFor(() => expect(result.current.inv.status).toBe('waiting-for-sign-in'));
      expect(window.location.hash).toBe('');
      expect(window.location.search).toBe('?x=1');
      await act(async () => {
        await result.current.mero.signIn();
      });
      await waitFor(() => expect(result.current.inv.status).toBe('redeemed'));
      expect(client.orgs.inviteRedeem).toHaveBeenCalledTimes(1);
      expect(client.orgs.inviteRedeem).toHaveBeenCalledWith({ token: 'tok-9' });
      expect(sessionStorage.getItem('mero:pendingInvite')).toBeNull();
    });

    it('reports a refused invite', async () => {
      const client = fakeClient(
        {},
        { orgs: { inviteRedeem: vi.fn(async () => Promise.reject(errorFrom(404, { error: 'NOT_FOUND' }))) } },
      );
      const { result } = renderHook(() => useInviteRedemption(), { wrapper: wrapper(client) });
      await waitFor(() => expect(result.current.status).toBe('failed'));
      expect(result.current.failure?.kind).toBe('not-found');
    });
  });
});

describe('consent, blobs, presence', () => {
  it('opens the consent page and waits for the decision', async () => {
    const client = fakeClient();
    const navigate = vi.fn();
    const { result } = renderHook(() => useConsent(), { wrapper: wrapper(client) });
    const req = upgradeConsent('app-1', '1.2.0', 'https://app.test/back');
    await act(async () => {
      await result.current.open(req, navigate);
    });
    expect(navigate).toHaveBeenCalledWith('https://node.test/consent/req-1');
    expect(result.current.started?.requestId).toBe('req-1');
    expect(result.current.code).toBe('ABC-123');
    await act(async () => {
      await result.current.wait('req-1', { intervalMs: 5 });
    });
    expect(client.waitForConsent).toHaveBeenCalledWith('req-1', expect.objectContaining({ intervalMs: 5, expiresAt: 9e9 }));
    expect(result.current.result?.status).toBe('approved');
  });

  it('shows the confirmation code before opening the consent page', async () => {
    const client = fakeClient();
    const navigate = vi.fn();
    const seen: string[] = [];
    const { result } = renderHook(() => useConsent(), { wrapper: wrapper(client) });
    const req = upgradeConsent('app-1', '1.2.0', 'https://app.test/back');
    await act(async () => {
      await result.current.open(req, {
        navigate,
        confirm: (started) => {
          seen.push(started.code ?? '');
          return false;
        },
      });
    });
    expect(seen).toEqual(['ABC-123']);
    expect(navigate).not.toHaveBeenCalled();
    expect(result.current.code).toBe('ABC-123');
  });

  it('uploads a Blob as bytes to the app', async () => {
    const client = fakeClient();
    const { result } = renderHook(() => useUploadBlob(), { wrapper: wrapper(client) });
    await act(async () => {
      await result.current.upload(new Uint8Array([1, 2, 3]));
    });
    expect(client.apps.blobUpload).toHaveBeenCalledWith({
      app: 'app-1',
      body: new Uint8Array([1, 2, 3]),
      contentType: 'application/octet-stream',
    });
  });

  it('presence: follows the snapshot and changes, publishes and retracts', async () => {
    const hex = (v: unknown) => Array.from(new TextEncoder().encode(JSON.stringify(v)), (b) => b.toString(16).padStart(2, '0')).join('');
    let push: ((e: NodeEvent) => void) | null = null;
    const client = fakeClient({
      presenceStream: vi.fn(async function* (_app: string, signal: AbortSignal): AsyncGenerator<NodeEvent> {
        yield {
          id: '',
          kind: 'snapshot',
          data: [
            { account: 'bob', device: 'd1', state: hex({ cursor: 1 }) },
            { account: SESSION.accountId, device: 'me', state: hex({ cursor: 0 }) },
          ],
        };
        const queue: NodeEvent[] = [];
        let wake: (() => void) | null = null;
        push = (e) => {
          queue.push(e);
          wake?.();
        };
        while (!signal.aborted) {
          if (queue.length) yield queue.shift() as NodeEvent;
          else await new Promise<void>((r) => (wake = r));
        }
      }),
    });
    const { result, unmount } = renderHook(() => useEphemeral<{ cursor: number }>('app-1', { throttleMs: 0 }), {
      wrapper: wrapper(client),
    });
    await waitFor(() => expect(result.current.peers.get('bob')).toEqual({ cursor: 1 }));
    expect(result.current.peers.has(SESSION.accountId)).toBe(false);
    act(() => push?.({ id: '', kind: 'presence', data: { account: 'bob', device: 'd1', state: null } }));
    await waitFor(() => expect(result.current.peers.size).toBe(0));

    act(() => result.current.setPresence({ cursor: 4 }));
    await waitFor(() => expect(client.apps.presencePublish).toHaveBeenCalledTimes(1));
    const [published] = client.apps.presencePublish.mock.calls[0] as unknown as [{ app: string; seq: number; state: string }];
    expect(published.app).toBe('app-1');
    expect(published.state).toBe(hex({ cursor: 4 }));
    unmount();
    expect(client.apps.presencePublish).toHaveBeenLastCalledWith({ app: 'app-1', seq: expect.any(Number), state: null });
  });
});

describe('the generated method groups', () => {
  it('reads through them: account, ABI, team, blobs, packages', async () => {
    const client = fakeClient(
      {},
      {
        apps: {
          abi: vi.fn(async () => ({ bundle: 'b', abi: {} })),
          blobList: vi.fn(async () => ({ blobs: [{ blobId: 'b1', id: 'h1', size: 1 }] })),
        },
        orgs: { teamGet: vi.fn(async () => ({ teamId: 'team-1' })) },
        packages: { list: vi.fn(async () => ({ packages: [{ package: 'com.x' }] })) },
      },
    );
    const { result } = renderHook(
      () => ({
        account: useAccount(),
        abi: useAppAbi(),
        team: useTeam('org-1', 'team-1'),
        blobs: useBlobs(),
        blob: useBlobInfo('b58'),
        packages: usePackages(),
      }),
      { wrapper: wrapper(client) },
    );
    await waitFor(() => {
      expect(result.current.account.data).toEqual({ accountId: SESSION.accountId });
      expect(result.current.abi.data).toEqual({ bundle: 'b', abi: {} });
      expect(result.current.team.data).toEqual({ teamId: 'team-1' });
      expect(result.current.blobs.data).toEqual([{ blobId: 'b1', id: 'h1', size: 1 }]);
      expect(result.current.blob.data).toEqual({ blobId: 'b58', id: 'hex', size: 3 });
      expect(result.current.packages.data).toEqual([{ package: 'com.x' }]);
    });
    expect(client.apps.abi).toHaveBeenCalledWith({ app: 'app-1' });
    expect(client.orgs.teamGet).toHaveBeenCalledWith({ org: 'org-1', team: 'team-1' });
    expect(client.apps.blobList).toHaveBeenCalledWith({ app: 'app-1' });
    expect(client.apps.blobInfo).toHaveBeenCalledWith({ app: 'app-1', id: 'b58' });
  });

  it('writes through them, with the passkey where the node asks for it', async () => {
    const client = fakeClient();
    const { result } = renderHook(
      () => ({
        createOrg: useCreateOrg(),
        deleteOrg: useDeleteOrg(),
        orgMeta: useSetOrgMetadata(),
        memberMeta: useSetMemberMetadata(),
        updateTeam: useUpdateTeam(),
        deleteTeam: useDeleteTeam(),
        appMeta: useSetAppMetadata(),
        deleteApp: useDeleteApp(),
        upgrade: useUpgradeApp(),
        deleteBlob: useDeleteBlob(),
        passkeys: usePasskeys(),
      }),
      { wrapper: wrapper(client) },
    );
    let orgId: string | null = null;
    let upgraded: unknown = null;
    await act(async () => {
      orgId = await result.current.createOrg.create('acme');
      await result.current.deleteOrg.deleteOrg('org-1');
      await result.current.orgMeta.setMetadata('org-1', { a: 1 });
      await result.current.memberMeta.setMetadata('org-1', 'acct-2', { name: 'Ada' });
      await result.current.updateTeam.update('org-1', 'team-1', { parent: null });
      await result.current.deleteTeam.deleteTeam('org-1', 'team-1');
      await result.current.appMeta.setMetadata('app-1', { title: 'x' });
      await result.current.deleteApp.deleteApp('app-1');
      upgraded = await result.current.upgrade.upgrade({ version: '1.2.0' });
      await result.current.deleteBlob.deleteBlob('b58');
      await result.current.passkeys.label('cred-1', 'laptop');
    });
    expect(orgId).toBe('org-1');
    expect(upgraded).toEqual({ index: 1, status: 'ok', error: null });
    expect(client.orgs.create).toHaveBeenCalledWith({ name: 'acme' });
    expect(client.orgs.delete.mock.calls[0][0]).toEqual({ org: 'org-1' });
    await expectConfirmedBy(client.orgs.delete.mock.calls[0][1], passkey);
    expect(client.orgs.metadata).toHaveBeenCalledWith({ org: 'org-1', metadata: { a: 1 } });
    expect(client.orgs.memberMetadata).toHaveBeenCalledWith({ org: 'org-1', account: 'acct-2', metadata: { name: 'Ada' } });
    expect(client.orgs.teamUpdate).toHaveBeenCalledWith({ org: 'org-1', team: 'team-1', parent: null });
    expect(client.orgs.teamDelete.mock.calls[0][0]).toEqual({ org: 'org-1', team: 'team-1' });
    await expectConfirmedBy(client.orgs.teamDelete.mock.calls[0][1], passkey);
    expect(client.apps.metadata).toHaveBeenCalledWith({ app: 'app-1', metadata: { title: 'x' } });
    expect(client.apps.delete).toHaveBeenCalledWith({ app: 'app-1' });
    expect(client.apps.upgrade.mock.calls[0][0]).toEqual({ app: 'app-1', version: '1.2.0' });
    await expectConfirmedBy(client.apps.upgrade.mock.calls[0][1], passkey);
    expect(client.apps.blobDelete).toHaveBeenCalledWith({ app: 'app-1', id: 'b58' });
    expect(client.auth.passkeyLabel).toHaveBeenCalledWith({ cred: 'cred-1', label: 'laptop' });
  });

  it('creates an app from a bundle hash', async () => {
    const client = fakeClient();
    const { result } = renderHook(() => useCreateApp(), { wrapper: wrapper(client) });
    await act(async () => {
      await result.current.create({ name: 'kv', bundle: 'abc', initArgs: { n: 1 } });
    });
    expect(client.apps.create).toHaveBeenCalledWith({ name: 'kv', bundle: 'abc', initArgs: { n: 1 }, access: undefined });
  });

  it('requests consent and checks its result', async () => {
    const client = fakeClient();
    const { result } = renderHook(() => useConsent(), { wrapper: wrapper(client) });
    const req = upgradeConsent('app-1', '1.2.0', 'https://app.test/back');
    await act(async () => {
      await result.current.request(req);
    });
    expect(client.consent.pageCreate).toHaveBeenCalledWith({ body: req });
    expect(result.current.code).toBe('ABC-123');
    await act(async () => {
      await result.current.check('req-1');
    });
    expect(client.consent.pageResult).toHaveBeenCalledWith({ request: 'req-1' });
    expect(result.current.result?.status).toBe('approved');
  });
});
