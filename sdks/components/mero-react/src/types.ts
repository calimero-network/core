import type {
  CalimeroClient,
  CalimeroSession,
  RetryOptions,
  PasskeyProvider,
} from '@calimero-network/mero-js';
import type { PolicyOptions } from './client';
import type { Failure } from './errors';
import type { SessionStore } from './storage';

/** What `MeroProvider` takes, besides `children`. */
export interface MeroProviderConfig extends PolicyOptions {
  /**
   * The node to talk to. Ignored when `servedByNode` (the page's own origin is
   * the node). Without it (and without `cluster`) the provider uses the node
   * or cluster kept from last time, or waits for `connectToNode` /
   * `connectToCluster`.
   */
  nodeUrl?: string;
  /**
   * Other nodes of the same cluster, tried in order when a node answers
   * `421 WRONG_NODE` or stays unavailable. Each is attested under the same
   * policy first. Only with `nodeUrl`.
   */
  nodes?: string[];
  /**
   * A Calimero Cloud cluster by name (e.g. `eu-shared-1`), the alternative to
   * `nodeUrl`: the client reads the cluster's nodes from the node directory
   * (mero-js `CalimeroClient.connect`) and fails over between them. The
   * directory only picks which nodes to try; each is attested under the
   * policy first. Ignored when `servedByNode`; `nodeUrl` wins when both are
   * given.
   */
  cluster?: string;
  /** With `cluster`: try this zone's nodes first (e.g. `europe-west4-a`). */
  zone?: string;
  /** With `cluster`: the node directory to ask. Default mero-js `DEFAULT_DIRECTORY`. */
  directory?: string;
  /**
   * Set when a node serves this page (`{label}.{apps-domain}`): the node is
   * `window.location.origin`, and a passkey sign-in here opens a session bound
   * to this app's package. `ConnectButton` then skips the node field.
   */
  servedByNode?: boolean;
  /** The app instance hooks use when they are given none. */
  appId?: string;
  /** Where the session is kept. Default: `localStorageSessionStore`. */
  sessionStore?: SessionStore;
  /** The passkey prompt. Default: mero-js `browserPasskey()` (WebAuthn). */
  passkey?: PasskeyProvider;
  /** The `fetch` the client sends attestation and sealed envelopes with. */
  fetch?: typeof fetch;
  /** How calls and views retry a `503` or a dropped connection. */
  retry?: RetryOptions;
  /**
   * Use this client instead of building one (tests, or an app that needs a
   * client configured beyond these props). Session reporting is then the
   * caller's job.
   */
  client?: CalimeroClient;
  /** Called with every failure a hook or a sign-in reports. */
  onError?: (failure: Failure) => void;
}

/** What `useMero()` answers. */
export interface MeroContextValue {
  /**
   * The 1.0 client, or `null` until a node is known (and, for a cluster,
   * until the directory has named its nodes).
   */
  client: CalimeroClient | null;
  /** @deprecated The same object as `client`; kept so `const { mero } = useMero()` still compiles. */
  mero: CalimeroClient | null;
  /** The node the client talks to (for a cluster, the node in use when it connected). */
  nodeUrl: string | null;
  /** The cluster the client was connected to by name, or `null`. */
  cluster: string | null;
  /** The node directory is being read for `cluster`. */
  isConnecting: boolean;
  /** Talk to another node (not when `servedByNode`). Signs out of the previous one locally. */
  connectToNode: (url: string) => void;
  /**
   * Talk to a Calimero Cloud cluster by name (not when `servedByNode`), with
   * failover between its nodes. Signs out of the previous one locally.
   */
  connectToCluster: (name: string) => void;
  /** Forget the node or cluster, and the session. */
  disconnect: () => void;
  /** The passkey session, or `null`. */
  session: CalimeroSession | null;
  /** The signed-in account. */
  accountId: string | null;
  isAuthenticated: boolean;
  /** A sign-in, sign-up or sign-out is under way. */
  isLoading: boolean;
  /** Sign in with an existing passkey. */
  signIn: () => Promise<CalimeroSession | null>;
  /** Create an account with a new passkey, and sign in. */
  signUp: (name: string) => Promise<CalimeroSession | null>;
  /** End this session (on the node too). */
  signOut: () => Promise<void>;
  /** End every session of the account, on every device. */
  signOutEverywhere: () => Promise<void>;
  /** @deprecated Use `signOut`. */
  logout: () => void;
  /** The app instance hooks default to. */
  appId: string | null;
  setAppId: (appId: string | null) => void;
  /** The passkey prompt hooks use for confirmations. */
  passkey: PasskeyProvider;
  servedByNode: boolean;
  /** Whether mock attestation is accepted (`merod local`). */
  allowMock: boolean;
  /** The last sign-in, connection or session failure. */
  failure: Failure | null;
  /** Hooks report what they caught here; a `LoginRequired` ends the session. */
  reportError: (error: unknown) => void;
}
