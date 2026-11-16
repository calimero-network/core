/**
 * A `MeroContext` with fixed values, for stories and component tests that do
 * not need a client.
 */

import React from 'react';
import { MeroContext } from '../context';
import type { MeroContextValue } from '../types';

export interface MockMeroProviderProps extends Partial<MeroContextValue> {
  children: React.ReactNode;
}

const noop = () => undefined;
const none = async () => null;

export function MockMeroProvider({ children, ...overrides }: MockMeroProviderProps) {
  const isAuthenticated = overrides.isAuthenticated ?? false;
  const value: MeroContextValue = {
    client: null,
    mero: null,
    nodeUrl: 'https://node.example.com',
    cluster: null,
    isConnecting: false,
    connectToNode: noop,
    connectToCluster: noop,
    disconnect: noop,
    session: null,
    accountId: isAuthenticated ? 'acc_3f9a2c7d1e5b8a4f6c0d' : null,
    isAuthenticated,
    isLoading: false,
    signIn: none,
    signUp: none,
    signOut: async () => undefined,
    signOutEverywhere: async () => undefined,
    logout: noop,
    appId: null,
    setAppId: noop,
    passkey: { create: () => Promise.reject(new Error('mock')), get: () => Promise.reject(new Error('mock')) },
    servedByNode: false,
    allowMock: false,
    failure: null,
    reportError: noop,
    ...overrides,
  };
  return <MeroContext.Provider value={value}>{children}</MeroContext.Provider>;
}
