/**
 * The check every cloud client runs before it sends anything: the node's quote
 * must be genuine (or, in development, an explicitly allowed mock), from an
 * image this client trusts, and its report data must bind this nonce, the
 * expected app and the transport key the sealed channel will be opened to.
 *
 * Report data, as core's `calimero-cloud-client::attest` builds it:
 * `nonce(32) || SHA-256("calimero.tee-attest.transport-key.v1" | app_hash | transport_key)`.
 *
 * A real quote is judged as the Rust client judges it (`attest::verify` with
 * `Measurements::Release`): the node names the mero-tee release it runs; that
 * release must be at least `minReleaseVersion`; its `published-mrtds.json` is
 * fetched and its Sigstore signature checked against the node release workflow;
 * the quote's signature chain and TCB status are checked against Intel's
 * collateral (fetched here: the node sends none, and cannot forge it); the TD
 * must not be a debug TD; and its registers must be the release's image for the
 * trusted profile, `locked-read-only` unless a debug profile is explicitly
 * allowed. Nothing here reads the environment: mock quotes pass only with
 * `allowMock`.
 */

import { fromHex, hex } from '../crypto/internal.js';
import {
  DEFAULT_COLLATERAL_MIRROR,
  DEFAULT_RELEASE_MIRROR,
  cloudNodeReleaseUrl,
  compareReleaseVersions,
  fetchGithubNodeRelease,
  fetchNodeRelease,
  releaseVersion,
  trustSignedRelease,
  type SignedNodeRelease,
} from '../sealed/release.js';
import { transportKeyBinding, type VerifyTransportQuote } from '../sealed/sealed.js';
import { PRODUCTION_PROFILE, createQuoteVerifier, type DcapCollateral, type DcapVerify } from '../sealed/verify.js';

/** The mock quote format development clusters use. */
export const MOCK_QUOTE_HEADER = 'MOCK_TDX_QUOTE_V1';

/** How the signed release a node names is trusted (core's `Measurements::Release`). */
export interface NodeReleasePolicy {
  /**
   * The oldest mero-tee release trusted, e.g. `2.3.109`. Required: every
   * release ever signed stays validly signed, so without a floor a node could
   * name an old release with a flaw since fixed.
   */
  minReleaseVersion: string;
  /** The image profile trusted; defaults to `locked-read-only`. */
  profile?: string;
  /** Trust a debug profile. Its image is readable by the operator: testing only. */
  allowDebugProfile?: boolean;
  /**
   * Where the release's signed measurements come from: `'github'` (the release
   * assets, what core's Rust client reads; no CORS, so not in browsers), the
   * base URL of a mirror (default {@link DEFAULT_RELEASE_MIRROR}), or a function.
   * Untrusted either way: the Sigstore signature is checked here.
   */
  releaseSource?: 'github' | string | ((version: string) => Promise<SignedNodeRelease>);
  /** DCAP verification; defaults to `verify` from `@phala/dcap-qvl`, loaded on first use. */
  dcapVerify?: DcapVerify;
  /**
   * Intel's collateral for a quote. Defaults to `@phala/dcap-qvl`'s
   * `getCollateral` from `collateralSource`.
   */
  fetchCollateral?: (quote: Uint8Array) => Promise<DcapCollateral>;
  /**
   * Where the default `fetchCollateral` gets Intel's collateral: a PCCS base
   * URL (default {@link DEFAULT_COLLATERAL_MIRROR}, the cloud's mirror, which a
   * browser on any origin can read), or `'intel'` for Intel's PCS itself (no
   * CORS: not in browsers). Untrusted either way: every item is signed by
   * Intel and verified up to Intel's root CA, which `@phala/dcap-qvl` carries.
   */
  collateralSource?: 'intel' | string;
  /**
   * Refuse collateral whose TCB info Intel issued more than this many days
   * ago. Unset: only Intel's own `nextUpdate` (about 30 days out) bounds how
   * old it may be. A mirror cannot forge collateral, but it can replay an
   * older, still valid copy; this shortens that window. Too small a value
   * refuses every node if Intel is slow to re-sign.
   */
  maxCollateralAgeDays?: number;
  /** Override the release signer (defaults to the mero-tee node release workflow). */
  identity?: string;
  issuer?: string;
  /** The current time in milliseconds; defaults to `Date.now`. For tests. */
  now?: () => number;
}

