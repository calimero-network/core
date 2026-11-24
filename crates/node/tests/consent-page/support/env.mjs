import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const HERE = path.dirname(fileURLToPath(import.meta.url));
export const SUITE = path.resolve(HERE, '..');
export const ROOT = path.resolve(SUITE, '../../../..');

const bin = (name) => process.env[name.toUpperCase()] || path.join(ROOT, 'target', 'debug', name);
export const MEROD = process.env.MEROD_BINARY || bin('merod');
export const MEROCTL = process.env.MEROCTL || bin('meroctl');
export const KV_WASM = process.env.KV_STORE_WASM || path.join(ROOT, 'target', 'app-components', 'kv.wasm');

/** Set by global-setup.mjs: where this run keeps its files, and the nodes' ports. */
export const runDir = () => process.env.CONSENT_RUN_DIR;
export const ports = () => JSON.parse(process.env.CONSENT_PORTS);
export const SCREENSHOTS = process.env.CONSENT_SCREENSHOTS || path.join(SUITE, 'test-results', 'screens');
