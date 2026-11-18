/**
 * Checking a pipeline run's receipt as core's `calimero-client` does (`Client::verify_app_cluster`,
 * `attest::verify_receipt_key`, `Client::verify_receipt`): the receipt is signed by the answering
 * node's receipt key, and that key is one every replica's quote attests, bound to the cluster and
 * node the entry node names. A quote of the unbound `receipt-key.v2` form is never accepted. A
 * version 2 receipt also proves each job's report: its job key signed what the proof says, and a
 * job node's quote under the client's policy binds that key, node, cluster and cluster identity.
 */

import { concat, fromHex, hex } from '../crypto/internal.js';
import { governanceCanonical, receiptKeyBinding } from '../recovery/backup-files.js';
import { jcs } from '../package/pack.js';
import { verifyQuoteBinding, type AttestationDoc, type NodePolicy } from '../runtime/attestation.js';
import type { AppsRoutes } from '../generated/methods.js';
import type { Governance } from './approvals.js';
import { buf } from './recovery-key.js';

const PIPELINE_RECEIPT_DOMAIN = 'calimero.pipeline.receipt.v1\0';
const utf8 = (s: string) => new TextEncoder().encode(s);

/** A receipt-key attestation: the quote binds the key, the governance, and the cluster and node asked for. */
export type ReceiptKeyDoc = AttestationDoc & { receiptKey?: string; governance?: Governance };

/** What `GET /apps/{app}/pipeline-runs/{run}/receipt` answers: the receipt as sent, the signing node, its signature (hex). Checked at run time. */
export interface ReceiptAnswer {
  receipt: unknown;
  node: unknown;
  signature: unknown;
}

/** A file named by id, size and plaintext SHA-256 (hex). */
export interface FileRef {
  id: string;
  size: number;
  sha256: string;
}

/** What one step run's proof records: which job node ran which assignment, and what came out. */
export interface StepRunProof {
  run: string;
  step: string;
  index: number;
  outcome: string;
  attempt: number;
  node?: number | null;
  jobKey?: string | null;
  assignmentDigest?: string | null;
  reportSha256?: string | null;
  signature?: string | null;
  outputs: FileRef[];
  usage: { cpuMicros: string; gibMicros: string; fuel: string };
  /** What the job's report signs that the fields above do not (version 2 only). */
  job?: JobEvidence | null;
}

/** A file as a job's report signs it: id, plaintext size and SHA-256, and each ciphertext segment's SHA-256 (lowercase hex). */
export interface JobOutput {
  id: string;
  size: number;
  sha256: string;
  segments: string[];
}

/** The rest of what a job's report signs: the result's hash, the failure text, each output's segment hashes, the module log. */
export interface JobEvidence {
  resultSha256: string;
  message?: string;
  segments: string[][];
  log?: JobOutput | null;
}

/** A job node's `purpose=jobs` attestation: the quote binds its transport key, job key, release, cluster, node and cluster identity. */
export type JobAttestationDoc = AttestationDoc & { jobKey?: string; clusterIdentity?: string };

/** A run answered from the step cache: it ran nothing, so it has no proof. */
export interface CachedRun {
  index: number;
  cached: boolean;
  cacheKey: string;
}

export interface StepReceipt {
  status: string;
  reason?: string | null;
  runs: (CachedRun | StepRunProof)[];
}

/** What the node states about a finished pipeline run (core's `PipelineReceipt`). `cluster` is the cluster identity key, hex. */
export interface PipelineReceipt {
  version: number;
  cluster: string;
  app: string;
  run: string;
  pipeline: string;
  release: string;
  pipelineHash: string;
  modules: Record<string, string>;
  inputs: Record<string, FileRef[]>;
  params: Record<string, unknown>;
  state: string;
  startTsMs: number;
  endTsMs: number;
  steps: Record<string, StepReceipt>;
  costSoFar?: string | null;
  /** Each job key a proof names, its job node's attestation (version 2 only). */
  jobAttestations?: Record<string, JobAttestationDoc>;
}

/** Every replica of an app, attested: its receipt key by node id, the governance they all attest, and the cluster name every quote bound. */
export interface AttestedCluster {
  keys: Map<number, Uint8Array>;
  governance: Governance;
  cluster: string;
}

