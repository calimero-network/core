import {
  createAgent,
  IDIDManager,
  IResolver,
  IDataStore,
  IKeyManager,
  ICredentialPlugin,
} from '@veramo/core';

// Core identity manager plugin
import {DIDManager} from '@veramo/did-manager';

// Web did identity provider
import {WebDIDProvider} from '@veramo/did-provider-web';

// Storage plugin using TypeOrm
import {
  Entities,
  KeyStore,
  DIDStore,
  PrivateKeyStore,
  migrations,
} from '@veramo/data-store';

// Core key manager plugin
import {KeyManager} from '@veramo/key-manager';

// Custom key management system for RN
import {KeyManagementSystem, SecretBox} from '@veramo/kms-local';

// TypeORM is installed with `@veramo/data-store`
import {DataSource} from 'typeorm';

// W3C Verifiable Credential plugin
import {CredentialPlugin} from '@veramo/credential-w3c';

// Custom resolvers
import {DIDResolverPlugin} from '@veramo/did-resolver';
import {Resolver} from 'did-resolver';
import {getResolver as webDidResolver} from 'web-did-resolver';
import {DhtDidProvider} from '../dht/DhtDidProvider.js';

// This will be the name for the local sqlite database for demo purposes
const DATABASE_FILE = 'database.sqlite';

const dbConnection = new DataSource({
  type: 'sqlite',
  database: DATABASE_FILE,
  synchronize: false,
  migrations,
  migrationsRun: true,
  logging: ['error', 'info', 'warn'],
  entities: Entities,
}).initialize();

// This will be the secret key for the KMS
const KMS_SECRET_KEY =
  '11b574d316903ced6cc3f4787bbcc3047d9c72d1da4d83e36fe714ef785d10c1';

export const agent = createAgent<
  IDIDManager & IKeyManager & IDataStore & IResolver & ICredentialPlugin
>({
  plugins: [
    new KeyManager({
      store: new KeyStore(dbConnection),
      kms: {
        local: new KeyManagementSystem(
          new PrivateKeyStore(dbConnection, new SecretBox(KMS_SECRET_KEY)),
        ),
      },
    }),
    new DIDManager({
      store: new DIDStore(dbConnection),
      defaultProvider: 'did:cali',
      providers: {
        'did:cali': new DhtDidProvider({
          defaultKms: 'local',
        }),
        'did:web': new WebDIDProvider({
          defaultKms: 'local',
        }),
      },
    }),
    new DIDResolverPlugin({
      resolver: new Resolver({
        ...webDidResolver(),
      }),
    }),
    new CredentialPlugin(),
  ],
});
