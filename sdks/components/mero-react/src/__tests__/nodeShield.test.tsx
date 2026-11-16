import { useLayoutEffect } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { verifyNodeUrl, type CalimeroClient, type NodeVerificationState } from '@calimero-network/mero-js';
import { NodeShield, type AppCodeCheck } from '../node-shield';
import { MockMeroProvider } from '../test-utils/MockMeroProvider';

const NODE = 'https://node-3.relay.example.net';
const BUNDLE = '7f3a'.padEnd(60, '0') + 'c91e';

const sealed: NodeVerificationState = {
  status: 'verified',
  url: NODE,
  mock: false,
  release: '2.3.148',
  profile: 'locked-read-only',
  checkedAt: Date.now(),
};
const mock: NodeVerificationState = { status: 'verified', url: 'http://127.0.0.1:2528', mock: true, checkedAt: Date.now() };
const refused: NodeVerificationState = { status: 'refused', url: NODE, reason: 'wrong enclave key', checkedAt: Date.now() };
const unreachable: NodeVerificationState = { status: 'unreachable', url: NODE, reason: 'network down', checkedAt: Date.now() };
const match: AppCodeCheck = { status: 'match', appHash: BUNDLE, checkedAt: Date.now() };

function fakeClient(first: NodeVerificationState) {
  let answer = first;
  const fake = {
    nodeUrl: first.url,
    verification: vi.fn(() => answer),
    verify: vi.fn(async () => answer),
  };
  return {
    client: fake as unknown as CalimeroClient,
    verify: fake.verify,
    answer: (next: NodeVerificationState) => void (answer = next),
    failOverTo: (url: string) => void (fake.nodeUrl = url),
  };
}

const OTHER = 'https://node-9.relay.example.net';
const chip = () => screen.getByTestId('node-shield');

/** Records the chip's label as committed, before any effect of that render runs. */
function LabelProbe({ seen }: { seen: string[] }) {
  useLayoutEffect(() => {
    seen.push(document.querySelector('[data-testid="node-shield"]')?.getAttribute('aria-label') ?? '');
  });
  return null;
}
const open = () => fireEvent.click(chip());

