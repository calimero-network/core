import { HttpClient } from "../httpClient";

export interface Application {
  id: string;
  version: string;
}

export interface SigningKey {
  signingKey: string;
}

export interface Context {
  applicationId: string;
  id: string;
  signingKey: SigningKey;
}

export interface ContextsList<T> {
  joined: T[];
  invited: T[];
}

export interface RootKey {
  signingKey: string;
  walletType: string;
  createdAt: number;
}

export interface ApiRootKey {
  signing_key: string;
  wallet_type: string;
  created_at: number;
}

interface ClientKey {
  signing_key: string;
  wallet_type: string;
  date: number;
}

interface RootkeyResponse {
  client_keys: ClientKey[];
  contexts: Context[];
  root_keys: ApiRootKey[];
}

export class NodeDataSource {
  private client: HttpClient;

  constructor(client: HttpClient) {
    this.client = client;
  }

  async getInstalledApplications(): Promise<Application[]> {
    try {
      const response = await this.client.get<Application[]>("/admin-api/applications");
      // @ts-ignore with adminAPI update TODO: fix admin api response
      return response?.apps ?? [];
    } catch (error) {
      console.error("Error fetching installed applications:", error);
      return [];
    }
  }

  async getContexts(): Promise<ContextsList<Context>> {
    try {
      const response = await this.client.get<Context[]>("/admin-api/contexts");
      if (response?.data) {
        // invited is empty for now as we don't have this endpoint available
        // will be left as "no invites" until this becomes available
        return {
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

  async getDidList(): Promise<RootKey[]> {
    try {
      const response = await this.client.get<RootkeyResponse>("/admin-api/did");
      if (response?.data?.root_keys) {
        const rootKeys: RootKey[] = response?.data?.root_keys?.map((obj: { signing_key: string, wallet_type: string, created_at: number }) => ({
          signingKey: obj.signing_key,
          walletType: obj.wallet_type,
          createdAt: obj.created_at
        }));
        return rootKeys;
      } else {
        return [];
      }
    } catch (error) {
      console.error("Error fetching DID list:", error);
      return [];
    }
  }
}
