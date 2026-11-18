/**
 * Trusting a TEE node's image by its signed release, fetched at run time from
 * anywhere, instead of shipping the release's `published-mrtds.json` with the
 * app.
 *
 * Each mero-tee node release publishes `published-mrtds.json` (the
 * measurements of its image) with a cosign keyless bundle, signed by the
 * mero-tee release workflow on GitHub Actions. {@link verifySignedNodeRelease}
 * checks that signature here, against the Sigstore trust root embedded in this
 * package, so the file can come from any source: the cloud's mirror
 * (`GET https://cloud.calimero.network/api/tee/node-releases/{version}`), a
 * mirror of your own, or a file bundled with the app. None of them is
 * trusted; a file whose signature does not verify is refused.
 *
 * What is trusted:
 *
 * - the signer: the node release workflow of `calimero-network/mero-tee` on
 *   `master` ({@link NODE_RELEASE_SIGNER}), the identity core pins as
 *   `NODE_RELEASE_IDENTITY`;
 * - the Sigstore public-good trust root: Fulcio's CAs and Rekor's key;
 * - the minimum release you pass, below which no release is accepted.
 *
 * What is not: the transport, the server that serves the release, and the
 * node's choice of release. A node can present any genuinely signed release
 * at or above the minimum, and no other; its quote must then match that
 * release's measurements exactly, so it cannot claim one release and run
 * another.
 */

import { verifyCosignBundle, type SignerIdentity } from './sigstore.js';
import {
  createQuoteVerifier,
  trustedMeasurementsFromReleases,
  type DcapVerify,
  type PublishedMrtds,
  type QuoteVerifier,
  type QuoteVerifierOptions,
  type TrustedProfileOptions,
} from './verify.js';

/** The workflow that signs every mero-tee node release, and the issuer of its identity. */
export const NODE_RELEASE_SIGNER: Readonly<SignerIdentity> = Object.freeze({
  identity:
    'https://github.com/calimero-network/mero-tee/.github/workflows/release-node-image-gcp.yaml@refs/heads/master',
  issuer: 'https://token.actions.githubusercontent.com',
});

const MEASUREMENTS = ['mrtd', 'rtmr0', 'rtmr1', 'rtmr2', 'rtmr3'] as const;

/**
 * The public mirror of signed node releases, used when no other source is
 * given. Only a transport: what it serves is verified, so it can fail to
 * answer but cannot make a release trusted.
 */
export const DEFAULT_RELEASE_MIRROR = 'https://cloud.calimero.network';

/**
 * Where browsers get Intel's collateral for a quote: the cloud's mirror of the
 * PCCS paths `@phala/dcap-qvl` reads, open to any origin. Intel's PCS sends no
 * CORS header. The mirror only carries Intel-signed items.
 */
export const DEFAULT_COLLATERAL_MIRROR = `${DEFAULT_RELEASE_MIRROR}/api/tee/pcs`;

/**
 * A node release as the servers hand it out, wrapped as `{"data": ...}`: the
 * release's `published-mrtds.json`, byte for byte, and its cosign bundle.
 * Unverified until {@link verifySignedNodeRelease} says otherwise.
 */
export interface SignedNodeRelease {
  /** The release, e.g. `2.3.87`. It must be the signed file's `tag`. */
  version: string;
  /** `published-mrtds.json`, exactly as signed. */
  publishedMrtds: string;
  /** `published-mrtds.json.bundle.json`, as JSON text. */
  bundle: string;
}

export interface VerifySignedNodeReleaseOptions {
  /** Require the file's `tag` to be this release. */
  expectedVersion?: string;
  /** The certificate identity required. Defaults to {@link NODE_RELEASE_SIGNER}'s. */
  identity?: string;
  /** The OIDC issuer required. Defaults to {@link NODE_RELEASE_SIGNER}'s. */
  issuer?: string;
  /** The release's detached `published-mrtds.json.sig`, when you have it; it must be the bundle's. */
  signature?: string;
}

