import { HttpClient } from "../httpClient";

export interface Application {
  id: string;
  version: string;
}

export class AppsDataSource {
  private client: HttpClient;

  constructor(client: HttpClient) {
    this.client = client;
  }

  async getInstalledApplications(): Promise<Application[]> {
    try {
      const response = await this.client.get<any>("/admin-api/applications");
      // @ts-ignore with adminAPI update
      if (response?.apps) {
        // @ts-ignore
        return response.apps;
      } else {
        return [];
      }
    } catch (error) {
      console.error("Error fetching installed applications:", error);
      return [];
    }
  }
}
