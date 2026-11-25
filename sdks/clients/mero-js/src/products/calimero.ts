// `Calimero.fromEnv()`: a client from a service's environment, and `kv()`: the generated
// Confidential KV client on it.

import { CalimeroClient } from '../client/calimero-client.js';
import type { NodePolicy } from '../runtime/attestation.js';
import { CLOUD_ENVIRONMENTS, type CloudEnvironment } from '../runtime/directory.js';
import { cloudCollateralUrl } from '../sealed/release.js';
import { KvClient } from './kv/client.js';

/** The environment `fromEnv` reads, `process.env` by default. */
export type CalimeroEnv = Record<string, string | undefined>;

export class Calimero {
  private constructor(
    /** The signed-in client, for every route the KV client does not cover. */
    readonly client: CalimeroClient,
    private readonly store: string | undefined,
  ) {}

  /**
   * A client signed in with `CALIMERO_API_KEY` to `CALIMERO_CLUSTER` (or the dev node `CALIMERO_NODE_URL`),
   * trusting the `CALIMERO_TEE_PROFILE` image (default `locked-read-only`) of releases from `CALIMERO_MIN_RELEASE`.
   * `CALIMERO_ENVIRONMENT=staging` uses staging's directory, release and collateral mirrors and trusts its image
   * profiles, as a page staging serves does (default `production`); `CALIMERO_DIRECTORY` names another directory.
   * Throws naming the variable that is missing or wrong; a mock quote passes only with `CALIMERO_ALLOW_UNATTESTED=1`.
   */
  static async fromEnv(env: CalimeroEnv = globalThis.process?.env ?? {}): Promise<Calimero> {
    const { cluster, nodeUrl, apiKey, policy, environment, directory } = settingsOf(env);
    const client = cluster
      ? await CalimeroClient.connect({ cluster, policy, ...(environment ? { environment } : {}), ...(directory ? { directory } : {}) })
      : new CalimeroClient({ baseUrl: nodeUrl, policy });
    client.useApiKey(apiKey);
    return new Calimero(client, env.CALIMERO_KV_STORE || undefined);
  }

  /** The Confidential KV `store` (its app id; default `CALIMERO_KV_STORE`). */
  kv(store = this.store): KvClient<CalimeroClient> {
    if (!store) throw new Error('CALIMERO_KV_STORE is not set: set it to the store\'s app id, or pass one to kv()');
    return new KvClient(this.client, store);
  }
}

/** The Calimero Cloud `CALIMERO_ENVIRONMENT` names, or `undefined` when it is unset. */
function environmentOf(value: string | undefined): CloudEnvironment | undefined {
  if (!value) return undefined;
  if (value === 'production' || value === 'staging') return value;
  throw new Error(`CALIMERO_ENVIRONMENT must be production or staging (or unset for production), not ${JSON.stringify(value)}`);
}

interface Settings {
  cluster?: string;
  nodeUrl: string;
  apiKey: string;
  policy: NodePolicy;
  environment?: CloudEnvironment;
  directory?: string;
}

/** What `env` says to connect to, or the error naming the variable at fault. */
function settingsOf(env: CalimeroEnv): Settings {
  const { CALIMERO_CLUSTER: cluster, CALIMERO_NODE_URL: nodeUrl, CALIMERO_API_KEY: apiKey, CALIMERO_MIN_RELEASE: minReleaseVersion } = env;
  const { CALIMERO_ALLOW_UNATTESTED: unattested, CALIMERO_TEE_PROFILE: profile, CALIMERO_DIRECTORY: directory } = env;
  const environment = environmentOf(env.CALIMERO_ENVIRONMENT);
  if (cluster && nodeUrl) throw new Error('set CALIMERO_CLUSTER or CALIMERO_NODE_URL, not both');
  if (!cluster && !nodeUrl) throw new Error('CALIMERO_CLUSTER is not set: the cluster to connect to (CALIMERO_NODE_URL for a dev node)');
  if (unattested && unattested !== '1') throw new Error('CALIMERO_ALLOW_UNATTESTED must be 1 or unset');
  if (unattested && cluster) throw new Error('CALIMERO_ALLOW_UNATTESTED is for a dev node at CALIMERO_NODE_URL, never a cluster');
  if (!apiKey) throw new Error('CALIMERO_API_KEY is not set');
  if (!minReleaseVersion && !unattested) {
    throw new Error('CALIMERO_MIN_RELEASE is not set: the oldest mero-tee release to trust, e.g. 2.3.109');
  }
  if (profile && !minReleaseVersion) throw new Error('CALIMERO_TEE_PROFILE needs CALIMERO_MIN_RELEASE: a profile is trusted within a release');
  if (directory && !cluster) throw new Error('CALIMERO_DIRECTORY is for a cluster by name (CALIMERO_CLUSTER), not CALIMERO_NODE_URL');
  if (directory && !/^https?:\/\/[^/]/.test(directory)) throw new Error(`CALIMERO_DIRECTORY must be an http(s) URL, not ${JSON.stringify(directory)}`);
  // As the console's policyFor: naming a debug* profile is the opt-in a debug image needs. Without one, the
  // environment's own trust (staging: its three profiles and the debug opt-in), as a page on that Cloud adds.
  const cloud = environment ? CLOUD_ENVIRONMENTS[environment] : undefined;
  const trusted = profile ? { profile, allowDebugProfile: profile.startsWith('debug') } : { ...cloud?.trust };
  // A service has no page hostname to pick the Cloud from: name its mirrors, as a page on that Cloud defaults to.
  const mirrors = cloud ? { releaseSource: cloud.cloudUrl, collateralSource: cloudCollateralUrl(cloud.cloudUrl) } : {};
  const policy: NodePolicy = {
    ...(unattested ? { allowMock: true } : {}),
    ...(minReleaseVersion ? { release: { minReleaseVersion, ...trusted, ...mirrors } } : {}),
  };
  return {
    cluster,
    nodeUrl: nodeUrl ?? '',
    apiKey,
    policy,
    ...(environment ? { environment } : {}),
    ...(directory ? { directory } : {}),
  };
}
