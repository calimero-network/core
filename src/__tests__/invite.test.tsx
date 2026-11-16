import type React from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ORG_CAPABILITIES } from '@calimero-network/mero-js';
import { AcceptInviteDialog, InviteDialog, type InviteDialogProps } from '../invite';
import { fakeClient, wrapper } from './fake';

const NODE = 'https://node.test';
const BOUND = { accountId: 'acct-1', app: 'app-1' };
const UNBOUND = { accountId: 'acct-1' };
const C = ORG_CAPABILITIES;
const MEMBER_ORG = { members: { 'acct-1': 'member' }, capabilities: { 'acct-1': C.CREATE_APP | C.INVITE_MEMBERS | C.JOIN_OPEN_TEAMS }, defaultCapabilities: C.JOIN_OPEN_TEAMS };
const ADMIN_ORG = { members: { 'acct-1': 'admin' }, capabilities: {}, defaultCapabilities: C.JOIN_OPEN_TEAMS };
const created = { requestId: 'req-1', kind: 'createInvite' as const, decidedAt: 1 };
const approved = { ...created, status: 'approved' as const, outcome: { token: 'tok-9' } };
const declined = { ...created, status: 'declined' as const };
const INTENT_KEY = 'mero-react:invite-intent';
const ACCEPT_KEY = 'mero-react:accept-invite-intent';

let decide: (r: unknown) => void = () => undefined;
function popupWindow() {
  const popup = { closed: false, close: vi.fn(() => void (popup.closed = true)), focus: vi.fn(), location: { href: '' } };
  return popup;
}
let popup: ReturnType<typeof popupWindow>;
let open: ReturnType<typeof vi.fn>;

function setup(opts: { me?: unknown; org?: unknown | Error; props?: Partial<InviteDialogProps>; overrides?: Record<string, unknown> } = {}) {
  const { me = UNBOUND, org = MEMBER_ORG, props = {}, overrides = {} } = opts;
  const client = fakeClient(
    {
      waitForConsent: vi.fn(
        (_id: string, o: { signal: AbortSignal }) =>
          new Promise((resolve) => {
            decide = resolve;
            o.signal.addEventListener('abort', () => resolve({ ...created, status: 'pending', decidedAt: 0 }));
          }),
      ),
      ...overrides,
    },
    {
      auth: { me: vi.fn(async () => me) },
      orgs: {
        get: vi.fn(async () => (org instanceof Error ? Promise.reject(org) : org)),
        inviteCreate: vi.fn(async () => ({ token: 'tok-1' })),
      },
    },
  );
  client.consent.pageCreate.mockResolvedValue({ requestId: 'req-1', url: `${NODE}/consent/req-1`, expiresAt: 9e9, code: 'ABC-123' });
  client.openConsentPage.mockImplementation(async (req: unknown, how?: unknown) => {
    const started = await client.consent.pageCreate({ body: req });
    const options = (typeof how === 'function' ? { navigate: how } : (how ?? {})) as Record<string, (...a: unknown[]) => unknown>;
    if (options.confirm && !(await options.confirm(started))) return started;
    options.navigate?.((started as { url: string }).url);
    return started;
  });
  const onInvited = vi.fn();
  const onClose = vi.fn();
  const view = render(<InviteDialog open org="org-1" teams={['team-1']} appName="Mero Chat" onInvited={onInvited} onClose={onClose} {...props} />, {
    wrapper: wrapper(client),
  });
  return { client, onInvited, onClose, ...view };
}

const create = () => screen.getByRole('button', { name: 'Create invite' }) as HTMLButtonElement;
const ready = () => waitFor(() => expect(create().disabled).toBe(false));
const body = (client: ReturnType<typeof fakeClient>) =>
  (client.orgs.inviteCreate.mock.calls[0] as unknown[])[0] as { org: string; role: string; teams?: string[]; capabilities?: number; uses?: number; ttlSecs?: number; allApps?: boolean };
