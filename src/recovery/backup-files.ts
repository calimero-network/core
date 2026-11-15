/**
 * The recovery disk's files, as their holder's device handles them.
 *
 * A Calimero node (`merod --recovery-dir`) writes each app's backup sealed to
 * the X25519 public key of whoever upgrades the app: the operator, or the
 * app's organization or owner account. Only that key's private half opens a
 * file, and it never reaches a node. The device:
 *
 * 1. makes the key pair ({@link newBackupKeyPair}) and registers the public
 *    half (`POST /orgs/{org}/backup-key` or `/auth/backup-key`, with a passkey);
 * 2. after a loss, gets the files' headers (`GET /ops/recovery-files`, through
 *    the operator), checks each with {@link verifyRecoveryHeader} and opens
 *    each one's data key with {@link openDataKey};
 * 3. sends those keys straight to the node, over the attested sealed channel
 *    (`POST /recover/holder`, {@link holderRecoveryBody}).
 *
 * The format is core's `calimero_client::recovery`: HPKE (RFC 9180) base mode,
 * DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, AES-256-GCM, `info`
 * {@link HPKE_INFO}, `aad` {@link HPKE_AAD}; the header is JSON signed by the
 * node's Ed25519 receipt key over {@link SIGNING_LABEL} and its bytes. The
 * tests open what core sealed. {@link verifyRecoveryAttestation} checks, as
 * core's `verify_attestation` does, that the signing key is an attested node's
 * receipt key: give it the quote verifier the client trusts nodes with.
 */

import { concat, fromHex, hex } from '../crypto/internal.js';
import { dh, x25519KeyPair } from '../sealed/noise.js';
import type { Governance } from '../client/approvals.js';
import type { VerifyTransportQuote } from '../sealed/sealed.js';

export const RECOVERY_FORMAT = 1;
export const SIGNING_LABEL = new TextEncoder().encode('calimero.recovery-backup.v1\0');
export const HPKE_INFO = new TextEncoder().encode('calimero.recovery-backup.v1');
export const HPKE_AAD = new TextEncoder().encode('calimero.recovery-backup.key.v1');

const KEM_ID = 0x0020; // DHKEM(X25519, HKDF-SHA256)
const KDF_ID = 0x0001; // HKDF-SHA256
const AEAD_ID = 0x0002; // AES-256-GCM
const MODE_BASE = 0x00;

const utf8 = (s: string) => new TextEncoder().encode(s);
const i2osp2 = (n: number) => new Uint8Array([(n >> 8) & 0xff, n & 0xff]);
const KEM_SUITE = concat(utf8('KEM'), i2osp2(KEM_ID));
const HPKE_SUITE = concat(utf8('HPKE'), i2osp2(KEM_ID), i2osp2(KDF_ID), i2osp2(AEAD_ID));

/** Who holds a backup key: `{ kind: 'operator' }`, `{ kind: 'org', id }`, `{ kind: 'account', id }`. */
export type BackupHolder = { kind: 'operator' } | { kind: 'org'; id: string } | { kind: 'account'; id: string };

/** What a recovery file's header says (core's `recovery::Header`). */
export interface RecoveryHeader {
  format: number;
  kind: 'app' | 'holder' | 'system' | 'code';
  holder: BackupHolder;
  cluster: string;
  app?: string;
  org?: string;
  hash?: string;
  group: number;
  index: number;
  writtenAt: number;
  merod: string;
  release: string;
  node: number;
  recipient: string;
  enc: string;
  wrappedKey: string;
  size: number;
  sha256: string;
  bodyBytes: number;
  bodySha256: string;
  signingKey: string;
  attestation: unknown;
}

/** A listed file as `GET /ops/recovery-files` answers it. */
export interface ListedRecoveryFile {
  name: string;
  /** The header as signed (JSON text). */
  header: string;
  /** The node's Ed25519 signature, hex. */
  signature: string;
}

