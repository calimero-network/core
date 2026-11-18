/**
 * Verifying a cosign keyless signature, as `cosign verify-blob --bundle` does,
 * with WebCrypto and the Sigstore trust root embedded in this package.
 *
 * mero-tee signs each release asset with cosign keyless: there is no release
 * key, only a Fulcio certificate that lives ten minutes, issued to one GitHub
 * Actions workflow run, and a Rekor log entry that proves the signature was
 * made while the certificate was valid. So the checks are: the signature over
 * the bytes verifies with the certificate's key; Rekor's signed entry
 * timestamp verifies, and the entry it signs is this signature over these
 * bytes by this certificate; the certificate chains to a Fulcio CA and was
 * valid when Rekor logged it; and the certificate names the workflow expected.
 * The same checks core runs in `crates/tee-release/src/sigstore_verify.rs`.
 *
 * Nothing here reads the clock. Every time is Rekor's `integratedTime`, which
 * its signature covers, so a verification that passes once passes forever,
 * whatever the client's clock says.
 */

import { hex } from '../crypto/internal.js';
import { FULCIO_AUTHORITIES, REKOR_LOGS } from './sigstore-trust-root.js';
import {
  children,
  contents,
  expectTag,
  fromBase64,
  objectIdentifiers,
  parseCertificate,
  parseDer,
  pemCertificate,
  verifyEcdsa,
  type Certificate,
} from './x509.js';

const SUBJECT_ALT_NAME = '2.5.29.17';
const EXTENDED_KEY_USAGE = '2.5.29.37';
const CODE_SIGNING = '1.3.6.1.5.5.7.3.3';
/** Fulcio's OIDC issuer extension, a DER UTF8String. */
const FULCIO_ISSUER_V2 = '1.3.6.1.4.1.57264.1.8';
/** Its deprecated predecessor, the raw issuer bytes; read only when the other is absent. */
const FULCIO_ISSUER_V1 = '1.3.6.1.4.1.57264.1.1';
const SAN_URI = 0x86;
const UTF8_STRING = 0x0c;
const SEQUENCE = 0x30;

const utf8 = new TextEncoder();
// Not `fatal`: not every runtime's TextDecoder takes options. A malformed byte
// decodes to U+FFFD, which no pinned identity holds, so it can only fail a match.
const fromUtf8 = (bytes: Uint8Array) => new TextDecoder().decode(bytes);

/** Who must have signed: the certificate's SAN URI and its OIDC issuer, each matched exactly. */
export interface SignerIdentity {
  identity: string;
  issuer: string;
}

/** A legacy cosign bundle (`cosign sign-blob --bundle`), as the fields verified here. */
interface CosignBundle {
  base64Signature: string;
  cert: string;
  rekorBundle: {
    SignedEntryTimestamp: string;
    Payload: { body: string; integratedTime: number; logIndex: number; logID: string };
  };
}

/**
 * Verify a cosign keyless `bundle` over `body`, signed by `signer`. Resolves
 * with the time Rekor logged it (seconds); rejects with the reason otherwise.
 * `signature`, when given, is the detached `.sig` published beside the file,
 * and must be the bundle's.
 */
