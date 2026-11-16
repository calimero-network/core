import { createHash, randomBytes } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { CalimeroClient, generatePublisherKey, packComputePackage } from '../../src/index.js';
import { KV_STORE_WASM, startCluster, type LocalCluster } from './cluster.js';
import { SoftPasskey } from './soft-passkey.js';

const ORIGIN = 'http://localhost:3000';
const FILE_PART_BYTES = 16 * 1024 * 1024;
// The compute fixture's component and its directory (core's apps/v2-fixtures/compute); the package test needs both.
const COMPUTE_WASM = process.env.COMPUTE_WASM;
const COMPUTE_DIR = process.env.COMPUTE_FIXTURE_DIR;
const sha256 = (b: Uint8Array) => createHash('sha256').update(b).digest('hex');
const SIZES = [300_000, 120_000, 70_000];
// Set by core's CI: a missing compute fixture or job node fails the suite instead of skipping the pipeline test.
const REQUIRE_COMPUTE = process.env.MERO_JS_REQUIRE_COMPUTE === '1';

let cluster: LocalCluster;

async function person(i: number, name: string): Promise<CalimeroClient> {
  const c = new CalimeroClient({ baseUrl: cluster.urls[i], policy: { allowMock: true } });
  await c.signUp(new SoftPasskey('localhost', ORIGIN), name);
  return c;
}

async function kvApp(c: CalimeroClient, name: string): Promise<string> {
  const bundle = (await c.packages.bundlePut({ body: new Uint8Array(readFileSync(KV_STORE_WASM)), contentType: 'application/wasm' })).hash;
  return (await c.apps.create({ name, bundle })).appId;
}

beforeAll(async () => {
  if (REQUIRE_COMPUTE && (!COMPUTE_WASM || !COMPUTE_DIR)) throw new Error('MERO_JS_REQUIRE_COMPUTE is set but COMPUTE_WASM or COMPUTE_FIXTURE_DIR is not');
  cluster = await startCluster(3, { jobNode: Boolean(COMPUTE_WASM) });
});

afterAll(() => cluster?.stop());

type Receipt = { version: number; steps: Record<string, { runs: { outputs?: { id: string; size: number }[]; jobKey?: string }[] }>; jobAttestations?: Record<string, unknown> };

describe('sealed files', () => {
  it('uploads a multi-part file, reads its info and exports it back byte for byte', async () => {
    const alice = await person(0, 'alice-files');
    const app = await kvApp(alice, 'sealed');
    const data = new Uint8Array(randomBytes(FILE_PART_BYTES + 3_000_000));
    const up = await alice.uploadFile(app, data);
    expect(up.size).toBe(data.length);
    expect(up.sha256).toBe(sha256(data));

    const info = await alice.apps.fileInfo({ app, id: up.id });
    expect(info).toMatchObject({ id: up.id, size: data.length, sha256: up.sha256 });

    const back = await alice.exportFile(app, up.id);
    expect(back.length).toBe(data.length);
    expect(sha256(back)).toBe(up.sha256);
    expect(Buffer.compare(back, data)).toBe(0);

    const chunks: Uint8Array[] = [];
    const exported = await alice.exportFileTo(app, up.id, { write: (c) => void chunks.push(c) });
    expect(exported.sha256).toBe(up.sha256);
    expect(sha256(Buffer.concat(chunks))).toBe(up.sha256);
  });

  it('refuses an export to a stranger', async () => {
    const alice = await person(0, 'alice-owner');
    const mallory = await person(1, 'mallory');
    const app = await kvApp(alice, 'private-files');
    const up = await alice.uploadFile(app, new Uint8Array(randomBytes(50_000)));
    await expect(mallory.exportFile(app, up.id)).rejects.toMatchObject({ code: 'NOT_FOUND' });
    await expect(mallory.apps.fileInfo({ app, id: up.id })).rejects.toMatchObject({ code: 'NOT_FOUND' });
  });
});

