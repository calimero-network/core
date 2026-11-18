const LOCAL_HOSTS = new Set(['localhost', '127.0.0.1', '[::1]']);
const NODE_HOST = /^(.+)-n\d+\./; // a Cloud node is `{cluster}-n{k}.{domain}`

/** The cluster a node belongs to (`eu-shared-1`), from its URL; `merod local` for a loopback node, else the host. */
export function clusterName(nodeUrl: string): string {
  let host: string;
  try {
    host = new URL(nodeUrl).hostname;
  } catch {
    return nodeUrl;
  }
  if (LOCAL_HOSTS.has(host)) return 'merod local';
  return NODE_HOST.exec(host)?.[1] ?? host;
}