export async function verifyCosignBundle(
  body: Uint8Array,
  bundle: unknown,
  signer: SignerIdentity,
  signature?: string,
): Promise<{ integratedTime: number }> {
  const { base64Signature, cert, rekorBundle } = cosignBundle(bundle);
  const payload = rekorBundle.Payload;
  if (signature !== undefined && signature.trim() !== base64Signature.trim()) {
    throw new Error('The detached signature is not the one in the bundle');
  }
  const sig = fromBase64(base64Signature, 'The bundle signature');
  const leafDer = pemCertificate(fromUtf8(fromBase64(cert, 'The bundle certificate')), 'The bundle certificate');
  const leaf = parseCertificate(leafDer);

  // Rekor vouches for the entry and its time: nothing below is trusted before this.
  const log = REKOR_LOGS.find((candidate) => candidate.logId === payload.logID.toLowerCase());
  if (!log) throw new Error(`The bundle names Rekor log ${payload.logID}, which the Sigstore trust root does not know`);
  const canonical = utf8.encode(
    `{"body":${JSON.stringify(payload.body)},"integratedTime":${payload.integratedTime},` +
      `"logID":${JSON.stringify(payload.logID)},"logIndex":${payload.logIndex}}`,
  );
  const setVerifies = await verifyEcdsa(
    fromBase64(log.publicKey, 'The Rekor key'),
    'P-256',
    'SHA-256',
    fromBase64(rekorBundle.SignedEntryTimestamp, 'The signed entry timestamp'),
    canonical,
  );
  if (!setVerifies) {
    throw new Error("Rekor's signed entry timestamp does not verify: the log entry, its time or its index was altered");
  }
  const at = payload.integratedTime * 1000;
  if (!within(at, log.validFor)) {
    throw new Error(`Rekor log ${log.baseUrl} was not in use at ${iso(at)}, when the entry says it was logged`);
  }

  // The entry Rekor signed is this signature, over these bytes, by this certificate.
  const entry = hashedRekord(payload.body);
  const digest = hex(new Uint8Array(await crypto.subtle.digest('SHA-256', body)));
  if (entry.hash !== digest) throw new Error("The file's SHA-256 is not the one Rekor logged: the file is not the one signed");
  if (!equal(fromBase64(entry.signature, 'The logged signature'), sig)) {
    throw new Error("The bundle's signature is not the one Rekor logged");
  }
  const loggedCert = pemCertificate(fromUtf8(fromBase64(entry.publicKey, 'The logged certificate')), 'The logged certificate');
  if (!equal(loggedCert, leafDer)) throw new Error("The bundle's certificate is not the one Rekor logged");

  if (leaf.curve !== 'P-256' || !(await verifyEcdsa(leaf.spki, 'P-256', 'SHA-256', sig, body))) {
    throw new Error("The signature over the file does not verify with the bundle's certificate");
  }

  await chainsToFulcio(leaf, at);
  checkSigner(leaf, signer);
  return { integratedTime: payload.integratedTime };
}

/**
 * Check that `leaf` was issued by a Fulcio CA in the trust root, and that it,
 * the CA's certificates and the CA itself (by the trust root's `validFor`)
 * were all valid at `at` (milliseconds): when Rekor logged the signature, not
 * now, since a Fulcio certificate lives ten minutes. Every signature on the
 * way is checked, with the algorithm its certificate names.
 */
export async function chainsToFulcio(leaf: Certificate, at: number): Promise<void> {
  if (!validAt(leaf, at)) {
    throw new Error(
      `The signing certificate was valid ${iso(leaf.notBefore)} to ${iso(leaf.notAfter)}, not at ${iso(at)} when Rekor logged it`,
    );
  }
  const reasons: string[] = [];
  for (const authority of FULCIO_AUTHORITIES) {
    const chain = authority.certChain.map((der) => parseCertificate(fromBase64(der, 'A Fulcio certificate')));
    if (!(await issuedBy(leaf, chain[0]))) continue;
    // The issuer is found; from here a failure is this CA's, so say which.
    const reason = await authorityRefusal(chain, authority.validFor, at);
    if (!reason) return;
    reasons.push(`${authority.uri} (valid from ${authority.validFor.start}): ${reason}`);
  }
  throw new Error(
    reasons.length > 0
      ? `The signing certificate's CA is not trusted at ${iso(at)}: ${reasons.join('; ')}`
      : 'The signing certificate was not issued by a Fulcio CA in the Sigstore trust root',
  );
}

async function authorityRefusal(
  chain: Certificate[],
  validFor: { start: string; end?: string },
  at: number,
): Promise<string | undefined> {
  if (!within(at, validFor)) return 'the trust root says it was not issuing certificates then';
  for (const [index, certificate] of chain.entries()) {
    if (!validAt(certificate, at)) return `its certificate ${index} was not valid then`;
    const parent = chain[index + 1] ?? certificate; // the root signs itself
    if (!(await issuedBy(certificate, parent))) return `its certificate ${index} is not signed by the next`;
  }
  return undefined;
}

async function issuedBy(child: Certificate, parent: Certificate): Promise<boolean> {
  if (!equal(child.issuer, parent.subject)) return false;
  return verifyEcdsa(parent.spki, parent.curve, child.signatureHash, child.signature, child.tbs);
}