export interface NodePolicy {
  /** Accept mock quotes. Development clusters only; never set from the environment. */
  allowMock?: boolean;
  /** Trust real quotes from the signed release the node runs. */
  release?: NodeReleasePolicy;
  /**
   * Verify a real quote yourself instead (e.g. `createQuoteVerifier` with
   * pinned measurements). Takes precedence over `release`.
   */
  verifyQuote?: VerifyTransportQuote;
  /** The app whose code the node must attest: it binds its own record of the bundle hash, which the record carries. */
  app?: string;
  /** With `app`: the hash the node must bind. Alone: sent and bound, but only echoed, so never recorded. */
  appHash?: string;
}

export interface AttestationDoc {
  quote: string;
  transportKey: string;
  appHash: string;
  nonce: string;
  release: { version: string; profile: string };
  cluster: string;
  node: number;
  mock: boolean;
}

/**
 * Only what the check verified. Sealed means `profile === 'locked-read-only'`; no profile or a mock is not
 * sealed, and no release means no verifier link. `appHash` is the one the node bound for `policy.app`.
 */
export interface NodeVerification {
  url: string;
  mock: boolean;
  release?: string;
  profile?: string;
  appHash?: string;
  /** When the check passed (ms since the epoch). */
  checkedAt: number;
}

function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function equal(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a[i] ^ b[i];
  return diff === 0;
}

type DcapModule = {
  verify?: unknown;
  getCollateral?: unknown;
  getCollateralFromPcs?: unknown;
  default?: { verify?: unknown; getCollateral?: unknown; getCollateralFromPcs?: unknown };
};
let dcapModule: Promise<DcapModule> | undefined;

/** A function out of `@phala/dcap-qvl`, whatever shape the bundler gave the (CommonJS) module. */
async function dcap<T>(name: 'verify' | 'getCollateral' | 'getCollateralFromPcs'): Promise<T> {
  dcapModule ??= import('@phala/dcap-qvl') as Promise<DcapModule>;
  const ns = await dcapModule;
  const f = typeof ns[name] === 'function' ? ns[name] : ns.default?.[name];
  if (typeof f !== 'function') throw new TypeError(`@phala/dcap-qvl loaded without ${name}`);
  return f as T;
}

/**
 * The release a node names, checked against the floor before anything is
 * fetched. Accepts `2.3.109` and the tag form `mero-tee-v2.3.109`.
 */
export function claimedRelease(doc: Pick<AttestationDoc, 'release'>, minReleaseVersion: string): string {
  const floor = releaseVersion(minReleaseVersion, 'minReleaseVersion');
  const raw = String(doc.release?.version ?? '').trim().replace(/^mero-tee-v/, '').replace(/^v/, '');
  const version = releaseVersion(raw, 'The release the node names');
  if (compareReleaseVersions(version, floor) < 0) {
    throw new Error(`mero-tee release ${version} is older than the minimum ${floor}`);
  }
  return version;
}

/** `host` gave no answer: its fetch rejected (`cause`) or it answered a 5xx `status`. Says nothing about the quote. */
export class Unanswered extends Error {
  constructor(
    readonly host: string,
    readonly status?: number,
    readonly cause?: unknown,
  ) {
    super(status ? `${host} answered HTTP ${status}` : `${host} could not be reached: ${(cause as Error)?.message}`);
  }
}

/** `error` as {@link Unanswered} from `host` when it is a fetch's `TypeError`: no connection, or one dropped mid-body. */
function unanswered(host: string, error: unknown): unknown {
  return error instanceof TypeError ? new Unanswered(host, undefined, error) : error;
}

/** A status that is no answer yet: the host is overloaded or failing, not judging the request. */
const unansweredStatus = (status: number) => status === 429 || status >= 500;

