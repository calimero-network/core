import { ClientKey } from './types';

export const CLIENT_KEY = 'client-key';
export const APP_URL = 'app-url';
export const AUTHORIZED = 'node-authorized';

export const setStorageClientKey = (clientKey: ClientKey) => {
  localStorage.setItem(CLIENT_KEY, JSON.stringify(clientKey));
};

export const getStorageClientKey = (): ClientKey | null => {
  if (typeof window !== 'undefined' && window.localStorage) {
    const clientKey = localStorage.getItem(CLIENT_KEY);
    if (!clientKey) {
      return null;
    }
    let clientKeystore: ClientKey = JSON.parse(clientKey);
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

export const getStorageNodeAuthorized = (): boolean | null => {
  if (typeof window !== 'undefined' && window.localStorage) {
    let authorized: boolean = JSON.parse(
      localStorage.getItem(AUTHORIZED) ?? '',
    );
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
    let url: String = JSON.parse(localStorage.getItem(APP_URL) ?? '');
    if (url) {
      return url;
    }
  }
  return null;
};