type AttestationRoutes = Pick<AppsRoutes, 'attestation'>;

// The receipt's schema, as core's serde types read it: `deny_unknown_fields`, required fields, typed values.
type Check = (value: unknown, path: string) => void;
const fail = (path: string, what: string): never => {
  throw new Error(`the receipt's ${path} ${what}`);
};
const hasOwn = (o: object, k: string) => Object.prototype.hasOwnProperty.call(o, k);
const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const str: Check = (v, p) => typeof v === 'string' || fail(p, 'is not a string');
const bool: Check = (v, p) => typeof v === 'boolean' || fail(p, 'is not a boolean');
const any: Check = () => undefined;
const obj: Check = (v, p) => isObject(v) || fail(p, 'is not an object');
const uint =
  (max: number): Check =>
  (v, p) =>
    (typeof v === 'number' && Number.isInteger(v) && v >= 0 && v <= max) || fail(p, 'is not an unsigned integer in range');
const u32 = uint(0xffffffff);
const u64: Check = (v, p) =>
  (typeof v === 'number' && Number.isInteger(v) && v >= 0 && v < 2 ** 64) || fail(p, 'is not an unsigned integer in range'); // 2^64 itself is not a u64
const list =
  (item: Check): Check =>
  (v, p) => {
    if (!Array.isArray(v)) fail(p, 'is not an array');
    (v as unknown[]).forEach((x, i) => item(x, `${p}[${i}]`));
  };
const map =
  (value: Check): Check =>
  (v, p) => {
    if (!isObject(v)) fail(p, 'is not an object');
    for (const [k, x] of Object.entries(v as Record<string, unknown>)) value(x, `${p}.${k}`);
  };
// `optional` fields are Options (null reads as absent); `defaulted` ones may be absent but never null;
// an `open` struct allows other fields, as a serde type without `deny_unknown_fields`.
const struct =
  (required: Record<string, Check>, optional: Record<string, Check> = {}, defaulted: Record<string, Check> = {}, open = false): Check =>
  (v, p) => {
    if (!isObject(v)) return fail(p, 'is not an object');
    for (const k of Object.keys(v)) {
      if (!open && !hasOwn(required, k) && !hasOwn(optional, k) && !hasOwn(defaulted, k)) fail(`${p}.${k}`, 'is not a known field');
    }
    for (const [k, check] of Object.entries(required)) {
      if (!hasOwn(v, k)) fail(`${p}.${k}`, 'is missing');
      check(v[k], `${p}.${k}`);
    }
    for (const [k, check] of Object.entries(optional)) {
      if (hasOwn(v, k) && v[k] !== null) check(v[k], `${p}.${k}`);
    }
    for (const [k, check] of Object.entries(defaulted)) {
      if (hasOwn(v, k)) check(v[k], `${p}.${k}`);
    }
  };

const fileRef = struct({ id: str, size: u64, sha256: str });
const jobOutput = struct({ id: str, size: u64, sha256: str, segments: list(str) });
const jobEvidence = struct({ resultSha256: str, segments: list(list(str)) }, { log: jobOutput }, { message: str });
const attestationDoc = struct(
  {
    quote: str,
    transportKey: str,
    appHash: str,
    nonce: str,
    release: struct({ version: str, profile: str }, {}, {}, true),
    cluster: str,
    node: u64,
    mock: bool,
  },
  { receiptKey: str, governance: obj, kms: obj, jobKey: str, clusterIdentity: str },
  {},
  true,
);
const stepRun = struct(
  {
    run: str,
    step: str,
    index: u32,
    outcome: str,
    attempt: u32,
    outputs: list(fileRef),
    usage: struct({ cpuMicros: str, gibMicros: str, fuel: str }),
  },
  { node: u64, jobKey: str, assignmentDigest: str, reportSha256: str, signature: str, job: jobEvidence },
);
const cachedRun = struct({ index: u32, cached: bool, cacheKey: str });
// Core's `RunEntry` is untagged: a cached run first, else a step run proof.
const runEntry: Check = (v, p) => {
  try {
    cachedRun(v, p);
  } catch {
    stepRun(v, p);
  }
};
const pipelineReceipt = struct(
  {
    version: u32,
    cluster: str,
    app: str,
    run: str,
    pipeline: str,
    release: str,
    pipelineHash: str,
    modules: map(str),
    inputs: map(list(fileRef)),
    params: map(any),
    state: str,
    startTsMs: u64,
    endTsMs: u64,
    steps: map(struct({ status: str, runs: list(runEntry) }, { reason: str })),
  },
  { costSoFar: str },
  { jobAttestations: map(attestationDoc) },
);

