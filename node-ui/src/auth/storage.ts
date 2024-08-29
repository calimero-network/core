import { ClientKey } from './types';

export const CLIENT_KEY = 'client-key';
export const APP_URL = 'app-url';
export const AUTHORIZED = 'node-authorized';
export const CALLBACK_URL = 'callback-url';
export const APPLICATION_ID = 'application-id';

export const setStorageCallbackUrl = (callbackUrl: string) => {
  localStorage.setItem(CALLBACK_URL, JSON.stringify(callbackUrl));
};

export const getStorageCallbackUrl = (): string | null => {
  if (typeof window !== 'undefined' && window.localStorage) {
    const storageCallbackUrl = localStorage.getItem(CALLBACK_URL);
    if (storageCallbackUrl) {
      let callbackUrl: string = JSON.parse(storageCallbackUrl);
      return callbackUrl;
    } else {
      return null;
    }
  }
  return null;
};

export const setStorageApplicationId = (applicationId: string) => {
  localStorage.setItem(APPLICATION_ID, JSON.stringify(applicationId));
};

export const getStorageApplicationId = (): string | null => {
  if (typeof window !== 'undefined' && window.localStorage) {
    const storageApplicationId = localStorage.getItem(APPLICATION_ID);
    if (storageApplicationId) {
      let applicationId: string = JSON.parse(storageApplicationId);
      return applicationId;
    } else {
      return null;
    }
  }
  return null;
};

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
    const isAuthorized = localStorage.getItem(AUTHORIZED);
    if (isAuthorized !== null) {
      let authorized: boolean = JSON.parse(isAuthorized);
      if (authorized) {
        return authorized;
      }
    } else {
      return null;
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
