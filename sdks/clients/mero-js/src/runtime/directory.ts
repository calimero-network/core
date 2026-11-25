/**
 * The Calimero Cloud node directory: which nodes of a cluster take traffic now,
 * and in what order this client tries them.
 *
 * `GET {directory}/{cluster}` (no auth) answers
 * `{ cluster, nodes: [{ id, url, zone, weight }], refreshAfterSeconds }`. The
 * directory is not trusted for security: it only picks which nodes to try.
 * Every node is attested under the caller's own policy before anything is sent
 * to it, so a bad list can only make connecting fail.
 */

import { DEFAULT_RELEASE_MIRROR } from '../sealed/release.js';

/** The Calimero Cloud control plane's public node directory. */
export const DEFAULT_DIRECTORY = 'https://manager.cloud.calimero.network/api/directory';

/** A Calimero Cloud: production, or staging, which runs its own control plane and debug node images. */
export type CloudEnvironment = 'production' | 'staging';

/**
 * One Calimero Cloud: where its directory and its mirrors are, where it serves
 * frontends, and which node images it runs.
 */
export interface CloudEnvironmentInfo {
  directory: string;
  /**
   * The Cloud's public site, which mirrors what a browser needs to attest a
   * node and cannot read from the source (no CORS there): signed node releases
   * (`{cloudUrl}/api/tee/node-releases/{version}`) and Intel's collateral
   * (`{cloudUrl}/api/tee/pcs`). Only a transport: what it serves is verified
   * (Sigstore, Intel's signatures), so it can fail to answer but cannot make a
   * node trusted. The default `releaseSource` and `collateralSource` of a page
   * on this Cloud.
   */
  cloudUrl: string;
  appsDomain: string;
  /**
   * What to add to `policy.release` to trust the images this Cloud's clusters
   * run: nothing for production (`locked-read-only` only); for staging, which
   * allocates all three profiles, the list and the debug opt-in.
   */
  trust: { profiles?: string[]; allowDebugProfile?: boolean };
}

/**
 * Where each Calimero Cloud keeps its node directory and its mirrors, serves
 * app frontends (`{label}.{appsDomain}`, on shared and dedicated clusters
 * alike), and what a client adds to its release policy to trust its nodes.
 *
 * A page on a Cloud's apps domain ({@link cloudEnvironment}) defaults to that
 * Cloud's `directory` and `cloudUrl`; anywhere else (another host, Node)
 * defaults to production's.
 */
export const CLOUD_ENVIRONMENTS: Readonly<Record<CloudEnvironment, Readonly<CloudEnvironmentInfo>>> = {
  production: {
    directory: DEFAULT_DIRECTORY,
    cloudUrl: DEFAULT_RELEASE_MIRROR,
    appsDomain: 'apps.calimero.network',
    trust: {},
  },
  staging: {
    directory: 'https://manager.staging.cloud.calimero.network/api/directory',
    cloudUrl: 'https://staging.cloud.calimero.network',
    appsDomain: 'apps.relay.staging.cloud.calimero.network',
    trust: { profiles: ['locked-read-only', 'debug', 'debug-read-only'], allowDebugProfile: true },
  },
};

/**
 * The Calimero Cloud whose apps domain `hostname` is under (by default the
 * page's own, `location.hostname`), or `null` for any other host: a custom
 * domain, Vercel, `localhost`, or no page at all.
 *
 * Read from the hostname alone, which the browser has authenticated over TLS,
 * and never from anything a server answers: a page may use it to decide what
 * it trusts (`...CLOUD_ENVIRONMENTS[cloudEnvironment() ?? 'production'].trust` in
 * `policy.release`), and a
 * production page cannot be made to think it runs on staging.
 */
export function cloudEnvironment(hostname?: string): CloudEnvironment | null {
  const host = (hostname ?? (globalThis as { location?: { hostname?: string } }).location?.hostname ?? '')
    .toLowerCase()
    .replace(/\.$/, '');
  for (const [env, { appsDomain }] of Object.entries(CLOUD_ENVIRONMENTS) as Array<[CloudEnvironment, { appsDomain: string }]>) {
    if (host.endsWith(`.${appsDomain}`)) return env;
  }
  return null;
}

/**
 * The Calimero Cloud a client defaults to: the one whose apps domain the page
 * is under ({@link cloudEnvironment}), production anywhere else. Read from the
 * page's hostname alone, so only the page's host can pick staging.
 */
export function defaultCloudEnvironment(): CloudEnvironment {
  return cloudEnvironment() ?? 'production';
}

/** How long a listing stands when the directory does not say. */
const DEFAULT_REFRESH_SECONDS = 60;

