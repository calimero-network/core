import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { describe, expect, it, vi } from 'vitest';

import { fromHex, hex } from '../crypto/internal.js';
import { MOCK_QUOTE_HEADER, type NodePolicy } from '../runtime/attestation.js';
import type { Governance } from './approvals.js';
import { privateKey } from './recovery-key.js';
import { governanceCanonical, receiptKeyBinding } from '../recovery/backup-files.js';
import binding from '../recovery/rust-binding.json';
import { CalimeroClient } from './calimero-client.js';
import {
  jobKeyBinding,
  reportMessage,
  verifyAppCluster,
  verifyJobReports,
  verifyPipelineReceipt,
  verifyReceiptSignature,
  receiptMessage,
  verifyReceiptKey,
  type PipelineReceipt,
  type ReceiptAnswer,
  type ReceiptKeyDoc,
  type ReportFields,
} from './receipts.js';

// fixtures/receipt-vectors.json is core's `receipt_vectors` test output (refresh it with
// `pnpm refresh:receipt-vectors`); core's e2e `clients` job fails when this copy differs from its own.
interface Vectors {
  bindings: { receiptKey: string; governance: Governance; governanceCanonical: string; cluster: string; node: number; binding: string }[];
  nonce: string;
  cluster: string;
  governance: Governance;
  nodes: Record<string, string>;
  mockDoc: ReceiptKeyDoc;
  unboundMockDoc: ReceiptKeyDoc;
  receiptAnswer: ReceiptAnswer;
  /** Raw JSON of answers signed over params with floats, and with integers past 2^53. */
  floatAnswerText: string;
  bigNumberAnswerText: string;
  reportMessages: (ReportFields & { messageHex: string; sha256: string })[];
  jobKeyBindings: { transportKey: string; jobKey: string; release: string; cluster: string; node: number; clusterIdentity: string | null; binding: string }[];
  jobReceipt: { clusterIdentity: string; jobNonce: string; answer: ReceiptAnswer };
}
const v = JSON.parse(
  readFileSync(fileURLToPath(new URL('./fixtures/receipt-vectors.json', import.meta.url)), 'utf8'),
) as Vectors;

const MOCK: NodePolicy = { allowMock: true };
const APP = 'app-1';
const RUN = 'run-1';
const nonce = fromHex(v.nonce, 'nonce', 32);
const keyOf = (node: string) => fromHex(v.nodes[node], 'key', 32);

const mockQuote = (n: Uint8Array, suffix: string) => {
  const q = new Uint8Array([...new TextEncoder().encode(MOCK_QUOTE_HEADER), ...n, ...fromHex(suffix, 'suffix', 32)]);
  return btoa(String.fromCharCode(...q));
};

interface Replica {
  node: number;
  /** The node id the document claims; defaults to `node`. */
  docNode?: number;
  /** What the quote binds, defaulting to the right cluster, node and governance. */
  bind?: { cluster?: string; node?: number; governance?: Governance };
  governance?: Governance;
  error?: string;
  /** Use the unbound `receipt-key.v2` form. */
  unbound?: boolean;
}

/** A client whose `apps.attestation` answers with the replicas, quoting over the nonce it was asked with. */
function cluster(replicas: Replica[], entryCluster = v.cluster) {
  return {
    apps: {
      attestation: async ({ nonce: asked }: { app: string; nonce: string }) => ({
        appId: APP,
        cluster: entryCluster,
        replicas: await Promise.all(
          replicas.map(async (r) => {
            if (r.error) return { node: r.node, error: r.error };
            const key = keyOf(String(r.node));
            const governance = r.governance ?? v.governance;
            const bound = r.bind ?? {};
            const suffix = r.unbound
              ? await unboundBinding(key, governance)
              : await receiptKeyBinding(key, bound.governance ?? governance, bound.cluster ?? entryCluster, bound.node ?? r.node);
            return {
              node: r.node,
              attestation: {
                quote: mockQuote(fromHex(asked, 'nonce', 32), suffix),
                transportKey: hex(new Uint8Array(32)),
                appHash: hex(new Uint8Array(32)),
                nonce: asked,
                release: { version: '2.3.109', profile: 'locked-read-only' },
                cluster: bound.cluster ?? entryCluster,
                node: r.docNode ?? r.node,
                mock: true,
                receiptKey: hex(key),
                governance,
              },
            };
          }),
        ),
      }),
    },
  } as unknown as Parameters<typeof verifyAppCluster>[0];
}

async function unboundBinding(key: Uint8Array, governance: Governance): Promise<string> {
  const g = new Uint8Array(await crypto.subtle.digest('SHA-256', governanceCanonical(governance)));
  const all = new Uint8Array([...new TextEncoder().encode('calimero.tee-attest.receipt-key.v2'), ...key, ...g]);
  return hex(new Uint8Array(await crypto.subtle.digest('SHA-256', all)));
}