beforeEach(() => {
  Object.assign(navigator, { clipboard: { writeText: vi.fn(async () => undefined) } });
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe('NodeShield', () => {
  it.each([
    ['a sealed node is green', sealed, 'Verified node', 'verified'],
    ['a test server is grey, whatever its quote', mock, 'Test server: not sealed', 'unsealed'],
    ['a refused node is red', refused, 'Node refused', 'refused'],
    ['an unreachable node reads as checking', unreachable, 'Checking node…', 'checking'],
  ])('%s', async (_name, state, label, kind) => {
    render(<NodeShield client={fakeClient(state).client} />);
    await waitFor(() => expect(chip().getAttribute('aria-label')).toBe(label));
    expect(chip().textContent).toContain(label);
    expect(chip().getAttribute('data-kind')).toBe(kind);
  });

  it('is green only for the sealed profile on a real quote', async () => {
    render(<NodeShield client={fakeClient({ ...sealed, profile: 'debug' }).client} />);
    await waitFor(() => expect(chip().getAttribute('aria-label')).toBe('Test server: not sealed'));
  });

  it('works with an explicit client and no MeroProvider', async () => {
    const { client, verify } = fakeClient(sealed);
    render(<NodeShield client={client} />);
    await screen.findByText('Verified node');
    expect(verify).toHaveBeenCalled();
  });

  it("uses the MeroProvider's client and cluster", async () => {
    const { client } = fakeClient(sealed);
    render(
      <MockMeroProvider client={client} cluster="eu-shared-2">
        <NodeShield />
      </MockMeroProvider>,
    );
    await screen.findByText('Verified node');
    open();
    expect((await screen.findByRole('dialog')).textContent).toContain('eu-shared-2');
  });

  it('renders nothing with no client to check', () => {
    render(
      <MockMeroProvider>
        <NodeShield />
      </MockMeroProvider>,
    );
    expect(screen.queryByTestId('node-shield')).toBeNull();
    cleanup();
    render(<NodeShield />);
    expect(screen.queryByTestId('node-shield')).toBeNull();
  });

  it('shows nothing for client={null}, even inside a MeroProvider with a client', () => {
    render(
      <MockMeroProvider client={fakeClient(sealed).client}>
        <NodeShield client={null} />
      </MockMeroProvider>,
    );
    expect(screen.queryByTestId('node-shield')).toBeNull();
  });

  it("leaves the cluster out for cluster={null}, even inside a MeroProvider with one", async () => {
    render(
      <MockMeroProvider client={fakeClient(sealed).client} cluster="eu-shared-2">
        <NodeShield cluster={null} />
      </MockMeroProvider>,
    );
    await screen.findByText('Verified node');
    open();
    expect((await screen.findByRole('dialog')).textContent).not.toContain('Cluster');
  });

  it("never shows the previous client's state for a new client on the same node", async () => {
    const seen: string[] = [];
    const a = fakeClient(sealed);
    const b = fakeClient(sealed);
    b.client.verification = vi.fn(() => undefined) as never;
    b.verify.mockImplementation(() => new Promise(() => {}));
    const { rerender } = render(<><NodeShield client={a.client} /><LabelProbe seen={seen} /></>);
    await screen.findByText('Verified node');
    seen.length = 0;
    rerender(<><NodeShield client={b.client} /><LabelProbe seen={seen} /></>);
    expect(seen[0]).toBe('Checking node…');
    expect(chip().getAttribute('aria-label')).toBe('Checking node…');
  });

  it('never describes the node the client failed over from, even before the next poll', async () => {
    vi.useFakeTimers();
    const f = fakeClient(sealed);
    render(<NodeShield client={f.client} />);
    await act(async () => {});
    expect(chip().getAttribute('aria-label')).toBe('Verified node');
    f.failOverTo(OTHER);
    open();
    expect(chip().getAttribute('aria-label')).toBe('Checking node…');
    expect(screen.getByRole('dialog').textContent).toContain('node-9.relay.example.net');
  });

  it('shows the refused reason', async () => {
    render(<NodeShield client={fakeClient(refused).client} />);
    await screen.findByText('Node refused');
    open();
    const dialog = await screen.findByRole('dialog');
    expect(dialog.textContent).toContain('This app refused the node and sent nothing.');
    expect(dialog.textContent).toContain('wrong enclave key');
  });

  it('opens a dialog on click, moves focus into it, closes on Escape and returns focus to the chip', async () => {
    render(<NodeShield client={fakeClient(sealed).client} cluster="eu-shared-2" />);
    await screen.findByText('Verified node');
    expect(screen.queryByRole('dialog')).toBeNull();
    open();
    const dialog = await screen.findByRole('dialog');
    expect(dialog.textContent).toContain('You are talking to a genuine Calimero TEE node');
    expect(dialog.textContent).toContain('eu-shared-2');
    await waitFor(() => expect(document.activeElement).toBe(dialog));
    expect(chip().getAttribute('aria-expanded')).toBe('true');
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(document.activeElement).toBe(chip());
  });

  it('links the public verifier for a real node and offers no link for a test server', async () => {
    render(<NodeShield client={fakeClient(sealed).client} />);
    await screen.findByText('Verified node');
    open();
    const link = await screen.findByRole('link', { name: /Verify independently/ });
    expect(link.getAttribute('href')).toBe(verifyNodeUrl(NODE, '2.3.148'));
    expect(link.getAttribute('target')).toBe('_blank');
    expect(link.getAttribute('rel')).toBe('noopener noreferrer');
    cleanup();

    render(<NodeShield client={fakeClient(mock).client} />);
    await screen.findByText('Test server: not sealed');
    open();
    await screen.findByRole('dialog');
    expect(screen.queryByRole('link', { name: /Verify independently/ })).toBeNull();
  });

  it('copies the facts it shows', async () => {
    render(<NodeShield client={fakeClient(sealed).client} cluster="eu-shared-2" />);
    await screen.findByText('Verified node');
    open();
    fireEvent.click(await screen.findByRole('button', { name: 'Copy details' }));
    await waitFor(() => expect(navigator.clipboard.writeText).toHaveBeenCalled());
    const text = vi.mocked(navigator.clipboard.writeText).mock.calls[0]?.[0] as string;
    expect(text).toContain(`Node: ${NODE}`);
    expect(text).toContain('Cluster: eu-shared-2');
    await screen.findByRole('button', { name: 'Copied' });
  });

  it('says it could not copy when the browser has no clipboard', async () => {
    render(<NodeShield client={fakeClient(sealed).client} />);
    await screen.findByText('Verified node');
    open();
    Object.assign(navigator, { clipboard: undefined });
    fireEvent.click(await screen.findByRole('button', { name: 'Copy details' }));
    await screen.findByRole('button', { name: 'Could not copy' });
  });

  it('asks again when it is opened, and keeps retrying an unreachable node', async () => {
    vi.useFakeTimers();
    const { client, verify, answer } = fakeClient(unreachable);
    render(<NodeShield client={client} />);
    await act(async () => {});
    const first = verify.mock.calls.length;
    expect(first).toBeGreaterThan(0);

    open();
    await act(async () => {});
    expect(screen.getByRole('dialog').textContent).toContain('network down');
    const opened = verify.mock.calls.length;
    expect(opened).toBeGreaterThan(first);

    answer(sealed);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_000);
    });
    expect(verify.mock.calls.length).toBeGreaterThan(opened);
    expect(chip().textContent).toContain('Verified node');
  });

  it('checks the node again when the client fails over to another', async () => {
    vi.useFakeTimers();
    const f = fakeClient(sealed);
    f.verify.mockImplementation(async () => ({ ...sealed, url: f.client.nodeUrl }));
    render(<NodeShield client={f.client} />);
    await act(async () => {});
    open();
    await act(async () => {});
    expect(screen.getByRole('dialog').textContent).toContain('node-3.relay.example.net');

    f.failOverTo('https://node-9.relay.example.net');
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });
    const text = screen.getByRole('dialog').textContent;
    expect(text).toContain('node-9.relay.example.net');
    expect(text).not.toContain('node-3.relay.example.net');
  });

  it('does not take the app down when the client cannot be checked', async () => {
    render(<NodeShield client={{ nodeUrl: NODE } as unknown as CalimeroClient} />);
    await waitFor(() => expect(chip().textContent).toContain('Checking node…'));
    open();
    const dialog = await screen.findByRole('dialog');
    expect(dialog.textContent).toContain('checking the node before anything is sent');
  });
});

