/**
 * `client.updates`: an instance's update as the node answers it, the banner's state, the
 * first approval and the node's pages (update sheet, updates page, policy change, install
 * sheet), opened in a pop-up or the window given. A stand-in host; synthetic data only.
 */

import { describe, expect, it, vi } from 'vitest';

import type { ConsentRequest, OpenConsentOptions } from './approvals.js';
import {
  Updates,
  installConsent,
  updateBanner,
  updatePolicyConsent,
  updateSheetConsent,
  updatesPageConsent,
  type UpdateView,
  type UpdatesHost,
} from './updates.js';

const BACK = 'https://notes.example/app/';

function view(over: Partial<UpdateView> = {}): UpdateView {
  return {
    app: 'app-1',
    running: { package: 'com.acme.notes', version: '1.4.0' },
    available: {
      package: 'com.acme.notes',
      version: '1.5.0',
      source: 'deployment',
      class: 'red',
      summary: 'This update can send data to a new place: api.acme.com',
      approvalsNeeded: 1,
    },
    mayApprove: true,
    approvers: 'owner-and-admins',
    unit: false,
    policy: { autoApply: 'never', holdHours: 24, secondApprover: false, source: 'default' },
    pendingApprovals: [],
    ...over,
  } as UpdateView;
}

function host(v: UpdateView = view()) {
  const opened: { request: ConsentRequest; options: OpenConsentOptions }[] = [];
  const h = {
    apps: {
      update: vi.fn(async () => v),
      approveUpgrade: vi.fn(async (args: { app: string; version: string; unit?: boolean }) => ({ app: args.app, approval: {} })),
      policy: vi.fn(async ({ app }: { app: string }) => ({ app, org: 'o', policy: {}, source: 'default' })),
    },
    orgs: { policy: vi.fn(async ({ org }: { org: string }) => ({ org, policy: {}, source: 'org' })) },
    openConsentPage: vi.fn(async (request: ConsentRequest, options: OpenConsentOptions) => {
      opened.push({ request, options });
      options.navigate?.('https://node.example/consent/1.abc');
      return { requestId: '1.abc', url: 'https://node.example/consent/1.abc', code: 'ABC-DEF', expiresAt: 9 };
    }),
  };
  return { h: h as unknown as UpdatesHost & typeof h, opened };
}

/** A window the page opens in, recording where it was sent. */
function fakeWindow() {
  return { location: { href: '' }, close: vi.fn(), closed: false } as unknown as Window & { close: ReturnType<typeof vi.fn> };
}

describe('update requests', () => {
  it('builds each page the node serves', () => {
    expect(updateSheetConsent('app-1', view(), BACK)).toEqual({
      kind: 'upgrade',
      subject: { app: 'app-1', toVersion: '1.5.0' },
      returnTo: BACK,
    });
    expect(updateSheetConsent('app-1', view({ unit: true }), BACK)?.kind).toBe('upgradeUnit');
    expect(updateSheetConsent('app-1', view({ available: null }), BACK)).toBeNull();
    expect(updatePolicyConsent({ org: 'o' }, { secondApprover: true }, BACK)).toEqual({
      kind: 'updatePolicy',
      subject: { org: 'o', policy: { secondApprover: true } },
      returnTo: BACK,
    });
    expect(updatePolicyConsent({ app: 'a' }, null, BACK).subject).toEqual({ app: 'a', policy: null });
    expect(updatesPageConsent(BACK)).toEqual({ kind: 'updates', subject: {}, returnTo: BACK });
    expect(updatesPageConsent(BACK, 'o').subject).toEqual({ org: 'o' });
    expect(installConsent({ package: 'p', name: 'notes', deployment: 'd' }, BACK).kind).toBe('install');
  });

  it("states a banner from the node's words", () => {
    expect(updateBanner(view())).toEqual({
      show: true,
      version: '1.5.0',
      class: 'red',
      summary: 'This update can send data to a new place: api.acme.com',
      mayApprove: true,
      approvedBy: null,
    });
    const none = updateBanner(view({ available: null, mayApprove: false }));
    expect(none.show).toBe(false);
    expect(none.mayApprove).toBe(false);
  });
});

describe('client.updates', () => {
  it('reads the update and approves the available version first', async () => {
    const { h } = host(view({ unit: true }));
    const updates = new Updates(h);
    expect((await updates.get('app-1')).available?.version).toBe('1.5.0');
    await updates.approve('app-1');
    expect(h.apps.approveUpgrade).toHaveBeenCalledWith({ app: 'app-1', version: '1.5.0', unit: true }, undefined);
    await updates.approve('app-1', { version: '1.6.0', unit: false });
    expect(h.apps.approveUpgrade).toHaveBeenLastCalledWith({ app: 'app-1', version: '1.6.0', unit: false }, undefined);
  });

  it('refuses to approve when nothing is available', async () => {
    const { h } = host(view({ available: null }));
    await expect(new Updates(h).approve('app-1')).rejects.toThrow('no update is available');
  });

  it("opens the update sheet in the window given and returns to the app's page", async () => {
    const { h, opened } = host();
    const win = fakeWindow();
    const started = await new Updates(h).openSheet('app-1', { window: win, returnTo: BACK });
    expect(opened[0].request).toEqual({ kind: 'upgrade', subject: { app: 'app-1', toVersion: '1.5.0' }, returnTo: BACK });
    expect(win.location.href).toBe('https://node.example/consent/1.abc');
    expect(started.window).toBe(win);
    expect(started.code).toBe('ABC-DEF');
  });

  it('opens a pop-up as the call starts, and closes it when there is nothing to show', async () => {
    const { h } = host(view({ available: null }));
    const win = fakeWindow();
    const open = vi.fn(() => win);
    vi.stubGlobal('window', { open });
    try {
      await expect(new Updates(h).openSheet('app-1', { returnTo: BACK })).rejects.toThrow('no update');
      expect(open).toHaveBeenCalledWith('', 'mero-consent', expect.stringContaining('popup'));
      expect(win.close).toHaveBeenCalled();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('navigates this tab when no pop-up opens', async () => {
    const { h, opened } = host();
    const navigate = vi.fn();
    const started = await new Updates(h).openSheet('app-1', { navigate, returnTo: BACK });
    expect(navigate).toHaveBeenCalledWith('https://node.example/consent/1.abc');
    expect(started.window).toBeNull();
    expect(opened).toHaveLength(1);
  });

  it('reads policies and changes them on the page', async () => {
    const { h, opened } = host();
    const updates = new Updates(h);
    await updates.policy.get({ app: 'a' });
    await updates.policy.get({ org: 'o' });
    expect(h.apps.policy).toHaveBeenCalledWith({ app: 'a' }, undefined);
    expect(h.orgs.policy).toHaveBeenCalledWith({ org: 'o' }, undefined);
    await updates.policy.set({ org: 'o' }, { autoApply: 'noDestinations', holdHours: 48 }, { window: fakeWindow(), returnTo: BACK });
    expect(opened[0].request).toEqual({
      kind: 'updatePolicy',
      subject: { org: 'o', policy: { autoApply: 'noDestinations', holdHours: 48 } },
      returnTo: BACK,
    });
  });

  it('opens the updates page and the install sheet', async () => {
    const { h, opened } = host();
    const updates = new Updates(h);
    await updates.manage({ window: fakeWindow(), returnTo: BACK, org: 'o' });
    await updates.install({ package: 'com.acme.notes', name: 'notes', deployment: 'acme' }, { window: fakeWindow(), returnTo: BACK });
    expect(opened.map((o) => o.request.kind)).toEqual(['updates', 'install']);
  });
});
