import { IdentityType, VerifiableCredentialsMetadata } from './did/types.js';
import { IdentityManager } from './identity.js';
import { DIDResolutionResult } from '@veramo/core-types';

const identityManager = new IdentityManager();
const methodName = process.argv[2];

//test
//tsc && node dist/demo.js 1

const id = 'vuki2';

switch (methodName) {
  case '1':
    console.log('create identity');
    identityManager.createIdentity(id, IdentityType.NODE);
    break;
  case '2':
    identityManager.getIdentifiers();
    break;
  case '3': {
    const credentials: VerifiableCredentialsMetadata = {
      id: id,
      subject: 'vuki.near',
    };
    identityManager.createCredentials(credentials);
    break;
  }
  case '4': {
    const didDocument: DIDResolutionResult =
      await identityManager.getIdentityById('did:cali' + id);
    console.log('didDocument', didDocument);
    break;
  }
  case '5': {
    identityManager.getIdentityById(id);
    break;
  }
  default:
    console.log('Invalid method name.');
}
