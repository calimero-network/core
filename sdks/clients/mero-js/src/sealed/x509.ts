/**
 * Just enough DER and X.509 to check a Sigstore signing certificate with
 * WebCrypto: read a certificate's signed part, validity, key and extensions,
 * and verify the ECDSA signatures (P-256 and P-384) Sigstore uses.
 *
 * Written here, rather than taken from a library, so the SDK keeps no runtime
 * dependencies and runs wherever WebCrypto does. It parses strictly and throws
 * on anything it does not expect; it is not a general X.509 implementation.
 */

const SEQUENCE = 0x30;
const OID = 0x06;
const INTEGER = 0x02;
const BIT_STRING = 0x03;
const OCTET_STRING = 0x04;
const BOOLEAN = 0x01;
const UTC_TIME = 0x17;
const GENERALIZED_TIME = 0x18;
const CONTEXT_0 = 0xa0;
const CONTEXT_3 = 0xa3;

const EC_PUBLIC_KEY = '1.2.840.10045.2.1';
const CURVES: Record<string, Curve> = {
  '1.2.840.10045.3.1.7': 'P-256',
  '1.3.132.0.34': 'P-384',
};
const CURVE_SIZE: Record<Curve, number> = { 'P-256': 32, 'P-384': 48 };
/** The certificate signature algorithms accepted: ECDSA with SHA-256 or SHA-384. */
const SIGNATURE_HASHES: Record<string, Hash> = {
  '1.2.840.10045.4.3.2': 'SHA-256',
  '1.2.840.10045.4.3.3': 'SHA-384',
};

export type Curve = 'P-256' | 'P-384';
export type Hash = 'SHA-256' | 'SHA-384';

/** One DER element: its tag, and where it and its contents lie in `bytes`. */
export interface Element {
  tag: number;
  bytes: Uint8Array;
  /** Offset of the element's first (tag) byte. */
  offset: number;
  /** Offset of its contents. */
  start: number;
  /** Offset just past it. */
  end: number;
}

function read(bytes: Uint8Array, offset: number, limit: number): Element {
  if (offset + 2 > limit) throw new Error('it is truncated');
  const tag = bytes[offset];
  if ((tag & 0x1f) === 0x1f) throw new Error('it uses a tag this parser does not read');
  let length = bytes[offset + 1];
  let start = offset + 2;
  if (length & 0x80) {
    const count = length & 0x7f;
    if (count === 0 || count > 4 || start + count > limit) throw new Error('it has a length this parser does not read');
    length = 0;
    for (let i = 0; i < count; i += 1) length = length * 256 + bytes[start + i];
    start += count;
  }
  const end = start + length;
  if (end > limit) throw new Error('it is truncated');
  return { tag, bytes, offset, start, end };
}

/** Parse one complete DER element, refusing trailing bytes. */
export function parseDer(bytes: Uint8Array): Element {
  const element = read(bytes, 0, bytes.length);
  if (element.end !== bytes.length) throw new Error('it has trailing bytes');
  return element;
}

export function children(element: Element): Element[] {
  const out: Element[] = [];
  for (let offset = element.start; offset < element.end; ) {
    const child = read(element.bytes, offset, element.end);
    out.push(child);
    offset = child.end;
  }
  return out;
}

/** `element` if it has `tag`; otherwise throws naming `what`. */
export function expectTag(element: Element | undefined, tag: number, what: string): Element {
  if (!element || element.tag !== tag) throw new Error(`${what} is not where it should be`);
  return element;
}

export function contents(element: Element): Uint8Array {
  return element.bytes.slice(element.start, element.end);
}

/** The whole element, header included. */
function whole(element: Element): Uint8Array {
  return element.bytes.slice(element.offset, element.end);
}

function oid(element: Element | undefined): string {
  const value = contents(expectTag(element, OID, 'An object identifier'));
  if (value.length === 0 || value[value.length - 1] & 0x80) throw new Error('it has a malformed object identifier');
  const arcs: number[] = [];
  let arc = 0;
  for (const byte of value) {
    arc = arc * 128 + (byte & 0x7f);
    if (!(byte & 0x80)) {
      arcs.push(arc);
      arc = 0;
    }
  }
  const first = arcs.shift() as number;
  const head = first < 80 ? [Math.floor(first / 40), first % 40] : [2, first - 80];
  return [...head, ...arcs].join('.');
}

