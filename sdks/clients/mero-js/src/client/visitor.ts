// An anonymous visitor through a public link (core's `/apps/{app}/public-links`): the
// credential each request carries to call an instance's `#[app::public]` methods, made of the
// link's token and the visitor's own Ed25519 key, signed fresh per request. The node names the
// visitor to the app as `Caller::Visitor { link, key }`. Mirrors core's
// `calimero_client::visitor`; the vector in `visitor.test.ts` is the one both check.

const PUBLIC_LINK_DOMAIN = 'calimero.public-link.v1\0';
const VISITOR_DOMAIN = 'calimero.visitor.v1\0';
/** The PKCS#8 prefix of an Ed25519 private key whose 32-byte seed follows. */
const PKCS8_PREFIX = [0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];

const encoder = new TextEncoder();

function hex(bytes: ArrayBuffer | Uint8Array): string {
  return Array.from(new Uint8Array(bytes), (b) => b.toString(16).padStart(2, '0')).join('');
}

function fromBase64Url(text: string): Uint8Array {
  const b64 = text.replace(/-/g, '+').replace(/_/g, '/');
  const bin = atob(b64 + '='.repeat((4 - (b64.length % 4)) % 4));
  return Uint8Array.from(bin, (c) => c.charCodeAt(0));
}

function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

/** The Ed25519 key whose seed is `seed`, and its public key in hex. */
async function keyFromSeed(seed: Uint8Array): Promise<{ key: CryptoKey; publicKey: string }> {
  if (seed.length !== 32) throw new Error('an Ed25519 seed is 32 bytes');
  const der = concat(Uint8Array.from(PKCS8_PREFIX), seed);
  const key = await crypto.subtle.importKey('pkcs8', der as BufferSource, { name: 'Ed25519' }, true, ['sign']);
  const jwk = await crypto.subtle.exportKey('jwk', key);
  if (!jwk.x) throw new Error('the Ed25519 key has no public half');
  return { key, publicKey: hex(fromBase64Url(jwk.x)) };
}

/** A public link token's key (hex): how the node and the app name the link. */
export async function publicLinkKey(token: string): Promise<string> {
  const seed = new Uint8Array(await crypto.subtle.digest('SHA-256', encoder.encode(PUBLIC_LINK_DOMAIN + token) as BufferSource));
  return (await keyFromSeed(seed)).publicKey;
}

/** A new visitor seed: 32 random bytes. Keep it (in local storage, say) to stay the same visitor. */
export function newVisitorSeed(): Uint8Array {
  return crypto.getRandomValues(new Uint8Array(32));
}

/** A visitor of one public link: its token and the visitor's key. */
export class Visitor {
  private constructor(
    private readonly token: string,
    /** The link's key (hex). */
    readonly link: string,
    private readonly key: CryptoKey,
    /** The visitor's public key (64 lowercase hex): how the app names it. */
    readonly publicKey: string,
  ) {}

  /** The visitor of the link whose token is `token`, with the key whose seed is `seed`. */
  static async create(token: string, seed: Uint8Array): Promise<Visitor> {
    const [link, { key, publicKey }] = await Promise.all([publicLinkKey(token), keyFromSeed(seed)]);
    return new Visitor(token, link, key, publicKey);
  }

  /** The bearer credential for a request at `at` (Unix seconds; default now): valid within 5 minutes of the node's clock. */
  async bearer(at: number = Math.floor(Date.now() / 1000)): Promise<string> {
    const message = concat(
      encoder.encode(VISITOR_DOMAIN + this.link),
      Uint8Array.of(0),
      encoder.encode(this.publicKey),
      Uint8Array.of(0),
      encoder.encode(String(at)),
    );
    const signature = await crypto.subtle.sign('Ed25519', this.key, message as BufferSource);
    return `vis1.${this.token}.${this.publicKey}.${at}.${hex(signature)}`;
  }
}
