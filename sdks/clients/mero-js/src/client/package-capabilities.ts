/** What `GET /packages/{package}/{version}/capabilities` answers: a package version's manifest facts and its app's ABI. */

export type AppMethodKind = 'init' | 'call' | 'view' | 'timer' | 'action' | 'compute';

export interface AppAbiMethod {
  kind: AppMethodKind;
  /** `[name, type]` per parameter. */
  params: [string, string][];
  returns: string | null;
  confirm: boolean;
  public: boolean;
  allowSystem: boolean;
  every?: string;
  rate?: string;
  allowApps?: string[];
  allowAction?: boolean;
  compute?: Record<string, unknown>;
  computeResult?: boolean;
  doc?: string;
}

/** The app's `calimero:abi` document. */
export interface AppAbi {
  abiVersion: string;
  world: string;
  state: string;
  methods: Record<string, AppAbiMethod>;
  errors: unknown[];
  events: unknown[];
  types?: unknown;
}

export interface PackageCapabilities {
  package: string;
  version: string;
  service: string | null;
  bundle: string;
  mpk: string;
  manifestHash: string;
  signer: string;
  name: string | null;
  author: string | null;
  /** Hosts the app may call out to. */
  outbound: string[];
  abi: AppAbi | null;
}