/** `operator`, `org:<id>` or `account:<id>`: how routes name a holder. */
export function holderKey(holder: BackupHolder): string {
  return holder.kind === 'operator' ? 'operator' : `${holder.kind}:${holder.id}`;
}

/**
 * A new backup key pair: keep `secret` (32 raw bytes) off every node, offline;
 * register `publicKey` (64 hex).
 */
export async function newBackupKeyPair(): Promise<{ secret: Uint8Array; publicKey: string }> {
  const secret = crypto.getRandomValues(new Uint8Array(32));
  return { secret, publicKey: await backupPublicKey(secret) };
}

/** The public key (64 hex) of a backup key's secret. */
export async function backupPublicKey(secret: Uint8Array): Promise<string> {
  return hex((await x25519KeyPair(secret)).publicKey);
}

/**
 * Check a header against the node's signature and read it. Refuses a format
 * newer than this release reads and a signature that does not hold.
 */
export async function verifyRecoveryHeader(file: Pick<ListedRecoveryFile, 'header' | 'signature'>): Promise<RecoveryHeader> {
  const bytes = utf8(file.header);
  const header = JSON.parse(file.header) as RecoveryHeader;
  if (typeof header.format !== 'number' || header.format > RECOVERY_FORMAT) {
    throw new Error(`recovery file format ${header.format} is not one this release reads (${RECOVERY_FORMAT})`);
  }
  const key = await crypto.subtle.importKey(
    'raw',
    fromHex(header.signingKey, 'signing key', 32),
    { name: 'Ed25519' },
    false,
    ['verify'],
  );
  const ok = await crypto.subtle.verify(
    'Ed25519',
    key,
    fromHex(file.signature, 'signature', 64),
    concat(SIGNING_LABEL, bytes),
  );
  if (!ok) throw new Error("the node's signature over the header does not hold");
  return header;
}