function checkSigner(leaf: Certificate, signer: SignerIdentity): void {
  const san = leaf.extensions.get(SUBJECT_ALT_NAME);
  if (!san) throw new Error('The signing certificate names no identity');
  const names = children(expectTag(parseDer(san), SEQUENCE, 'The subject alternative name'));
  const uris = names.filter((name) => name.tag === SAN_URI).map((name) => fromUtf8(contents(name)));
  if (uris.length !== 1 || names.length !== 1 || uris[0] !== signer.identity) {
    throw new Error(
      `The release was signed by ${JSON.stringify(uris.length === 1 ? uris[0] : uris)}, not by ${signer.identity}`,
    );
  }

  const v2 = leaf.extensions.get(FULCIO_ISSUER_V2);
  const v1 = leaf.extensions.get(FULCIO_ISSUER_V1);
  const issuer = v2
    ? fromUtf8(contents(expectTag(parseDer(v2), UTF8_STRING, 'The OIDC issuer')))
    : v1
      ? fromUtf8(v1)
      : undefined;
  if (issuer !== signer.issuer) {
    throw new Error(
      `The signing certificate's OIDC issuer is ${issuer === undefined ? 'missing' : JSON.stringify(issuer)}, not ${signer.issuer}`,
    );
  }

  const usage = leaf.extensions.get(EXTENDED_KEY_USAGE);
  if (!usage || !objectIdentifiers(usage).includes(CODE_SIGNING)) {
    throw new Error('The signing certificate is not for code signing');
  }
}

function cosignBundle(value: unknown): CosignBundle {
  let bundle = value;
  if (typeof bundle === 'string') {
    try {
      bundle = JSON.parse(bundle);
    } catch {
      throw new Error('The bundle is not JSON');
    }
  }
  const b = bundle as Partial<CosignBundle> | null;
  const p = b?.rekorBundle?.Payload;
  const ok =
    typeof b?.base64Signature === 'string' &&
    typeof b.cert === 'string' &&
    typeof b.rekorBundle?.SignedEntryTimestamp === 'string' &&
    typeof p?.body === 'string' &&
    typeof p.logID === 'string' &&
    /^[0-9a-fA-F]{64}$/.test(p.logID) &&
    Number.isSafeInteger(p.integratedTime) &&
    p.integratedTime >= 0 &&
    Number.isSafeInteger(p.logIndex) &&
    p.logIndex >= 0;
  if (!ok) throw new Error('The bundle is not a cosign bundle with a Rekor entry');
  return b as CosignBundle;
}

/** The fields of a logged `hashedrekord` entry that are compared. */
function hashedRekord(body: string): { hash: string; signature: string; publicKey: string } {
  let entry: {
    kind?: unknown;
    spec?: {
      data?: { hash?: { algorithm?: unknown; value?: unknown } };
      signature?: { content?: unknown; publicKey?: { content?: unknown } };
    };
  };
  try {
    entry = JSON.parse(fromUtf8(fromBase64(body, 'The logged entry')));
  } catch {
    throw new Error('The logged entry is not JSON');
  }
  const hash = entry?.spec?.data?.hash;
  const signature = entry?.spec?.signature;
  if (
    entry?.kind !== 'hashedrekord' ||
    hash?.algorithm !== 'sha256' ||
    typeof hash.value !== 'string' ||
    typeof signature?.content !== 'string' ||
    typeof signature.publicKey?.content !== 'string'
  ) {
    throw new Error('The logged entry is not a SHA-256 hashedrekord');
  }
  return { hash: hash.value.toLowerCase(), signature: signature.content, publicKey: signature.publicKey.content };
}

function validAt(certificate: Certificate, at: number): boolean {
  return certificate.notBefore <= at && at <= certificate.notAfter;
}

function within(at: number, window: { start: string; end?: string }): boolean {
  return Date.parse(window.start) <= at && (window.end === undefined || at <= Date.parse(window.end));
}

function iso(ms: number): string {
  return new Date(ms).toISOString();
}

function equal(a: Uint8Array, b: Uint8Array): boolean {
  return a.length === b.length && a.every((byte, index) => byte === b[index]);
}