const subject = (client: ReturnType<typeof fakeClient>) =>
  ((client.consent.pageCreate.mock.calls[0] as unknown[])[0] as { body: { kind: string; subject: { org: string; role: string; teams?: string[]; capabilities?: number }; returnTo: string } }).body;
const custom = () => fireEvent.click(screen.getByRole('radio', { name: /Custom/ }));

beforeEach(() => {
  popup = popupWindow();
  open = vi.fn(() => popup);
  window.open = open as unknown as typeof window.open;
  Object.defineProperty(window, 'innerWidth', { value: 1024, configurable: true });
  Object.assign(navigator, { clipboard: { writeText: vi.fn(async () => undefined) } });
  sessionStorage.clear();
  decide = () => undefined;
});
afterEach(() => cleanup());

describe('permissions step', () => {
  it('offers the workspace defaults first, in plain words', async () => {
    setup();
    expect((screen.getByRole('radio', { name: /Default/ }) as HTMLInputElement).checked).toBe(true);
    expect(await screen.findByText(/Join open teams/)).toBeTruthy();
    expect(screen.queryByRole('checkbox')).toBeNull();
  });

  it('lists every capability under Custom, enabling only the ones the inviter holds', async () => {
    setup();
    await ready();
    custom();
    const held = ['Add apps', 'Invite people', 'Join open teams'];
    for (const name of held) expect((screen.getByRole('checkbox', { name }) as HTMLInputElement).disabled).toBe(false);
    const child = screen.getByRole('checkbox', { name: 'Create child teams' }) as HTMLInputElement;
    expect(child.disabled).toBe(true);
    expect(child.checked).toBe(false);
    expect(within(child.closest('.mero-cw-cap') as HTMLElement).getByText("You don't have this")).toBeTruthy();
  });

  it('starts Custom from the workspace defaults the inviter holds', async () => {
    setup();
    await ready();
    custom();
    expect((screen.getByRole('checkbox', { name: 'Join open teams' }) as HTMLInputElement).checked).toBe(true);
    expect((screen.getByRole('checkbox', { name: 'Add apps' }) as HTMLInputElement).checked).toBe(false);
  });

  it('lets an admin grant everything', async () => {
    setup({ org: ADMIN_ORG });
    await ready();
    custom();
    for (const box of screen.getAllByRole('checkbox')) expect((box as HTMLInputElement).disabled).toBe(false);
  });

  it('offers only the default when the workspace cannot be read', async () => {
    setup({ org: new Error('boom') });
    await screen.findByText(/could not read/i);
    expect((screen.getByRole('radio', { name: /Custom/ }) as HTMLInputElement).disabled).toBe(true);
    await ready();
  });
});