describe('the open prop', () => {
  it('opens from code like a click: placed, focused, and checking the app code', async () => {
    const appCode = vi.fn(async () => match);
    const { rerender } = render(<NodeShield client={fakeClient(sealed).client} appCode={appCode} open={false} />);
    await screen.findByText('Verified node');
    expect(screen.queryByRole('dialog')).toBeNull();
    rerender(<NodeShield client={fakeClient(sealed).client} appCode={appCode} open />);
    const dialog = await screen.findByRole('dialog');
    await waitFor(() => expect(document.activeElement).toBe(dialog));
    expect(dialog.style.visibility).not.toBe('hidden');
    expect(chip().getAttribute('aria-expanded')).toBe('true');
    await waitFor(() => expect(appCode).toHaveBeenCalledTimes(1));
  });

  it('asks the parent to close on Escape and stays open until it does, then returns focus to the chip', async () => {
    const { client } = fakeClient(sealed);
    const onOpenChange = vi.fn();
    const { rerender } = render(<NodeShield client={client} open onOpenChange={onOpenChange} />);
    const dialog = await screen.findByRole('dialog');
    await waitFor(() => expect(document.activeElement).toBe(dialog));
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(screen.getByRole('dialog')).toBe(dialog);
    rerender(<NodeShield client={client} open={false} onOpenChange={onOpenChange} />);
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(document.activeElement).toBe(chip());
  });

  it('asks the parent to open on a chip click, and to close on an outside click', async () => {
    const onOpenChange = vi.fn();
    const { rerender } = render(<NodeShield client={fakeClient(sealed).client} open={false} onOpenChange={onOpenChange} />);
    await screen.findByText('Verified node');
    open();
    expect(onOpenChange).toHaveBeenLastCalledWith(true);
    expect(screen.queryByRole('dialog')).toBeNull();
    rerender(<NodeShield client={fakeClient(sealed).client} open onOpenChange={onOpenChange} />);
    await screen.findByRole('dialog');
    fireEvent.mouseDown(document.body);
    expect(onOpenChange).toHaveBeenLastCalledWith(false);
    expect(screen.getByRole('dialog')).toBeTruthy();
  });

  it('reports its own opening and closing when uncontrolled', async () => {
    const onOpenChange = vi.fn();
    render(<NodeShield client={fakeClient(sealed).client} onOpenChange={onOpenChange} />);
    await screen.findByText('Verified node');
    open();
    expect(onOpenChange).toHaveBeenLastCalledWith(true);
    await screen.findByRole('dialog');
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onOpenChange).toHaveBeenLastCalledWith(false);
    expect(screen.queryByRole('dialog')).toBeNull();
  });
});

