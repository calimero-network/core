import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { UpdateBanner } from '../updates';
import { fakeClient, wrapper } from './fake';

function updateView(over: Record<string, unknown> = {}) {
  return {
    app: 'app-1',
    running: { package: 'com.acme.notes', version: '1.3.0' },
    available: {
      package: 'com.acme.notes',
      version: '1.4.0',
      source: 'deployment',
      class: 'green',
      summary: 'Nothing changes where your data goes',
      approvalsNeeded: 1,
    },
    mayApprove: true,
    approvers: 'owner-and-admins',
    unit: false,
    policy: { autoApply: 'never', holdHours: 24, secondApprover: false, source: 'default' },
    pendingApprovals: [],
    ...over,
  };
}

function client(view: Record<string, unknown>) {
  return fakeClient({}, { apps: { update: vi.fn(async () => view) } });
}

afterEach(() => vi.unstubAllGlobals());

describe('UpdateBanner', () => {
  it("shows the update's class and the node's line, and nothing without one", async () => {
    const c = client(updateView());
    const { container, unmount } = render(<UpdateBanner />, { wrapper: wrapper(c) });
    await screen.findByText('Update 1.4.0 is ready');
    expect(screen.getByText('Green')).toBeTruthy();
    expect(screen.getByText(/Nothing changes where your data goes/)).toBeTruthy();
    expect(container.querySelector('.mero-update-banner--green')).toBeTruthy();
    expect(c.apps.update).toHaveBeenCalledWith({ app: 'app-1' });
    unmount();

    const none = client(updateView({ available: null }));
    const { container: empty } = render(<UpdateBanner app="app-2" />, { wrapper: wrapper(none) });
    await waitFor(() => expect(none.apps.update).toHaveBeenCalledWith({ app: 'app-2' }));
    expect(empty.querySelector('.mero-update-banner')).toBeNull();
  });

  it('tells a member who may not approve who does, with no button', async () => {
    const c = client(
      updateView({
        mayApprove: false,
        available: { ...updateView().available, class: 'red', summary: 'This update can send data to a new place: api.acme.com' },
      }),
    );
    render(<UpdateBanner />, { wrapper: wrapper(c) });
    await screen.findByText('An owner or admin approves updates');
    expect(screen.getByText('Red')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Review' })).toBeNull();
  });

  it("opens the node's update sheet in a pop-up and reads the update again", async () => {
    const win = { location: { href: '' }, closed: false, close: vi.fn() };
    const open = vi.fn(() => win);
    vi.stubGlobal('open', open);
    const c = client(updateView({ unit: true }));
    const onReviewed = vi.fn();
    render(<UpdateBanner onReviewed={onReviewed} />, { wrapper: wrapper(c) });
    const button = await screen.findByRole('button', { name: 'Review' });
    await act(async () => {
      fireEvent.click(button);
    });
    await waitFor(() => expect(onReviewed).toHaveBeenCalled());
    expect(open).toHaveBeenCalledWith('', 'mero-consent', expect.stringContaining('popup'));
    expect(c.openConsentPage).toHaveBeenCalledWith(
      expect.objectContaining({ kind: 'upgradeUnit', subject: { app: 'app-1', toVersion: '1.4.0' } }),
      expect.anything(),
    );
    expect(win.location.href).toBe('https://node.test/consent/req-1');
    expect(c.waitForConsent).toHaveBeenCalledWith('req-1', expect.objectContaining({ expiresAt: 9e9 }));
    expect(c.apps.update).toHaveBeenCalledTimes(2);
    expect(win.close).toHaveBeenCalled();
  });
});
