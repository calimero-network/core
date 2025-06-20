export const handleUrlParams = () => {
  // Get URL search params
  const searchParams = new URLSearchParams(window.location.search);
  const params: Record<string, string> = {};
  
  // Convert URLSearchParams to a plain object and store in localStorage
  searchParams.forEach((value, key) => {
    params[key] = value;
    localStorage.setItem(key, JSON.stringify(value));
  });
  
  // Clear URL parameters without reloading the page
  if (searchParams.toString()) {
    const newUrl = window.location.pathname + window.location.hash;
    window.history.replaceState({}, '', newUrl);
  }
  
  return params;
};

export const getStoredUrlParam = (key: string): string | null => {
  const value = localStorage.getItem(key);
  if (value) {
    return JSON.parse(value);
  }
  return null;
};

export const clearStoredUrlParams = () => {
  // Get all localStorage keys
  for (let i = 0; i < localStorage.length; i++) {
    const key = localStorage.key(i);
    if (key !== 'access-token' && key !== 'refresh-token') {
      localStorage.removeItem(key!);
    }
  }
}; 