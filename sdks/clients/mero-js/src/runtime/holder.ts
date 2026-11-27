/**
 * A session's holder key: a P-256 key the device keeps and no script can read.
 *
 * A refresh token is a bearer credential: whoever copies it (a malicious
 * dependency of the page, a cross-site-scripting bug) could refresh the session
 * from anywhere for as long as it lives. A session bound to a holder key is
 * refreshed only with that key's signature. The key is made with WebCrypto as
 * non-extractable, so its private half never exists as bytes a script can
 * send elsewhere; it is kept in IndexedDB as the `CryptoKey` itself, so a reload
 * keeps the session. A copied refresh token alone then refreshes nothing, and
 * the node keeps such a token 7 days rather than 30.
 *
 * Node side: `RefreshRecord::holder` and `proof::verify_refresh_holder` in
 * core's `calimero-node`; [Sessions](https://calimero-network.github.io/core/protocol/identities/#sessions).
 */

import { b64url } from './passkey.js';

/** What a holder signs, before the token's hash and the time: `calimero.refresh-holder.v1\0`. */
const HOLDER_DOMAIN = 'calimero.refresh-holder.v1\0';

/** A holder's proof that it uses a refresh token: `signature` by its key over the token's hash and `at`. */
export interface HolderProof {
  /** Unix seconds; the node takes a proof within 120 s of its clock. */
  at: number;
  /** base64url of the 64-byte `r ‖ s` of ECDSA P-256 with SHA-256. */
  signature: string;
}

/**
 * Who holds a session's key: the public half a sign-in binds the session to,
 * and the proof each refresh carries.
 */
export interface SessionHolder {
  /** base64url of the key's uncompressed public point (65 bytes), as `holderKey`. */
  publicKey(): Promise<string>;
  /** The proof for `refreshToken`, made now. */
  prove(refreshToken: string): Promise<HolderProof>;
}

const ALGORITHM = { name: 'ECDSA', namedCurve: 'P-256' } as const;
const SIGNING = { name: 'ECDSA', hash: 'SHA-256' } as const;

/**
 * A new holder key pair whose private half cannot be exported: `generateKey`
 * with `extractable: false`. The public half is exportable, as WebCrypto keeps
 * every public key.
 */
export function generateHolderKeyPair(): Promise<CryptoKeyPair> {
  return crypto.subtle.generateKey(ALGORITHM, false, ['sign', 'verify']);
}

/** The bytes a holder signs for a token: the domain, `b64url(SHA-256(token))` and `at`, each followed by a zero byte. */
export async function holderMessage(refreshToken: string, at: number): Promise<Uint8Array> {
  const hash = b64url(new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(refreshToken))));
  return new TextEncoder().encode(`${HOLDER_DOMAIN}${hash}\0${at}\0`);
}

/** A {@link SessionHolder} over a key pair the caller keeps. */
export function holderFromKeyPair(pair: CryptoKeyPair | Promise<CryptoKeyPair>): SessionHolder {
  const keys = Promise.resolve(pair);
  let exported: Promise<string> | undefined;
  return {
    publicKey() {
      exported ??= keys.then(async ({ publicKey }) => b64url(new Uint8Array(await crypto.subtle.exportKey('raw', publicKey))));
      return exported;
    },
    async prove(refreshToken) {
      const { privateKey } = await keys;
      const at = Math.floor(Date.now() / 1000);
      const message = await holderMessage(refreshToken, at);
      const signature = new Uint8Array(await crypto.subtle.sign(SIGNING, privateKey, message));
      return { at, signature: b64url(signature) };
    },
  };
}

/** Where {@link browserSessionHolder} keeps its key: the IndexedDB database and the record in it. */
export interface HolderKeyStoreOptions {
  /** The IndexedDB database. Default `mero-holder`. */
  database?: string;
  /** The record the key pair is kept under. Default `session`. */
  name?: string;
}

const STORE = 'keys';

function idb(): IDBFactory | undefined {
  try {
    return (globalThis as { indexedDB?: IDBFactory }).indexedDB;
  } catch {
    return undefined;
  }
}

function request<T>(req: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error('IndexedDB request failed'));
  });
}

async function openStore(database: string): Promise<IDBDatabase> {
  const factory = idb();
  if (!factory) throw new Error('no IndexedDB here');
  const open = factory.open(database, 1);
  open.onupgradeneeded = () => {
    if (!open.result.objectStoreNames.contains(STORE)) open.result.createObjectStore(STORE);
  };
  return request(open);
}

/** The pair kept under `name`, or a new one kept there. Throws when IndexedDB is not there or refuses. */
async function keptKeyPair(database: string, name: string): Promise<CryptoKeyPair> {
  const db = await openStore(database);
  try {
    const kept = (await request(db.transaction(STORE, 'readonly').objectStore(STORE).get(name))) as CryptoKeyPair | undefined;
    if (kept?.privateKey && kept.publicKey) return kept;
    const pair = await generateHolderKeyPair();
    await request(db.transaction(STORE, 'readwrite').objectStore(STORE).put(pair, name));
    return pair;
  } finally {
    db.close();
  }
}

/**
 * The device's holder key, kept in IndexedDB as a non-extractable `CryptoKey`
 * (structured clone keeps it unexportable), made on first use. Where IndexedDB
 * is not there or refuses (a private window, Node), the key lives as long as
 * this object, so a reload signs in again. Answers `undefined` where WebCrypto
 * is not there at all.
 */
export function browserSessionHolder(options: HolderKeyStoreOptions = {}): SessionHolder | undefined {
  if (typeof crypto === 'undefined' || !crypto.subtle) return undefined;
  const database = options.database ?? 'mero-holder';
  const name = options.name ?? 'session';
  const pair = keptKeyPair(database, name).catch(() => generateHolderKeyPair());
  return holderFromKeyPair(pair);
}

/**
 * Forget the device's holder key ({@link browserSessionHolder}), so the next
 * sign-in binds a new one. A session bound to the old key cannot be refreshed
 * after this. Does nothing where IndexedDB is not there.
 */
export async function forgetSessionHolder(options: HolderKeyStoreOptions = {}): Promise<void> {
  if (!idb()) return;
  const db = await openStore(options.database ?? 'mero-holder');
  try {
    await request(db.transaction(STORE, 'readwrite').objectStore(STORE).delete(options.name ?? 'session'));
  } finally {
    db.close();
  }
}
