import fs from 'node:fs';
import path from 'node:path';
import { runDir } from './env.mjs';

/**
 * One strictly increasing passkey counter for every authenticator of the run (meroctl's
 * file, the virtual authenticator, the software app passkey): the node refuses a counter
 * that does not move forward, whichever of them signed last.
 */
export function bump(by = 1) {
  const file = path.join(runDir(), 'counter');
  let c = 0;
  try {
    c = Number(fs.readFileSync(file, 'utf8'));
  } catch {
    // first use
  }
  c = Math.max(c + by, Math.floor(Date.now() / 1000));
  fs.writeFileSync(file, String(c));
  return c;
}
