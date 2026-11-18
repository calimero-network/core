import { beforeEach, describe, expect, it } from 'vitest';
import { clearAllStorage, getAppId, getNodeUrl, localStorageSessionStore, setAppId, setNodeUrl } from '../storage';
import { SESSION } from './fake';

describe('storage', () => {
  beforeEach(() => localStorage.clear());

  it('keeps and clears the session', () => {
    expect(localStorageSessionStore.load()).toBeNull();
    localStorageSessionStore.save(SESSION);
    expect(localStorageSessionStore.load()).toEqual(SESSION);
    setNodeUrl('https://n');
    setAppId('a');
    clearAllStorage();
    expect(localStorageSessionStore.load()).toBeNull();
    expect(getNodeUrl()).toBeNull();
    expect(getAppId()).toBeNull();
  });

  it('ignores a stored value that is not a session (a 0.x token, say)', () => {
    localStorage.setItem('mero:session', JSON.stringify({ access_token: 'x' }));
    expect(localStorageSessionStore.load()).toBeNull();
    localStorage.setItem('mero:session', '{not json');
    expect(localStorageSessionStore.load()).toBeNull();
  });
});
