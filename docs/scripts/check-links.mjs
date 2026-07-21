/**
 * Internal-link checker for the built site. Runs after `astro build` +
 * postbuild-base (see the `check` npm script). Fails on:
 *   - a root-absolute internal link missing the `/core` base (the bug that
 *     previously slipped through), or
 *   - a link that doesn't resolve to a built page.
 */
import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs';
import { join } from 'node:path';

const DIST = 'dist';
const BASE = '/core';

function htmlFiles(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...htmlFiles(p));
    else if (name.endsWith('.html')) out.push(p);
  }
  return out;
}

function resolve(urlPath) {
  let p = urlPath.startsWith(BASE) ? urlPath.slice(BASE.length) : urlPath;
  if (p === '' || p === '/') return join(DIST, 'index.html');
  if (p.endsWith('/')) return join(DIST, p, 'index.html');
  if (p.endsWith('.html')) return join(DIST, p);
  return existsSync(join(DIST, p)) ? join(DIST, p) : join(DIST, p, 'index.html');
}

const skip = (h) =>
  !h ||
  /^(https?:|mailto:|tel:|#|javascript:|data:)/.test(h) ||
  h.startsWith('//') ||
  /\.(css|js|svg|png|jpe?g|webp|ico|woff2?|xml|json|txt|map)$/.test(h.split('#')[0]);

const noBase = [], broken = [];
for (const file of htmlFiles(DIST)) {
  const html = readFileSync(file, 'utf8');
  for (const m of html.matchAll(/href="([^"]+)"/g)) {
    const href = m[1];
    if (skip(href) || !href.startsWith('/')) continue;
    if (!href.startsWith(`${BASE}/`) && href !== BASE) {
      noBase.push([file.replace(`${DIST}/`, ''), href]);
      continue;
    }
    const path = href.split('#')[0].split('?')[0];
    if (!existsSync(resolve(path))) broken.push([file.replace(`${DIST}/`, ''), href]);
  }
}

if (noBase.length || broken.length) {
  if (noBase.length) {
    console.error(`\n✗ ${noBase.length} internal link(s) missing the ${BASE} base:`);
    [...new Set(noBase.map((x) => x[1]))].slice(0, 20).forEach((h) => console.error(`  ${h}`));
  }
  if (broken.length) {
    console.error(`\n✗ ${broken.length} link(s) to a non-existent page:`);
    broken.slice(0, 20).forEach((x) => console.error(`  ${x[0]}  →  ${x[1]}`));
  }
  process.exit(1);
}
console.log('✓ internal links OK (base + targets)');
