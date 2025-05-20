import { ChallengeRequest, Provider, TokenResponse, ChallengeResponse } from '../types/auth';

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
export async function requestToken(provider: string, authData: any): Promise<TokenResponse> {
  const requestBody = {
    auth_method: provider,
    public_key: authData.public_key,
    wallet_address: authData.account_id,
    client_name: 'web-browser',
    permissions: [],
    signature: authData.signature,
    message: authData.message,
    timestamp: Math.floor(Date.now() / 1000)
  };

  console.log('Message being sent:', authData.message);
  console.log('Token request body:', requestBody);

  return fetchWithAuth('/auth/token', {
    method: 'POST',
    body: JSON.stringify(requestBody)
  });
}

// Get a challenge for authentication (e.g., for NEAR wallet)
export async function getChallenge(challengeRequest: ChallengeRequest): Promise<ChallengeResponse> {
  const params = new URLSearchParams({
    provider: challengeRequest.provider,
    redirect_uri: challengeRequest.redirect_uri || window.location.href,
    client_id: challengeRequest.client_id || 'web-browser'
  });
  
  return fetchWithAuth(`/auth/challenge?${params.toString()}`);
}

// Verify an existing token
export async function verifyToken(): Promise<{ valid: boolean, userId: string, permissions: string[] }> {
  try {
    const response = await fetchWithAuth('/auth/validate');
    return {
      valid: response.is_valid || false,
      userId: response.key_id || '',
      permissions: response.permissions || []
    };
  } catch (error) {
    return { valid: false, userId: '', permissions: [] };
  }
}