async function hmac(key: Uint8Array, data: Uint8Array): Promise<Uint8Array> {
  const raw = key.length === 0 ? new Uint8Array(32) : key;
  const k = await crypto.subtle.importKey('raw', raw, { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
  return new Uint8Array(await crypto.subtle.sign('HMAC', k, data));
}

async function expand(prk: Uint8Array, info: Uint8Array, length: number): Promise<Uint8Array> {
  const out = new Uint8Array(length);
  let previous = new Uint8Array(0);
  for (let i = 1, offset = 0; offset < length; i += 1) {
    previous = await hmac(prk, concat(previous, info, new Uint8Array([i])));
    out.set(previous.subarray(0, Math.min(previous.length, length - offset)), offset);
    offset += previous.length;
  }
  return out;
}

const labeledExtract = (suite: Uint8Array, salt: Uint8Array, label: string, ikm: Uint8Array) =>
  hmac(salt, concat(utf8('HPKE-v1'), suite, utf8(label), ikm));

const labeledExpand = (suite: Uint8Array, prk: Uint8Array, label: string, info: Uint8Array, length: number) =>
  expand(prk, concat(i2osp2(length), utf8('HPKE-v1'), suite, utf8(label), info), length);

/** Open a file's data key (32 bytes) with the holder's secret: HPKE base mode. */
export async function openDataKey(header: RecoveryHeader, secret: Uint8Array): Promise<Uint8Array> {
  const recipient = await x25519KeyPair(secret);
  if (hex(recipient.publicKey) !== header.recipient) {
    throw new Error('this key is not the one the file was sealed to');
  }
  const enc = fromHex(header.enc, 'encapsulated key', 32);
  const dhValue = await dh(recipient.privateKey, enc);
  const empty = new Uint8Array(0);
  const prk = await labeledExtract(KEM_SUITE, empty, 'eae_prk', dhValue);
  const shared = await labeledExpand(KEM_SUITE, prk, 'shared_secret', concat(enc, recipient.publicKey), 32);
  const pskIdHash = await labeledExtract(HPKE_SUITE, empty, 'psk_id_hash', empty);
  const infoHash = await labeledExtract(HPKE_SUITE, empty, 'info_hash', HPKE_INFO);
  const context = concat(new Uint8Array([MODE_BASE]), pskIdHash, infoHash);
  const schedule = await labeledExtract(HPKE_SUITE, shared, 'secret', empty);
  const key = await labeledExpand(HPKE_SUITE, schedule, 'key', context, 32);
  const nonce = await labeledExpand(HPKE_SUITE, schedule, 'base_nonce', context, 12);
  const aes = await crypto.subtle.importKey('raw', key, 'AES-GCM', false, ['decrypt']);
  let plain: Uint8Array;
  try {
    plain = new Uint8Array(
      await crypto.subtle.decrypt(
        { name: 'AES-GCM', iv: nonce, additionalData: HPKE_AAD },
        aes,
        fromHex(header.wrappedKey, 'wrapped key', 48),
      ),
    );
  } catch {
    throw new Error('this key does not open the file');
  }
  if (plain.length !== 32) throw new Error('the data key is not 32 bytes');
  return plain;
}

/** `{ file, key }`: a file and its opened data key, as the recovery routes take them. */
export interface OpenedRecoveryFile {
  file: string;
  key: string;
}

/**
 * The body of `POST /recover/holder` from a holder's listed files: the newest
 * `holder` file, the newest file of each app in `apps` (all of them when
 * absent) and every code file, each checked and opened with `secret`; with
 * `verifyQuote`, each one's attestation too ({@link verifyRecoveryAttestation}).
 */
export async function holderRecoveryBody(
  holder: BackupHolder,
  files: ListedRecoveryFile[],
  secret: Uint8Array,
  options: { apps?: string[]; verifyQuote?: VerifyTransportQuote } = {},
): Promise<{ holder: string; slice: OpenedRecoveryFile; apps: OpenedRecoveryFile[]; code: OpenedRecoveryFile[] }> {
  const checked: { name: string; header: RecoveryHeader }[] = [];
  for (const f of [...files].sort((a, b) => a.name.localeCompare(b.name))) {
    const header = await verifyRecoveryHeader(f);
    if (options.verifyQuote) await verifyRecoveryAttestation(header, options.verifyQuote);
    if (holderKey(header.holder) !== holderKey(holder)) {
      throw new Error(`${f.name} is ${holderKey(header.holder)}'s, not ${holderKey(holder)}'s`);
    }
    checked.push({ name: f.name, header });
  }
  const open = async (c: { name: string; header: RecoveryHeader }) => ({
    file: c.name,
    key: hex(await openDataKey(c.header, secret)),
  });
  const slice = checked.filter((c) => c.header.kind === 'holder').pop();
  if (!slice) throw new Error(`no holder file of ${holderKey(holder)}`);
  const newestByApp = new Map<string, { name: string; header: RecoveryHeader }>();
  for (const c of checked) {
    if (c.header.kind === 'app' && c.header.app && (!options.apps || options.apps.includes(c.header.app))) {
      newestByApp.set(c.header.app, c);
    }
  }
  return {
    holder: holderKey(holder),
    slice: await open(slice),
    apps: await Promise.all([...newestByApp.values()].map(open)),
    code: await Promise.all(checked.filter((c) => c.header.kind === 'code').map(open)),
  };
}

async function sha256(data: Uint8Array): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest('SHA-256', data));
}

/** The nonce a node's recovery attestation binds (core's `recovery::attestation_nonce`), hex. */
export async function recoveryAttestationNonce(cluster: string, node: number): Promise<string> {
  const name = utf8(cluster);
  const len = new Uint8Array(4);
  new DataView(len.buffer).setUint32(0, name.length, false);
  const id = new Uint8Array(8);
  new DataView(id.buffer).setBigUint64(0, BigInt(node), false);
  return hex(await sha256(concat(utf8('calimero.recovery-backup.attest.v1\0'), len, name, id)));
}