describe('receiptKeyBinding (core receipt_key_binding)', () => {
  it('matches every vector core produced', async () => {
    for (const b of v.bindings) {
      expect(new TextDecoder().decode(governanceCanonical(b.governance))).toBe(b.governanceCanonical);
      expect(await receiptKeyBinding(fromHex(b.receiptKey, 'key', 32), b.governance, b.cluster, b.node)).toBe(b.binding);
    }
  });

  it('drops defaults and keeps field order whatever the node serialized', () => {
    const noisy = { signUp: 'open', outbound: 'owner', deployers: 'operator', releases: 'owner', deployments: 'open', appUpgrades: 'owner' };
    expect(new TextDecoder().decode(governanceCanonical(noisy as Governance))).toBe(
      '{"releases":"owner","appUpgrades":"owner","outbound":"owner"}',
    );
    expect(() => governanceCanonical({ ...noisy, releases: 'nobody' } as unknown as Governance)).toThrow('releases');
  });
});

describe('verifyReceiptKey over core mock documents', () => {
  it('accepts the document core accepts, and answers its key and governance', async () => {
    const got = await verifyReceiptKey(v.mockDoc, nonce, v.cluster, 1, MOCK);
    expect(hex(got.key)).toBe(v.nodes['1']);
    expect(got.governance).toEqual(v.governance);
  });

  it('refuses the unbound receipt-key.v2 quote', async () => {
    await expect(verifyReceiptKey(v.unboundMockDoc, nonce, v.cluster, 1, MOCK)).rejects.toThrow('does not bind');
  });

  it("refuses another node's or cluster's binding", async () => {
    await expect(verifyReceiptKey(v.mockDoc, nonce, v.cluster, 2, MOCK)).rejects.toThrow('does not bind');
    await expect(verifyReceiptKey(v.mockDoc, nonce, 'other', 1, MOCK)).rejects.toThrow('does not bind');
  });

  it('refuses a mock quote without allowMock', async () => {
    await expect(verifyReceiptKey(v.mockDoc, nonce, v.cluster, 1, {})).rejects.toThrow('mock quote refused');
  });

  it('refuses a document without a receipt key or governance', async () => {
    await expect(verifyReceiptKey({ ...v.mockDoc, receiptKey: undefined }, nonce, v.cluster, 1, MOCK)).rejects.toThrow('no receipt key');
    await expect(verifyReceiptKey({ ...v.mockDoc, governance: undefined }, nonce, v.cluster, 1, MOCK)).rejects.toThrow('no governance');
  });
});

describe('verifyAppCluster', () => {
  it('answers each replica key and the shared governance', async () => {
    const got = await verifyAppCluster(cluster([{ node: 1 }, { node: 2 }]), APP, MOCK);
    expect([...got.keys].map(([n, k]) => [n, hex(k)])).toEqual([
      [1, v.nodes['1']],
      [2, v.nodes['2']],
    ]);
    expect(got.governance).toEqual(v.governance);
  });

  it('refuses a quote bound to another node id than the one named', async () => {
    await expect(verifyAppCluster(cluster([{ node: 1, bind: { node: 2 } }]), APP, MOCK)).rejects.toThrow('replica 1');
  });

  it('refuses a quote bound to another cluster than the entry node names', async () => {
    await expect(verifyAppCluster(cluster([{ node: 1, bind: { cluster: 'other' } }]), APP, MOCK)).rejects.toThrow('replica 1');
  });

  it('refuses a document that answers as another node', async () => {
    await expect(verifyAppCluster(cluster([{ node: 1, docNode: 2 }]), APP, MOCK)).rejects.toThrow('answered as node 2');
  });

  it('refuses replicas that attest other governance', async () => {
    const other = { ...v.governance, releases: 'owner' } as Governance;
    await expect(verifyAppCluster(cluster([{ node: 1 }, { node: 2, governance: other }]), APP, MOCK)).rejects.toThrow(
      'replica 2 attests other governance',
    );
  });

  it('fails on any replica error', async () => {
    await expect(verifyAppCluster(cluster([{ node: 1 }, { node: 2, error: 'unreachable' }]), APP, MOCK)).rejects.toThrow(
      'replica 2 gave no attestation: unreachable',
    );
  });

  it('refuses the unbound quote of a pre-binding node', async () => {
    await expect(verifyAppCluster(cluster([{ node: 1, unbound: true }]), APP, MOCK)).rejects.toThrow('replica 1');
  });

  it('refuses mock quotes without allowMock, and an app with no replica', async () => {
    await expect(verifyAppCluster(cluster([{ node: 1 }]), APP, {})).rejects.toThrow('mock quote refused');
    await expect(verifyAppCluster(cluster([]), APP, MOCK)).rejects.toThrow('no replica of app-1 attested');
  });
});

const jobAnswer = v.jobReceipt.answer;