function time(element: Element): number {
  const text = new TextDecoder().decode(contents(element));
  const utc = element.tag === UTC_TIME ? /^(\d{2})(\d{2})(\d{2})(\d{2})(\d{2})(\d{2})Z$/.exec(text) : null;
  const generalized =
    element.tag === GENERALIZED_TIME ? /^(\d{4})(\d{2})(\d{2})(\d{2})(\d{2})(\d{2})Z$/.exec(text) : null;
  const parts = utc ?? generalized;
  if (!parts) throw new Error(`it holds the time ${JSON.stringify(text)}, which is not one a certificate may hold`);
  let year = Number(parts[1]);
  if (utc) year += year < 50 ? 2000 : 1900;
  const [month, day, hour, minute, second] = parts.slice(2).map(Number);
  return Date.UTC(year, month - 1, day, hour, minute, second);
}

export interface Certificate {
  der: Uint8Array;
  /** The signed part, exactly as the issuer signed it. */
  tbs: Uint8Array;
  /** The hash of the ECDSA signature the issuer made over `tbs`. */
  signatureHash: Hash;
  /** That signature, DER. */
  signature: Uint8Array;
  /** The issuer and subject names, DER, to compare byte for byte. */
  issuer: Uint8Array;
  subject: Uint8Array;
  /** Validity, in milliseconds since the epoch, both ends inclusive. */
  notBefore: number;
  notAfter: number;
  /** The subject's key, DER SubjectPublicKeyInfo, and its curve. */
  spki: Uint8Array;
  curve: Curve;
  /** Each extension's value (the contents of its OCTET STRING), by object identifier. */
  extensions: Map<string, Uint8Array>;
}

/**
 * Parse a DER X.509 certificate with an ECDSA key, signed with ECDSA. Throws
 * naming what is wrong.
 */
export function parseCertificate(der: Uint8Array): Certificate {
  try {
    const [tbsElement, algorithm, signature, ...rest] = children(
      expectTag(parseDer(der), SEQUENCE, 'The certificate'),
    );
    if (rest.length > 0) throw new Error('it has fields after its signature');
    const fields = children(expectTag(tbsElement, SEQUENCE, 'The signed part'));
    // `version` is [0] EXPLICIT; everything after it is positional.
    const first = fields[0]?.tag === CONTEXT_0 ? 1 : 0;
    const [serial, innerAlgorithm, issuer, validity, subject, spki, ...optional] = fields.slice(first);
    expectTag(serial, INTEGER, 'The serial number');

    const algorithmId = oid(children(expectTag(algorithm, SEQUENCE, 'The signature algorithm'))[0]);
    const innerId = oid(children(expectTag(innerAlgorithm, SEQUENCE, 'The signature algorithm'))[0]);
    if (innerId !== algorithmId) throw new Error('it names two different signature algorithms');
    const signatureHash = SIGNATURE_HASHES[algorithmId];
    if (!signatureHash) throw new Error(`it is signed with ${algorithmId}, not ECDSA with SHA-256 or SHA-384`);

    const [notBefore, notAfter] = children(expectTag(validity, SEQUENCE, 'The validity')).map(time);
    if (notAfter === undefined) throw new Error('its validity is incomplete');

    const [keyAlgorithm, keyBits] = children(expectTag(spki, SEQUENCE, 'The public key'));
    expectTag(keyBits, BIT_STRING, 'The public key');
    const [keyType, keyCurve] = children(expectTag(keyAlgorithm, SEQUENCE, 'The key algorithm'));
    if (oid(keyType) !== EC_PUBLIC_KEY) throw new Error('its key is not an elliptic-curve key');
    const curve = CURVES[oid(keyCurve)];
    if (!curve) throw new Error('its key is on a curve other than P-256 or P-384');

    const extensions = new Map<string, Uint8Array>();
    const extensionsField = optional.find((field) => field.tag === CONTEXT_3);
    if (extensionsField) {
      const [list] = children(extensionsField);
      for (const extension of children(expectTag(list, SEQUENCE, 'The extensions'))) {
        const parts = children(expectTag(extension, SEQUENCE, 'An extension'));
        if (parts.length === 3) expectTag(parts[1], BOOLEAN, 'An extension\'s critical flag');
        else if (parts.length !== 2) throw new Error('it has a malformed extension');
        const id = oid(parts[0]);
        if (extensions.has(id)) throw new Error(`it has extension ${id} twice`);
        extensions.set(id, contents(expectTag(parts[parts.length - 1], OCTET_STRING, 'An extension value')));
      }
    }

    const signatureBits = contents(expectTag(signature, BIT_STRING, 'The signature'));
    if (signatureBits[0] !== 0) throw new Error('its signature is not a whole number of bytes');
    return {
      der,
      tbs: whole(tbsElement),
      signatureHash,
      signature: signatureBits.slice(1),
      issuer: whole(expectTag(issuer, SEQUENCE, 'The issuer')),
      subject: whole(expectTag(subject, SEQUENCE, 'The subject')),
      notBefore,
      notAfter,
      spki: whole(spki),
      curve,
      extensions,
    };
  } catch (error) {
    throw new Error(`The certificate is malformed: ${(error as Error).message}`);
  }
}

