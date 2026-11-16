import { describe, expect, it, vi } from 'vitest';
import { CalimeroClient } from '@calimero-network/mero-js';
import { reportSessions, resolvePolicy, SessionReportingClient } from '../client';
import { SESSION } from './fake';

describe('resolvePolicy', () => {
  it('builds a release policy from minReleaseVersion', () => {
    expect(resolvePolicy({ minReleaseVersion: '2.3.109' })).toEqual({ release: { minReleaseVersion: '2.3.109' } });
  });
  it('adds allowMock for merod local', () => {
    expect(resolvePolicy({ allowMock: true })).toEqual({ allowMock: true });
    expect(resolvePolicy({ minReleaseVersion: '1', allowMock: true })).toEqual({ release: { minReleaseVersion: '1' }, allowMock: true });
    expect(resolvePolicy({ policy: { appHash: 'ab' }, allowMock: true })).toEqual({ appHash: 'ab', allowMock: true });
  });
  it('refuses to trust nothing', () => {
    expect(() => resolvePolicy({})).toThrow(/minReleaseVersion/);
  });
});

describe('SessionReportingClient', () => {
  it('reports every session it takes, refreshes included, but not a restored one', () => {
    const onSession = vi.fn();
    const client = new SessionReportingClient({ baseUrl: 'https://node.test', policy: { allowMock: true } }, onSession);
    client.restore(SESSION);
    expect(onSession).not.toHaveBeenCalled();
    expect(client.session).toEqual(SESSION);
    const next = { ...SESSION, refreshToken: 'rt_2' };
    client.useSession(next);
    expect(onSession).toHaveBeenCalledWith(next);
  });
});

describe('reportSessions', () => {
  it('reports the sessions a client built elsewhere takes, but not a restored one', () => {
    const onSession = vi.fn();
    const client = new CalimeroClient({ baseUrl: 'https://node.test', policy: { allowMock: true } });
    const restore = reportSessions(client, onSession);
    restore(SESSION);
    expect(onSession).not.toHaveBeenCalled();
    expect(client.session).toEqual(SESSION);
    const next = { ...SESSION, refreshToken: 'rt_2' };
    client.useSession(next);
    expect(onSession).toHaveBeenCalledWith(next);
    expect(client.session).toEqual(next);
  });
});
