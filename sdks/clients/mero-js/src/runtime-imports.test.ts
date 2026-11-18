import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

// The SDK runs in browsers, Node 18+ and React Native, on Web Platform APIs
// alone, with no runtime dependencies. A Node built-in in `src` would build and
// pass every Node test, then break the other two. Tests may use them; nothing
// they import from `src` may.
const SRC = fileURLToPath(new URL('.', import.meta.url));
const NODE_BUILTINS = [
  'assert', 'buffer', 'child_process', 'crypto', 'events', 'fs', 'http', 'https', 'net',
  'os', 'path', 'process', 'stream', 'tls', 'url', 'util', 'worker_threads', 'zlib',
];
const SPECIFIER = /(?:\bfrom\s*|\bimport\s*\(\s*|\brequire\s*\(\s*|^\s*import\s+)['"]([^'"]+)['"]/gm;

function sources(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sources(path);
    return /\.ts$/.test(entry.name) && !/\.(test|spec)\.ts$/.test(entry.name) ? [path] : [];
  });
}

describe('runtime imports', () => {
  it('finds the SDK sources it guards', () => {
    expect(sources(SRC).map((path) => relative(SRC, path))).toEqual(
      expect.arrayContaining(['index.ts', join('sealed', 'release.ts'), join('sealed', 'x509.ts')]),
    );
  });

  it('imports no Node built-in anywhere in src', () => {
    const found = sources(SRC).flatMap((path) =>
      [...readFileSync(path, 'utf8').matchAll(SPECIFIER)]
        .map((match) => match[1])
        .filter((specifier) => specifier.startsWith('node:') || NODE_BUILTINS.includes(specifier.split('/')[0]))
        .map((specifier) => `${relative(SRC, path)}: ${specifier}`),
    );
    expect(found).toEqual([]);
  });

  it('would catch one', () => {
    const sample = "import { readFileSync } from 'node:fs';\nconst c = await import('crypto');\nrequire(\"fs\");";
    expect([...sample.matchAll(SPECIFIER)].map((match) => match[1])).toEqual(['node:fs', 'crypto', 'fs']);
  });
});