/** `fetchImpl`, throwing {@link Unanswered} for `host` instead of a rejection, a dropped body or a 429/5xx. */
function answered(fetchImpl: typeof fetch, host: string): typeof fetch {
  return async (input, init) => {
    const response = await fetchImpl(input, init).catch((error: unknown) => {
      throw unanswered(host, error);
    });
    if (unansweredStatus(response.status)) throw new Unanswered(host, response.status);
    const body = await response.arrayBuffer().catch((error: unknown) => {
      throw unanswered(host, error);
    });
    return new Response(body, response);
  };
}

async function releaseFor(version: string, policy: NodeReleasePolicy, fetchImpl: typeof fetch) {
  const source = policy.releaseSource;
  if (typeof source === 'function') return source(version);
  const fetchMirror = answered(fetchImpl, 'the release mirror');
  if (source === 'github') return fetchGithubNodeRelease(version, { fetch: fetchMirror });
  return fetchNodeRelease(cloudNodeReleaseUrl(source ?? DEFAULT_RELEASE_MIRROR, version), { fetch: fetchMirror });
}

/** The collateral fetch a policy gets unless it brings its own: `@phala/dcap-qvl` against `collateralSource`. */
function defaultCollateral(policy: NodeReleasePolicy): (quote: Uint8Array) => Promise<DcapCollateral> {
  const source = policy.collateralSource ?? DEFAULT_COLLATERAL_MIRROR;
  // dcap-qvl fetches with its own `fetch`: only its call's `TypeError` is no answer, never a bad URL or bundle.
  if (source === 'intel') {
    return async (quote) => {
      const get = await dcap<(q: Uint8Array) => Promise<DcapCollateral>>('getCollateralFromPcs');
      return get(quote).catch((error: unknown) => {
        throw unanswered('Intel PCS', error);
      });
    };
  }
  new URL(source);
  return async (quote) => {
    const get = await dcap<(url: string, q: Uint8Array) => Promise<DcapCollateral>>('getCollateral');
    return get(source, quote).catch((error: unknown) => {
      throw unanswered(source, error);
    });
  };
}

/** `fetch`, refusing collateral older than `policy.maxCollateralAgeDays` (by its TCB info's `issueDate`). */
export function freshCollateral(
  fetch: (quote: Uint8Array) => Promise<DcapCollateral>,
  policy: Pick<NodeReleasePolicy, 'maxCollateralAgeDays' | 'now'>,
): (quote: Uint8Array) => Promise<DcapCollateral> {
  const maxDays = policy.maxCollateralAgeDays;
  if (maxDays === undefined) return fetch;
  return async (quote) => {
    const collateral = await fetch(quote);
    let issued: number;
    try {
      issued = Date.parse((JSON.parse(collateral.tcb_info) as { issueDate?: string }).issueDate ?? '');
    } catch {
      issued = Number.NaN;
    }
    if (!Number.isFinite(issued)) throw new Error("The quote's collateral names no TCB info issue date");
    const ageDays = ((policy.now ?? Date.now)() - issued) / 86_400_000;
    if (ageDays > maxDays) {
      throw new Error(
        `The quote's collateral was issued ${Math.floor(ageDays)} days ago, more than the ${maxDays} this client accepts`,
      );
    }
    return collateral;
  };
}

/** Judge a real quote as core's `Measurements::Release` does. */
async function verifyRelease(
  doc: AttestationDoc,
  args: Parameters<VerifyTransportQuote>[0],
  policy: NodeReleasePolicy,
  fetchImpl: typeof fetch,
): Promise<Pick<NodeVerification, 'release' | 'profile'> | undefined> {
  // Refuse what no release could admit before reaching for the network.
  const version = claimedRelease(doc, policy.minReleaseVersion);
  const profile = policy.profile ?? PRODUCTION_PROFILE;
  const release = await releaseFor(version, policy, fetchImpl);
  if (release.version !== version) {
    throw new Error(`The node names mero-tee release ${version}, but the release served is ${release.version}`);
  }
  const trusted = await trustSignedRelease({
    release,
    profile,
    allowDebugProfile: policy.allowDebugProfile,
    minReleaseVersion: policy.minReleaseVersion,
    identity: policy.identity,
    issuer: policy.issuer,
  });
  const fetchCollateral = freshCollateral(policy.fetchCollateral ?? defaultCollateral(policy), policy);
  const verifier = createQuoteVerifier({
    dcapVerify: policy.dcapVerify ?? (await dcap<DcapVerify>('verify')),
    fetchCollateral,
    now: policy.now,
    ...trusted,
  });
  return (await verifier(args)) ? { release: version, profile } : undefined;
}