/** Hex as core's `hex::decode` reads it (either case), of exactly `bytes` bytes: no trimming, nothing else. */
function strictHex(value: unknown, label: string, bytes: number): Uint8Array {
  if (typeof value !== 'string' || value.length !== bytes * 2 || !/^[0-9a-fA-F]*$/.test(value)) {
    throw new Error(`${label} is not ${bytes} bytes of hex`);
  }
  return fromHex(value, label, bytes);
}

/** A node id: a non-negative safe integer, as a JSON number. */
function nodeId(value: unknown, label: string): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) throw new Error(`${label} is not a node id`);
  return value;
}

/**
 * Checks a receipt-key attestation as core's `verify_receipt_key` does: its quote binds the key, the
 * governance and the `cluster` and `node` the caller asked for (never the document's own fields, which
 * nothing signs). Answers the key and the governance rebuilt from its canonical fields.
 */
export async function verifyReceiptKey(
  doc: ReceiptKeyDoc,
  nonce: Uint8Array,
  cluster: string,
  node: number,
  policy: NodePolicy,
): Promise<{ key: Uint8Array; governance: Governance }> {
  if (typeof doc.quote !== 'string') throw new Error('the attestation carries no quote');
  if (!doc.receiptKey) throw new Error('the attestation names no receipt key');
  if (!isObject(doc.governance)) throw new Error('the attestation names no governance');
  const key = strictHex(doc.receiptKey, 'the receipt key', 32);
  const suffix = fromHex(await receiptKeyBinding(key, doc.governance, cluster, node), 'binding', 32);
  await verifyQuoteBinding(doc, nonce, suffix, policy, 'receipt key');
  return { key, governance: JSON.parse(new TextDecoder().decode(governanceCanonical(doc.governance))) as Governance };
}

/**
 * Verifies every replica of `app` (core's `Client::verify_app_cluster`): each one's quote over a fresh
 * nonce, held to `policy`, binding its receipt key, the cluster's governance, and the cluster and node
 * id the entry node names. Any replica error, a node id that differs from the document's, or replicas
 * that attest other governance fails the whole check.
 */
export async function verifyAppCluster(
  client: { apps: AttestationRoutes },
  app: string,
  policy: NodePolicy,
): Promise<AttestedCluster> {
  const nonce = crypto.getRandomValues(new Uint8Array(32));
  const answer: unknown = await client.apps.attestation({ app, nonce: hex(nonce) });
  if (!isObject(answer) || typeof answer.cluster !== 'string' || !Array.isArray(answer.replicas)) {
    throw new Error('the node answered a malformed attestation');
  }
  const cluster = answer.cluster;
  const keys = new Map<number, Uint8Array>();
  let governance: Governance | undefined;
  for (const replica of answer.replicas as unknown[]) {
    if (!isObject(replica)) throw new Error('a replica entry is not an object');
    const node = nodeId(replica.node, 'a replica');
    if (hasOwn(replica, 'error')) throw new Error(`replica ${node} gave no attestation: ${String(replica.error)}`);
    const doc = replica.attestation;
    if (!isObject(doc)) throw new Error(`replica ${node} gave neither an attestation nor an error`);
    if (nodeId(doc.node, `replica ${node}'s document`) !== node) throw new Error(`replica ${node} answered as node ${String(doc.node)}`);
    let attested;
    try {
      attested = await verifyReceiptKey(doc as unknown as ReceiptKeyDoc, nonce, cluster, node, policy);
    } catch (error) {
      throw new Error(`replica ${node}: ${error instanceof Error ? error.message : String(error)}`);
    }
    // verifyReceiptKey answers the governance rebuilt from its canonical bytes, so equal JSON is equal rules.
    if (governance && JSON.stringify(governance) !== JSON.stringify(attested.governance)) {
      throw new Error(`replica ${node} attests other governance`);
    }
    governance = attested.governance;
    keys.set(node, attested.key);
  }
  if (!governance) throw new Error(`no replica of ${app} attested`);
  return { keys, governance, cluster };
}