describe('verifyPipelineReceipt', () => {
  const both = () => cluster([{ node: 1 }, { node: 2 }]);

  it('accepts the answer core signed under its node key and answers the receipt', async () => {
    const receipt = await verifyPipelineReceipt(both(), APP, RUN, jobAnswer, MOCK);
    expect(receipt).toEqual(JSON.parse(JSON.stringify(jobAnswer.receipt)));
  });

  it('refuses the version 1 receipt core also signs, though its signature holds', async () => {
    await refuses(verifyPipelineReceipt(both(), APP, RUN, v.receiptAnswer, MOCK), 'the receipt is version 1');
    expect((await verifyReceiptSignature(v.receiptAnswer, keyOf('1'))).version).toBe(1);
  });

  it('refuses a receipt signed by a node that is not the one answering', async () => {
    await expect(verifyPipelineReceipt(both(), APP, RUN, { ...jobAnswer, node: 2 }, MOCK)).rejects.toThrow('not signed by that key');
  });

  it('refuses a node that is not an attested replica', async () => {
    await expect(verifyPipelineReceipt(cluster([{ node: 2 }]), APP, RUN, jobAnswer, MOCK)).rejects.toThrow('not an attested replica');
  });

  it('refuses a receipt with one byte changed', async () => {
    const receipt = { ...(jobAnswer.receipt as object), state: 'succeedeD' };
    await expect(verifyPipelineReceipt(both(), APP, RUN, { ...jobAnswer, receipt }, MOCK)).rejects.toThrow('not signed by that key');
  });

  it('refuses a signature that is not 64 bytes of hex', async () => {
    await expect(verifyPipelineReceipt(both(), APP, RUN, { ...jobAnswer, signature: 'abcd' }, MOCK)).rejects.toThrow('signature');
  });

  it('accepts params holding floats as core canonicalizes them', async () => {
    const answer = JSON.parse(v.floatAnswerText) as ReceiptAnswer;
    const receipt = await verifyReceiptSignature(answer, keyOf('1'));
    expect(receipt.params).toMatchObject({ a: 0.1, b: 1e21, d: 5e-324, e: 1.5 });
  });

  it('accepts integers past 2^53 as core canonicalizes them (every number as f64)', async () => {
    const answer = JSON.parse(v.bigNumberAnswerText) as ReceiptAnswer;
    const receipt = await verifyReceiptSignature(answer, keyOf('1'));
    expect(receipt.params).toMatchObject({ a: 9007199254740992, b: 5e18, c: -9223372036854775808, d: 1e19, e: 1e19 });
  });

  it('refuses a receipt for another app', async () => {
    const attestedFor = cluster([{ node: 1 }]);
    await expect(verifyPipelineReceipt(attestedFor, 'app-2', RUN, jobAnswer, MOCK)).rejects.toThrow('is for app app-1, not app-2');
  });

  it('refuses an unattested cluster before looking at the receipt', async () => {
    await expect(verifyPipelineReceipt(cluster([{ node: 1, unbound: true }]), APP, RUN, jobAnswer, MOCK)).rejects.toThrow('replica 1');
  });
});

describe('governanceCanonical against the code it replaced', () => {
  // The old binding hashed `JSON.stringify(governance)` of the node's own JSON.
  const shapes: Governance[] = [binding.governance as Governance, v.governance, ...v.bindings.map((b) => b.governance)];

  it('is byte-identical for every governance the Rust fixtures hold', () => {
    for (const g of shapes) {
      expect(governanceCanonical(g)).toEqual(new TextEncoder().encode(JSON.stringify(g)));
    }
  });
});

describe('the client verifies under its own policy', () => {
  it('fails, never passes, when the client was built with no attestation policy', async () => {
    const client = new CalimeroClient({ baseUrl: 'http://node.test', policy: undefined as unknown as NodePolicy });
    await expect(client.verifyAppCluster(APP)).rejects.toThrow('no attestation policy');
    await expect(client.verifyPipelineReceipt(APP, RUN, jobAnswer)).rejects.toThrow('no attestation policy');
  });

  it('uses the policy the client was built with: a mock quote needs allowMock there', async () => {
    const refused = new CalimeroClient({ baseUrl: 'http://node.test', policy: {} });
    // Routes answer through the sealed channel; stand in the attestation route directly.
    Object.assign(refused, { apps: cluster([{ node: 1 }]).apps });
    await expect(refused.verifyAppCluster(APP)).rejects.toThrow('mock quote refused');
    const allowed = new CalimeroClient({ baseUrl: 'http://node.test', policy: MOCK });
    Object.assign(allowed, { apps: cluster([{ node: 1 }]).apps });
    expect([...(await allowed.verifyAppCluster(APP)).keys.keys()]).toEqual([1]);
  });
});

/** An answer for `receipt`, signed by node 1's key (the Rust program's seed), as the node would. */
async function signed(receipt: unknown, node: unknown = 1): Promise<ReceiptAnswer> {
  const key = await privateKey(new Uint8Array(32).fill(11), false);
  const sig = new Uint8Array(await crypto.subtle.sign('Ed25519', key, receiptMessage(receipt)));
  return { receipt, node, signature: hex(sig) };
}
const goodReceipt = () => JSON.parse(JSON.stringify(jobAnswer.receipt)) as Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any