describe('an unbound session creates the invite itself', () => {
  it('sends a default invite with no capabilities, then shows the link', async () => {
    const { client, onInvited } = setup();
    await ready();
    fireEvent.click(create());
    const field = (await screen.findByLabelText('Invite link')) as HTMLInputElement;
    expect(body(client)).toEqual({ org: 'org-1', role: 'member', teams: ['team-1'] });
    expect('capabilities' in body(client)).toBe(false);
    expect(field.value).toContain('tok-1');
    expect(onInvited).toHaveBeenCalledWith({ token: 'tok-1', link: field.value });
    expect(open).not.toHaveBeenCalled();
  });

  it('sends the chosen bits for a custom invite', async () => {
    const { client, onInvited } = setup();
    await ready();
    custom();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Add apps' }));
    fireEvent.click(create());
    await screen.findByLabelText('Invite link');
    expect(body(client).capabilities).toBe(C.CREATE_APP | C.JOIN_OPEN_TEAMS);
    expect(onInvited).toHaveBeenCalledWith(expect.objectContaining({ capabilities: C.CREATE_APP | C.JOIN_OPEN_TEAMS }));
  });

  it('says who the link is for and what they can do, and copies it', async () => {
    setup();
    await ready();
    custom();
    fireEvent.click(create());
    const field = (await screen.findByLabelText('Invite link')) as HTMLInputElement;
    expect(screen.getByText(/join as a member/i)).toBeTruthy();
    expect(screen.getByText(/Join open teams/)).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Copy link' }));
    await waitFor(() => expect(navigator.clipboard.writeText).toHaveBeenCalledWith(field.value));
    expect(await screen.findByText('Copied')).toBeTruthy();
  });

  it('shows the node message when it refuses, keeping the choices', async () => {
    const { client, onInvited } = setup();
    client.orgs.inviteCreate.mockRejectedValue(Object.assign(new Error('403 FORBIDDEN: you cannot grant bits you do not hold'), { code: 'FORBIDDEN' }));
    await ready();
    custom();
    fireEvent.click(create());
    expect((await screen.findByRole('alert')).textContent).toContain('you cannot grant bits you do not hold');
    expect((screen.getByRole('radio', { name: /Custom/ }) as HTMLInputElement).checked).toBe(true);
    expect(onInvited).not.toHaveBeenCalled();
    await waitFor(() => expect(create().disabled).toBe(false));
  });

  it('explains FEATURE_NOT_ACTIVE and falls back to a default invite', async () => {
    const { client } = setup();
    client.orgs.inviteCreate.mockRejectedValueOnce(Object.assign(new Error('409 FEATURE_NOT_ACTIVE: not yet'), { code: 'FEATURE_NOT_ACTIVE' }));
    await ready();
    custom();
    fireEvent.click(create());
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain("doesn't support custom permissions yet");
    fireEvent.click(within(alert).getByRole('button', { name: 'Send a default invite' }));
    expect((screen.getByRole('radio', { name: /Default/ }) as HTMLInputElement).checked).toBe(true);
    expect(screen.queryByRole('alert')).toBeNull();
    await waitFor(() => expect(create().disabled).toBe(false));
    fireEvent.click(create());
    await screen.findByLabelText('Invite link');
    expect('capabilities' in (client.orgs.inviteCreate.mock.calls[1]![0] as object)).toBe(false);
  });
});