/** The receipt's canonical JSON (RFC 8785); any finite number is allowed, as core canonicalizes every number as f64. */
const canonicalReceipt = (receipt: unknown) => jcs(receipt as Parameters<typeof jcs>[0], true);

/** The bytes a receipt's signature covers (core's `receipt_message`): the domain, then the canonical JSON. */
export const receiptMessage = (receipt: unknown): Uint8Array => concat(utf8(PIPELINE_RECEIPT_DOMAIN), utf8(canonicalReceipt(receipt)));

/**
 * Whether `answer` is signed by `key` (core's `verify_pipeline_receipt`). Answers the receipt as parsed
 * from the canonical bytes that were verified, checked against core's receipt schema.
 */
export async function verifyReceiptSignature(answer: ReceiptAnswer, key: Uint8Array): Promise<PipelineReceipt> {
  if (!isObject(answer)) throw new Error('the receipt answer is not an object');
  const unknown = Object.keys(answer).find((k) => !['receipt', 'node', 'signature'].includes(k));
  if (unknown !== undefined) throw new Error(`the receipt answer's ${unknown} is not a known field`);
  const signature = strictHex(answer.signature, 'the signature', 64);
  const canonical = canonicalReceipt(answer.receipt);
  const publicKey = await crypto.subtle.importKey('raw', buf(key), { name: 'Ed25519' }, false, ['verify']);
  if (!(await crypto.subtle.verify('Ed25519', publicKey, buf(signature), buf(concat(utf8(PIPELINE_RECEIPT_DOMAIN), utf8(canonical)))))) {
    throw new Error('the receipt is not signed by that key');
  }
  const receipt: unknown = JSON.parse(canonical);
  pipelineReceipt(receipt, 'receipt');
  return receipt as PipelineReceipt;
}

const JOB_REPORT_DOMAIN = 'calimero.compute.report.v1\0';
const JOB_REPORT_LOG_DOMAIN = 'calimero.compute.report.log.v1\0';
const JOB_KEY_DOMAIN = 'calimero.tee-attest.job-key.v1';
const RECEIPT_VERSION_JOBS = 2;
const JOB_FAIL_REASONS = ['fuel', 'memory', 'trap', 'app-error', 'integrity', 'cancelled'];
const LEADER_FAIL_REASONS = ['lost', 'refused', 'timeout', 'quota', 'not-granted', 'too-large'];
const U64_MAX = 2n ** 64n - 1n;

/** One job report's signed fields, in the order {@link reportMessage} writes them (core's `ReportFields`). */
export interface ReportFields {
  /** SHA-256 of the assignment message the job ran (hex). */
  assignment: string;
  app: string;
  run: string;
  attempt: number;
  /** `null` for a done report, else why it failed. */
  reason: string | null;
  message: string;
  resultSha256: string;
  outputs: JobOutput[];
  /** CPU micros, GiB micros, fuel, as decimal strings. */
  usage: [string, string, string];
  log?: JobOutput | null;
}

const sha256 = async (bytes: Uint8Array) => new Uint8Array(await crypto.subtle.digest('SHA-256', buf(bytes)));

const u32be = (n: number) => {
  const out = new Uint8Array(4);
  new DataView(out.buffer).setUint32(0, n, false);
  return out;
};

const u64be = (n: number | bigint) => {
  const big = BigInt(n);
  if (big < 0n || big > U64_MAX) throw new Error('a number is past 2^64-1');
  const out = new Uint8Array(8);
  new DataView(out.buffer).setBigUint64(0, big, false);
  return out;
};

const prefixed = (text: string) => {
  const bytes = utf8(text);
  return concat(u32be(bytes.length), bytes);
};

/** The 32 bytes of a hash spelled as the wire spells it: 64 lowercase hex digits (core's `hash_bytes`). */
function hashBytes(text: unknown): Uint8Array {
  if (typeof text !== 'string' || !/^[0-9a-f]{64}$/.test(text)) throw new Error('a hash is 64 lowercase hex digits');
  return fromHex(text, 'hash', 32);
}

