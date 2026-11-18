/**
 * Verifying a TEE node's quote in the page, before trusting the key it binds.
 *
 * A quote proves what runs in a TD only once it has been checked: its
 * signatures against Intel's root, the TCB status of the platform, the
 * measurements of the image, and that its report data is the nonce and the
 * binding this client asked for. {@link createQuoteVerifier} does all of that
 * here. A Calimero 1.0 node sends no collateral: the verifier fetches Intel's
 * (`fetchCollateral`), which the node can neither forge nor choose.
 *
 * The DCAP verification itself — parsing the quote, checking its signature
 * chain and appraising its TCB level against the collateral — comes from a
 * library you pass in, normally `verify` from `@phala/dcap-qvl`, a pure
 * JavaScript implementation that runs in browsers, Node and React Native. It
 * is passed in rather than bundled so this SDK keeps no runtime dependencies,
 * and applications that never talk to a TEE node pay nothing for it.
 */

import { hex } from '../crypto/internal.js';
import type { VerifyTransportQuote } from './sealed.js';

/**
 * The collateral a quote is verified against — TCB info, QE identity, CRLs and
 * their chains — in dcap-qvl's form: byte fields are hex strings or byte
 * arrays. What `@phala/dcap-qvl`'s `getCollateral` fetches.
 */
export interface DcapCollateral {
  pck_crl_issuer_chain: string;
  root_ca_crl: number[] | string;
  pck_crl: number[] | string;
  tcb_info_issuer_chain: string;
  tcb_info: string;
  tcb_info_signature: number[] | string;
  qe_identity_issuer_chain: string;
  qe_identity: string;
  qe_identity_signature: number[] | string;
}

/** The fields of a TDX report this verifier reads. */
interface TdReport {
  mrTd: Uint8Array;
  rtMr0: Uint8Array;
  rtMr1: Uint8Array;
  rtMr2: Uint8Array;
  rtMr3: Uint8Array;
  reportData: Uint8Array;
  tdAttributes?: Uint8Array;
}

/** What a DCAP verifier returns for a quote whose signature chain verified. */
export interface DcapVerifiedReport {
  status: string;
  advisory_ids?: string[];
  report: { type: string; data: unknown };
}

/**
 * Verify a quote's signature chain against `collateral` as of `nowSecs`, and
 * appraise its TCB level. Throws when it does not verify. `verify` from
 * `@phala/dcap-qvl` has exactly this shape.
 */
export type DcapVerify = (
  quote: Uint8Array,
  collateral: DcapCollateral,
  nowSecs: number,
) => DcapVerifiedReport;

/**
 * One image's measurements, all five registers, hex. A quote matches it only
 * when every register does, so measurements of different images never combine.
 */
export interface TrustedMeasurements {
  mrtd: string;
  rtmr0: string;
  rtmr1: string;
  rtmr2: string;
  rtmr3: string;
}

export interface QuoteVerifierOptions {
  /** DCAP verification: `verify` from `@phala/dcap-qvl`. */
  dcapVerify: DcapVerify;
  /**
   * The images trusted, each by all of its measurements. The way to say which
   * code may read the traffic: build it with {@link trustedMeasurementsFromReleases}
   * from the `published-mrtds.json` of the releases you trust.
   */
  allowedMeasurements?: TrustedMeasurements[];
  /**
   * MRTDs (hex) accepted. On its own this does not name an image: on GCP the
   * MRTD measures the platform's TD firmware, which every image shares, and
   * the image is in RTMR1–3. So a verifier needs either
   * {@link allowedMeasurements}, or this with all of `allowedRtmr1`–`3`.
   */
  allowedMrtd?: string[];
  /** RTMR allowlists (hex); a register is not checked when its list is left out. */
  allowedRtmr0?: string[];
  allowedRtmr1?: string[];
  allowedRtmr2?: string[];
  allowedRtmr3?: string[];
  /**
   * TCB statuses accepted. Defaults to `['UpToDate']`. `Revoked` is refused
   * even when listed.
   */
  allowedTcbStatuses?: string[];
  /**
   * Where to get collateral (a Calimero 1.0 node sends none): for example
   * `(quote) => getCollateral(PHALA_PCCS_URL, quote)` from `@phala/dcap-qvl`.
   * Without it, a quote that came without collateral is refused.
   */
  fetchCollateral?: (quote: Uint8Array) => Promise<DcapCollateral>;
  /** The current time in milliseconds; defaults to `Date.now`. For tests. */
  now?: () => number;
}

/** A quote verifier as this package builds one ({@link createQuoteVerifier}). */
export type QuoteVerifier = VerifyTransportQuote & { readonly includeCollateral: true };

const MEASUREMENT_HEX = /^[0-9a-f]{96}$/;
const REGISTERS = ['mrtd', 'rtmr0', 'rtmr1', 'rtmr2', 'rtmr3'] as const;

