export interface Provider {
  name: string;
  displayName: string;
  description?: string;
  icon?: string;
}

export interface TokenResponse {
  access_token: string;
  refresh_token: string;
  token_type: string;
  expires_in: number;
  client_id: string;
}

export interface AuthState {
  isAuthenticated: boolean;
  loading: boolean;
  userId: string | null;
  permissions: string[];
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
  message: string;
  timestamp: number;
  network: string;
  rpc_url: string;
  wallet_url: string;
  redirect_uri: string;
  recipient?: string;
}

export interface SignedMessage {
  accountId: string;
  publicKey: string;
  signature: string;
}