/** A decimal `u64` as the receipt spells usage: digits only. */
function decimal(text: string): bigint {
  if (!/^[0-9]+$/.test(text)) throw new Error(`${JSON.stringify(text)} is not a decimal integer`);
  const n = BigInt(text);
  if (n > U64_MAX) throw new Error(`${JSON.stringify(text)} is not a decimal integer`);
  return n;
}

function outputBytes(o: JobOutput): Uint8Array {
  return concat(hashBytes(o.id), u64be(o.size), hashBytes(o.sha256), u32be(o.segments.length), ...o.segments.map(hashBytes));
}

/**
 * The bytes a job node signs for one report (core's `report_message`): the domain, the assignment digest,
 * the length-prefixed app, run, reason and message, the result's hash, the outputs, the usage, then the
 * log after its own domain; integers big-endian, hashes as their 32 bytes.
 */
export function reportMessage(f: ReportFields): Uint8Array {
  return concat(
    utf8(JOB_REPORT_DOMAIN),
    hashBytes(f.assignment),
    prefixed(f.app),
    prefixed(f.run),
    u32be(f.attempt),
    new Uint8Array([f.reason === null ? 0 : 1]),
    prefixed(f.reason ?? ''),
    prefixed(f.message),
    hashBytes(f.resultSha256),
    u32be(f.outputs.length),
    ...f.outputs.map(outputBytes),
    ...f.usage.map((n) => u64be(decimal(n))),
    ...(f.log ? [utf8(JOB_REPORT_LOG_DOMAIN), outputBytes(f.log)] : []),
  );
}

/**
 * What a job-node attestation's quote binds after the nonce (core's `job_key_binding`): both keys, the
 * release and cluster (length-prefixed), the node id, and the cluster identity when there is one. Hex.
 */
export async function jobKeyBinding(
  transportKey: Uint8Array,
  jobKey: Uint8Array,
  release: string,
  cluster: string,
  node: number,
  clusterIdentity?: Uint8Array,
): Promise<string> {
  return hex(
    await sha256(
      concat(utf8(JOB_KEY_DOMAIN), transportKey, jobKey, prefixed(release), prefixed(cluster), u64be(node), clusterIdentity ?? new Uint8Array(0)),
    ),
  );
}

/** The reason a job signed for `outcome`: `{ reason }` (`null` when done), or `null` for an end only the leader gives. */
function signedReason(outcome: string, reported: boolean): { reason: string | null } | null {
  if (outcome === 'done') return { reason: null };
  if (outcome === 'cancelled') return reported ? { reason: 'cancelled' } : null;
  const bad = () => new Error(`the outcome ${JSON.stringify(outcome)} is not one a run ends with`);
  if (!outcome.startsWith('failed:')) throw bad();
  const reason = outcome.slice('failed:'.length);
  if (JOB_FAIL_REASONS.includes(reason)) return { reason };
  if (!LEADER_FAIL_REASONS.includes(reason)) throw bad();
  return null;
}

// The Ed25519 group order: an S at or above it is a malleated signature, refused as verify_strict does.
const L = 2n ** 252n + 27742317777372353535851937790883648493n;
// The Ed25519 points of order dividing 8, for `verify_strict`'s refusal of a small-order key or R.
const P = 2n ** 255n - 19n;
const D = (((P - 121665n) % P) * modPow(121666n, P - 2n)) % P;
function modPow(base: bigint, exp: bigint): bigint {
  let result = 1n;
  base %= P;
  for (; exp > 0n; exp >>= 1n, base = (base * base) % P) if (exp & 1n) result = (result * base) % P;
  return result;
}

function isSmallOrder(encoded: Uint8Array): boolean {
  let y = 0n;
  for (let i = 31; i >= 0; i--) y = (y << 8n) | BigInt(encoded[i] & (i === 31 ? 0x7f : 0xff));
  y %= P;
  const sign = (encoded[31] >> 7) & 1;
  const y2 = (y * y) % P;
  const x2 = (((y2 - 1n + P) % P) * modPow((D * y2 + 1n) % P, P - 2n)) % P;
  let x = modPow(x2, (P + 3n) / 8n);
  if ((x * x) % P !== x2) x = (x * modPow(2n, (P - 1n) / 4n)) % P;
  if ((x * x) % P !== x2) return false;
  if (Number(x & 1n) !== sign) x = (P - x) % P;
  let pt: [bigint, bigint] = [x, y];
  for (let i = 0; i < 3; i++) {
    const [x1, y1] = pt;
    const k = (D * x1 * x1 * y1 * y1) % P;
    pt = [(2n * x1 * y1 * modPow((1n + k) % P, P - 2n)) % P, ((y1 * y1 + x1 * x1) * modPow((1n - k + P) % P, P - 2n)) % P];
  }
  return pt[0] === 0n && pt[1] === 1n;
}

