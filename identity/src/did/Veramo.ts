import { DidIdentifier } from './DidIdentifier.js';
import {
  IIdentifier,
  IVerifyCredentialArgs,
  DIDResolutionResult,
} from '@veramo/core-types';
import { agent } from './setup.js';
import { VerifiableCredentialsMetadata } from './types.js';

export class Veramo implements DidIdentifier {
  async getIdentifier(id: string): Promise<DIDResolutionResult> {
    const didDocument: DIDResolutionResult = await agent.resolveDid({
      didUrl: id,
    });
    return didDocument;
  }

  async createCredentials(
    credential: VerifiableCredentialsMetadata,
  ): Promise<string> {
    const identifier = await agent.didManagerGetByAlias({
      alias: credential.id,
    });

    const verifiableCredential = await agent.createVerifiableCredential({
      credential: {
        issuer: { id: identifier.did },
        credentialSubject: {
          id: credential.id,
          you: credential.subject,
        },
      },
      proofFormat: 'jwt',
    });
    console.log(`New credential created`);
    const json = JSON.stringify(verifiableCredential, null, 2);
    console.log(json);
    return json;
  }

  async verifyCredentials(
    credentials: IVerifyCredentialArgs,
  ): Promise<boolean> {
    const result = await agent.verifyCredential(credentials);
    return result.verified;
  }

  async getIdentifiers(): Promise<string[]> {
    const identifiers = await agent.didManagerFind();
    console.log(`There are ${identifiers.length} identifiers`);
    const identifiersList: string[] = [];
    if (identifiers.length > 0) {
      identifiers.map((id) => {
        identifiers.push(id);
        console.log(id);
        console.log('..................');
      });
    }
    return identifiersList;
  }

  async createIdentifier(id: string): Promise<string> {
    //create identifier alias per node id

    const identifier: IIdentifier = await agent.didManagerGetOrCreate({
      alias: id,
    });

    console.log(`New identifier created`);
    const json = JSON.stringify(identifier, null, 2);
    console.log(json);
    return json;
  }
}