/** The object identifiers in a DER `SEQUENCE OF OBJECT IDENTIFIER` (an extended key usage). */
export function objectIdentifiers(der: Uint8Array): string[] {
  return children(expectTag(parseDer(der), SEQUENCE, 'The list')).map(oid);
}

/**
 * An ECDSA signature as WebCrypto takes it (IEEE P1363: `r || s`, each the
 * curve's size), from the DER `SEQUENCE { r INTEGER, s INTEGER }` X.509 and
 * cosign use.
 */
export function ecdsaSignatureToP1363(der: Uint8Array, curve: Curve): Uint8Array {
  const size = CURVE_SIZE[curve];
  const integers = children(expectTag(parseDer(der), SEQUENCE, 'The signature'));
  if (integers.length !== 2) throw new Error('The signature is not an ECDSA signature');
  const out = new Uint8Array(size * 2);
  integers.forEach((integer, index) => {
    let value = contents(expectTag(integer, INTEGER, 'The signature'));
    // r and s are positive: a set top bit with no leading zero is a negative number.
    if (value.length === 0 || value[0] & 0x80) throw new Error('The signature is not an ECDSA signature');
    while (value.length > 1 && value[0] === 0) value = value.subarray(1);
    if (value.length > size) throw new Error('The signature is not an ECDSA signature');
    out.set(value, index * size + size - value.length);
  });
  return out;
}

/** Verify a DER ECDSA `signature` over `data` with a DER SubjectPublicKeyInfo key. */
export async function verifyEcdsa(
  spki: Uint8Array,
  curve: Curve,
  hash: Hash,
  signature: Uint8Array,
  data: Uint8Array,
): Promise<boolean> {
  let p1363: Uint8Array;
  try {
    p1363 = ecdsaSignatureToP1363(signature, curve);
  } catch {
    return false;
  }
  const key = await crypto.subtle.importKey('spki', spki, { name: 'ECDSA', namedCurve: curve }, false, ['verify']);
  return crypto.subtle.verify({ name: 'ECDSA', hash }, key, p1363, data);
}

/** Decode base64 (standard alphabet), refusing anything else. */
export function fromBase64(value: string, what: string): Uint8Array {
  const clean = value.replace(/\s+/g, '');
  if (!/^[A-Za-z0-9+/]*={0,2}$/.test(clean) || clean.length % 4 !== 0) {
    throw new Error(`${what} is not base64`);
  }
  const binary = atob(clean);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

/** The DER of the one `CERTIFICATE` block a PEM holds. */
export function pemCertificate(pem: string, what: string): Uint8Array {
  const blocks = [...pem.matchAll(/-----BEGIN CERTIFICATE-----([^-]*)-----END CERTIFICATE-----/g)];
  if (blocks.length !== 1 || pem.replace(blocks[0][0], '').trim() !== '') {
    throw new Error(`${what} is not exactly one PEM certificate`);
  }
  return fromBase64(blocks[0][1], what);
}