/** Ed25519 as core's `verify_strict` reads it: a non-canonical S and a small-order key or R are refused as well as a bad signature. */
async function verifyStrict(key: Uint8Array, message: Uint8Array, signature: Uint8Array): Promise<boolean> {
  let s = 0n;
  for (let i = 63; i >= 32; i--) s = (s << 8n) | BigInt(signature[i]);
  if (s >= L || isSmallOrder(key) || isSmallOrder(signature.slice(0, 32))) return false;
  const publicKey = await crypto.subtle.importKey('raw', buf(key), { name: 'Ed25519' }, false, ['verify']);
  return crypto.subtle.verify('Ed25519', publicKey, buf(signature), buf(message));
}

/**
 * Checks one step run's proof (core's `check_proof`): it is the run the receipt's run, step and index
 * name, and either the leader ended it (no report, outputs or usage) or its job node signed a report that
 * says exactly what the proof says. Answers the job key and node of a signed one.
 */
async function checkProof(receipt: PipelineReceipt, step: string, p: StepRunProof): Promise<{ key: string; node: number } | null> {
  const expected = `${receipt.run}.${step}.${p.index}`;
  if (p.run !== expected || p.step !== step) throw new Error(`the proof of ${p.run} is listed as ${expected}`);
  const reported = p.signature != null;
  const signed = signedReason(p.outcome, reported);
  if (!signed) {
    const zero = [p.usage.cpuMicros, p.usage.gibMicros, p.usage.fuel].every((n) => n === '0');
    if (reported || p.reportSha256 != null || p.job != null || p.outputs.length > 0 || !zero) {
      throw new Error(`${p.run} ended ${p.outcome} by the leader, yet names a report, outputs or usage`);
    }
    return null;
  }
  const missing = (what: string) => new Error(`${p.run} ended ${p.outcome} but its proof has no ${what}`);
  if (p.node == null) throw missing('job node');
  if (p.jobKey == null) throw missing('job key');
  if (p.assignmentDigest == null) throw missing('assignment digest');
  if (p.reportSha256 == null) throw missing('report hash');
  if (p.signature == null) throw missing('report signature');
  if (p.job == null) throw missing('job evidence (it ended before receipts carried it)');
  const job = p.job;
  if (job.segments.length !== p.outputs.length) {
    throw new Error(`${p.run}: the job evidence lists segments for ${job.segments.length} outputs, the proof ${p.outputs.length}`);
  }
  const outputs = p.outputs.map((o, i) => ({ id: o.id, size: o.size, sha256: o.sha256, segments: job.segments[i] }));
  let message: Uint8Array;
  try {
    message = reportMessage({
      assignment: p.assignmentDigest,
      app: receipt.app,
      run: p.run,
      attempt: p.attempt,
      reason: signed.reason,
      message: job.message ?? '',
      resultSha256: job.resultSha256,
      outputs,
      usage: [p.usage.cpuMicros, p.usage.gibMicros, p.usage.fuel],
      log: job.log,
    });
  } catch (error) {
    throw new Error(`${p.run}: ${error instanceof Error ? error.message : String(error)}`);
  }
  if (hex(await sha256(message)) !== p.reportSha256) throw new Error(`${p.run}: the report hash is not that of what the proof says`);
  const key = hashBytes(p.jobKey);
  if (!/^[0-9a-fA-F]{128}$/.test(p.signature)) throw new Error(`${p.run}: the report signature is not 64 bytes of hex`);
  if (!(await verifyStrict(key, message, fromHex(p.signature, 'signature', 64)))) {
    throw new Error(`${p.run}: the job key did not sign what the proof says`);
  }
  return { key: p.jobKey, node: p.node };
}

