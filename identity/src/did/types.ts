export interface VerifiableCredentialsMetadata {
  id: string;
  subject: string;
}

export enum IdentityType {
  NEAR_WALLET,
  ETH_WALLET,
  NODE,
}
