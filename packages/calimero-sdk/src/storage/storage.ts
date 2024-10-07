export const APP_URL = 'app-url';
export const CONTEXT_IDENTITY = 'context-identity';
export const ACCESS_TOKEN = 'access-token';
export const REFRESH_TOKEN = 'refresh-token';

export const setAccessToken = (accessToken: string) => {
  localStorage.setItem(ACCESS_TOKEN, JSON.stringify(accessToken));
};

export const getAccessToken = (): string | null => {
  if (typeof window !== 'undefined' && window.localStorage) {
    const storageContextId = localStorage.getItem(ACCESS_TOKEN);
    if (storageContextId) {
      return JSON.parse(storageContextId);
    }
  }
  return null;
};

export const abcs = (): string | null => {
  if (typeof window !== 'undefined' && window.localStorage) {
    const storageContextId = localStorage.getItem(ACCESS_TOKEN);
    if (storageContextId) {
      return JSON.parse(storageContextId);
    }
  }
  return null;
};

console.log('lol');

export const setRefreshToken = (refreshToken: string) => {
  localStorage.setItem(REFRESH_TOKEN, JSON.stringify(refreshToken));
};

export const getRefreshToken = (): string | null => {
  if (typeof window !== 'undefined' && window.localStorage) {
    const storageContextId = localStorage.getItem(REFRESH_TOKEN);
    if (storageContextId) {
      return JSON.parse(storageContextId);
    }
  }
  return null;
};

export const setExecutorPublicKey = (publicKey: string) => {
  localStorage.setItem(CONTEXT_IDENTITY, JSON.stringify(publicKey));
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

export const clearAppEndpoint = () => {
  localStorage.removeItem(APP_URL);
};

export const clearJWT = () => {
  localStorage.removeItem(ACCESS_TOKEN);
  localStorage.removeItem(REFRESH_TOKEN);
};