/** Rejects with a plain `Error` carrying `message`, never a `TypeError` from poking at bad input. */
async function refuses(p: Promise<unknown>, message: string | RegExp) {
  const error = await p.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(Error);
  expect(error).not.toBeInstanceOf(TypeError);
  expect((error as Error).message).toMatch(message);
}
const check = (answer: unknown, app = APP, run = RUN) =>
  verifyPipelineReceipt(cluster([{ node: 1 }, { node: 2 }]), app, run, answer as ReceiptAnswer, MOCK);

describe('the receipt is for this run', () => {
  it('refuses a receipt for another run of the same app', async () => {
    await refuses(check(jobAnswer, APP, 'run-2'), 'is for run run-1, not run-2');
  });

  it('answers the cluster every quote bound', async () => {
    expect((await verifyAppCluster(cluster([{ node: 1 }]), APP, MOCK)).cluster).toBe(v.cluster);
  });
});

describe('the receipt keeps to core schema', () => {
  it('accepts a signed receipt with an absent or null optional field', async () => {
    const r = goodReceipt();
    r.costSoFar = null;
    expect((await check(await signed(r))).run).toBe(RUN);
  });

  const mutations: [string, (r: Record<string, any>) => void, RegExp][] = [ // eslint-disable-line @typescript-eslint/no-explicit-any
    ['an unknown field', (r) => (r.extra = 1), /receipt\.extra is not a known field/],
    ['a missing field', (r) => delete r.pipeline, /receipt\.pipeline is missing/],
    ['a wrong type', (r) => (r.state = 5), /receipt\.state is not a string/],
    ['a negative size', (r) => (r.inputs.data[0].size = -1), /inputs\.data\[0\]\.size is not an unsigned integer/],
    ['an unknown nested field', (r) => (r.steps.load.extra = 1), /steps\.load\.extra is not a known field/],
    ['a wrong nested type', (r) => (r.steps.load.runs[0].index = '0'), /steps\.load\.runs\[0\]/],
    ['a run that is neither cached nor a proof', (r) => (r.steps.load.runs[1] = { index: 1 }), /steps\.load\.runs\[1\]/],
    ['params that are not an object', (r) => (r.params = []), /receipt\.params is not an object/],
  ];
  for (const [name, mutate, message] of mutations) {
    it(`refuses ${name}, even under a valid signature`, async () => {
      const r = goodReceipt();
      mutate(r);
      await refuses(check(await signed(r)), message);
    });
  }
});

describe('node ids', () => {
  for (const node of ['1', true, [1], -1, 2 ** 53, 1.5, 1n, null, undefined]) {
    it(`refuses ${String(typeof node)} ${String(node)} as the answering node`, async () => {
      await refuses(check({ ...jobAnswer, node }), 'not a node id');
    });
  }

  it('refuses a replica or document whose node is not a node id', async () => {
    const odd = (patch: (a: any) => void) => { // eslint-disable-line @typescript-eslint/no-explicit-any
      const c = cluster([{ node: 1 }]);
      const ask = c.apps.attestation.bind(c.apps);
      c.apps.attestation = async (args) => {
        const a = (await ask(args)) as any; // eslint-disable-line @typescript-eslint/no-explicit-any
        patch(a);
        return a;
      };
      return verifyAppCluster(c, APP, MOCK);
    };
    await refuses(odd((a) => (a.replicas[0].node = '1')), 'not a node id');
    await refuses(odd((a) => (a.replicas[0].attestation.node = '1')), 'not a node id');
    await refuses(odd((a) => (a.replicas[0].node = 2 ** 53)), 'not a node id');
  });
});

describe('hex is read as core reads it', () => {
  it('accepts an upper-case signature', async () => {
    const answer = { ...jobAnswer, signature: (jobAnswer.signature as string).toUpperCase() };
    expect((await check(answer)).run).toBe(RUN);
  });

  it('refuses a padded signature, which fromHex would trim', async () => {
    await refuses(check({ ...jobAnswer, signature: ` ${jobAnswer.signature as string}\n` }), 'signature');
  });

  it('refuses a padded receipt key', async () => {
    const c = cluster([{ node: 1 }]);
    const ask = c.apps.attestation.bind(c.apps);
    c.apps.attestation = async (args) => {
      const a = (await ask(args)) as any; // eslint-disable-line @typescript-eslint/no-explicit-any
      a.replicas[0].attestation.receiptKey = ` ${a.replicas[0].attestation.receiptKey}`;
      return a;
    };
    await refuses(verifyAppCluster(c, APP, MOCK), 'receipt key');
  });
});

describe('what is returned is what was signed', () => {
  it('returns the receipt parsed from the verified bytes, whatever getters the input had', async () => {
    const r = goodReceipt();
    let reads = 0;
    const real = r.state;
    Object.defineProperty(r, 'state', { enumerable: true, get: () => (reads++ === 0 ? real : 'other') });
    const receipt = await check({ ...(await signed(goodReceipt())), receipt: r });
    expect(receipt.state).toBe(real);
  });

  it('returns the governance from its canonical fields only', async () => {
    const c = cluster([{ node: 1 }]);
    const ask = c.apps.attestation.bind(c.apps);
    c.apps.attestation = async (args) => {
      const a = (await ask(args)) as any; // eslint-disable-line @typescript-eslint/no-explicit-any
      a.replicas[0].attestation.governance = { ...a.replicas[0].attestation.governance, evil: 1 };
      return a;
    };
    expect((await verifyAppCluster(c, APP, MOCK)).governance).toEqual(v.governance);
  });
});