const one = <T extends string>(value: unknown, allowed: readonly T[], label: string, fallback?: T): T => {
  if (value === undefined && fallback) return fallback;
  if (!allowed.includes(value as T)) throw new Error(`the governance's ${label} is not one of ${allowed.join(', ')}`);
  return value as T;
};

/** The bytes a quote binds: the JSON, fields in declaration order, the later ones only when not their default (core's `Governance::canonical`). */
export function governanceCanonical(g: Governance): Uint8Array {
  const out: Record<string, string> = {
    releases: one(g.releases, ['owner', 'operator'], 'releases'),
    appUpgrades: one(g.appUpgrades, ['owner', 'operator'], 'appUpgrades'),
    outbound: one(g.outbound, ['owner', 'operator'], 'outbound'),
  };
  const deployments = one(g.deployments, ['open', 'granted'], 'deployments', 'open');
  if (deployments !== 'open') out.deployments = deployments;
  const deployers = one(g.deployers, ['operator', 'owner'], 'deployers', 'operator');
  if (deployers !== 'operator') out.deployers = deployers;
  const signUp = one(g.signUp, ['open', 'invite', 'closed'], 'signUp', 'open');
  if (signUp !== 'open') out.signUp = signUp;
  return utf8(JSON.stringify(out));
}

/**
 * What a receipt-key quote binds after its nonce (core's `receipt_key_binding`,
 * `receipt-key.v3`), hex: the key, the cluster's governance as the node
 * serialized it, and the cluster (u32-BE length-prefixed) and node id (u64 BE),
 * so one node's quote never stands for another node's or another cluster's.
 */
export async function receiptKeyBinding(
  receiptKey: Uint8Array,
  governance: Governance,
  cluster: string,
  node: number,
): Promise<string> {
  const g = await sha256(governanceCanonical(governance));
  const name = utf8(cluster);
  const len = new Uint8Array(4);
  new DataView(len.buffer).setUint32(0, name.length, false);
  const id = new Uint8Array(8);
  new DataView(id.buffer).setBigUint64(0, BigInt(node), false);
  return hex(await sha256(concat(utf8('calimero.tee-attest.receipt-key.v3'), receiptKey, g, len, name, id)));
}

interface ReceiptAttestation {
  quote: string;
  nonce: string;
  cluster: string;
  node: number;
  receiptKey?: string;
  governance?: Governance;
}

/**
 * Check that the key that signed `header` is a node's receipt key, attested
 * (`purpose=receipts`) by a quote `verifyQuote` accepts: attested hardware
 * wrote the file. Rejects with the reason.
 */
export async function verifyRecoveryAttestation(
  header: RecoveryHeader,
  verifyQuote: VerifyTransportQuote,
): Promise<void> {
  const doc = header.attestation as ReceiptAttestation | null;
  if (!doc || typeof doc.quote !== 'string') throw new Error('the file carries no attestation');
  if (doc.cluster !== header.cluster || doc.node !== header.node) {
    throw new Error("the attestation is another node's");
  }
  if (doc.receiptKey !== header.signingKey) {
    throw new Error('the attested key is not the one that signed the file');
  }
  const nonce = await recoveryAttestationNonce(header.cluster, header.node);
  if (doc.nonce !== nonce) throw new Error('the attestation is not over the recovery nonce');
  if (!doc.governance) throw new Error('the attestation names no governance');
  const reportDataSuffix = await receiptKeyBinding(
    fromHex(header.signingKey, 'signing key', 32),
    doc.governance,
    header.cluster,
    header.node,
  );
  const ok = await verifyQuote({ quoteB64: doc.quote, nonce, reportDataSuffix });
  if (!ok) throw new Error('the quote was not accepted');
}
