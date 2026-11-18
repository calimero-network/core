/**
 * A software passkey (ES256) for tests: answers registration and assertion
 * ceremonies the way a platform authenticator would, attestation format `none`.
 */

import { b64url, type AssertionJSON, type PasskeyProvider, type RegistrationJSON } from '../../src/runtime/passkey.js';

const enc = new TextEncoder();

// --- minimal CBOR encoder (unsigned/negative ints, byte/text strings, maps) ---
function head(major: number, n: number): number[] {
  if (n < 24) return [(major << 5) | n];
  if (n < 256) return [(major << 5) | 24, n];
  if (n < 65536) return [(major << 5) | 25, n >> 8, n & 0xff];
  return [(major << 5) | 26, (n >>> 24) & 0xff, (n >>> 16) & 0xff, (n >>> 8) & 0xff, n & 0xff];
}

type Cbor = number | string | Uint8Array | Array<[Cbor, Cbor]>;

function cbor(v: Cbor): number[] {
  if (typeof v === 'number') return v >= 0 ? head(0, v) : head(1, -1 - v);
  if (typeof v === 'string') {
    const b = enc.encode(v);
    return [...head(3, b.length), ...b];
  }
  if (v instanceof Uint8Array) return [...head(2, v.length), ...v];
  return [...head(5, v.length), ...v.flatMap(([k, x]) => [...cbor(k), ...cbor(x)])];
}

/** IEEE P1363 (r || s) to ASN.1 DER, which WebAuthn ES256 signatures use. */
function toDer(sig: Uint8Array): Uint8Array {
  const int = (x: Uint8Array): number[] => {
    let i = 0;
    while (i < x.length - 1 && x[i] === 0) i++;
    const t = [...x.slice(i)];
    if (t[0] & 0x80) t.unshift(0);
    return [0x02, t.length, ...t];
  };
  const body = [...int(sig.slice(0, 32)), ...int(sig.slice(32))];
  return new Uint8Array([0x30, body.length, ...body]);
}

async function sha256(data: Uint8Array): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest('SHA-256', data));
}

export class SoftPasskey implements PasskeyProvider {
  private keys?: CryptoKeyPair;
  private readonly credentialId = crypto.getRandomValues(new Uint8Array(16));
  private counter = 0;

  constructor(private readonly rpId: string, private readonly origin: string) {}

  get id(): string {
    return b64url(this.credentialId);
  }

  private async authData(flags: number, attested?: Uint8Array): Promise<Uint8Array> {
    const out = [...(await sha256(enc.encode(this.rpId))), flags, ...[24, 16, 8, 0].map((s) => (this.counter >>> s) & 0xff)];
    return new Uint8Array(attested ? [...out, ...attested] : out);
  }

  private clientData(type: string, challenge: Uint8Array): Uint8Array {
    return enc.encode(JSON.stringify({ type, challenge: b64url(challenge), origin: this.origin, crossOrigin: false }));
  }

  async create(options: Record<string, unknown>): Promise<RegistrationJSON> {
    this.keys = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign', 'verify']);
    const raw = new Uint8Array(await crypto.subtle.exportKey('raw', this.keys.publicKey));
    const cose = new Uint8Array(cbor([[1, 2], [3, -7], [-1, 1], [-2, raw.slice(1, 33)], [-3, raw.slice(33, 65)]]));
    const attested = new Uint8Array([...new Uint8Array(16), 0, this.credentialId.length, ...this.credentialId, ...cose]);
    const authData = await this.authData(0x01 | 0x04 | 0x40, attested);
    const att = new Uint8Array(cbor([['fmt', 'none'], ['attStmt', []], ['authData', authData]]));
    const challenge = (options as { challenge: string }).challenge;
    const padded = challenge.replace(/-/g, '+').replace(/_/g, '/');
    const bytes = Uint8Array.from(atob(padded + '==='.slice((padded.length + 3) % 4)), (c) => c.charCodeAt(0));
    return { attestationObject: b64url(att), clientDataJson: b64url(this.clientData('webauthn.create', bytes)) };
  }

  async get(challenge: Uint8Array): Promise<AssertionJSON> {
    if (!this.keys) throw new Error('no passkey yet');
    this.counter += 1;
    const clientData = this.clientData('webauthn.get', challenge);
    const authData = await this.authData(0x01 | 0x04);
    const message = new Uint8Array([...authData, ...(await sha256(clientData))]);
    const sig = new Uint8Array(await crypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-256' }, this.keys.privateKey, message));
    return {
      credentialId: this.id,
      authenticatorData: b64url(authData),
      clientDataJson: b64url(clientData),
      signature: b64url(toDer(sig)),
    };
  }
}