/**
 * Verify a node release's `published-mrtds.json` against its cosign bundle,
 * as `cosign verify-blob` would, and return the file parsed. Rejects, with the
 * reason, unless:
 *
 * - the signature over the exact bytes verifies with the bundle's certificate;
 * - that certificate chains to a Fulcio CA in the Sigstore trust root, and it
 *   and the CA were valid when Rekor logged the signature;
 * - it names the node release workflow and GitHub Actions as its issuer;
 * - Rekor's signed entry timestamp verifies, for an entry that is this
 *   signature over this file by this certificate;
 * - the file is a node release (`role: "node"`), and `expectedVersion`, when
 *   given, is its `tag`.
 *
 * Nothing is read from the clock: the certificate is checked at the time Rekor
 * logged the signature, so a release that verifies once always does.
 */
export async function verifySignedNodeRelease(
  publishedMrtds: string | Uint8Array,
  bundle: string | object,
  options: VerifySignedNodeReleaseOptions = {},
): Promise<PublishedMrtds> {
  const body = typeof publishedMrtds === 'string' ? new TextEncoder().encode(publishedMrtds) : publishedMrtds;
  await verifyCosignBundle(
    body,
    bundle,
    {
      identity: options.identity ?? NODE_RELEASE_SIGNER.identity,
      issuer: options.issuer ?? NODE_RELEASE_SIGNER.issuer,
    },
    options.signature,
  );

  let release: PublishedMrtds;
  try {
    release = JSON.parse(new TextDecoder().decode(body));
  } catch {
    throw new Error('The signed release is not JSON');
  }
  if (release?.role !== 'node') {
    throw new Error(`The signed release is a ${JSON.stringify(release?.role)} release, not a node one`);
  }
  // The workflow signs other files too, some with a `role` and `profiles` of
  // their own (`release-provenance.json`): the signature covers bytes, not a
  // file name, so check this is the measurements file.
  const profiles = Object.values(release.profiles ?? {});
  const measured = (image: unknown) =>
    MEASUREMENTS.every((register) => /^[0-9a-f]{96}$/i.test(String((image as Record<string, unknown>)?.[register])));
  if (typeof release.tag !== 'string' || profiles.length === 0 || !profiles.every(measured)) {
    throw new Error("The signed file is not a release's published-mrtds.json: it lists no measured profiles");
  }
  if (options.expectedVersion !== undefined && release.tag !== options.expectedVersion) {
    throw new Error(`The signed release is ${release.tag}, not ${options.expectedVersion}`);
  }
  return release;
}

/**
 * Fetch a {@link SignedNodeRelease} from a server that wraps it as
 * `{"data": ...}`, such as the cloud's {@link cloudNodeReleaseUrl}. It is
 * only fetched: pass it to
 * {@link trustSignedRelease}, or {@link verifySignedNodeRelease}, before using it.
 */
export async function fetchNodeRelease(
  url: string,
  options: { fetch?: typeof fetch } = {},
): Promise<SignedNodeRelease> {
  const response = await (options.fetch ?? fetch)(url, { headers: { accept: 'application/json' } });
  if (!response.ok) throw new Error(`Fetching the node release from ${url} failed: HTTP ${response.status}`);
  let release: Partial<SignedNodeRelease> | undefined;
  try {
    release = ((await response.json()) as { data?: Partial<SignedNodeRelease> })?.data;
  } catch {
    throw new Error(`${url} did not answer with JSON`);
  }
  if (
    typeof release?.version !== 'string' ||
    typeof release.publishedMrtds !== 'string' ||
    typeof release.bundle !== 'string'
  ) {
    throw new Error(`${url} did not answer with a signed node release`);
  }
  return { version: release.version, publishedMrtds: release.publishedMrtds, bundle: release.bundle };
}

/** Where mero-tee publishes its releases' assets, as core's `calimero-tee-release` fetches them. */
export const GITHUB_RELEASES = 'https://github.com/calimero-network/mero-tee/releases/download';

