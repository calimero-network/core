import { IVerifyCredentialArgs, DIDResolutionResult } from '@veramo/core-types';
import { VerifiableCredentialsMetadata } from './types.js';

export interface DidIdentifier {
  createIdentifier(id: string): Promise<string>;
  getIdentifiers(): Promise<string[]>;
  createCredentials(credential: VerifiableCredentialsMetadata): Promise<string>;
  //TODO create generic type
  verifyCredentials(credentials: IVerifyCredentialArgs): Promise<boolean>;
  getIdentifier(id: string): Promise<DIDResolutionResult>;
}
