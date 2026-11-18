import { act, render, renderHook, screen, waitFor } from '@testing-library/react';
import React from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { errorFrom } from '@calimero-network/mero-js';
import { MeroProvider, useMero } from '../context';
import { memorySessionStore } from '../storage';
import { fakeClient, passkey, SESSION, wrapper } from './fake';

describe('MeroProvider', () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => localStorage.clear());

  it('resumes a kept session', () => {
    const { result } = renderHook(() => useMero(), { wrapper: wrapper(fakeClient()) });
    expect(result.current.isAuthenticated).toBe(true);
    expect(result.current.accountId).toBe('acct-1');
    expect(result.current.appId).toBe('app-1');
  });

  it('signs in with the passkey and keeps the session', async () => {
    const client = fakeClient();
    const store = memorySessionStore();
    const { result } = renderHook(() => useMero(), { wrapper: wrapper(client, { store }) });
    expect(result.current.isAuthenticated).toBe(false);
    await act(async () => {
      await result.current.signIn();
    });
    expect(client.signIn).toHaveBeenCalledWith(passkey);
    expect(result.current.isAuthenticated).toBe(true);
    expect(store.load()).toEqual(SESSION);
  });

  it('signs up under a name', async () => {
    const client = fakeClient();
    const { result } = renderHook(() => useMero(), { wrapper: wrapper(client, { store: memorySessionStore() }) });
    await act(async () => {
      await result.current.signUp('ada');
    });
    expect(client.signUp).toHaveBeenCalledWith(passkey, 'ada');
  });

  it('reports a failed sign-in as a typed failure', async () => {
    const client = fakeClient({ signIn: async () => Promise.reject(errorFrom(421, { error: 'WRONG_NODE' })) });
    const { result } = renderHook(() => useMero(), { wrapper: wrapper(client, { store: memorySessionStore() }) });
    await act(async () => {
      expect(await result.current.signIn()).toBeNull();
    });
    expect(result.current.failure?.kind).toBe('wrong-node');
    expect(result.current.isAuthenticated).toBe(false);
  });

  it('signs out on the node and here', async () => {
    const client = fakeClient();
    const store = memorySessionStore(SESSION);
    const { result } = renderHook(() => useMero(), { wrapper: wrapper(client, { store }) });
    await act(async () => {
      await result.current.signOut();
    });
    expect(client.signOut).toHaveBeenCalled();
    expect(result.current.isAuthenticated).toBe(false);
    expect(store.load()).toBeNull();
  });

  it('ends the session when a refresh was refused (LoginRequired)', () => {
    const store = memorySessionStore(SESSION);
    const { result } = renderHook(() => useMero(), { wrapper: wrapper(fakeClient(), { store }) });
    act(() => result.current.reportError(errorFrom(401, { error: 'UNAUTHENTICATED' })));
    expect(result.current.isAuthenticated).toBe(false);
    expect(result.current.failure?.kind).toBe('login');
    expect(store.load()).toBeNull();
  });

  it('keeps the session through other failures', () => {
    const { result } = renderHook(() => useMero(), { wrapper: wrapper(fakeClient()) });
    act(() => result.current.reportError(errorFrom(422, { error: 'APP_ERROR' })));
    expect(result.current.isAuthenticated).toBe(true);
  });

  it('talks to the page origin when the node serves the page', () => {
    function Probe() {
      const { nodeUrl, client, servedByNode, connectToNode } = useMero();
      return (
        <button onClick={() => connectToNode('https://elsewhere.test')}>
          {String(servedByNode)} {nodeUrl} {client?.nodeUrl}
        </button>
      );
    }
    render(
      <MeroProvider servedByNode allowMock sessionStore={memorySessionStore()} nodeUrl="https://ignored.test">
        <Probe />
      </MeroProvider>,
    );
    const origin = window.location.origin;
    expect(screen.getByRole('button').textContent).toBe(`true ${origin} ${origin}`);
    act(() => screen.getByRole('button').click());
    expect(screen.getByRole('button').textContent).toBe(`true ${origin} ${origin}`);
  });

  it('builds a client for a chosen node and remembers it', async () => {
    function Probe() {
      const { client, connectToNode } = useMero();
      return <button onClick={() => connectToNode('https://n2.test/')}>{client?.nodeUrl ?? 'none'}</button>;
    }
    render(
      <MeroProvider minReleaseVersion="2.3.109" sessionStore={memorySessionStore()}>
        <Probe />
      </MeroProvider>,
    );
    expect(screen.getByRole('button').textContent).toBe('none');
    act(() => screen.getByRole('button').click());
    await waitFor(() => expect(screen.getByRole('button').textContent).toBe('https://n2.test'));
    expect(localStorage.getItem('mero:nodeUrl')).toBe('https://n2.test');
  });

  it('connects to a cluster by name through the node directory', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      expect(String(input)).toBe('https://dir.test/eu-shared-1');
      return new Response(
        JSON.stringify({
          cluster: 'eu-shared-1',
          nodes: [{ id: 'n1', url: 'https://n1.test', zone: 'z', weight: 1 }],
          refreshAfterSeconds: 60,
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      );
    });
    function Probe() {
      const { client, cluster, nodeUrl, isConnecting } = useMero();
      return (
        <span>
          {cluster} {isConnecting ? 'connecting' : (client?.nodeUrl ?? 'none')} {nodeUrl ?? '-'}
        </span>
      );
    }
    render(
      <MeroProvider
        cluster="eu-shared-1"
        directory="https://dir.test"
        fetch={fetch as unknown as typeof globalThis.fetch}
        minReleaseVersion="2.3.109"
        sessionStore={memorySessionStore()}
      >
        <Probe />
      </MeroProvider>,
    );
    await waitFor(() => expect(screen.getByText(/eu-shared-1/).textContent).toBe('eu-shared-1 https://n1.test https://n1.test'));
  });

  it('reports a cluster the directory does not know', async () => {
    const fetch = vi.fn(async () => new Response('{}', { status: 404 }));
    const onError = vi.fn();
    const { result } = renderHook(() => useMero(), {
      wrapper: ({ children }: { children: React.ReactNode }) => (
        <MeroProvider
          cluster="nope"
          directory="https://dir.test"
          fetch={fetch as unknown as typeof globalThis.fetch}
          minReleaseVersion="2.3.109"
          sessionStore={memorySessionStore()}
          onError={onError}
        >
          {children}
        </MeroProvider>
      ),
    });
    await waitFor(() => expect(result.current.failure?.kind).toBe('directory'));
    expect(result.current.client).toBeNull();
    expect(onError).toHaveBeenCalled();
  });

  it('refuses a provider that trusts no release', () => {
    const quiet = console.error;
    console.error = () => undefined;
    try {
      expect(() =>
        render(
          <MeroProvider nodeUrl="https://n.test" sessionStore={memorySessionStore()}>
            <div />
          </MeroProvider>,
        ),
      ).toThrow(/minReleaseVersion/);
    } finally {
      console.error = quiet;
    }
  });

  it('throws outside a provider', () => {
    const quiet = console.error;
    console.error = () => undefined;
    try {
      expect(() => renderHook(() => useMero())).toThrow(/MeroProvider/);
    } finally {
      console.error = quiet;
    }
  });
});
