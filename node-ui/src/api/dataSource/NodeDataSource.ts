import { getAppEndpointKey } from '../../utils/storage';
import { HttpClient } from '../httpClient';
import { ApiResponse, ResponseData } from '../response';
import { NodeApi } from '../nodeApi';
import translations from '../../constants/en.global.json';
import { createAppMetadata } from '../../utils/metadata';
import { Signature } from 'starknet';
import { getNearEnvironment } from '../../utils/node';
import { createAuthHeader, Header } from '../../auth/headers';

const t = translations.nodeDataSource;

export enum Network {
  NEAR = 'NEAR',
  ETH = 'ETH',
  BNB = 'BNB',
  ARB = 'ARB',
  ZK = 'ZK',
  STARKNET = 'STARKNET',
  ICP = 'ICP',
}

export interface ContextClientKeysList {
  clientKeys: ClientKey[];
}

export interface ContextUsersList {
  identities: string[];
}

export interface Identity {
  identity: string;
}

export interface Application {
  id: string;
  blob: string;
  version: string | null;
  source: string;
  contract_app_id: string | null;
  name: string | null;
  description: string | null;
  repository: string | null;
  owner: string | null;
}

export interface InstalledApplication {
  id: string;
  blob: string;
  version: string | null;
  source: string;
  metadata: number[];
}

export interface SigningKey {
  signingKey: string;
}

// This is most likely obsolete
export interface Context {
  applicationId: string;
  id: string;
  rootHash: String;
}

export interface CreateContextResponse {
  contextId: string;
  memberPublicKey: SigningKey;
}

export interface GetContextsResponse {
  contexts: Context[];
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

export interface StarknetRootKey extends RootKey {
  type: String;
}

export interface IcpRootKey extends RootKey {
  type: Network.ICP;
}

interface NetworkType {
  type: Network;
  chainId?: number;
  walletName?: string;
}

export interface ApiRootKey {
  signing_key: string;
  wallet: NetworkType;
  created_at: number;
}

export interface ClientKey {
  signingKey: string;
  wallet: NetworkType;
  createdAt: number;
  applicationId: string;
}

interface Did {
  client_keys: ClientKey[];
  contexts: Context[];
  root_keys: ApiRootKey[];
}

export interface DidResponse {
  did: Did;
}

export interface GetInstalledApplicationsResponse {
  apps: InstalledApplication[];
}

export interface HealthRequest {
  url: String;
}

export interface HealthStatus {
  status: String;
}

export interface ContextStorage {
  sizeInBytes: number;
}

export interface DeleteContextResponse {
  isDeleted: boolean;
}

export interface JoinContextResponse {
  data: null;
}

export interface SignatureMessage {
  nodeSignature: String;
  publicKey: String;
}

export interface SignatureMessageMetadata {
  publicKey: String;
  nodeSignature: String;
  nonce: String;
  timestamp: number;
  message: string; //signed message by wallet
}

interface WalletTypeBase<T extends Uppercase<string>> {
  type: T;
}

interface ETHWalletType extends WalletTypeBase<'ETH'> {
  chainId: number;
}

interface NEARWalletType extends WalletTypeBase<'NEAR'> {
  networkId: string;
}

interface SNWalletType extends WalletTypeBase<'STARKNET'> {
  walletName: string;
}

interface IcpWalletType extends WalletTypeBase<'ICP'> {
  canisterId: string;
}
// TODO: Legacy code, refacture to be used as Interface
export type WalletType =
  | ETHWalletType
  | NEARWalletType
  | SNWalletType
  | IcpWalletType;

// eslint-disable-next-line @typescript-eslint/no-redeclare
export namespace WalletType {
  export function NEAR({
    networkId = 'mainnet',
  }: {
    networkId?: string;
  }): WalletType {
    return { type: 'NEAR', networkId } as NEARWalletType;
  }

  export function ETH({ chainId = 1 }: { chainId?: number }): WalletType {
    return { type: 'ETH', chainId } as ETHWalletType;
  }

  export function STARKNET({
    walletName = 'MS',
  }: {
    walletName?: string;
  }): WalletType {
    return { type: 'STARKNET', walletName } as SNWalletType;
  }

  // ID of production ICP canister used for signing messages
  const IcpCanisterId = 'rdmx6-jaaaa-aaaaa-aaadq-cai';