describe.skipIf(!REQUIRE_COMPUTE && (!COMPUTE_WASM || !COMPUTE_DIR))('a signed compute package', () => {
  it('is built and signed in JS, installed, run as a pipeline and its outputs downloaded', async () => {
    const alice = await person(0, 'alice-pipelines');
    const key = await generatePublisherKey();
    const version = '1.0.0';
    const mpk = await packComputePackage(
      {
        package: 'com.mero-js.pipelines',
        version,
        compute: new Uint8Array(readFileSync(COMPUTE_WASM as string)),
        pipelines: [{ path: 'pipelines/fanout.yaml', source: readFileSync(join(COMPUTE_DIR as string, 'pipelines/fanout.yaml'), 'utf8') }],
      },
      key,
    );
    const installed = await alice.packages.upload({ body: mpk, contentType: 'application/octet-stream' });
    expect(installed).toMatchObject({ package: 'com.mero-js.pipelines', version });

    const app = (await alice.apps.create({ name: 'pipes', package: 'com.mero-js.pipelines', version })).appId;
    const files = SIZES.map((n) => new Uint8Array(randomBytes(n)));
    const ids = [];
    for (const f of files) ids.push((await alice.uploadFile(app, f)).id);

    // Starting a run needs a job node; without one (no MEROCTL to register it) the package is only installed.
    if (!cluster.jobUrl) {
      if (REQUIRE_COMPUTE) throw new Error('MERO_JS_REQUIRE_COMPUTE is set but no job node started (MEROCTL)');
      return;
    }
    const { run } = await alice.transport.json<{ run: string }>('POST', `apps/${app}/pipelines/fanout/runs`, {
      body: { inputs: { docs: ids }, params: {} },
    });
    type View = { state: string; steps: Record<string, { outputs: { id: string }[] }> };
    let view: View;
    const deadline = Date.now() + 90_000;
    for (;;) {
      view = await alice.transport.json<View>('GET', `apps/${app}/pipeline-runs/${run}`);
      if (view.state !== 'running') break;
      if (Date.now() > deadline) throw new Error('the pipeline run did not end');
      await new Promise((r) => setTimeout(r, 500));
    }
    expect(view.state).toBe('succeeded');

    const each = await Promise.all(view.steps.each.outputs.map((o) => alice.exportFile(app, o.id)));
    expect(Buffer.compare(each[0], Buffer.concat([files[0], files[1]]))).toBe(0);
    expect(Buffer.compare(each[1], files[2])).toBe(0);
    const all = await alice.exportFile(app, view.steps.all.outputs[0].id);
    expect(all.length).toBe(SIZES.reduce((a, b) => a + b));

    const answer = await alice.pipelineReceipt(app, run);
    const receipt = (await alice.verifyPipelineReceipt(app, run, answer)) as unknown as Receipt;
    expect(receipt.version).toBe(2);
    for (const [name, step] of Object.entries(receipt.steps)) {
      const outputs = step.runs.flatMap((r) => r.outputs ?? []);
      expect(outputs.map((o) => o.id)).toEqual((view.steps[name]?.outputs ?? []).map((o) => o.id));
      for (const r of step.runs) if (r.jobKey) expect(receipt.jobAttestations).toHaveProperty([r.jobKey]);
    }
    expect(Object.keys(receipt.jobAttestations ?? {}).length).toBeGreaterThan(0);

    const forged = JSON.parse(JSON.stringify(answer)) as { receipt: Receipt };
    const withOutput = Object.values(forged.receipt.steps).flatMap((s) => s.runs).find((r) => r.outputs?.length);
    (withOutput?.outputs as { size: number }[])[0].size += 1;
    await expect(alice.verifyPipelineReceipt(app, run, forged as never)).rejects.toThrow('not signed by that key');
    await expect(alice.verifyPipelineReceipt(app, `${run}-other`, answer)).rejects.toThrow(`is for run ${run}`);
  });
});
