import { HttpClient } from "../httpClient";

export interface Context {
  applicationId: string;
  id: string;
  signingKey: { signingKey: string };
}

export interface NodeContexts {
  joined: Context[];
  invited: Context[];
}

export class ContextDataSource {
  private client: HttpClient;

  constructor(client: HttpClient) {
    this.client = client;
  }

  async getContexts(): Promise<NodeContexts> {
    try {
      const response = await this.client.get("/admin-api/contexts");
      if (response?.data) {
        // invited is empty for now as we don't have this endpoint available
        // will be left as "no invites" until this becomes available
        return {
          // @ts-ignore
          joined: response.data,
          invited: [],
        };
      } else {
        return { joined: [], invited: [] };
      }
    } catch (error) {
      console.error("Error fetching contexts:", error);
      return { joined: [], invited: [] };
    }
  }

  async getContext(contextId: string): Promise<Context | null> {
    try {
      const response = await this.client.get<Context>(
        `/admin-api/contexts/${contextId}`
      );
      response?.data;
      if (response?.data) {
        return response.data;
      } else {
        return null;
      }
    } catch (error) {
      console.error("Error fetching context:", error);
      return null;
    }
  }

  async deleteContext(contextId: string): Promise<boolean> {
    try {
      const response = await this.client.delete<boolean>(
        `/admin-api/contexts/${contextId}`
      );
      if (response?.data) {
        return response.data;
      } else {
        return false;
      }
    } catch (error) {
      console.error("Error deleting context:", error);
      return false;
    }
  }

  async startContexts(
    applicationId: string,
    initFunction: string,
    initArguments: string
  ): Promise<boolean> {
    try {
      const response = await this.client.post<Context>("/admin-api/contexts", {
        applicationId: applicationId,
        ...(initFunction && { initFunction }),
        ...(initArguments && { initArgs: JSON.stringify(initArguments) }),
      });
      if (response?.data) {
        return !!response.data;
      } else {
        return false;
      }
    } catch (error) {
      console.error("Error starting contexts:", error);
      return true;
    }
  }
}
