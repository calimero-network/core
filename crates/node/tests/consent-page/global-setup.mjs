import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { build } from 'esbuild';
import { startCluster } from './support/cluster.mjs';
import { HERE, ROOT } from './support/env.mjs';

/** mero-js as one browser script (`Mero`), the way an app frontend carries it. */
async function bundleMeroJs(out) {
  await build({
    entryPoints: [path.join(ROOT, 'sdks/clients/mero-js/src/index.ts')],
    bundle: true,
    platform: 'browser',
    format: 'iife',
    globalName: 'Mero',
    target: 'es2020',
    alias: { '@phala/dcap-qvl': path.join(HERE, 'browser/dcap-stub.js') },
    outfile: out,
    logLevel: 'warning',
  });
}

export default async function globalSetup() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'consent-page-'));
  await bundleMeroJs(path.join(dir, 'mero.iife.js'));
  const cluster = await startCluster(dir);
  process.env.CONSENT_RUN_DIR = dir;
  process.env.CONSENT_PORTS = JSON.stringify(cluster.ports);
  return async () => {
    cluster.stop();
    if (!process.env.CONSENT_KEEP_RUN_DIR) fs.rmSync(dir, { recursive: true, force: true, maxRetries: 3 });
  };
}
