import { ApiResponse } from '../../types/api-response';
import {
  ContextResponse,
  HealthRequest,
  HealthStatus,
  LoginRequest,
  LoginResponse,
  NodeApi,
  NodeChallenge,
  RootKeyRequest,
  RootKeyResponse,
} from '../nodeApi';
import { HttpClient } from '../httpClient';
import { Header, createAuthHeader } from '../../auth/headers';

export class NodeApiDataSource implements NodeApi {
  private client: HttpClient;

  constructor(client: HttpClient) {
    this.client = client;
  }

  async requestChallenge(
    rpcBaseUrl: string,
    contextId: string,
  ): ApiResponse<NodeChallenge> {
    return await this.client.post<NodeChallenge>(
      `${rpcBaseUrl}/admin-api/request-challenge`,
      {
        contextId,
      },
    );
  }

  async login(
    loginRequest: LoginRequest,
    rpcBaseUrl: string,
  ): ApiResponse<LoginResponse> {
    console.log('Send request to node with params', loginRequest);

    return await this.client.post<LoginRequest>(
      `${rpcBaseUrl}/admin-api/add-client-key`,
      {
        ...loginRequest,
      },
    );
  }

  async getContextIdentity(rpcBaseUrl: string, contextId: string): ApiResponse<ContextResponse> {
    const headers: Header | null = await createAuthHeader(contextId);

    return await this.client.get<ContextResponse>(
      `${rpcBaseUrl}/admin-api/contexts/${contextId}/identities`,
      headers
    );
  }

  async addRootKey(
    rootKeyRequest: RootKeyRequest,
    rpcBaseUrl: string,
    contextId: string,
  ): ApiResponse<RootKeyResponse> {
    console.log('Send request to node with params', rootKeyRequest);

    const headers: Header | null = await createAuthHeader(
      JSON.stringify(rootKeyRequest),
      contextId
    );

    return await this.client.post<LoginRequest>(
      `${rpcBaseUrl}/admin-api/root-key`,
      {
        ...rootKeyRequest,
      },
      headers
    );
  }

  async health(request: HealthRequest): ApiResponse<HealthStatus> {
    return await this.client.get<HealthStatus>(
      `${request.url}/admin-api/health`,
    );
  }
}