describe('malformed input is an Error', () => {
  for (const [name, answer] of [
    ['a null answer', null],
    ['a non-string signature', { ...jobAnswer, signature: 5 }],
    ['an undefined receipt', { ...jobAnswer, receipt: undefined }],
    ['a receipt that is a string', { ...jobAnswer, receipt: 'x' }],
  ] as [string, unknown][]) {
    it(`rejects ${name}`, async () => {
      await refuses(check(answer), /./);
    });
  }

  it('rejects replicas that are not an array, and a replica with neither an attestation nor an error', async () => {
    const answer = (replicas: unknown) => ({ apps: { attestation: async () => ({ appId: APP, cluster: v.cluster, replicas }) } }) as never;
    await refuses(verifyAppCluster(answer('x'), APP, MOCK), 'malformed attestation');
    await refuses(verifyAppCluster(answer([{ node: 1 }]), APP, MOCK), 'neither an attestation nor an error');
    await refuses(verifyAppCluster(answer([null]), APP, MOCK), 'not an object');
  });

});

describe("each job's report (core receipt_vectors)", () => {
  it('rebuilds every report message core signs, and its hash', async () => {
    for (const m of v.reportMessages) {
      const message = reportMessage(m);
      expect(hex(message)).toBe(m.messageHex);
      expect(hex(new Uint8Array(await crypto.subtle.digest('SHA-256', message)))).toBe(m.sha256);
    }
  });

  it('rebuilds every job key binding core computes', async () => {
    for (const b of v.jobKeyBindings) {
      const identity = b.clusterIdentity === null ? undefined : fromHex(b.clusterIdentity, 'identity', 32);
      const got = await jobKeyBinding(fromHex(b.transportKey, 'key', 32), fromHex(b.jobKey, 'key', 32), b.release, b.cluster, b.node, identity);
      expect(got).toBe(b.binding);
    }
  });

  it('refuses a number or a hash it cannot write', () => {
    const [good] = v.reportMessages;
    expect(() => reportMessage({ ...good, usage: ['18446744073709551616', '0', '0'] })).toThrow('not a decimal integer');
    expect(() => reportMessage({ ...good, usage: ['+1', '0', '0'] })).toThrow('not a decimal integer');
    expect(() => reportMessage({ ...good, usage: ['1', '0', ''] })).toThrow('not a decimal integer');
    expect(() => reportMessage({ ...good, assignment: good.assignment.toUpperCase() })).toThrow('lowercase hex');
    expect(() => reportMessage({ ...good, outputs: [{ ...good.outputs[0], size: 2 ** 64 }] })).toThrow('past 2^64-1');
  });
});

type Doc = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any
const JOB_KEY = hex(fromHex(v.jobKeyBindings[0].jobKey, 'job key', 32));
const loadRun = (r: Doc): Doc => r.steps.load.runs[0];
const mergeRun = (r: Doc): Doc => r.steps.merge.runs[0];

/** Refuses the receipt `r`, signed again by node 1, as run `run`. */
async function refusedAs(r: Doc, message: string | RegExp, run = RUN, policy: NodePolicy = MOCK, replicas = [{ node: 1 }, { node: 2 }]) {
  await refuses(verifyPipelineReceipt(cluster(replicas), APP, run, await signed(r), policy), message);
}

/** Re-signs the report of `run` with the job key (seed 0x41) over the fields it now has, as the job node would. */
async function rereport(r: Doc, run: Doc, reason: string | null, seed = 0x41) {
  const message = reportMessage({
    assignment: run.assignmentDigest,
    app: r.app,
    run: run.run,
    attempt: run.attempt,
    reason,
    message: run.job.message ?? '',
    resultSha256: run.job.resultSha256,
    outputs: run.outputs.map((o: Doc, i: number) => ({ ...o, segments: run.job.segments[i] })),
    usage: [run.usage.cpuMicros, run.usage.gibMicros, run.usage.fuel],
    log: run.job.log,
  });
  const key = await privateKey(new Uint8Array(32).fill(seed), false);
  run.reportSha256 = hex(new Uint8Array(await crypto.subtle.digest('SHA-256', message)));
  run.signature = hex(new Uint8Array(await crypto.subtle.sign('Ed25519', key, message)));
}

