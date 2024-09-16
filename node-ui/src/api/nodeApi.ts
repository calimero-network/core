import { ApiResponse } from '@calimero-is-near/calimero-p2p-sdk';
import {
  ContextStorage,
  Context,
  HealthRequest,
  HealthStatus,
  ContextClientKeysList,
  ContextUsersList,
  GetInstalledApplicationsResponse,
  ApiContext,
  ContextList,
  DidResponse,
  DeleteContextResponse,
  JoinContextResponse,
  LoginRequest,
  LoginResponse,
  NodeChallenge,
  RootKeyResponse,
  InstallApplicationResponse,
  InstalledApplication,
  ContextIdentitiesResponse,
  CreateTokenResponse,
} from './dataSource/NodeDataSource';

export interface NodeApi {
  getInstalledApplications(): ApiResponse<GetInstalledApplicationsResponse>;
  getInstalledApplicationDetails(
    appId: string,
  ): ApiResponse<InstalledApplication>;
  getContexts(): ApiResponse<ContextList>;
  getContext(contextId: string): ApiResponse<ApiContext>;
  getContextClientKeys(contextId: string): ApiResponse<ContextClientKeysList>;
  getContextUsers(contextId: string): ApiResponse<ContextUsersList>;
  deleteContext(contextId: string): ApiResponse<DeleteContextResponse>;
  startContexts(
    applicationId: string,
    initArguments: string,
  ): ApiResponse<Context>;
  getDidList(): ApiResponse<DidResponse>;
  health(request: HealthRequest): ApiResponse<HealthStatus>;
  getContextStorageUsage(contextId: string): ApiResponse<ContextStorage>;
  installApplication(
    selectedPackageId: string,
    selectedVersion: string,
    ipfsPath: string,
    hash: string,
  ): ApiResponse<InstallApplicationResponse>;
  joinContext(contextId: string): ApiResponse<JoinContextResponse>;
  login(loginRequest: LoginRequest): ApiResponse<LoginResponse>;
  requestChallenge(): ApiResponse<NodeChallenge>;
  addRootKey(rootKeyRequest: LoginRequest): ApiResponse<RootKeyResponse>;
  getContextIdentity(contextId: string): ApiResponse<ContextIdentitiesResponse>;
  createAccessToken(
    contextId: string,
    contextIdentity: string,
  ): ApiResponse<CreateTokenResponse>;
}