/** Why {@link DirectoryError} was thrown. */
export type DirectoryFailure = 'unreachable' | 'not-found' | 'bad-response' | 'empty';

/**
 * The directory could not name a node to try: it could not be reached or
 * answered an error (`unreachable`), does not know the cluster (`not-found`,
 * a 404), answered something that is not a listing (`bad-response`), or listed
 * no usable node (`empty`).
 */
export class DirectoryError extends Error {
  constructor(
    readonly cluster: string,
    readonly reason: DirectoryFailure,
    message: string,
    readonly status?: number,
    readonly cause?: unknown,
  ) {
    super(`directory for cluster ${cluster}: ${message}`);
    this.name = 'DirectoryError';
  }
}

/** One usable node of a listing. */
export interface DirectoryNode {
  url: string;
  zone?: string;
  weight: number;
}

export interface DirectoryListing {
  nodes: DirectoryNode[];
  refreshAfterSeconds: number;
}

/** The URL of `cluster`'s listing in `directory`. */
export function directoryUrl(directory: string, cluster: string): string {
  return `${directory.replace(/\/+$/, '')}/${encodeURIComponent(cluster)}`;
}

/**
 * Read `cluster`'s listing, keeping only the entries with an http(s) URL and a
 * positive integer weight (the first of a repeated URL).
 */
export async function readDirectory(directory: string, cluster: string, fetcher: typeof fetch): Promise<DirectoryListing> {
  let response: Response;
  try {
    response = await fetcher(directoryUrl(directory, cluster), { method: 'GET', headers: { accept: 'application/json' } });
  } catch (error) {
    throw new DirectoryError(cluster, 'unreachable', `could not be reached: ${String(error)}`, undefined, error);
  }
  if (response.status === 404) throw new DirectoryError(cluster, 'not-found', 'unknown cluster', 404);
  if (!response.ok) throw new DirectoryError(cluster, 'unreachable', `answered ${response.status}`, response.status);
  let body: unknown;
  try {
    body = await response.json();
  } catch (error) {
    throw new DirectoryError(cluster, 'bad-response', 'answered something other than JSON', response.status, error);
  }
  const listing = body as { nodes?: unknown; refreshAfterSeconds?: unknown } | null;
  if (!listing || typeof listing !== 'object' || !Array.isArray(listing.nodes)) {
    throw new DirectoryError(cluster, 'bad-response', 'answered no node list', response.status);
  }
  const nodes: DirectoryNode[] = [];
  const seen = new Set<string>();
  for (const entry of listing.nodes) {
    const node = usable(entry);
    if (node && !seen.has(node.url)) {
      seen.add(node.url);
      nodes.push(node);
    }
  }
  if (nodes.length === 0) throw new DirectoryError(cluster, 'empty', 'lists no usable node', response.status);
  const after = listing.refreshAfterSeconds;
  const refreshAfterSeconds =
    typeof after === 'number' && Number.isFinite(after) && after >= 1 ? after : DEFAULT_REFRESH_SECONDS;
  return { nodes, refreshAfterSeconds };
}

function usable(entry: unknown): DirectoryNode | undefined {
  if (!entry || typeof entry !== 'object') return undefined;
  const { url, zone, weight } = entry as Record<string, unknown>;
  if (typeof url !== 'string' || typeof weight !== 'number' || !Number.isInteger(weight) || weight < 1) return undefined;
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return undefined;
  }
  if (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') return undefined;
  return { url: url.replace(/\/+$/, ''), zone: typeof zone === 'string' ? zone : undefined, weight };
}

/**
 * The order to try `nodes` in: those in `zone` first (when given), each group
 * in weighted-random order, a node's weight its relative chance to come next.
 * `random` answers in [0, 1), as `Math.random` does.
 */
export function orderNodes(nodes: DirectoryNode[], zone: string | undefined, random: () => number): string[] {
  const near = zone === undefined ? [] : nodes.filter((node) => node.zone === zone);
  const far = zone === undefined ? nodes : nodes.filter((node) => node.zone !== zone);
  return [...weightedOrder(near, random), ...weightedOrder(far, random)];
}

function weightedOrder(nodes: DirectoryNode[], random: () => number): string[] {
  const left = [...nodes];
  const order: string[] = [];
  while (left.length > 0) {
    let roll = random() * left.reduce((sum, node) => sum + node.weight, 0);
    let pick = 0;
    while (pick < left.length - 1 && roll >= left[pick].weight) roll -= left[pick++].weight;
    order.push(left.splice(pick, 1)[0].url);
  }
  return order;
}