/**
 * A verifier that checks a TEE node's quote here, in the page, and accepts it
 * only when every check passes:
 *
 * - its signatures verify against Intel's root, with collateral that is valid
 *   now;
 * - the platform's TCB status is one allowed (`UpToDate` by default), and
 *   never `Revoked`;
 * - its measurements are those of a trusted image: one of
 *   `allowedMeasurements` in full, and the MRTD and any RTMRs listed;
 * - its report data is the nonce followed by the binding the client computed.
 *
 * Any failure rejects with the reason; nothing is trusted from a quote that
 * does not pass. Mock quotes never pass: they carry no signature.
 *
 * ```ts
 * import { verify as dcapVerify } from '@phala/dcap-qvl';
 *
 * const verifier = createQuoteVerifier({
 *   dcapVerify,
 *   ...trustedMeasurementsFromReleases([publishedMrtds], { profile: 'locked-read-only' }),
 * });
 * const client = new CalimeroClient({ baseUrl, policy: { verifyQuote: verifier } });
 * ```
 */
export function createQuoteVerifier(options: QuoteVerifierOptions): QuoteVerifier {
  const allowedMrtd = measurements(options.allowedMrtd ?? [], 'allowedMrtd');
  const allowedImages = (options.allowedMeasurements ?? []).map((image, index) => {
    const label = `allowedMeasurements[${index}]`;
    return REGISTERS.map((register) => measurements([image?.[register] ?? ''], `${label}.${register}`)[0]).join();
  });
  const allowedRtmr = [
    measurements(options.allowedRtmr0 ?? [], 'allowedRtmr0'),
    measurements(options.allowedRtmr1 ?? [], 'allowedRtmr1'),
    measurements(options.allowedRtmr2 ?? [], 'allowedRtmr2'),
    measurements(options.allowedRtmr3 ?? [], 'allowedRtmr3'),
  ];
  if (options.allowedMeasurements !== undefined && allowedImages.length === 0) {
    throw new Error('allowedMeasurements is empty, so no quote would ever be accepted');
  }
  if (allowedImages.length === 0) {
    if (allowedMrtd.length === 0) {
      throw new Error('No image is trusted: give allowedMeasurements, or allowedMrtd with allowedRtmr1-3');
    }
    if (allowedRtmr.slice(1).some((list) => list.length === 0)) {
      throw new Error(
        'allowedMrtd alone does not name an image: the MRTD measures the TD firmware, which ' +
          'images share, and the image is in RTMR1-3. Give allowedMeasurements, or allowedRtmr1-3 too',
      );
    }
  }
  const allowedStatuses = options.allowedTcbStatuses ?? ['UpToDate'];
  const now = options.now ?? Date.now;

  const verifier = async ({
    quoteB64,
    nonce,
    reportDataSuffix,
    collateral,
  }: Parameters<VerifyTransportQuote>[0]): Promise<boolean> => {
    const quote = fromBase64(quoteB64);
    const against = collateral ?? (await options.fetchCollateral?.(quote));
    if (!against) {
      throw new Error(
        'The node sent no collateral for its quote, and no fetchCollateral was given to get it',
      );
    }

    let verified: DcapVerifiedReport;
    try {
      verified = options.dcapVerify(quote, against, Math.floor(now() / 1000));
    } catch (error) {
      throw new Error(`The quote did not verify: ${(error as Error)?.message ?? String(error)}`);
    }

    if (verified.status === 'Revoked' || !allowedStatuses.includes(verified.status)) {
      throw new Error(`The platform's TCB status is ${verified.status}, which is not accepted`);
    }
    const td = tdReport(verified.report);
    // TUD bit 0. A debug TD's memory and registers are open to the host, so its
    // measurements prove nothing about what runs in it. Checked here, not left
    // to `dcapVerify`, which a caller may swap for one that allows debug; an
    // attribute field that cannot be read counts as debug.
    if (td.tdAttributes?.length !== 8 || (td.tdAttributes[0] & 0x01) !== 0) {
      throw new Error('The quote is from a debug TD, or its TD attributes cannot be read');
    }
    const measured = [td.mrTd, td.rtMr0, td.rtMr1, td.rtMr2, td.rtMr3].map((register) => hex(register));
    if (allowedImages.length > 0 && !allowedImages.includes(measured.join())) {
      throw new Error(
        `The measurements (MRTD ${measured[0]}, RTMR0-3 ${measured.slice(1).join(', ')}) are not an image this verifier trusts`,
      );
    }
    if (allowedMrtd.length > 0 && !allowedMrtd.includes(measured[0])) {
      throw new Error(`MRTD ${measured[0]} is not an image this verifier trusts`);
    }
    const registers = [td.rtMr0, td.rtMr1, td.rtMr2, td.rtMr3];
    registers.forEach((register, index) => {
      const allowed = allowedRtmr[index];
      if (allowed.length > 0 && !allowed.includes(hex(register))) {
        throw new Error(`RTMR${index} ${hex(register)} is not one this verifier trusts`);
      }
    });
    if (hex(td.reportData) !== `${nonce}${reportDataSuffix}`.toLowerCase()) {
      throw new Error('The quote does not commit to this nonce and binding');
    }
    return true;
  };
  return Object.assign(verifier, { includeCollateral: true as const });
}

