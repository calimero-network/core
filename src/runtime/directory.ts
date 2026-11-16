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

/** The Calimero Cloud control plane's public node directory. */
export const DEFAULT_DIRECTORY = 'https://manager.cloud.calimero.network/api/directory';

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
