import { ApiResponse } from "../../api-response";
import {
  HealthRequest,
  HealthStatus,
  LoginRequest,
  LoginResponse,
  NodeApi,
  NodeChallenge,
  RootKeyRequest,
  RootKeyResponse,
} from "../../nodeApi";
import { HttpClient } from "../httpClient";

export class NodeApiDataSource implements NodeApi {
  private client: HttpClient;

  constructor(client: HttpClient) {
    this.client = client;
  }

  async requestChallenge(
    rpcBaseUrl: string,
    applicationId: string
  ): ApiResponse<NodeChallenge> {
    return await this.client.post<NodeChallenge>(
      `${rpcBaseUrl}/admin-api/request-challenge`,
      {
        applicationId: applicationId,
      }
    );
  }

  async login(
    loginRequest: LoginRequest,
    rpcBaseUrl: string
  ): ApiResponse<LoginResponse> {
    console.log("Send request to node with params", loginRequest);

    return await this.client.post<LoginRequest>(
      `${rpcBaseUrl}/admin-api/add-client-key`,
      {
        ...loginRequest,
      }
    );
  }

  async addRootKey(
    rootKeyRequest: RootKeyRequest,
    rpcBaseUrl: string
  ): ApiResponse<RootKeyResponse> {
    console.log("Send request to node with params", rootKeyRequest);

    return await this.client.post<LoginRequest>(
      `${rpcBaseUrl}/admin-api/root-key`,
      {
        ...rootKeyRequest,
      }
    );
  }

  async health(request: HealthRequest): ApiResponse<HealthStatus> {
    return await this.client.get<HealthStatus>(
      `${request.url}/admin-api/health`
    );
  }
}