  export function ICP({
    canisterId = IcpCanisterId,
    walletName = 'II',
  }: {
    canisterId?: string;
    walletName?: string;
  }): WalletType {
    return {
      type: 'ICP',
      canisterId,
      walletName,
    } as IcpWalletType;
  }
}

export interface WalletMetadata {
  wallet: WalletType;
  verifyingKey: String;
  walletAddress?: String;
  networkMetadata?: NetworkMetadata;
}

export interface NetworkMetadata {
  chainId: String;
  rpcUrl: String;
  canisterId?: String;
}

export interface Payload {
  message: SignatureMessageMetadata;
  metadata: SignatureMetadata;
}

export interface SignData {
  signature: Signature;
  messageHash: String;
}

export interface LoginRequest {
  walletSignature: SignData | string;
  payload: Payload;
  walletMetadata: WalletMetadata;
}

export interface LoginResponse {}
export interface RootKeyResponse {}
export interface SignatureMetadata {}

export interface NodeChallenge {
  nonce: String;
  contextId: String;
  timestamp: number;
  nodeSignature: String;
}

export interface NearSignatureMessageMetadata extends SignatureMetadata {
  recipient: String;
  callbackUrl: String;
  nonce: String;
}

export interface EthSignatureMessageMetadata extends SignatureMetadata {}

export interface StarknetSignatureMessageMetadata extends SignatureMetadata {}

export interface IcpSignatureMessageMetadata extends SignatureMetadata {}

export interface WalletSignatureData {
  payload: Payload | undefined;
  publicKey: String | undefined;
}

export interface InstallApplicationResponse {
  applicationId: string;
}

export interface UninstallApplicationResponse
  extends InstallApplicationResponse {}

export interface ContextIdentitiesResponse {
  identities: string[];
}

export interface JsonWebToken {
  accessToken: string;
  refreshToken: string;
}

export interface CreateTokenResponse {
  data: JsonWebToken;
}

export interface CapabilitiesRequest {
  capabilities: Array<[string, string]>; // [ContextIdentity, Capability]
  signer_id: string;
}

export class NodeDataSource implements NodeApi {
  private client: HttpClient;

  constructor(client: HttpClient) {
    this.client = client;
  }

  async getInstalledApplications(): ApiResponse<GetInstalledApplicationsResponse> {
    try {
      const headers: Header | null = await createAuthHeader(
        getAppEndpointKey() as string,
        getNearEnvironment(),
      );
      const response: ResponseData<GetInstalledApplicationsResponse> =
        await this.client.get<GetInstalledApplicationsResponse>(
          `${getAppEndpointKey()}/admin-api/applications`,
          headers ?? {},
        );
      return response;
    } catch (error) {
      console.error('Error fetching installed applications:', error);
      return {
        error: {
          code: 500,
          message: 'Failed to fetch installed applications.',
        },
      };
    }
  }

  async getInstalledApplicationDetails(
    appId: string,
  ): ApiResponse<InstalledApplication> {
    try {
      const headers: Header | null = await createAuthHeader(
        getAppEndpointKey() as string,
        getNearEnvironment(),
      );
      const response: ResponseData<InstalledApplication> =
        await this.client.get<InstalledApplication>(
          `${getAppEndpointKey()}/admin-api/applications/${appId}`,
          headers ?? {},
        );
      return response;
    } catch (error) {
      console.error('Error fetching installed application:', error);
      return {
        error: {
          code: 500,
          message: 'Failed to fetch installed application.',
        },
      };
    }
  }

  async getContexts(): ApiResponse<GetContextsResponse> {
    try {
      const headers: Header | null = await createAuthHeader(
        getAppEndpointKey() as string,
        getNearEnvironment(),
      );
      const response = await this.client.get<GetContextsResponse>(
        `${getAppEndpointKey()}/admin-api/contexts`,
        headers ?? {},
      );
      return response;
    } catch (error) {
      console.error('Error fetching contexts:', error);
      return { error: { code: 500, message: 'Failed to fetch context data.' } };
    }
  }

  async getContext(contextId: string): ApiResponse<Context> {
    try {
      const headers: Header | null = await createAuthHeader(
        contextId,
        getNearEnvironment(),
      );
      const response = await this.client.get<Context>(
        `${getAppEndpointKey()}/admin-api/contexts/${contextId}`,
        headers ?? {},
      );
      return response;
    } catch (error) {
      console.error('Error fetching context:', error);
      return { error: { code: 500, message: 'Failed to fetch context data.' } };
    }
  }

  async getContextClientKeys(
    contextId: string,
  ): ApiResponse<ContextClientKeysList> {
    try {
      const headers: Header | null = await createAuthHeader(
        contextId,
        getNearEnvironment(),
      );
      const response = await this.client.get<ContextClientKeysList>(
        `${getAppEndpointKey()}/admin-api/contexts/${contextId}/client-keys`,
        headers ?? {},
      );
      return response;
    } catch (error) {
      console.error('Error fetching context:', error);
      return {
        error: { code: 500, message: 'Failed to fetch context client keys.' },
      };
    }
  }