/** Fetch the node's attestation and return its transport key once it checks out. */
export async function fetchAttestedKey(
  baseUrl: string,
  policy: NodePolicy,
  fetchImpl: typeof fetch = (input, init) => fetch(input, init),
): Promise<Uint8Array> {
  return (await verifyNode(baseUrl, policy, fetchImpl)).key;
}

/** The app hash the node chose for `policy.app`, which must be real and the pinned one if any. */
function nodeBoundAppHash(doc: AttestationDoc, policy: NodePolicy): Uint8Array {
  const bound = fromHex(doc.appHash, 'appHash', 32);
  if (bound.every((byte) => byte === 0)) throw new Error('The node bound no app: its app hash is all zero');
  if (policy.appHash && hex(bound) !== policy.appHash.toLowerCase()) {
    throw new Error(`The node's app hash ${hex(bound)} is not the pinned ${policy.appHash}`);
  }
  return bound;
}

/** {@link fetchAttestedKey}, with the record of what the check verified. */
export async function verifyNode(
  baseUrl: string,
  policy: NodePolicy,
  fetchImpl: typeof fetch = (input, init) => fetch(input, init),
): Promise<{ key: Uint8Array; verification: NodeVerification }> {
  const url = baseUrl.replace(/\/+$/, '');
  const nonce = hex(crypto.getRandomValues(new Uint8Array(32)));
  const attestation = new URL(`${url}/attestation`);
  attestation.searchParams.set('nonce', nonce);
  if (policy.app) attestation.searchParams.set('app', policy.app);
  else if (policy.appHash) attestation.searchParams.set('appHash', policy.appHash);
  const response = await fetchImpl(attestation.toString());
  if (unansweredStatus(response.status)) throw new Unanswered('the node', response.status);
  if (!response.ok) throw new Error(`The node refused to attest: HTTP ${response.status}`);
  const doc = (await response.json().catch((error: unknown) => {
    throw unanswered('the node', error);
  })) as AttestationDoc;
  const transportKey = fromHex(doc.transportKey, 'transportKey', 32);
  const boundApp = policy.app ? nodeBoundAppHash(doc, policy) : undefined;
  const inner = boundApp ?? (policy.appHash ? fromHex(policy.appHash, 'appHash', 32) : new Uint8Array(32));
  const suffix = await transportKeyBinding(inner, transportKey);
  const quote = base64ToBytes(doc.quote);
  const header = new TextEncoder().encode(MOCK_QUOTE_HEADER);
  const verification = (facts: Pick<NodeVerification, 'mock' | 'release' | 'profile'>): NodeVerification => ({
    url,
    ...facts,
    ...(boundApp ? { appHash: hex(boundApp) } : {}),
    checkedAt: Date.now(),
  });
  if (equal(quote.slice(0, header.length), header)) {
    if (!policy.allowMock) {
      throw new Error('mock quote refused: this client only trusts real TEEs');
    }
    const report = quote.slice(header.length, header.length + 64);
    if (!equal(report.slice(0, 32), fromHex(nonce, 'nonce', 32)) || !equal(report.slice(32), suffix)) {
      throw new Error('the quote does not bind this transport key, app and nonce');
    }
    return { key: transportKey, verification: verification({ mock: true }) };
  }
  const args = { quoteB64: doc.quote, nonce, reportDataSuffix: hex(suffix) };
  let facts: Pick<NodeVerification, 'release' | 'profile'> | undefined;
  if (policy.verifyQuote) {
    facts = (await policy.verifyQuote(args)) ? {} : undefined;
  } else if (policy.release) {
    facts = await verifyRelease(doc, args, policy.release, fetchImpl);
  } else {
    throw new Error('real quote refused: this client trusts no TD image (set policy.release)');
  }
  if (!facts) throw new Error('The attestation did not verify, so its transport key is not trusted');
  return { key: transportKey, verification: verification({ mock: false, ...facts }) };
}
