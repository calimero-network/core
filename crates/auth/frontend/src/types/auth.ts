export interface Provider {
  name: string;
  type: string;
  description: string;
  configured: boolean;
  config?: Record<string, any>;
}

// Base token request interface
export interface BaseTokenRequest {
  auth_method: string;
  public_key: string;
  client_name: string;
  timestamp: number;
  permissions?: string[];
  provider_data: any; // This will be typed based on the auth method
}

// NEAR wallet specific request data
export interface NearWalletProviderData {
  wallet_address: string;
  message: string;
  signature: string;
  nonce: string;
  recipient: string;
  callback_url: string;
}

// Token request with NEAR wallet provider data
export interface NearWalletTokenRequest extends BaseTokenRequest {
  provider_data: NearWalletProviderData;
}

export interface TokenResponse {
  access_token: string;
  refresh_token: string;
  token_type: string;
  expires_in: number;
  client_id: string;
  error?: string;
}

export interface AuthState {
  isAuthenticated: boolean;
  loading: boolean;
  error: string | null;
}

export interface AuthContextType extends AuthState {
  login: (accessToken: string, refreshToken: string) => Promise<boolean>;
  logout: () => void;
}

export interface ChallengeRequest {
  provider: string;
  redirect_uri?: string;
  client_id?: string;
}

export interface ChallengeResponse {
  challenge: string;
  nonce: string;  // Base64 encoded nonce from server
}

export interface SignedMessage {
  accountId: string;
  publicKey: string;
  signature: string;
}