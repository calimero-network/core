import { ApiResponse } from '../../types/api-response';
import {
  HealthRequest,
  HealthStatus,
  JwtTokenResponse,
  NodeApi,
} from '../nodeApi';
import { HttpClient } from '../httpClient';

export class NodeApiDataSource implements NodeApi {
  private client: HttpClient;

  constructor(client: HttpClient) {
    this.client = client;
  }

  async refreshToken(
    refreshToken: string,
    rpcBaseUrl: string,
  ): ApiResponse<JwtTokenResponse> {
    return await this.client.post<JwtTokenResponse>(
      `${rpcBaseUrl}/admin-api/refresh-jwt-token`,
      {
        refreshToken,
      },
    );
  }

  async health(request: HealthRequest): ApiResponse<HealthStatus> {
    return await this.client.get<HealthStatus>(
      `${request.url}/admin-api/health`,
    );
  }
}