/**
 * Node release `version` straight from its GitHub release (`mero-tee-v{version}`):
 * `published-mrtds.json` and its cosign bundle, the files core's Rust client
 * verifies. Only fetched: verify it with {@link trustSignedRelease}. GitHub's
 * downloads send no CORS headers, so in a browser use a mirror
 * ({@link cloudNodeReleaseUrl}) instead.
 */
export async function fetchGithubNodeRelease(
  version: string,
  options: { fetch?: typeof fetch; base?: string } = {},
): Promise<SignedNodeRelease> {
  const tag = `mero-tee-v${releaseVersion(version, 'version')}`;
  const base = `${(options.base ?? GITHUB_RELEASES).replace(/\/+$/, '')}/${tag}/published-mrtds.json`;
  const get = async (url: string): Promise<string> => {
    const response = await (options.fetch ?? fetch)(url);
    if (!response.ok) throw new Error(`Fetching ${url} failed: HTTP ${response.status}`);
    return response.text();
  };
  const [publishedMrtds, bundle] = await Promise.all([get(base), get(`${base}.bundle.json`)]);
  return { version, publishedMrtds, bundle };
}

/** Where the cloud at `cloudUrl` (e.g. `https://cloud.calimero.network`) mirrors node release `version`. */
export function cloudNodeReleaseUrl(cloudUrl: string, version: string): string {
  return `${cloudUrl.replace(/\/+$/, '')}/api/tee/node-releases/${releaseVersion(version, 'version')}`;
}

export interface TrustSignedReleaseOptions extends TrustedProfileOptions {
  /** The release, from any source; it is verified here. */
  release: SignedNodeRelease;
  /**
   * The oldest release accepted, e.g. `2.3.87`. Required: every release ever
   * signed stays validly signed, so without a floor a node could present an
   * old release, with a flaw since fixed, and a quote from that old image
   * would pass. Raise it when a release must no longer be trusted.
   */
  minReleaseVersion: string;
  /** Override the signer; defaults to {@link NODE_RELEASE_SIGNER}. */
  identity?: string;
  issuer?: string;
}

/**
 * The {@link QuoteVerifierOptions} that trust one profile of a signed node
 * release, from an untrusted source: its signature is verified, its signed
 * `tag` must be `release.version` and no older than `minReleaseVersion`, and
 * then it is read as {@link trustedMeasurementsFromReleases} reads a bundled
 * file.
 *
 * ```ts
 * const version = claimedRelease(attestationDoc, '2.3.109'); // the release the node names
 * const release = await fetchNodeRelease(cloudNodeReleaseUrl(DEFAULT_RELEASE_MIRROR, version));
 * const verifier = createQuoteVerifier({
 *   dcapVerify,
 *   ...(await trustSignedRelease({ release, profile: 'locked-read-only', minReleaseVersion: '2.3.87' })),
 * });
 * ```
 */
export async function trustSignedRelease(
  options: TrustSignedReleaseOptions,
): Promise<Required<Pick<QuoteVerifierOptions, 'allowedMeasurements' | 'allowedTcbStatuses'>>> {
  const minimum = releaseVersion(options.minReleaseVersion, 'minReleaseVersion');
  const version = releaseVersion(options.release?.version, 'The release version');
  const published = await verifySignedNodeRelease(options.release.publishedMrtds, options.release.bundle, {
    expectedVersion: version,
    identity: options.identity,
    issuer: options.issuer,
  });
  if (compareReleaseVersions(version, minimum) < 0) {
    throw new Error(`Release ${version} is older than the minimum trusted, ${minimum}`);
  }
  return trustedMeasurementsFromReleases([published], {
    profile: options.profile,
    allowDebugProfile: options.allowDebugProfile,
  });
}

