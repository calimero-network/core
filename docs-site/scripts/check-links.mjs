/**
 * Zero-dependency internal-link checker for the built Starlight site.
 * Walks dist/, extracts internal <a href> links, and verifies each points at a
 * real built page. Exits non-zero on any broken link. Run after `astro build`
 * (see the `check` npm script); also runs in docs CI.
 */
import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs';
import { join, dirname } from 'node:path';

const DIST = 'dist';
const BASE = '/core'; // keep in sync with astro.config.mjs `base`

function htmlFiles(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...htmlFiles(p));
    else if (name.endsWith('.html')) out.push(p);
  }
  return out;
}

// Map a rendered URL path to the file that should exist on disk.
function resolve(urlPath) {
  let p = urlPath;
  if (BASE && p.startsWith(BASE)) p = p.slice(BASE.length);
  if (p === '' || p === '/') return join(DIST, 'index.html');
  if (p.endsWith('/')) return join(DIST, p, 'index.html');
  if (p.endsWith('.html')) return join(DIST, p);
  // extensionless (asset or pretty URL without trailing slash)
  if (existsSync(join(DIST, p))) return join(DIST, p);
  return join(DIST, p, 'index.html');
}

const skip = (h) =>
  !h ||
  /^(https?:|mailto:|tel:|#|javascript:)/.test(h) ||
  h.startsWith('/_astro/') ||
  h.startsWith(`${BASE}/_astro/`) ||
  h.startsWith('/pagefind/') ||
  h.startsWith(`${BASE}/pagefind/`) ||
  /\.(css|js|svg|png|jpe?g|webp|ico|woff2?|xml|json|txt|map)$/.test(h.split('#')[0]);

const broken = [];
for (const file of htmlFiles(DIST)) {
  const html = readFileSync(file, 'utf8');
  const hrefs = [...html.matchAll(/href="([^"]+)"/g)].map((m) => m[1]);
  for (const href of hrefs) {
    if (skip(href)) continue;
    if (!href.startsWith('/')) continue; // only check root-absolute internal links
    const path = href.split('#')[0].split('?')[0];
    if (!existsSync(resolve(path))) {
      broken.push({ file: file.replace(`${DIST}/`, ''), href });
    }
  }
}

if (broken.length) {
  console.error(`\n✗ ${broken.length} broken internal link(s):\n`);
  for (const b of broken) console.error(`  ${b.file}  →  ${b.href}`);
  process.exit(1);
}
console.log('✓ internal links OK');
