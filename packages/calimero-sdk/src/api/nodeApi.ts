import { ApiResponse } from '../types/api-response';

enum AlgorithmType {
  Ed25519,
}

interface WalletTypeBase<T extends Uppercase<string>> {
  type: T;
}

interface ETHWalletType extends WalletTypeBase<'ETH'> {
  chainId: number;
}

interface NEARWalletType extends WalletTypeBase<'NEAR'> {}

export type WalletType = ETHWalletType | NEARWalletType;

export namespace WalletType {
  export let NEAR: WalletType = { type: 'NEAR' } as NEARWalletType;

  export function ETH({ chainId = 1 }: { chainId?: number }): WalletType {
    return { type: 'ETH', chainId } as ETHWalletType;
  }
}

enum VerifiableCredentialType {
  Wallet,
}

interface WalletVerifiableCredential {
  wallet_type: WalletType;
  address: String;
  public_key: number[];
  peer_id: String;
}
interface VerifiableCredential {
  algorithm_type: AlgorithmType;
  credential_subject: VerifiableCredentialType | WalletVerifiableCredential;
  proof: number[];
}
// @ts-ignore
interface VerifiablePresentation {
  challenge: String;
  verifiable_credential: VerifiableCredential;
  signature: number[];
}

export interface LoginRequest {
  walletSignature: String;
  payload: Payload;
  walletMetadata: WalletMetadata;
  contextId: String;
}

export interface RootKeyRequest {
  walletSignature: String;
  payload: Payload;
  walletMetadata: WalletMetadata;
}

export interface NodeChallenge {
  nonce: String;
  contextId: String;
  timestamp: number;
  nodeSignature: String;
}

export interface NearMetadata extends WalletMetadata {
  type: NEARWalletType;
  signingKey: 'e.g.: ed25519:DfRy7qn3upQS4KFTLChpMG9DmiR29zDMdR1YuUG7cYML';
}

export interface EthMetadata extends WalletMetadata {
  type: ETHWalletType;
  signingKey: 'e.g.: 0x63f9a92d8d61b48a9fff8d58080425a3012d05c8';
}

export interface SignatureMessage {
  nodeSignature: String;
  publicKey: String;
}

export interface SignatureMessageMetadata {
  publicKey: String;
  nodeSignature: String;
  nonce: String;
  contextId: String;
  timestamp: number;
  message: string; //signed message by wallet
}

export interface Payload {
  message: SignatureMessageMetadata;
  metadata: SignatureMetadata;
}

export interface WalletMetadata {
  wallet: WalletType;
  signingKey: String;
}

export interface SignatureMetadata {
  //
}

export interface NearSignatureMessageMetadata extends SignatureMetadata {
  recipient: String;
  callbackUrl: String;
  nonce: String;
}

export interface EthSignatureMessageMetadata extends SignatureMetadata {
  //
}

export interface WalletSignatureData {
  payload: Payload | undefined;
  publicKey: String | undefined;
}

export interface LoginResponse {}
export interface RootKeyResponse {}

export interface HealthRequest {
  url: string;
}

export interface HealthStatus {
  status: string;
}

export interface NodeApi {
  login(
    loginRequest: LoginRequest,
    rpcBaseUrl: string,
  ): ApiResponse<LoginResponse>;
  requestChallenge(
    rpcBaseUrl: string,
    contextId: string,
  ): ApiResponse<NodeChallenge>;
  addRootKey(
    rootKeyRequest: RootKeyRequest,
    rpcBaseUrl: string,
  ): ApiResponse<RootKeyResponse>;
  health(request: HealthRequest): ApiResponse<HealthStatus>;
}
