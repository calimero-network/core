/** Start a local merod cluster (dev mode: mock quotes) for tests. */

import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

export const BINARY = process.env.MEROD_BINARY ?? resolve(__dirname, '../../../core/target/debug/merod');
/** `merod run FLAGS`; `MEROD_RUN=0` runs the binary with the flags alone (a wrapper script, say). */
export const RUN_SUBCOMMAND = process.env.MEROD_RUN !== '0';
export const KV_STORE_WASM =
  process.env.KV_STORE_WASM ?? resolve(__dirname, '../../../core/target/app-components/kv.wasm');

async function freePort(): Promise<number> {
  return new Promise((ok, fail) => {
    const s = createServer();
    s.listen(0, '127.0.0.1', () => {
      const port = (s.address() as { port: number }).port;
      s.close(() => ok(port));
    });
    s.on('error', fail);
  });
}

/** meroctl, to register a job node (the operator's decision is signed by it); without it no job node starts. */
export const MEROCTL = process.env.MEROCTL;
const OPS_TOKEN = `opsmero${'0'.repeat(26)}`;
const OPERATOR_SEED = '09'.repeat(32); // the node tests' operator key
const OPERATOR_PUBLIC = 'fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618'; // its verify key

export interface LocalCluster {
  urls: string[];
  /** The job node's URL, when `startCluster` was asked for one and `MEROCTL` is set. */
  jobUrl?: string;
  stop(i?: number): void;
}

export async function startCluster(n = 3, options: { jobNode?: boolean } = {}): Promise<LocalCluster> {
  const job = Boolean(options.jobNode && MEROCTL);
  const ports = await Promise.all(Array.from({ length: n }, freePort));
  const urls = ports.map((p) => `http://127.0.0.1:${p}`);
  const peers = urls.map((u, i) => `${i + 1}=${u}/`).join(',');
  const dirs = urls.map(() => mkdtempSync(join(tmpdir(), 'merod-')));
  const procs: Array<ChildProcess | undefined> = urls.map((url, i) =>
    spawn(
      BINARY,
      // `merod run FLAGS`: a node from flags alone, no `init`.
      [
        ...(RUN_SUBCOMMAND ? ['run'] : []),
        '--node-id', String(i + 1),
        '--listen', `127.0.0.1:${ports[i]}`,
        '--public-url', `${url}/`,
        '--data-dir', dirs[i],
        '--peers', peers,
        '--cluster', 'mero-js-test',
        '--dev',
        '--origins', 'http://localhost:3000',
        ...(i === 0 ? ['--bootstrap'] : []),
        ...(job ? ['--ops-token', OPS_TOKEN, '--operator-key', OPERATOR_PUBLIC] : []),
      ],
      { stdio: process.env.MEROD_V2_LOGS ? 'inherit' : 'ignore' },
    ),
  );
  const deadline = Date.now() + 30_000;
  for (;;) {
    try {
      const r = await fetch(`${urls[0]}/health`);
      const h = (await r.json()) as { systemLeader?: number };
      if (h.systemLeader) break;
    } catch {
      /* not up yet */
    }
    if (Date.now() > deadline) throw new Error('cluster did not start');
    await new Promise((r) => setTimeout(r, 200));
  }
  // Let the leader register nodes and mint the first session key.
  await new Promise((r) => setTimeout(r, 1500));
  let jobUrl: string | undefined;
  if (job) {
    const port = await freePort();
    jobUrl = `http://127.0.0.1:${port}`;
    const dir = mkdtempSync(join(tmpdir(), 'merod-job-'));
    dirs.push(dir);
    const ops = await fetch(`${urls[0]}/ops/cluster`, { headers: { authorization: `Bearer ${OPS_TOKEN}` } });
    const { identity } = (await ops.json()) as { identity: string };
    procs.push(
      spawn(
        BINARY,
        [
          ...(RUN_SUBCOMMAND ? ['run'] : []),
          '--job-node', '--node-id', String(n + 1), '--listen', `127.0.0.1:${port}`, '--data-dir', dir,
          '--cluster', 'mero-js-test', '--dev', '--cluster-identity', identity,
        ],
        { stdio: process.env.MEROD_V2_LOGS ? 'inherit' : 'ignore' },
      ),
    );
    const keyFile = join(dir, 'operator.key');
    writeFileSync(keyFile, OPERATOR_SEED);
    execFileSync(
      MEROCTL as string,
      ['--home', join(dir, 'cfg'), '--api', urls[0], '--allow-mock', '--ops-token', OPS_TOKEN, '--operator-key-file', keyFile,
        'ops', 'add-node', String(n + 1), jobUrl, '--role', 'job'],
      { stdio: 'ignore' },
    );
    await new Promise((r) => setTimeout(r, 3000));
  }
  return {
    urls,
    jobUrl,
    stop(i?: number) {
      const idx = i === undefined ? procs.map((_, j) => j) : [i];
      for (const j of idx) {
        procs[j]?.kill('SIGKILL');
        procs[j] = undefined;
      }
      // The whole cluster stopped: its data goes too (a single node stopped
      // for a failover test keeps its directory).
      if (i === undefined) {
        for (const d of dirs) rmSync(d, { recursive: true, force: true, maxRetries: 3 });
      }
    },
  };
}