describe('a job-signed receipt', () => {
  it('verifies and names the one job key and its node', async () => {
    const keys = await verifyJobReports((await verifyReceiptSignature(jobAnswer, keyOf('1'))) as PipelineReceipt);
    expect([...keys]).toEqual([[JOB_KEY, 4]]);
  });

  it('verifies with a done report with no log, and a failure that carries its message', async () => {
    const r = goodReceipt();
    loadRun(r).job.log = null;
    await rereport(r, loadRun(r), null);
    expect((await check(await signed(r))).run).toBe(RUN);
    Object.assign(loadRun(r), { outcome: 'failed:app-error', outputs: [] });
    Object.assign(loadRun(r).job, { message: 'invalid input: row 7', segments: [] });
    await rereport(r, loadRun(r), 'app-error');
    expect((await check(await signed(r))).run).toBe(RUN);
  });

  it('verifies a cancel the job reported as failed:cancelled and as cancelled', async () => {
    for (const outcome of ['failed:cancelled', 'cancelled']) {
      const r = goodReceipt();
      Object.assign(loadRun(r), { outcome, outputs: [] });
      Object.assign(loadRun(r).job, { segments: [], log: null });
      await rereport(r, loadRun(r), 'cancelled');
      expect((await check(await signed(r))).run).toBe(RUN);
    }
  });

  it('refuses a report that says it was not cancelled when the job reported a cancel', async () => {
    const r = goodReceipt();
    Object.assign(loadRun(r), { outcome: 'cancelled', outputs: [] });
    Object.assign(loadRun(r).job, { segments: [], log: null });
    await rereport(r, loadRun(r), null);
    await refusedAs(r, 'the report hash is not that of what the proof says');
  });
});

describe('anything the job did not sign is refused', () => {
  const edits: [string, (r: Doc) => void][] = [
    ['signature', (r) => (loadRun(r).signature = '06'.repeat(64))],
    ['job key', (r) => (loadRun(r).jobKey = '08'.repeat(32))],
    ['assignment', (r) => (loadRun(r).assignmentDigest = '45'.repeat(32))],
    ['attempt', (r) => (loadRun(r).attempt = 3)],
    ['outcome', (r) => (loadRun(r).outcome = 'failed:trap')],
    ['output id', (r) => (loadRun(r).outputs[0].id = '11'.repeat(32))],
    ['output size', (r) => (loadRun(r).outputs[0].size += 1)],
    ['output hash', (r) => (loadRun(r).outputs[1].sha256 = '99'.repeat(32))],
    ['output order', (r) => loadRun(r).outputs.reverse()],
    [
      'dropped output',
      (r) => {
        loadRun(r).outputs.pop();
        loadRun(r).job.segments.pop();
      },
    ],
    ['segment count', (r) => loadRun(r).job.segments.pop()],
    ['an extra segment list', (r) => loadRun(r).job.segments.push([])],
    ['segment', (r) => (loadRun(r).job.segments[0][0] = '77'.repeat(32))],
    ['result', (r) => (loadRun(r).job.resultSha256 = '34'.repeat(32))],
    ['message', (r) => (loadRun(r).job.message = 'bang')],
    ['log', (r) => (loadRun(r).job.log = null)],
    ['log id', (r) => (loadRun(r).job.log.id = '12'.repeat(32))],
    ['usage', (r) => (loadRun(r).usage.fuel = String(BigInt(loadRun(r).usage.fuel) + 1n))],
    ['report hash', (r) => (loadRun(r).reportSha256 = '00'.repeat(32))],
  ];
  for (const [what, edit] of edits) {
    it(`the ${what}`, async () => {
      const r = goodReceipt();
      edit(r);
      await refusedAs(r, /run-1\.load\.0|job evidence|segments/);
    });
  }

  it('the job evidence', async () => {
    const r = goodReceipt();
    delete loadRun(r).job;
    await refusedAs(r, 'has no job evidence');
  });

  it('a hash spelled in upper case', async () => {
    const r = goodReceipt();
    loadRun(r).assignmentDigest = loadRun(r).assignmentDigest.toUpperCase();
    await refusedAs(r, 'lowercase hex');
  });

  it('a job key or result hash spelled in upper case', async () => {
    const upperKey = goodReceipt();
    loadRun(upperKey).jobKey = loadRun(upperKey).jobKey.toUpperCase();
    await refusedAs(upperKey, 'lowercase hex');
    const upperResult = goodReceipt();
    loadRun(upperResult).job.resultSha256 = loadRun(upperResult).job.resultSha256.toUpperCase();
    await refusedAs(upperResult, 'lowercase hex');
  });

  // With a lax Ed25519 (some browsers) only verifyStrict's own checks refuse these, so they are tested alone.
  const SMALL_ORDER = [
    `01${'00'.repeat(31)}`, // identity
    `ec${'ff'.repeat(30)}7f`, // order 2
    '00'.repeat(32), // order 4
    `${'00'.repeat(31)}80`,
    'c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a', // order 8
    'c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa',
    '26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05',
    '26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc85',
  ];
  const ORDER = 2n ** 252n + 27742317777372353535851937790883648493n;

  it('a small-order R or key, or a malleated S, even where Ed25519 itself would accept it', async () => {
    const lax = vi.spyOn(crypto.subtle, 'verify').mockResolvedValue(true);
    try {
      for (const point of SMALL_ORDER) {
        const smallR = goodReceipt();
        loadRun(smallR).signature = point + loadRun(smallR).signature.slice(64);
        await expect(verifyJobReports(smallR as PipelineReceipt), `R ${point}`).rejects.toThrow('the job key did not sign');
        const smallKey = goodReceipt();
        loadRun(smallKey).jobKey = point;
        await expect(verifyJobReports(smallKey as PipelineReceipt), `key ${point}`).rejects.toThrow('the job key did not sign');
      }
      const malleated = goodReceipt();
      const sig = loadRun(malleated).signature as string;
      const s = BigInt(`0x${sig.slice(64).match(/../g)!.reverse().join('')}`) + ORDER;
      loadRun(malleated).signature = sig.slice(0, 64) + s.toString(16).padStart(64, '0').match(/../g)!.reverse().join('');
      await expect(verifyJobReports(malleated as PipelineReceipt)).rejects.toThrow('the job key did not sign');
      expect([...(await verifyJobReports(goodReceipt() as PipelineReceipt))]).toEqual([[JOB_KEY, 4]]);
    } finally {
      lax.mockRestore();
    }
  });

  it('a signature by a small-order key, which verifies for any message in a lax check', async () => {
    const r = goodReceipt();
    const identity = `01${'00'.repeat(31)}`;
    loadRun(r).jobKey = identity;
    await rereport(r, loadRun(r), null);
    loadRun(r).signature = identity + '00'.repeat(32);
    r.jobAttestations[identity] = { ...r.jobAttestations[JOB_KEY], jobKey: identity };
    delete r.jobAttestations[JOB_KEY];
    await refusedAs(r, 'the job key did not sign');
  });
});

