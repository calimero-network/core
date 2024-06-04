import { HttpClient } from "../httpClient";

enum Network {
  NEAR = "NEAR",
  ETH = "ETH",
}

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
  createdAt: number;
}

export interface ETHRootKey extends RootKey {
  type: Network.ETH;
  chainId: number;
}

export interface NearRootKey extends RootKey {
  type: Network.NEAR;
}

interface NetworkType {
  type: Network;
  chainId?: number;
}

interface ApiRootKey {
  signing_key: string;
  wallet: NetworkType;
  created_at: number;
}

interface ClientKey {
  signing_key: string;
  type: Network;
  created_at: number;
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
      const response = await this.client.get<Application[]>(
        "/admin-api/applications"
      );
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

  async getDidList(): Promise<(ETHRootKey | NearRootKey)[]> {
    try {
      const response = await this.client.get<RootkeyResponse>("/admin-api/did");
      if (response?.data?.root_keys) {
        const rootKeys: (ETHRootKey | NearRootKey)[] =
          response?.data?.root_keys?.map(
            (obj: ApiRootKey) => {
              if (obj.wallet.type === Network.NEAR) {
                return {
                  signingKey: obj.signing_key,
                  type: Network.NEAR,
                  chainId: obj.wallet.chainId ?? 1,
                  createdAt: obj.created_at,
                } as NearRootKey;
              } else {
                return {
                  signingKey: obj.signing_key,
                  type: Network.ETH,
                  createdAt: obj.created_at,
                  ...(obj.wallet.chainId !== undefined && { chainId: obj.wallet.chainId }),
                } as ETHRootKey;
              }
            }
          );
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