  async getContextUsers(contextId: string): ApiResponse<ContextUsersList> {
    try {
      const headers: Header | null = await createAuthHeader(
        contextId,
        getNearEnvironment(),
      );
      const response = await this.client.get<ContextUsersList>(
        `${getAppEndpointKey()}/admin-api/contexts/${contextId}/identities`,
        headers ?? {},
      );
      return response;
    } catch (error) {
      console.error('Error fetching context:', error);
      return {
        error: { code: 500, message: 'Failed to fetch context users.' },
      };
    }
  }

  async getContextStorageUsage(contextId: string): ApiResponse<ContextStorage> {
    try {
      const headers: Header | null = await createAuthHeader(
        contextId,
        getNearEnvironment(),
      );
      const response = await this.client.get<ContextStorage>(
        `${getAppEndpointKey()}/admin-api/contexts/${contextId}/storage`,
        headers ?? {},
      );
      return response;
    } catch (error) {
      console.error('Error fetching context storage usage:', error);
      return {
        error: { code: 500, message: 'Failed to fetch context storage usage.' },
      };
    }
  }

  async deleteContext(contextId: string): ApiResponse<DeleteContextResponse> {
    try {
      const headers: Header | null = await createAuthHeader(
        contextId,
        getNearEnvironment(),
      );
      const response = await this.client.delete<DeleteContextResponse>(
        `${getAppEndpointKey()}/admin-api/contexts/${contextId}`,
        headers ?? {},
      );
      return response;
    } catch (error) {
      console.error('Error deleting context:', error);
      return { error: { code: 500, message: 'Failed to delete context.' } };
    }
  }

  async createContexts(
    applicationId: string,
    initArguments: string,
    protocol: string,
  ): ApiResponse<CreateContextResponse> {
    try {
      const headers: Header | null = await createAuthHeader(
        JSON.stringify({
          applicationId,
          initArguments,
        }),
        getNearEnvironment(),
      );
      const encoder = new TextEncoder();
      const encodedArgs = encoder.encode(JSON.stringify(initArguments));
      const initializationParams = Array.from(encodedArgs);

      const response = await this.client.post<CreateContextResponse>(
        `${getAppEndpointKey()}/admin-api/contexts`,
        {
          applicationId,
          initializationParams: initializationParams,
          protocol,
        },
        headers ?? {},
      );
      return response;
    } catch (error) {
      console.error('Error starting contexts:', error);
      return { error: { code: 500, message: 'Failed to start context.' } };
    }
  }

  async getDidList(): ApiResponse<DidResponse> {
    try {
      const headers: Header | null = await createAuthHeader(
        getAppEndpointKey() as string,
        getNearEnvironment(),
      );
      const response = await this.client.get<DidResponse>(
        `${getAppEndpointKey()}/admin-api/did`,
        headers ?? {},
      );
      return response;
    } catch (error) {
      console.error('Error fetching root keys:', error);
      return { error: { code: 500, message: 'Failed to fetch root keys.' } };
    }
  }

  async health(request: HealthRequest): ApiResponse<HealthStatus> {
    return await this.client.get<HealthStatus>(
      `${request.url}/admin-api/health`,
    );
  }

  async installApplication(
    selectedPackageId: string,
    selectedVersion: string,
    ipfsPath: string,
    hash: string,
  ): ApiResponse<InstallApplicationResponse> {
    try {
      const headers: Header | null = await createAuthHeader(
        JSON.stringify({
          selectedPackageId,
          selectedVersion,
          hash,
        }),
        getNearEnvironment(),
      );

      const response: ResponseData<InstallApplicationResponse> =
        await this.client.post<InstallApplicationResponse>(
          `${getAppEndpointKey()}/admin-api/install-application`,
          {
            url: ipfsPath,
            version: selectedVersion,
            // TODO: parse hash to format
            metadata: createAppMetadata(selectedPackageId),
          },
          headers ?? {},
        );
      return response;
    } catch (error) {
      console.error('Error installing application:', error);
      return {
        error: { code: 500, message: 'Failed to install application.' },
      };
    }
  }

  async uninstallApplication(
    applicationId: string,
  ): ApiResponse<UninstallApplicationResponse> {
    try {
      const headers: Header | null = await createAuthHeader(
        JSON.stringify({
          applicationId,
        }),
        getNearEnvironment(),
      );

      const response: ResponseData<UninstallApplicationResponse> =
        await this.client.post<UninstallApplicationResponse>(
          `${getAppEndpointKey()}/admin-api/uninstall-application`,
          {
            applicationId,
          },
          headers ?? {},
        );
      return response;
    } catch (error) {
      console.error('Error uninstalling application:', error);
      return {
        error: { code: 500, message: 'Failed to uninstall application.' },
      };
    }
  }

