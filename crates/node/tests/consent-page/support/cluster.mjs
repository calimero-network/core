import { spawn } from 'node:child_process';
import fs from 'node:fs';
import { createServer } from 'node:net';
import path from 'node:path';
import { MEROD } from './env.mjs';

const OPS_TOKEN = 'consentops000000000000000000000000';

function freePort() {
  return new Promise((ok, fail) => {
    const s = createServer();
    s.on('error', fail);
    s.listen(0, '127.0.0.1', () => {
      const { port } = s.address();
      s.close(() => ok(port));
    });
  });
}

/** Three `merod run --dev` processes. Their public URL is `localhost`, the passkey's RP id. */
export async function startCluster(dir) {
  const ports = [await freePort(), await freePort(), await freePort()];
  const urls = ports.map((p) => `http://localhost:${p}`);
  const peers = urls.map((u, i) => `${i + 1}=${u}/`).join(',');
  const procs = urls.map((url, i) => {
    const log = fs.openSync(path.join(dir, `node${i + 1}.log`), 'w');
    return spawn(
      MEROD,
      [
        'run',
        ...['--node-id', String(i + 1), '--listen', `127.0.0.1:${ports[i]}`, '--public-url', `${url}/`],
        ...['--data-dir', path.join(dir, `node${i + 1}`), '--peers', peers, '--cluster', 'consent-page'],
        ...['--dev', '--rp-id', 'localhost', '--apps-domain', 'localhost'],
        ...['--origins', 'http://localhost:3000', '--ops-token', OPS_TOKEN],
        ...(i === 0 ? ['--bootstrap'] : []),
      ],
      { stdio: ['ignore', log, log] },
    );
  });
  const deadline = Date.now() + 60_000;
  for (;;) {
    try {
      const h = await (await fetch(`${urls[0]}/health`)).json();
      if (h.systemLeader) break;
    } catch {
      // not up yet
    }
    if (Date.now() > deadline) throw new Error('the cluster did not come up');
    await new Promise((r) => setTimeout(r, 300));
  }
  // The leader registers the nodes and mints the first session key.
  await new Promise((r) => setTimeout(r, 2500));
  return {
    ports,
    stop() {
      for (const p of procs) p.kill('SIGTERM');
    },
  };
}