export interface SignedReleaseVerifierOptions extends Omit<TrustSignedReleaseOptions, 'release'> {
  /** DCAP verification: `verify` from `@phala/dcap-qvl`. */
  dcapVerify: DcapVerify;
  /**
   * Where to get the node's release, asked again each time the node is
   * attested (a restarted node may run a newer one). Untrusted: it is
   * verified, and the quote must match it.
   */
  release: () => Promise<SignedNodeRelease>;
  /** As {@link QuoteVerifierOptions.fetchCollateral}. */
  fetchCollateral?: QuoteVerifierOptions['fetchCollateral'];
  /** The current time in milliseconds, for the quote's collateral; defaults to `Date.now`. For tests. */
  now?: () => number;
}

/**
 * A {@link createQuoteVerifier} whose trusted image is the signed release the
 * node says it runs. On each attestation it gets the release, checks it with
 * {@link trustSignedRelease}, and verifies the quote against that release's
 * measurements only.
 *
 * The node's choice of release is the one thing taken from it, and it is
 * bounded: only a genuinely signed release at or above `minReleaseVersion`
 * passes, and the quote must then be of exactly that release's image. Pass it
 * as `NodePolicy.verifyQuote` to choose where the release comes from yourself.
 */
export function createSignedReleaseVerifier(options: SignedReleaseVerifierOptions): QuoteVerifier {
  releaseVersion(options.minReleaseVersion, 'minReleaseVersion');
  const verifier = async (args: Parameters<QuoteVerifier>[0]): Promise<boolean> => {
    const trusted = await trustSignedRelease({
      release: await options.release(),
      profile: options.profile,
      allowDebugProfile: options.allowDebugProfile,
      minReleaseVersion: options.minReleaseVersion,
      identity: options.identity,
      issuer: options.issuer,
    });
    return createQuoteVerifier({
      dcapVerify: options.dcapVerify,
      fetchCollateral: options.fetchCollateral,
      now: options.now,
      ...trusted,
    })(args);
  };
  return Object.assign(verifier, { includeCollateral: true as const });
}

const RELEASE_VERSION = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/;

/** `value` if it is a release version (`2.3.87`, `2.3.88-rc.1`); throws otherwise. */
export function releaseVersion(value: unknown, what: string): string {
  const match = typeof value === 'string' ? RELEASE_VERSION.exec(value) : null;
  const pre = match?.[4];
  if (!match || (pre !== undefined && pre.split('.').some((id) => id === ''))) {
    throw new Error(`${what} is ${JSON.stringify(value)}, not a release version like 2.3.87`);
  }
  return value as string;
}

/**
 * Order two release versions as semver does, and as core's
 * `compare_release_versions`: numerically by `X.Y.Z`, a pre-release before its
 * release, build metadata ignored.
 */
export function compareReleaseVersions(a: string, b: string): number {
  const [aCore, aPre] = splitVersion(a);
  const [bCore, bPre] = splitVersion(b);
  for (let i = 0; i < 3; i += 1) {
    if (aCore[i] !== bCore[i]) return aCore[i] < bCore[i] ? -1 : 1;
  }
  if (aPre === undefined || bPre === undefined) return aPre === bPre ? 0 : aPre === undefined ? 1 : -1;
  const aIds = aPre.split('.');
  const bIds = bPre.split('.');
  for (let i = 0; i < Math.max(aIds.length, bIds.length); i += 1) {
    const [x, y] = [aIds[i], bIds[i]];
    if (x === undefined || y === undefined) return x === undefined ? -1 : 1;
    const [xNum, yNum] = [/^\d+$/.test(x), /^\d+$/.test(y)];
    const order =
      xNum && yNum ? Math.sign(Number(x) - Number(y)) : xNum !== yNum ? (xNum ? -1 : 1) : x < y ? -1 : x > y ? 1 : 0;
    if (order !== 0) return order;
  }
  return 0;
}

function splitVersion(version: string): [number[], string | undefined] {
  const match = RELEASE_VERSION.exec(version) as RegExpExecArray;
  return [[Number(match[1]), Number(match[2]), Number(match[3])], match[4]];
}