describe("a proof is bound to its run, step and index, and each step's runs are listed once in order", () => {
  it('refuses another pipeline run, step or index', async () => {
    await refusedAs(goodReceipt(), 'is for run run-1, not run-2', 'run-2');
    const other = goodReceipt();
    other.run = 'run-2';
    await refusedAs(other, 'run-1.load.0 is listed as run-2.load.0', 'run-2');
    const step = goodReceipt();
    step.steps.other = step.steps.load;
    delete step.steps.load;
    await refusedAs(step, 'run-1.load.0 is listed as run-1.other.0');
    const index = goodReceipt();
    loadRun(index).index = 1;
    await refusedAs(index, 'lists run 1 where run 0 belongs');
  });

  it('refuses a run left out, listed twice, or cached while saying it was not', async () => {
    const gap = goodReceipt();
    gap.steps.load.runs[1].index = 2;
    await refusedAs(gap, 'lists run 2 where run 1 belongs');
    const twice = goodReceipt();
    twice.steps.load.runs[1].index = 0;
    await refusedAs(twice, 'lists run 0 where run 1 belongs');
    const notCached = goodReceipt();
    notCached.steps.load.runs[1].cached = false;
    await refusedAs(notCached, 'run 1 is listed as not cached');
  });

  it('refuses one job key named on two nodes', async () => {
    const r = goodReceipt();
    const second = { ...JSON.parse(JSON.stringify(loadRun(r))), run: 'run-1.load.1', index: 1, node: 5 };
    r.steps.load.runs[1] = second;
    await rereport(r, second, null);
    await refusedAs(r, `job key ${JOB_KEY} is named on two nodes`);
    second.node = 4;
    expect((await check(await signed(r))).run).toBe(RUN);
  });

  it('refuses a version 1 receipt', async () => {
    const r = goodReceipt();
    r.version = 1;
    await refusedAs(r, 'the receipt is version 1');
  });
});

describe('a run the leader ended names no report, outputs or usage', () => {
  it('accepts the leader-ended run core lists, and every reason only the leader gives', async () => {
    for (const outcome of ['failed:lost', 'failed:refused', 'failed:timeout', 'failed:quota', 'failed:not-granted', 'failed:too-large', 'cancelled']) {
      const r = goodReceipt();
      mergeRun(r).outcome = outcome;
      expect((await check(await signed(r))).run).toBe(RUN);
    }
  });

  const edits: [string, (r: Doc) => void, string | RegExp][] = [
    ['outputs', (r) => mergeRun(r).outputs.push({ id: '10'.repeat(32), size: 1, sha256: '11'.repeat(32) }), 'by the leader'],
    ['usage', (r) => (mergeRun(r).usage.fuel = '5'), 'by the leader'],
    ['usage spelled 00', (r) => (mergeRun(r).usage.cpuMicros = '00'), 'by the leader'],
    ['a report hash', (r) => (mergeRun(r).reportSha256 = '05'.repeat(32)), 'by the leader'],
    ['a signature', (r) => (mergeRun(r).signature = '06'.repeat(64)), 'by the leader'],
    ['job evidence', (r) => (mergeRun(r).job = { ...loadRun(r).job, segments: [] }), 'by the leader'],
    ['done without a report', (r) => (mergeRun(r).outcome = 'done'), 'has no assignment digest'],
    ['a job failure without its report', (r) => (mergeRun(r).outcome = 'failed:fuel'), 'has no assignment digest'],
    ['a reason no run ends with', (r) => (mergeRun(r).outcome = 'failed:nonsense'), 'is not one a run ends with'],
    ['an unknown outcome', (r) => (mergeRun(r).outcome = 'exploded'), 'is not one a run ends with'],
  ];
  for (const [what, edit, message] of edits) {
    it(`refuses ${what}`, async () => {
      const r = goodReceipt();
      edit(r);
      await refusedAs(r, message);
    });
  }
});