describe('the app code check', () => {
  it('runs only once the popover is opened on a verified node, and shows a match', async () => {
    const appCode = vi.fn(async () => match);
    render(<NodeShield client={fakeClient(sealed).client} appCode={appCode} />);
    await screen.findByText('Verified node');
    await act(async () => {});
    expect(appCode).not.toHaveBeenCalled();
    open();
    const dialog = await screen.findByRole('dialog');
    await waitFor(() => expect(dialog.textContent).toContain('Node runs the code bundle this app was built for'));
    expect(dialog.textContent).toContain('7f3a…c91e');
    expect(appCode).toHaveBeenCalledTimes(1);
  });

  it('is told which client and node it checks', async () => {
    const { client } = fakeClient(sealed);
    const appCode = vi.fn(async () => match);
    render(<NodeShield client={client} appCode={appCode} />);
    await screen.findByText('Verified node');
    open();
    await waitFor(() => expect(appCode).toHaveBeenCalledWith(client, NODE));
  });

  it('checks again with a new appCode, and hides the old answer until the new one arrives', async () => {
    const { client } = fakeClient(sealed);
    const first = vi.fn(async () => match);
    const { rerender } = render(<NodeShield client={client} appCode={first} />);
    await screen.findByText('Verified node');
    open();
    await waitFor(() => expect(screen.getByRole('dialog').textContent).toContain('App code'));
    let finish!: () => void;
    const second = vi.fn(
      () => new Promise<AppCodeCheck>((resolve) => (finish = () => resolve({ status: 'mismatch', appHash: 'ab'.repeat(32), expected: BUNDLE }))),
    );
    rerender(<NodeShield client={client} appCode={second} />);
    await waitFor(() => expect(second).toHaveBeenCalled());
    expect(screen.getByRole('dialog').textContent).not.toContain('App code');
    await act(async () => finish());
    expect(screen.getByRole('dialog').textContent).toContain("Does not match this app's release");
  });

  it('hides the code check of the node the client failed over from until the new node answers', async () => {
    vi.useFakeTimers();
    const f = fakeClient(sealed);
    f.verify.mockImplementation(async () => ({ ...sealed, url: f.client.nodeUrl }));
    let finish: (() => void) | null = null;
    const appCode = vi.fn((_client: CalimeroClient, url: string) =>
      url === NODE ? Promise.resolve(match) : new Promise<AppCodeCheck>((resolve) => (finish = () => resolve(match))),
    );
    render(<NodeShield client={f.client} appCode={appCode} />);
    await act(async () => {});
    open();
    await act(async () => {});
    expect(screen.getByRole('dialog').textContent).toContain('App code');

    f.failOverTo(OTHER);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });
    expect(appCode).toHaveBeenCalledWith(f.client, OTHER);
    expect(screen.getByRole('dialog').textContent).toContain('node-9.relay.example.net');
    expect(screen.getByRole('dialog').textContent).not.toContain('App code');

    await act(async () => finish!());
    expect(screen.getByRole('dialog').textContent).toContain('App code');
  });

  it('shows a mismatch as a problem, never as a check', async () => {
    const appCode = async (): Promise<AppCodeCheck> => ({ status: 'mismatch', appHash: 'ab'.repeat(32), expected: BUNDLE });
    render(<NodeShield client={fakeClient(sealed).client} appCode={appCode} />);
    await screen.findByText('Verified node');
    open();
    const dialog = await screen.findByRole('dialog');
    await waitFor(() => expect(dialog.textContent).toContain("Does not match this app's release"));
    expect(dialog.textContent).not.toContain('Node runs the code bundle this app was built for');
  });

  it('is never asked about a test server', async () => {
    const appCode = vi.fn(async () => match);
    render(<NodeShield client={fakeClient(mock).client} appCode={appCode} />);
    await screen.findByText('Test server: not sealed');
    open();
    await screen.findByRole('dialog');
    await act(async () => {});
    expect(appCode).not.toHaveBeenCalled();
    expect(screen.getByRole('dialog').textContent).not.toContain('App code');
  });
});

