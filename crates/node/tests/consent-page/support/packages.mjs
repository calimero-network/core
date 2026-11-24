import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { KV_WASM, MEROCTL, runDir } from './env.mjs';
import { ctl } from './ctl.mjs';

const B32 = 'abcdefghijklmnopqrstuvwxyz234567';

/** The DNS label of a package's app origin (the node's `app_host::label`). */
export function label(pkg) {
  const h = createHash('sha256').update(`calimero.app-host.v1\0${pkg}`).digest();
  let bits = 0;
  let acc = 0;
  let out = '';
  for (const b of h) {
    acc = (acc << 8) | b;
    bits += 8;
    while (bits >= 5) {
      out += B32[(acc >>> (bits - 5)) & 31];
      bits -= 5;
    }
    acc &= (1 << bits) - 1;
  }
  return bits ? out + B32[(acc << (5 - bits)) & 31] : out;
}

/** The kv fixture with a custom section `tag`: the same behaviour in another module (another bundle hash). */
export function variant(tag) {
  const out = path.join(runDir(), `kv-${tag}.wasm`);
  const t = Buffer.from(tag);
  fs.writeFileSync(
    out,
    Buffer.concat([fs.readFileSync(KV_WASM), Buffer.from([0, 1 + t.length, t.length]), t]),
  );
  return out;
}

const key = () => {
  const file = path.join(runDir(), 'publisher.key');
  if (!fs.existsSync(file)) execFileSync(MEROCTL, ['app', 'signer', 'new', file], { stdio: 'pipe' });
  return file;
};

/** A signed `.mpk` of `pkg@version` on the `tag` module. `patch` edits the manifest before it is signed again. */
export function pack({ pkg, version, tag, extra = [], patch }) {
  const out = path.join(runDir(), `${pkg}-${version}.mpk`);
  const args = ['app', 'pack', '--wasm', variant(tag), '--package', pkg, '--app-version', version];
  execFileSync(MEROCTL, [...args, '--name', pkg.split('.').pop(), '--key', key(), '--out', out, ...extra], {
    stdio: 'pipe',
  });
  if (patch) {
    const dir = fs.mkdtempSync(path.join(runDir(), 'mpk-'));
    execFileSync('tar', ['xzf', out, '-C', dir]);
    const manifest = path.join(dir, 'manifest.json');
    const m = JSON.parse(fs.readFileSync(manifest, 'utf8'));
    patch(m);
    fs.writeFileSync(manifest, JSON.stringify(m));
    execFileSync(MEROCTL, ['app', 'sign', manifest, '--key', key()], { stdio: 'pipe' });
    execFileSync('tar', ['czf', out, '-C', dir, ...fs.readdirSync(dir)]);
  }
  return out;
}

/** Install a release on the cluster as `account`. */
export function install(account, spec) {
  ctl(account, ['app', 'install', '--path', pack(spec)]);
}
