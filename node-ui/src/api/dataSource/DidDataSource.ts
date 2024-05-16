import { HttpClient } from "../httpClient";

export interface RootKey {
    signing_key: string;
  }

export class DidDataSource {
  private client: HttpClient;

  constructor(client: HttpClient) {
    this.client = client;
  }

  async getDidList(): Promise<RootKey[]> {
    try {
      const response = await this.client.get<any>("/admin-api/did");
      if (response?.data?.root_keys) {
        return response.data.root_keys;
      } else {
        return [];
      }
    } catch (error) {
      console.error("Error fetching installed applications:", error);
      return [];
    }
  }
}