describe('colours', () => {
  it('sets the colors prop as CSS custom properties on the root', async () => {
    render(<NodeShield client={fakeClient(sealed).client} colors={{ verified: '#00aa55', surface: 'black' }} />);
    await screen.findByText('Verified node');
    const root = chip().parentElement!;
    expect(root.style.getPropertyValue('--node-shield-verified')).toBe('#00aa55');
    expect(root.style.getPropertyValue('--node-shield-surface')).toBe('black');
    expect(root.style.getPropertyValue('--node-shield-refused')).toBe('');
  });

  it('carries the colours into the popover, which renders outside the root', async () => {
    render(<NodeShield client={fakeClient(sealed).client} colors={{ text: 'rgb(1, 2, 3)' }} />);
    await screen.findByText('Verified node');
    open();
    const dialog = await screen.findByRole('dialog');
    expect(dialog.style.getPropertyValue('--node-shield-text')).toBe('rgb(1, 2, 3)');
  });

  it('carries a colour the chip inherits from an ancestor into the popover', async () => {
    // jsdom does not inherit custom properties, so the browser's cascade is stood in for here.
    const real = window.getComputedStyle;
    vi.spyOn(window, 'getComputedStyle').mockImplementation((el: Element) => {
      const style = real(el);
      if (!(el as HTMLElement).classList.contains('mero-node-shield')) return style;
      return { getPropertyValue: (name: string) => (name === '--node-shield-verified' ? ' rgb(9, 8, 7)' : '') } as CSSStyleDeclaration;
    });
    try {
      render(<NodeShield client={fakeClient(sealed).client} />);
      await screen.findByText('Verified node');
      open();
      const dialog = await screen.findByRole('dialog');
      expect(dialog.style.getPropertyValue('--node-shield-verified')).toBe('rgb(9, 8, 7)');
    } finally {
      vi.restoreAllMocks();
    }
  });
});

describe('the app code row', () => {
  it('keeps the short hash in one unbreakable piece', async () => {
    const named: AppCodeCheck = { ...match, package: 'com.calimero.crm', version: '1.4.0' } as AppCodeCheck;
    render(<NodeShield client={fakeClient(sealed).client} appCode={async () => named} />);
    await screen.findByText('Verified node');
    open();
    const hash = await screen.findByText('7f3a…c91e');
    expect(hash.tagName).toBe('SPAN');
    expect(hash.className).toBe('mero-node-shield-pop__nowrap');
  });
});

describe('the compact shield', () => {
  function viewport(matching: string[]) {
    const listeners = new Set<() => void>();
    window.matchMedia = ((query: string) => ({
      get matches() {
        return matching.includes(query);
      },
      media: query,
      addEventListener: (_: string, l: () => void) => listeners.add(l),
      removeEventListener: (_: string, l: () => void) => listeners.delete(l),
    })) as unknown as typeof window.matchMedia;
    return (next: string[]) => {
      matching = next;
      listeners.forEach((l) => l());
    };
  }
  const compact = () => chip().parentElement!.classList.contains('mero-node-shield--compact');

  afterEach(() => void (window.matchMedia = undefined as never));

  it('is icon only at or below the width it is given, and follows the window', async () => {
    const resize = viewport(['(max-width: 1100px)']);
    render(<NodeShield client={fakeClient(sealed).client} compactBelow={1100} />);
    await waitFor(() => expect(chip().getAttribute('aria-label')).toBe('Verified node'));
    expect(compact()).toBe(true);
    act(() => resize([]));
    expect(compact()).toBe(false);
  });

  it('is icon only at every width with compact, and does not need matchMedia', async () => {
    render(<NodeShield client={fakeClient(sealed).client} compact compactBelow={0} />);
    await waitFor(() => expect(chip().getAttribute('aria-label')).toBe('Verified node'));
    expect(compact()).toBe(true);
  });

  it('is never compact without a width, and does not need matchMedia', async () => {
    render(<NodeShield client={fakeClient(sealed).client} />);
    await waitFor(() => expect(chip().getAttribute('aria-label')).toBe('Verified node'));
    expect(compact()).toBe(false);
  });
});