describe('each job key carries its job node attestation', () => {
  const edits: [string, (r: Doc) => void, string | RegExp][] = [
    ['missing', (r) => delete r.jobAttestations, 'carries no attestation for it'],
    ['for another key', (r) => (r.jobAttestations[JOB_KEY].jobKey = '08'.repeat(32)), "attestation is another key's"],
    ['for no key', (r) => delete r.jobAttestations[JOB_KEY].jobKey, "attestation is another key's"],
    ['for another node', (r) => (r.jobAttestations[JOB_KEY].node = 5), 'not node 4 of eu-shared-1'],
    ['for another cluster', (r) => (r.jobAttestations[JOB_KEY].cluster = 'eu-other'), 'not node 4 of eu-shared-1'],
    ['for another cluster identity', (r) => (r.jobAttestations[JOB_KEY].clusterIdentity = '22'.repeat(32)), 'another cluster identity'],
    ['without a cluster identity', (r) => delete r.jobAttestations[JOB_KEY].clusterIdentity, 'another cluster identity'],
    ['under a receipt of another cluster identity', (r) => (r.cluster = '22'.repeat(32)), 'another cluster identity'],
    ['with a nonce that is not 32 bytes of hex', (r) => (r.jobAttestations[JOB_KEY].nonce = 'zz'), 'nonce is not 32 bytes of hex'],
  ];
  for (const [what, edit, message] of edits) {
    it(`refuses an attestation ${what}, naming the job key`, async () => {
      const r = goodReceipt();
      edit(r);
      await refusedAs(r, new RegExp(`^job key ${JOB_KEY}: .*${String(message)}`));
    });
  }

  it('refuses a quote that binds another job key, node or cluster identity', async () => {
    const doc = (r: Doc) => r.jobAttestations[JOB_KEY];
    const rebind = async (r: Doc, over: { key?: string; node?: number; identity?: string }) => {
      const d = doc(r);
      const suffix = await jobKeyBinding(
        fromHex(d.transportKey, 'key', 32),
        fromHex(over.key ?? d.jobKey, 'key', 32),
        d.release.version,
        d.cluster,
        over.node ?? d.node,
        fromHex(over.identity ?? d.clusterIdentity, 'identity', 32),
      );
      d.quote = mockQuote(fromHex(d.nonce, 'nonce', 32), suffix);
    };
    for (const over of [{ key: '08'.repeat(32) }, { node: 5 }, { identity: '22'.repeat(32) }]) {
      const r = goodReceipt();
      await rebind(r, over);
      await refusedAs(r, /^job key .*does not bind/);
    }
    const ok = goodReceipt();
    await rebind(ok, {});
    expect((await check(await signed(ok))).run).toBe(RUN);
  });

  it('refuses a mock job quote under a policy that stops allowing mock after the replicas', async () => {
    let reads = 0;
    const policy: NodePolicy = { get allowMock() { return reads++ === 0; } };
    await refusedAs(goodReceipt(), /^job key .*mock quote refused/, RUN, policy, [{ node: 1 }]);
    expect(reads).toBe(2);
  });

  it('refuses every mock quote under a policy without allowMock', async () => {
    await refusedAs(goodReceipt(), 'mock quote refused', RUN, {});
  });

  it('refuses a doc missing a required field, allows an unknown one, refuses attestations that are not a map', async () => {
    const r = goodReceipt();
    delete r.jobAttestations[JOB_KEY].transportKey;
    await refusedAs(r, /jobAttestations\.[0-9a-f]+\.transportKey is missing/);
    const extra = goodReceipt();
    extra.jobAttestations[JOB_KEY].future = 1;
    expect((await check(await signed(extra))).run).toBe(RUN);
    const bad = goodReceipt();
    bad.jobAttestations = null;
    await refusedAs(bad, /jobAttestations is not an object/);
  });
});

describe('the answer and the attestation keep to core schema', () => {
  it('refuses a field the answer does not have', async () => {
    await refuses(verifyReceiptSignature({ ...jobAnswer, extra: 1 } as ReceiptAnswer, keyOf('1')), 'extra is not a known field');
  });

  it('refuses a u64 of 2^64, which core does not read', async () => {
    const r = goodReceipt();
    r.startTsMs = 2 ** 64;
    await refusedAs(r, 'unsigned integer in range');
  });

  it('refuses an attestation whose governance or kms is not an object', async () => {
    for (const field of ['governance', 'kms']) {
      const r = goodReceipt();
      r.jobAttestations[JOB_KEY][field] = 5;
      await refusedAs(r, `${field} is not an object`);
    }
  });
});