/**
 * Checks every step run's proof of a version 2 receipt (core's `verify_job_reports`), each step's runs
 * listed once in index order, and answers each job key that signed a report with the node it ran on; a
 * key named on two nodes is refused. Whose key each is (its attestation) is for the caller to verify.
 * What a job signed is proven; the rest (step and run states, cached runs, and the node and key a
 * leader-ended run names) is the signing replica's word.
 */
export async function verifyJobReports(receipt: PipelineReceipt): Promise<Map<string, number>> {
  if (receipt.version !== RECEIPT_VERSION_JOBS) {
    throw new Error(`the receipt is version ${receipt.version}; only version ${RECEIPT_VERSION_JOBS} carries what each job signed`);
  }
  const keys = new Map<string, number>();
  for (const [step, s] of Object.entries(receipt.steps)) {
    for (const [i, entry] of s.runs.entries()) {
      if (entry.index !== i) throw new Error(`step ${step} lists run ${entry.index} where run ${i} belongs`);
      if ('cached' in entry) {
        if (!entry.cached) throw new Error(`step ${step} run ${i} is listed as not cached`);
        continue;
      }
      const proved = await checkProof(receipt, step, entry);
      if (proved && (keys.get(proved.key) ?? proved.node) !== proved.node) throw new Error(`job key ${proved.key} is named on two nodes`);
      if (proved) keys.set(proved.key, proved.node);
    }
  }
  return keys;
}

/**
 * Checks that `receipt` carries the attestation of job key `key`: a job node's quote, under `policy`,
 * binding that key to node `node` of `cluster` taking work from the cluster identity the receipt names.
 */
async function verifyJobAttestation(receipt: PipelineReceipt, cluster: string, key: string, node: number, policy: NodePolicy): Promise<void> {
  const doc = receipt.jobAttestations && hasOwn(receipt.jobAttestations, key) ? receipt.jobAttestations[key] : undefined;
  if (!doc) throw new Error('the receipt carries no attestation for it');
  if (doc.jobKey !== key) throw new Error("its attestation is another key's");
  if (doc.node !== node || doc.cluster !== cluster) {
    throw new Error(`it is attested as node ${doc.node} of cluster ${doc.cluster}, not node ${node} of ${cluster}`);
  }
  if (doc.clusterIdentity !== receipt.cluster) throw new Error('its job node takes work from another cluster identity');
  let nonce: Uint8Array;
  try {
    nonce = hashBytes(doc.nonce);
  } catch {
    throw new Error("its attestation's nonce is not 32 bytes of hex");
  }
  const binding = await jobKeyBinding(
    strictHex(doc.transportKey, 'the transport key', 32),
    fromHex(key, 'the job key', 32),
    doc.release.version,
    doc.cluster,
    doc.node,
    strictHex(doc.clusterIdentity, 'the cluster identity', 32),
  );
  await verifyQuoteBinding(doc, nonce, fromHex(binding, 'binding', 32), policy, 'job key');
}

/**
 * Checks that the node that answered holds a receipt key attested under `policy` ({@link verifyAppCluster})
 * and signed `answer` with it, that the receipt is for `app` and `run`, that every job report it names is
 * signed by its job key ({@link verifyJobReports}), and that each such key is a job node's of the attested
 * cluster and the receipt's cluster identity, under `policy`. A version 1 receipt is refused. Answers the receipt.
 */
export async function verifyPipelineReceipt(
  client: { apps: AttestationRoutes },
  app: string,
  run: string,
  answer: ReceiptAnswer,
  policy: NodePolicy,
): Promise<PipelineReceipt> {
  const { keys, cluster } = await verifyAppCluster(client, app, policy);
  if (!isObject(answer)) throw new Error('the receipt answer is not an object');
  const node = nodeId(answer.node, 'the receipt answer');
  const key = keys.get(node);
  if (!key) throw new Error(`node ${node} is not an attested replica of ${app}`);
  const receipt = await verifyReceiptSignature(answer, key);
  if (receipt.app !== app) throw new Error(`the receipt is for app ${receipt.app}, not ${app}`);
  if (receipt.run !== run) throw new Error(`the receipt is for run ${receipt.run}, not ${run}`);
  for (const [jobKey, jobNode] of await verifyJobReports(receipt)) {
    try {
      await verifyJobAttestation(receipt, cluster, jobKey, jobNode, policy);
    } catch (error) {
      throw new Error(`job key ${jobKey}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  return receipt;
}