  async joinContext(contextId: string): ApiResponse<JoinContextResponse> {
    try {
      const headers: Header | null = await createAuthHeader(
        contextId,
        getNearEnvironment(),
      );
      const response = await this.client.post<JoinContextResponse>(
        `${getAppEndpointKey()}/admin-api/contexts/${contextId}/join`,
        {},
        headers ?? {},
      );
      return response;
    } catch (error) {
      console.error(`${t.joinContextErrorTitle}: ${error}`);
      return { error: { code: 500, message: t.joinContextErrorMessage } };
    }
  }
  async login(loginRequest: LoginRequest): ApiResponse<LoginResponse> {
    return await this.client.post<LoginRequest>(
      `${getAppEndpointKey()}/admin-api/add-client-key`,
      {
        ...loginRequest,
      },
    );
  }
  async requestChallenge(): ApiResponse<NodeChallenge> {
    return await this.client.post<NodeChallenge>(
      `${getAppEndpointKey()}/admin-api/request-challenge`,
      {},
    );
  }
  async addRootKey(rootKeyRequest: LoginRequest): ApiResponse<RootKeyResponse> {
    const headers: Header | null = await createAuthHeader(
      JSON.stringify(rootKeyRequest),
      getNearEnvironment(),
    );
    if (!headers) {
      return { error: { code: 401, message: 'Unauthorized' } };
    }

    return await this.client.post<LoginRequest>(
      `${getAppEndpointKey()}/admin-api/root-key`,
      {
        ...rootKeyRequest,
      },
      headers,
    );
  }

  async getContextIdentity(
    contextId: string,
  ): ApiResponse<ContextIdentitiesResponse> {
    const headers: Header | null = await createAuthHeader(
      JSON.stringify(contextId),
      getNearEnvironment(),
    );
    if (!headers) {
      return { error: { code: 401, message: t.unauthorizedErrorMessage } };
    }

    return await this.client.get<ContextIdentitiesResponse>(
      `${getAppEndpointKey()}/admin-api/contexts/${contextId}/identities-owned`,
      headers,
    );
  }

  async createAccessToken(
    contextId: string,
    contextIdentity: string,
  ): ApiResponse<CreateTokenResponse> {
    const headers: Header | null = await createAuthHeader(
      JSON.stringify(contextId),
      getNearEnvironment(),
    );
    if (!headers) {
      return { error: { code: 401, message: t.unauthorizedErrorMessage } };
    }

    return await this.client.post<CreateTokenResponse>(
      `${getAppEndpointKey()}/admin-api/generate-jwt-token`,
      {
        contextId,
        executorPublicKey: contextIdentity,
      },
      headers,
    );
  }

  async grantCapabilities(
    contextId: string,
    request: CapabilitiesRequest,
  ): ApiResponse<void> {
    try {
      const headers: Header | null = await createAuthHeader(
        contextId,
        getNearEnvironment(),
      );
      if (!headers) {
        return { error: { code: 401, message: t.unauthorizedErrorMessage } };
      }

      const data = {
        capabilities: request.capabilities,
        signer_id: request.signer_id,
      };
      const url = `/contexts/${contextId}/capabilities/grant`;
      const response: ResponseData<void> = await this.client.post<void>(
        `${getAppEndpointKey()}/admin-api${url}`,
        data,
        headers,
      );
      return response;
    } catch (error) {
      console.error('Error granting capabilities:', error);
      return { error: { code: 500, message: 'Failed to grant capabilities.' } };
    }
  }

  async revokeCapabilities(
    contextId: string,
    request: CapabilitiesRequest,
  ): ApiResponse<void> {
    try {
      const headers: Header | null = await createAuthHeader(
        contextId,
        getNearEnvironment(),
      );
      if (!headers) {
        return { error: { code: 401, message: t.unauthorizedErrorMessage } };
      }

      const data = {
        capabilities: request.capabilities,
        signer_id: request.signer_id,
      };
      const url = `/contexts/${contextId}/capabilities/revoke`;
      const response: ResponseData<void> = await this.client.post<void>(
        `${getAppEndpointKey()}/admin-api${url}`,
        data,
        headers,
      );
      return response;
    } catch (error) {
      console.error('Error revoking capabilities:', error);
      return {
        error: { code: 500, message: 'Failed to revoke capabilities.' },
      };
    }
  }
}
