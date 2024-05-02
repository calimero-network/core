import { ApiResponse } from "../../api-response";
import { LoginRequest, LoginResponse, NodeApi, NodeChallenge } from "../../nodeApi";
import { HttpClient } from "../httpClient";


export class NodeApiDataSource implements NodeApi {
  private client: HttpClient;

  constructor(client: HttpClient) {
    this.client = client;
  }

  async requestChallenge(rpcBaseUrl: string, applicationId: string): ApiResponse<NodeChallenge> {
    return await this.client.post<NodeChallenge>(
      `${rpcBaseUrl}/admin-api/request-challenge`,
      {
        applicationId: applicationId,
      }
    );
  }

  async login(loginRequest: LoginRequest, rpcBaseUrl: string): ApiResponse<LoginResponse> {
    console.log("Send request to node with params", loginRequest);

    return await this.client.post<LoginRequest>(
      `${rpcBaseUrl}/admin-api/add-client-key`,
      {
        ...loginRequest,
      }
    );
  }
}
