import { ClientKey } from '../types/storage';

export const CLIENT_KEY = 'client-key';
export const APP_URL = 'app-url';
export const AUTHORIZED = 'node-authorized';
export const CONTEXT_IDENTITY = 'context-identity';
export const ACCESS_TOKEN = "access-token";
export const REFRESH_TOKEN = "refresh-token";

export const setAccessToken = (accessToken: string) => {
  localStorage.setItem(ACCESS_TOKEN, JSON.stringify(accessToken));
}

export const getAccessToken = (): string | null => {
  if (typeof window !== 'undefined' && window.localStorage) {
    const storageContextId = localStorage.getItem(ACCESS_TOKEN);
    if (storageContextId) {
      return JSON.parse(storageContextId);
    }
  }
  return null;
};

export const setRefreshToken = (refreshToken: string) => {
  localStorage.setItem(REFRESH_TOKEN, JSON.stringify(refreshToken));
}

export const getRefreshToken = (): string | null => {
  if (typeof window !== 'undefined' && window.localStorage) {
    const storageContextId = localStorage.getItem(REFRESH_TOKEN);
    if (storageContextId) {
      return JSON.parse(storageContextId);
    }
  }
  return null;
};

export const setStorageClientKey = (clientKey: ClientKey) => {
  localStorage.setItem(CLIENT_KEY, JSON.stringify(clientKey));
};

export const getStorageClientKey = (): ClientKey | null => {
  if (typeof window !== 'undefined' && window.localStorage) {
    let clientKeystore: ClientKey = JSON.parse(
      localStorage.getItem(CLIENT_KEY),
    );
    if (clientKeystore) {
      return clientKeystore;
    }
  }
  return null;
};

export const clearClientKey = () => {
  localStorage.removeItem(CLIENT_KEY);
};

export const setStorageNodeAuthorized = () => {
  localStorage.setItem(AUTHORIZED, JSON.stringify(true));
};

export const setExecutorPublicKey = (publicKey: string) => {
  localStorage.setItem(CONTEXT_IDENTITY, JSON.stringify(publicKey));
};

export const getStorageNodeAuthorized = (): boolean | null => {
  if (typeof window !== 'undefined' && window.localStorage) {
    let authorized: boolean = JSON.parse(localStorage.getItem(AUTHORIZED));
    if (authorized) {
      return authorized;
    }
  }
  return null;
};

export const clearNodeAuthorized = () => {
  localStorage.removeItem(AUTHORIZED);
};

export const clearAppEndpoint = () => {
  localStorage.removeItem(APP_URL);
};

export const setAppEndpointKey = (url: String) => {
  localStorage.setItem(APP_URL, JSON.stringify(url));
};

export const getAppEndpointKey = (): String | null => {
  if (typeof window !== 'undefined' && window.localStorage) {
    let url: String = JSON.parse(localStorage.getItem(APP_URL));
    if (url) {
      return url;
    }
  }
  return null;
};
