import { ChallengeRequest, Provider, TokenResponse, ChallengeResponse, BaseTokenRequest } from '../types/auth';

// API URL is relative by default
const API_URL = '';

// Helper for making authenticated requests
async function fetchWithAuth(url: string, options: RequestInit = {}) {
  const token = localStorage.getItem('auth_token');
  
  const headers = {
    'Content-Type': 'application/json',
    ...options.headers,
    ...(token ? { Authorization: `Bearer ${token}` } : {})
  };

  const response = await fetch(`${API_URL}${url}`, {
    ...options,
    headers
  });

  if (!response.ok) {
    if (response.status === 401) {
      // Clear the invalid token
      localStorage.removeItem('auth_token');
      throw new Error('Authentication required');
    }
    const error = await response.json().catch(() => ({}));
    throw new Error(error.message || `API Error: ${response.status}`);
  }

  return response.json();
}

// Get available authentication providers
export async function getProviders(): Promise<Provider[]> {
  const data = await fetchWithAuth('/providers');
  return data.providers || [];
}

// Request authentication token
export async function requestToken(requestBody: BaseTokenRequest): Promise<TokenResponse> {
  return fetchWithAuth('/auth/token', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json'
    },
    body: JSON.stringify(requestBody)
  });
}

// Get a challenge for authentication (e.g., for NEAR wallet)
export async function getChallenge(): Promise<ChallengeResponse> {
  return fetchWithAuth(`/auth/challenge`);
}

// Generate client key token
interface GenerateClientKeyRequest {
  context_id: string;
  context_identity: string;
}

export async function generateClientKey(rootToken: string, request: GenerateClientKeyRequest): Promise<TokenResponse> {
  return fetchWithAuth('/auth/client-key', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${rootToken}`
    },
    body: JSON.stringify(request)
  });
}