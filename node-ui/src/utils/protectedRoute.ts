const BASE_PATH = '/admin-dashboard';

export const getPathname = () => {
  return window.location.pathname.startsWith(BASE_PATH)
    ? window.location.pathname.slice(BASE_PATH.length)
    : window.location.pathname;
};
