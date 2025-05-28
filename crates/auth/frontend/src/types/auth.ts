export interface Provider {
  name: string;
  type: string;
  description: string;
  configured: boolean;
  config?: Record<string, any>;
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
}

export interface SignedMessage {
  accountId: string;
  publicKey: string;
  signature: string;
}