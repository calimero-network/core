import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { MEROCTL, ports, runDir } from './env.mjs';
import { bump } from './counter.mjs';

const apiUrl = (node) => `http://localhost:${ports()[node - 1]}`;

/** meroctl as `account` against node `node` (1-based). JSON when the output is JSON. */
export function ctl(account, args, { node = 1, passkey = false, allowFail = false } = {}) {
  const full = ['--home', account.home, '--api', apiUrl(node), '--allow-mock', ...args];
  if (passkey) {
    const pk = JSON.parse(fs.readFileSync(account.file, 'utf8'));
    pk.counter = bump();
    fs.writeFileSync(account.file, JSON.stringify(pk));
    full.push('--dev-passkey', account.file);
  }
  try {
    const out = execFileSync(MEROCTL, full, { stdio: ['ignore', 'pipe', 'pipe'], encoding: 'utf8' });
    if (passkey) syncCounter(account);
    try {
      return JSON.parse(out);
    } catch {
      return out;
    }
  } catch (e) {
    if (passkey) syncCounter(account);
    if (allowFail) return { failed: true, stderr: String(e.stderr || ''), stdout: String(e.stdout || '') };
    throw new Error(`meroctl ${args.join(' ')} failed: ${e.stderr || e.message}`);
  }
}

/** meroctl moves the file's counter itself; the shared counter must not fall behind it. */
function syncCounter(account) {
  const used = Number(JSON.parse(fs.readFileSync(account.file, 'utf8')).counter);
  bump(Math.max(1, used - bump(0) + 1));
}

let accounts = 0;
/** Sign up a new account on node 1: its passkey file and meroctl home live in the run dir. */
export function newAccount(label) {
  const name = `${label}${++accounts}`;
  const account = {
    name,
    file: path.join(runDir(), `${name}.json`),
    home: path.join(runDir(), `home-${name}`),
  };
  account.id = ctl(account, ['login', '--dev-passkey', account.file, '--name', name]).accountId;
  syncCounter(account);
  return account;
}
