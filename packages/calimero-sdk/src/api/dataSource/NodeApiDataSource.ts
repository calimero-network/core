import { ApiResponse } from '../../types/api-response';
import {
  HealthRequest,
  HealthStatus,
  LoginRequest,
  LoginResponse,
  NodeApi,
  NodeChallenge,
  RootKeyRequest,
  RootKeyResponse,
} from '../nodeApi';
import { Header, HttpClient } from '../httpClient';
import { createAuthHeader } from '../../crypto/crypto';

export class NodeApiDataSource implements NodeApi {
  private client: HttpClient;

  constructor(client: HttpClient) {
    this.client = client;
  }

  async requestChallenge(
    rpcBaseUrl: string,
    applicationId: string,
  ): ApiResponse<NodeChallenge> {
    const authHeaders: Header[] = await createAuthHeader(
      JSON.stringify(applicationId)
    );

    return await this.client.post<NodeChallenge>(
      `${rpcBaseUrl}/admin-api/request-challenge`,
      {
        applicationId: applicationId,
      },
      authHeaders
    );
  }

  async login(
    loginRequest: LoginRequest,
    rpcBaseUrl: string,
  ): ApiResponse<LoginResponse> {
    console.log('Send request to node with params', loginRequest);
    const authHeaders: Header[] = await createAuthHeader(
      JSON.stringify(loginRequest)
    );

    return await this.client.post<LoginRequest>(
      `${rpcBaseUrl}/admin-api/add-client-key`,
      {
        ...loginRequest,
      },
      authHeaders
    );
  }

  async addRootKey(
    rootKeyRequest: RootKeyRequest,
    rpcBaseUrl: string,
  ): ApiResponse<RootKeyResponse> {
    console.log('Send request to node with params', rootKeyRequest);
    const authHeaders: Header[] = await createAuthHeader(
      JSON.stringify(rootKeyRequest)
    );

    return await this.client.post<LoginRequest>(
      `${rpcBaseUrl}/admin-api/root-key`,
      {
        ...rootKeyRequest,
      },
      authHeaders
    );
  }

  async health(request: HealthRequest): ApiResponse<HealthStatus> {
    const authHeaders: Header[] = await createAuthHeader(
      JSON.stringify(request)
    );

    return await this.client.get<HealthStatus>(
      `${request.url}/admin-api/health`,
      authHeaders
    );
  }
}
