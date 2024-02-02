import { loginWithNear } from './index.js';

export enum IdentityType {
  NEAR_WALLET,
  ETH_WALLET,
}

export async function createIdentity(identityType: IdentityType) {
  console.log('Not implemented', identityType);
  switch (identityType) {
    case IdentityType.ETH_WALLET: {
      console.log('Not implemented login with Eth');
      break;
    }
    case IdentityType.NEAR_WALLET: {
      loginWithNear();
      break;
    }
    default: {
      console.error('Invalid identity type', identityType);
    }
  }
}

export async function importIdentity() {
  console.log('Not implemented');
}

export async function deleteIdentity(id: string) {
  console.log('Not implemented', id);
}

export async function getIdentityById(id: string) {
  console.log('Not implemented', id);
}