function measurements(values: string[], label: string): string[] {
  return values.map((value) => {
    const normalized = value.trim().toLowerCase();
    if (!MEASUREMENT_HEX.test(normalized)) {
      throw new Error(`${label} holds ${JSON.stringify(value)}, which is not a 48-byte hex measurement`);
    }
    return normalized;
  });
}

/** The TD report of a TDX quote (1.0 or 1.5); anything else is refused. */
function tdReport(report: DcapVerifiedReport['report']): TdReport {
  const data = report.data as (TdReport & { base?: TdReport }) | undefined;
  if (report.type === 'td10' && data) return data;
  if (report.type === 'td15' && data?.base) return data.base;
  throw new Error(`The quote is a ${report.type} quote, not a TDX one`);
}

function fromBase64(value: string): Uint8Array {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

/**
 * A mero-tee node release's `published-mrtds.json`: the measurements the
 * release workflow observed booting each profile of its image on TDX, signed
 * with the release. Only the fields read here are typed.
 */
export interface PublishedMrtds {
  role?: string;
  tag?: string;
  profiles: Record<
    string,
    {
      mrtd: string;
      rtmr0: string;
      rtmr1: string;
      rtmr2: string;
      rtmr3: string;
      allowed_tcb_statuses?: string[];
    }
  >;
}

/** TCB statuses as DCAP reports them; the release lists them in lower case. */
const TCB_STATUSES = [
  'UpToDate',
  'SWHardeningNeeded',
  'ConfigurationNeeded',
  'ConfigurationAndSWHardeningNeeded',
  'OutOfDate',
  'OutOfDateConfigurationNeeded',
];

/**
 * The {@link QuoteVerifierOptions} that trust exactly one profile of the given
 * releases: each release's image by all of its measurements, and only the TCB
 * statuses every one of those releases accepts.
 *
 * Pass every release a node you talk to may run. During a rollout that is the
 * old one and the new one; drop the old one once no node runs it.
 *
 * ```ts
 * import published from './trusted/mero-tee-v2.3.78.published-mrtds.json';
 *
 * const verifier = createQuoteVerifier({
 *   dcapVerify,
 *   ...trustedMeasurementsFromReleases([published], { profile: 'locked-read-only' }),
 * });
 * ```
 *
 * Where the file comes from is the trust decision. Shipping it with the app
 * (checked out of a release you verified) trusts that release; fetching it at
 * run time trusts whoever serves it.
 */
/**
 * The one image profile a release publishes for production. The `debug` and
 * `debug-read-only` images are signed and published beside it, but run with
 * SSH and the operator's tooling inside the TD, so trusting one trusts an
 * image the operator can read.
 */
export const PRODUCTION_PROFILE = 'locked-read-only';

export interface TrustedProfileOptions {
  /** The image profile to trust, e.g. `locked-read-only`. */
  profile: string;
  /**
   * Trust a profile other than {@link PRODUCTION_PROFILE}, such as `debug`.
   * Its image is readable by whoever operates the node, so only for testing.
   */
  allowDebugProfile?: boolean;
}

export function trustedMeasurementsFromReleases(
  releases: PublishedMrtds[],
  { profile, allowDebugProfile = false }: TrustedProfileOptions,
): Required<Pick<QuoteVerifierOptions, 'allowedMeasurements' | 'allowedTcbStatuses'>> {
  if (releases.length === 0) {
    throw new Error('No release given, so no image would be trusted');
  }
  let statuses: string[] | undefined;
  const allowedMeasurements = releases.map((release, index) => {
    const name = release?.tag ?? `releases[${index}]`;
    if (release?.role !== undefined && release.role !== 'node') {
      throw new Error(`${name} lists the measurements of a ${release.role}, not of a node image`);
    }
    const image = release?.profiles?.[profile];
    if (!image) {
      const known = Object.keys(release?.profiles ?? {}).join(', ') || 'none';
      throw new Error(`${name} has no profile ${JSON.stringify(profile)} (it has: ${known})`);
    }
    if (profile !== PRODUCTION_PROFILE && !allowDebugProfile) {
      throw new Error(
        `${JSON.stringify(profile)} is not a production profile: its image is readable by whoever ` +
          `operates the node. Trust ${JSON.stringify(PRODUCTION_PROFILE)}, or pass allowDebugProfile to test`,
      );
    }
    const accepted = (image.allowed_tcb_statuses ?? ['uptodate']).map((status) => tcbStatus(status, name));
    statuses = statuses === undefined ? accepted : statuses.filter((status) => accepted.includes(status));
    return {
      mrtd: image.mrtd,
      rtmr0: image.rtmr0,
      rtmr1: image.rtmr1,
      rtmr2: image.rtmr2,
      rtmr3: image.rtmr3,
    };
  });
  if (!statuses || statuses.length === 0) {
    throw new Error('The releases given accept no TCB status in common');
  }
  return { allowedMeasurements, allowedTcbStatuses: statuses };
}

function tcbStatus(status: string, release: string): string {
  const known = TCB_STATUSES.find((candidate) => candidate.toLowerCase() === String(status).toLowerCase());
  if (!known) {
    throw new Error(`${release} accepts TCB status ${JSON.stringify(status)}, which this verifier does not know`);
  }
  return known;
}
