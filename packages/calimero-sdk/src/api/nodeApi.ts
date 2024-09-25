import { ApiResponse } from '../types/api-response';

export interface HealthRequest {
  url: string;
}

export interface HealthStatus {
  status: string;
}

export interface JwtTokenResponse {
  access_token: string;
  refresh_token: string;
}

export interface NodeApi {
  refreshToken(
    refreshToken: string,
    rpcBaseUrl: string,
  ): ApiResponse<JwtTokenResponse>;
  health(request: HealthRequest): ApiResponse<HealthStatus>;
}
