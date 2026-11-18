/**
 * Which agent images to trust: a mero-kms release's signed agent allowlist,
 * `kms-agent-attestation-policy.<profile>.json`, and a quote verifier built
 * from it.
 *
 * The KMS release workflow measures the release's agent image and signs its
 * allowlist with cosign keyless, beside the rest of the release. The allowlist
 * can therefore come from anywhere (a release asset, mdma's mirror, a file in
 * the app): {@link verifySignedAgentPolicy} checks the signature here, against
 * the Sigstore trust root embedded in this package, and refuses anything else.
 */

import { verifyCosignBundle, type SignerIdentity } from '../sealed/sigstore.js';
import {
  PRODUCTION_PROFILE,
  createQuoteVerifier,
  type DcapCollateral,
  type DcapVerify,
  type QuoteVerifier,
} from '../sealed/verify.js';

/** The workflow that signs every mero-kms release, agent allowlists included. */
export const KMS_RELEASE_SIGNER: Readonly<SignerIdentity> = Object.freeze({
  identity: 'https://github.com/calimero-network/mero-tee/.github/workflows/release-kms.yaml@refs/heads/master',
  issuer: 'https://token.actions.githubusercontent.com',
});

/** An agent allowlist, as the KMS release publishes it. Every list must be non-empty. */
export interface AgentPolicy {
  schema_version?: number;
  role?: string;
  profile?: string;
  /** The mero-kms release it belongs to. */
  version?: string;
  allowed_tcb_statuses: string[];
  allowed_mrtd: string[];
  allowed_rtmr0: string[];
  allowed_rtmr1: string[];
  allowed_rtmr2: string[];
  allowed_rtmr3: string[];
}

const LISTS = [
  'allowed_tcb_statuses',
  'allowed_mrtd',
  'allowed_rtmr0',
  'allowed_rtmr1',
  'allowed_rtmr2',
  'allowed_rtmr3',
] as const;

export interface VerifySignedAgentPolicyOptions {
  /** Require this profile; defaults to `locked-read-only`, the only one whose host cannot reach in. */
  profile?: string;
  /** Require the file's `version` to be this mero-kms release. */
  expectedVersion?: string;
  /** The certificate identity required. Defaults to {@link KMS_RELEASE_SIGNER}'s. */
  identity?: string;
  /** The OIDC issuer required. Defaults to {@link KMS_RELEASE_SIGNER}'s. */
  issuer?: string;
  /**
   * The image role the allowlist must be for: `agent` (the default), or
   * `inference`, the release's `kms-inference-attestation-policy.<profile>.json`
   * for its private inference image. Both have the same shape and signer.
   */
  role?: 'agent' | 'inference';
}

/**
 * Verify an agent allowlist against its cosign bundle, as `cosign verify-blob`
 * would, and return it parsed. Rejects unless it was signed by the KMS release
 * workflow, it is an allowlist for `options.role` (default `agent`) and the profile asked
 * for, every list in it is non-empty, and `expectedVersion`, when given, is its
 * `version`.
 */
export async function verifySignedAgentPolicy(
  policy: string | Uint8Array,
  bundle: string | object,
  options: VerifySignedAgentPolicyOptions = {},
): Promise<AgentPolicy> {
  const body = typeof policy === 'string' ? new TextEncoder().encode(policy) : policy;
  await verifyCosignBundle(body, bundle, {
    identity: options.identity ?? KMS_RELEASE_SIGNER.identity,
    issuer: options.issuer ?? KMS_RELEASE_SIGNER.issuer,
  });
  let parsed: AgentPolicy;
  try {
    parsed = JSON.parse(new TextDecoder().decode(body));
  } catch {
    throw new Error('The signed agent policy is not JSON');
  }
  const role = options.role ?? 'agent';
  if (parsed?.role !== role) {
    throw new Error(`The signed file is a ${JSON.stringify(parsed?.role)} policy, not an ${role} one`);
  }
  const profile = options.profile ?? PRODUCTION_PROFILE;
  if (parsed.profile !== profile) {
    throw new Error(`The signed agent policy is for the ${JSON.stringify(parsed.profile)} profile, not ${profile}`);
  }
  if (options.expectedVersion !== undefined && parsed.version !== options.expectedVersion) {
    throw new Error(`The signed agent policy is release ${JSON.stringify(parsed.version)}, not ${options.expectedVersion}`);
  }
  checkLists(parsed);
  return parsed;
}

function checkLists(policy: AgentPolicy): void {
  for (const list of LISTS) {
    if (!Array.isArray(policy?.[list]) || policy[list].length === 0) {
      // As the gate's own verifier: an empty list accepts nothing.
      throw new Error(`The agent policy's ${list} is empty, so no agent would be accepted`);
    }
  }
}

/** DCAP's spelling of each TCB status; the policy may use any case. */
const TCB_STATUSES = [
  'UpToDate',
  'SWHardeningNeeded',
  'ConfigurationNeeded',
  'ConfigurationAndSWHardeningNeeded',
  'OutOfDate',
  'OutOfDateConfigurationNeeded',
];

function tcbStatus(value: string): string {
  const found = TCB_STATUSES.find((status) => status.toLowerCase() === value.trim().toLowerCase());
  if (!found) throw new Error(`The agent policy allows an unknown TCB status ${JSON.stringify(value)}`);
  return found;
}

const register = (value: string) => value.trim().replace(/^0x/i, '');

export interface AgentQuoteVerifierOptions {
  /** `verify` from `@phala/dcap-qvl`, or anything that verifies a quote the same way. */
  dcapVerify: DcapVerify;
  /**
   * The gate sends no collateral: fetch it, e.g.
   * `(quote) => getCollateral(PHALA_PCCS_URL, quote)` from `@phala/dcap-qvl`.
   */
  fetchCollateral: (quote: Uint8Array) => Promise<DcapCollateral>;
  /** The current time in milliseconds; defaults to `Date.now`. For tests. */
  now?: () => number;
}

/**
 * A verifier for {@link attestAgent} that accepts a quote only from an agent
 * image in `policy`: its signature and collateral verify, its TCB status is
 * one the policy allows (never `Revoked`), it is not a debug TD, and each of
 * MRTD and RTMR0-3 is in the policy's list. Mock quotes never pass.
 */
export function agentQuoteVerifier(policy: AgentPolicy, options: AgentQuoteVerifierOptions): QuoteVerifier {
  checkLists(policy);
  return createQuoteVerifier({
    dcapVerify: options.dcapVerify,
    fetchCollateral: options.fetchCollateral,
    now: options.now,
    allowedTcbStatuses: policy.allowed_tcb_statuses.map(tcbStatus),
    allowedMrtd: policy.allowed_mrtd.map(register),
    allowedRtmr0: policy.allowed_rtmr0.map(register),
    allowedRtmr1: policy.allowed_rtmr1.map(register),
    allowedRtmr2: policy.allowed_rtmr2.map(register),
    allowedRtmr3: policy.allowed_rtmr3.map(register),
  });
}
