import { DidIdentifier } from './did/DidIdentifier.js';
import { Veramo } from './did/Veramo.js';
import { IdentityType, VerifiableCredentialsMetadata } from './did/types.js';
import { loginWithNear } from './index.js';
import { IVerifyCredentialArgs } from '@veramo/core-types';
import { loginWithEth } from './wallets/eth-wallet.js';

export class IdentityManager {
  private didIdentifier: DidIdentifier = new Veramo();

  async createIdentity(id: string, identityType: IdentityType) {
    switch (identityType) {
      case IdentityType.ETH_WALLET: {
        console.log('Login with eth wallet');
        loginWithEth();
        //TODO Create DID
        //TODO store DID
        break;
      }
      case IdentityType.NEAR_WALLET: {
        console.log('Login with near wallet');
        loginWithNear();
        //TODO Create DID
        //TODO store DID
        break;
      }
      case IdentityType.NODE: {
        console.log('Create node identity');
        await this.didIdentifier.createIdentifier(id);
        break;
      }
      default: {
        console.error('Invalid identity type', identityType);
      }
    }
  }

  async getIdentifiers() {
    return this.didIdentifier.getIdentifiers();
  }

  async createCredentials(credentials: VerifiableCredentialsMetadata) {
    return this.didIdentifier.createCredentials(credentials);
  }

  async verifyCredentials(credentials: IVerifyCredentialArgs) {
    return this.didIdentifier.verifyCredentials(credentials);
  }

  async importIdentity() {
    console.log('Not implemented');
  }

  async deleteIdentity(id: string) {
    console.log('Not implemented', id);
  }

  async getIdentityById(id: string) {
    return this.didIdentifier.getIdentifier(id);
  }
}