describe('share step details', () => {
  it('names the inviter and adds the note', async () => {
    setup({ props: { inviterName: 'Ada', labels: { note: 'This invite gives Mero Chat only.' } } });
    await ready();
    fireEvent.click(create());
    await screen.findByLabelText('Invite link');
    expect(screen.getByText(/Ada invites you to Mero Chat/)).toBeTruthy();
    expect(screen.getByText('This invite gives Mero Chat only.')).toBeTruthy();
  });

  it('shows neither without them', async () => {
    setup();
    await ready();
    fireEvent.click(create());
    await screen.findByLabelText('Invite link');
    expect(screen.queryByText(/invites you/)).toBeNull();
  });

  it('words the permissions and share steps with the noun', async () => {
    setup({ props: { labels: { noun: 'table' } } });
    expect(await screen.findByText("They get the table's member defaults: Join open teams.")).toBeTruthy();
    await ready();
    fireEvent.click(create());
    await screen.findByLabelText('Invite link');
    expect(screen.getByText(/They get the table's member defaults\./)).toBeTruthy();
    expect(screen.queryByText(/workspace/i)).toBeNull();
  });

  it('keeps the default wording without a noun', async () => {
    setup();
    expect(await screen.findByText("They get the workspace's member defaults: Join open teams.")).toBeTruthy();
  });
});

describe('the link base survives a full-page approval', () => {
  const back = () => window.history.replaceState({}, '', '/?request_id=req-1&result=approved');
  afterEach(() => window.history.replaceState({}, '', '/'));

  it('keeps linkBase in the intent for the trip', async () => {
    setup({ me: BOUND, props: { linkBase: 'https://app.test/o/org-1' } });
    open.mockReturnValue(null);
    await ready();
    fireEvent.click(create());
    await screen.findByText('ABC-123');
    expect(JSON.parse(sessionStorage.getItem(INTENT_KEY)!).linkBase).toBe('https://app.test/o/org-1');
  });

  it('builds the shared link from the kept base, not this page', async () => {
    sessionStorage.setItem(INTENT_KEY, JSON.stringify({ role: 'member', linkBase: 'https://app.test/o/org-1' }));
    back();
    setup({ props: { open: false }, overrides: { resumeConsent: vi.fn(async () => approved) } });
    const field = (await screen.findByLabelText('Invite link')) as HTMLInputElement;
    expect(field.value.startsWith('https://app.test/o/org-1')).toBe(true);
    expect(field.value).toContain('tok-9');
  });
});

describe('invite terms', () => {
  it('passes uses, ttlSecs and allApps to the plain route', async () => {
    const { client } = setup({ props: { uses: 5, ttlSecs: 3600, allApps: true } });
    await ready();
    fireEvent.click(create());
    await screen.findByLabelText('Invite link');
    expect(body(client)).toMatchObject({ uses: 5, ttlSecs: 3600, allApps: true });
  });

  it('passes them in the consent subject', async () => {
    const { client } = setup({ me: BOUND, props: { uses: 1, ttlSecs: 60, allApps: true } });
    await ready();
    fireEvent.click(create());
    await screen.findByText('ABC-123');
    expect(subject(client).subject).toMatchObject({ uses: 1, ttlSecs: 60, allApps: true });
  });

  it('leaves them out when not given', async () => {
    const { client } = setup();
    await ready();
    fireEvent.click(create());
    await screen.findByLabelText('Invite link');
    for (const key of ['uses', 'ttlSecs', 'allApps']) expect(key in body(client)).toBe(false);
  });

  it('offers an expiry choice that sets ttlSecs', async () => {
    const { client } = setup({ props: { expiryOptions: [3600, 86400, 604800] } });
    await ready();
    const select = screen.getByLabelText('Expires after') as HTMLSelectElement;
    expect([...select.options].map((o) => o.text)).toEqual(['1 hour', '1 day', '7 days']);
    expect(select.value).toBe('3600');
    fireEvent.change(select, { target: { value: '604800' } });
    fireEvent.click(create());
    await screen.findByLabelText('Invite link');
    expect(body(client).ttlSecs).toBe(604800);
  });

  it('starts the expiry choice at ttlSecs when it is one of the options', async () => {
    setup({ props: { expiryOptions: [3600, 86400], ttlSecs: 86400 } });
    expect((screen.getByLabelText('Expires after') as HTMLSelectElement).value).toBe('86400');
  });

  it('shows no expiry choice without options', () => {
    setup();
    expect(screen.queryByLabelText('Expires after')).toBeNull();
  });
});

describe('permissions: false', () => {
  it('skips the picker and sends a default invite', async () => {
    const { client } = setup({ props: { permissions: false } });
    await ready();
    expect(screen.queryByRole('radio')).toBeNull();
    fireEvent.click(create());
    await screen.findByLabelText('Invite link');
    expect('capabilities' in body(client)).toBe(false);
  });
});

describe('reading the org', () => {
  it('does not read it while closed or with no org', async () => {
    const closed = setup({ props: { open: false } });
    await act(async () => undefined);
    expect(closed.client.orgs.get).not.toHaveBeenCalled();
    cleanup();
    const empty = setup({ props: { org: '' } });
    await act(async () => undefined);
    expect(empty.client.orgs.get).not.toHaveBeenCalled();
  });

  it('reads it once opened', async () => {
    const { client, rerender } = setup({ props: { open: false } });
    await act(async () => undefined);
    expect(client.orgs.get).not.toHaveBeenCalled();
    rerender(<InviteDialog open org="org-1" onInvited={vi.fn()} onClose={vi.fn()} />);
    await waitFor(() => expect(client.orgs.get).toHaveBeenCalledTimes(1));
  });
});

describe('share step slot', () => {
  it('renders shareExtra with the link under it', async () => {
    setup({ props: { shareExtra: (link: string) => <div data-testid="qr">{link}</div> } });
    await ready();
    fireEvent.click(create());
    const field = (await screen.findByLabelText('Invite link')) as HTMLInputElement;
    expect(screen.getByTestId('qr').textContent).toBe(field.value);
  });
});

describe('refusals the node names by code', () => {
  const refuse = (code: string) => Object.assign(new Error(`403 ${code}: node words`), { code });

  it('says you cannot give a permission you do not have, keeping the choices', async () => {
    const { client } = setup();
    client.orgs.inviteCreate.mockRejectedValue(refuse('CANNOT_GRANT_UNHELD'));
    await ready();
    custom();
    fireEvent.click(create());
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain("You can't give a permission you don't have");
    expect(alert.textContent).not.toContain('node words');
    expect(within(alert).queryByRole('button', { name: 'Send a default invite' })).toBeNull();
    expect((screen.getByRole('radio', { name: /Custom/ }) as HTMLInputElement).checked).toBe(true);
  });

  it('says a permission is unknown to the cluster and offers a default invite', async () => {
    const { client } = setup({ me: BOUND });
    client.consent.pageCreate.mockRejectedValue(refuse('UNKNOWN_CAPABILITIES'));
    await ready();
    custom();
    fireEvent.click(create());
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('does not know one of the permissions');
    fireEvent.click(within(alert).getByRole('button', { name: 'Send a default invite' }));
    expect((screen.getByRole('radio', { name: /Default/ }) as HTMLInputElement).checked).toBe(true);
    expect(screen.queryByRole('alert')).toBeNull();
  });
});

describe('a session bound to the app asks the node', () => {
  it('opens the window in the click, asks for a default invite and shows the link when approved', async () => {
    const { client, onInvited } = setup({ me: BOUND });
    await ready();
    fireEvent.click(create());
    expect(open).toHaveBeenCalledTimes(1);
    await screen.findByText('ABC-123');
    const sent = subject(client);
    expect(sent.kind).toBe('createInvite');
    expect(sent.subject).toEqual({ org: 'org-1', role: 'member', teams: ['team-1'] });
    expect(client.orgs.inviteCreate).not.toHaveBeenCalled();
    expect(popup.location.href).toBe(`${NODE}/consent/req-1`);
    await act(async () => decide(approved));
    const field = (await screen.findByLabelText('Invite link')) as HTMLInputElement;
    expect(field.value).toContain('tok-9');
    expect(onInvited).toHaveBeenCalledWith({ token: 'tok-9', link: field.value });
  });

  it('puts the chosen bits in the consent request', async () => {
    const { client } = setup({ me: BOUND });
    await ready();
    custom();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Invite people' }));
    fireEvent.click(create());
    await screen.findByText('ABC-123');
    expect(subject(client).subject.capabilities).toBe(C.INVITE_MEMBERS | C.JOIN_OPEN_TEAMS);
  });

  it('goes back to the permissions with a message when declined', async () => {
    const { onInvited } = setup({ me: BOUND });
    await ready();
    fireEvent.click(create());
    await screen.findByText('ABC-123');
    await act(async () => decide(declined));
    expect((await screen.findByRole('alert')).textContent).toContain('declined');
    expect(onInvited).not.toHaveBeenCalled();
  });

  it('says so when the approval carries no token', async () => {
    setup({ me: BOUND });
    await ready();
    fireEvent.click(create());
    await screen.findByText('ABC-123');
    await act(async () => decide({ ...approved, outcome: undefined }));
    expect((await screen.findByRole('alert')).textContent).toContain('no invite');
  });

  it('explains FEATURE_NOT_ACTIVE when the node refuses the request itself', async () => {
    const { client } = setup({ me: BOUND });
    client.consent.pageCreate.mockRejectedValue(Object.assign(new Error('409 FEATURE_NOT_ACTIVE: not yet'), { code: 'FEATURE_NOT_ACTIVE' }));
    await ready();
    custom();
    fireEvent.click(create());
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain("doesn't support custom permissions yet");
    expect(popup.close).toHaveBeenCalled();
  });

  it('shows the node message for any other refusal', async () => {
    const { client } = setup({ me: BOUND });
    client.consent.pageCreate.mockRejectedValue(Object.assign(new Error('403 FORBIDDEN: nope'), { code: 'FORBIDDEN' }));
    await ready();
    fireEvent.click(create());
    expect((await screen.findByRole('alert')).textContent).toContain('403 FORBIDDEN: nope');
  });

  it('falls back to the full page with the code when the window is blocked, keeping the choices', async () => {
    const { client } = setup({ me: BOUND });
    open.mockReturnValue(null);
    await ready();
    custom();
    fireEvent.click(create());
    await screen.findByText('ABC-123');
    expect(JSON.parse(sessionStorage.getItem(INTENT_KEY)!)).toMatchObject({ role: 'member' });
    expect(screen.getByRole('button', { name: 'Continue to your node' })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(sessionStorage.getItem(INTENT_KEY)).toBeNull());
    expect(client.openConsentPage).toHaveBeenCalledTimes(1);
  });
});

describe('resuming an invite after the full-page return', () => {
  const back = () => window.history.replaceState({}, '', '/?request_id=req-1&result=approved');
  afterEach(() => window.history.replaceState({}, '', '/'));

  it('shows the link even though the dialog is closed, and tells the app', async () => {
    sessionStorage.setItem(INTENT_KEY, JSON.stringify({ role: 'member', capabilities: C.CREATE_APP }));
    back();
    const { onInvited } = setup({ props: { open: false }, overrides: { resumeConsent: vi.fn(async () => approved) } });
    const field = (await screen.findByLabelText('Invite link')) as HTMLInputElement;
    expect(field.value).toContain('tok-9');
    expect(onInvited).toHaveBeenCalledWith({ token: 'tok-9', link: field.value, capabilities: C.CREATE_APP });
  });

  it('says when it was declined', async () => {
    sessionStorage.setItem(INTENT_KEY, JSON.stringify({ role: 'member' }));
    back();
    setup({ props: { open: false }, overrides: { resumeConsent: vi.fn(async () => declined) } });
    expect((await screen.findByRole('alert')).textContent).toContain('declined');
  });
});

describe('AcceptInviteDialog', () => {
  const acceptCreated = { requestId: 'req-1', kind: 'acceptInvite' as const, decidedAt: 1 };
  const accepted = { ...acceptCreated, status: 'approved' as const, outcome: { org: 'org-7' } };
  const join = () => screen.getByRole('button', { name: 'Join' }) as HTMLButtonElement;
  function acceptSetup(me: unknown = UNBOUND, overrides: Record<string, unknown> = {}, props: Partial<React.ComponentProps<typeof AcceptInviteDialog>> = {}) {
    const client = fakeClient(
      {
        waitForConsent: vi.fn(
          (_id: string, o: { signal: AbortSignal }) =>
            new Promise((resolve) => {
              decide = resolve;
              o.signal.addEventListener('abort', () => resolve({ ...acceptCreated, status: 'pending', decidedAt: 0 }));
            }),
        ),
        ...overrides,
      },
      { auth: { me: vi.fn(async () => me) }, orgs: { inviteRedeem: vi.fn(async () => ({ org: 'org-1' })) } },
    );
    client.consent.pageCreate.mockResolvedValue({ requestId: 'req-1', url: `${NODE}/consent/req-1`, expiresAt: 9e9, code: 'ABC-123' });
    client.openConsentPage.mockImplementation(async (req: unknown, how?: unknown) => {
      const started = await client.consent.pageCreate({ body: req });
      const options = (typeof how === 'function' ? { navigate: how } : (how ?? {})) as Record<string, (...a: unknown[]) => unknown>;
      if (options.confirm && !(await options.confirm(started))) return started;
      return started;
    });
    const onJoined = vi.fn();
    render(<AcceptInviteDialog open token="tok-5" appName="Mero Chat" onJoined={onJoined} onClose={vi.fn()} {...props} />, { wrapper: wrapper(client) });
    return { client, onJoined };
  }

  it('redeems directly on an unbound session', async () => {
    const { client, onJoined } = acceptSetup();
    await waitFor(() => expect(join().disabled).toBe(false));
    fireEvent.click(join());
    await waitFor(() => expect(onJoined).toHaveBeenCalledWith({ org: 'org-1', alreadyMember: false }));
    expect(client.orgs.inviteRedeem).toHaveBeenCalledWith({ token: 'tok-5' });
    expect(open).not.toHaveBeenCalled();
  });

  it('asks the node on a bound session and reads the joined org from its result', async () => {
    const { client, onJoined } = acceptSetup(BOUND);
    await waitFor(() => expect(join().disabled).toBe(false));
    fireEvent.click(join());
    expect(open).toHaveBeenCalledTimes(1);
    await screen.findByText('ABC-123');
    const sent = (client.consent.pageCreate.mock.calls[0]![0] as { body: { kind: string; subject: { token: string } } }).body;
    expect(sent).toMatchObject({ kind: 'acceptInvite', subject: { token: 'tok-5' } });
    await act(async () => decide(accepted));
    await waitFor(() => expect(onJoined).toHaveBeenCalledWith({ org: 'org-7', alreadyMember: false }));
    expect(client.orgs.inviteRedeem).not.toHaveBeenCalled();
  });

  it('says so when declined', async () => {
    const { onJoined } = acceptSetup(BOUND);
    await waitFor(() => expect(join().disabled).toBe(false));
    fireEvent.click(join());
    await screen.findByText('ABC-123');
    await act(async () => decide({ ...accepted, status: 'declined' }));
    expect((await screen.findByRole('alert')).textContent).toContain('declined');
    expect(onJoined).not.toHaveBeenCalled();
  });

  it('shows the node message when the invite is refused', async () => {
    const { client, onJoined } = acceptSetup();
    client.orgs.inviteRedeem.mockRejectedValue(new Error('404 NOT_FOUND: this invite has expired'));
    await waitFor(() => expect(join().disabled).toBe(false));
    fireEvent.click(join());
    expect((await screen.findByRole('alert')).textContent).toContain('this invite has expired');
    expect(onJoined).not.toHaveBeenCalled();
  });

  it('says so when the approval names no workspace', async () => {
    acceptSetup(BOUND);
    await waitFor(() => expect(join().disabled).toBe(false));
    fireEvent.click(join());
    await screen.findByText('ABC-123');
    await act(async () => decide({ ...accepted, outcome: undefined }));
    expect((await screen.findByRole('alert')).textContent).toContain('no workspace');
  });

  it('does not spend the invite when the account is already in the named org', async () => {
    const me = { ...UNBOUND, orgs: [{ orgId: 'org-1', name: "Ada's game" }] };
    const { client, onJoined } = acceptSetup(me, {}, { org: 'org-1' });
    expect(await screen.findByText("You're already in Ada's game")).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Join' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    expect(onJoined).toHaveBeenCalledWith({ org: 'org-1', alreadyMember: true });
    expect(client.orgs.inviteRedeem).not.toHaveBeenCalled();
    expect(client.consent.pageCreate).not.toHaveBeenCalled();
    expect(open).not.toHaveBeenCalled();
  });

  it('offers Join once it knows the account is not in the org', async () => {
    const me = { ...UNBOUND, orgs: [{ orgId: 'other', name: 'Other' }] };
    const { client, onJoined } = acceptSetup(me, {}, { org: 'org-1' });
    await waitFor(() => expect(join().disabled).toBe(false));
    fireEvent.click(join());
    await waitFor(() => expect(onJoined).toHaveBeenCalledWith({ org: 'org-1', alreadyMember: false }));
    expect(client.orgs.inviteRedeem).toHaveBeenCalled();
  });

  it('keeps Join waiting while membership is being read, and asks normally when it cannot be read', async () => {
    const answers: Array<(v: unknown) => void> = [];
    const client = fakeClient({}, { auth: { me: vi.fn(() => new Promise((resolve) => answers.push(resolve))) }, orgs: { inviteRedeem: vi.fn(async () => ({ org: 'org-1' })) } });
    render(<AcceptInviteDialog open token="t" org="org-1" onJoined={vi.fn()} onClose={vi.fn()} />, { wrapper: wrapper(client) });
    expect(join().disabled).toBe(true);
    await act(async () => answers.forEach((a) => a({ accountId: 'a' })));
    await waitFor(() => expect(join().disabled).toBe(false));
    cleanup();
    const broken = fakeClient({}, { auth: { me: vi.fn(async () => Promise.reject(new Error('x'))) } });
    render(<AcceptInviteDialog open token="t" org="org-1" onJoined={vi.fn()} onClose={vi.fn()} />, { wrapper: wrapper(broken) });
    await waitFor(() => expect(join().disabled).toBe(false));
  });

  it('takes its wording from labels, defaults unchanged', async () => {
    acceptSetup(UNBOUND, {}, { labels: { title: "Join Ada's game", body: 'Ada is waiting.', join: 'Play' } });
    expect(screen.getByRole('heading', { name: "Join Ada's game" })).toBeTruthy();
    expect(screen.getByText('Ada is waiting.')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Play' })).toBeTruthy();
    cleanup();
    acceptSetup();
    expect(screen.getByRole('heading', { name: 'Join workspace' })).toBeTruthy();
    expect(screen.getByText('You were invited to a workspace in Mero Chat. Join it with your account.')).toBeTruthy();
  });

  it('words the dialog with the noun', async () => {
    acceptSetup(UNBOUND, {}, { labels: { noun: 'arena' } });
    expect(screen.getByRole('heading', { name: 'Join arena' })).toBeTruthy();
    expect(screen.getByText('You were invited to an arena in Mero Chat. Join it with your account.')).toBeTruthy();
  });

  it('reports a refusal to onError, with the error itself', async () => {
    const onError = vi.fn();
    const { client } = acceptSetup(UNBOUND, {}, { onError });
    const refusal = Object.assign(new Error('400 REJECTED: this invite has expired'), { code: 'REJECTED' });
    client.orgs.inviteRedeem.mockRejectedValue(refusal);
    await waitFor(() => expect(join().disabled).toBe(false));
    fireEvent.click(join());
    await screen.findByRole('alert');
    expect(onError).toHaveBeenCalledWith(refusal);
  });

  it('does not call onError for a declined approval', async () => {
    const onError = vi.fn();
    acceptSetup(BOUND, {}, { onError });
    await waitFor(() => expect(join().disabled).toBe(false));
    fireEvent.click(join());
    await screen.findByText('ABC-123');
    await act(async () => decide({ ...accepted, status: 'declined' }));
    await screen.findByRole('alert');
    expect(onError).not.toHaveBeenCalled();
  });

  it('finishes after a full-page return while closed', async () => {
    sessionStorage.setItem(ACCEPT_KEY, JSON.stringify({}));
    window.history.replaceState({}, '', '/?request_id=req-1&result=approved');
    const { onJoined } = acceptSetup(UNBOUND, { resumeConsent: vi.fn(async () => accepted) }, { open: false });
    await waitFor(() => expect(onJoined).toHaveBeenCalledWith({ org: 'org-7', alreadyMember: false }));
    window.history.replaceState({}, '', '/');
  });
});
