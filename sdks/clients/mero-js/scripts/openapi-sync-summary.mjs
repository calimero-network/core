// Compares two versions of src/generated/methods.ts and prints, as Markdown,
// which generated methods were added, removed, or changed their signature.
// The openapi-sync workflow puts it in the pull request it opens.
//
//   node scripts/openapi-sync-summary.mjs <old methods.ts> <new methods.ts> [<old api.ts> <new api.ts>]
//
// With the two api.ts files it also lists the methods whose operation types
// (path, query, body or answer) changed: the signature text names those types
// by index, so it stays the same while what they index changes. Exit code 0
// always; the summary's first line says whether anything breaks: a removed
// method, or one whose signature changed, needs a major release.
import { readFileSync } from 'node:fs';

/** `group.method` -> its signature line, and the route its doc names. */
function methods(file) {
  const out = new Map();
  let group = null;
  let route = '';
  for (const line of readFileSync(file, 'utf8').split('\n')) {
    const g = line.match(/^ {4}(\w+): \{$/);
    if (g) group = g[1];
    const r = line.match(/`((?:GET|POST|PUT|PATCH|DELETE) [^`]+)` \(`(\w+)`\)/);
    if (r) route = `${r[1]} (${r[2]})`;
    const m = line.match(/^ {6}(\w+): \((.*)\): (Promise<.*>) =>$/);
    if (m && group) out.set(`${group}.${m[1]}`, { signature: `(${m[2]}): ${m[3]}`, route });
  }
  return out;
}

/** operationId -> the text of its `operations[...]` entry in api.ts. */
function operationTypes(file) {
  const out = new Map();
  if (!file) return out;
  const text = readFileSync(file, 'utf8');
  const start = text.indexOf('export interface operations {');
  if (start < 0) return out;
  const lines = text.slice(start).split('\n').slice(1);
  let id = null;
  let body = [];
  for (const line of lines) {
    if (line === '}') break;
    const m = line.match(/^ {4}(\w+): \{$/);
    if (m) {
      if (id) out.set(id, body.join('\n'));
      id = m[1];
      body = [];
    } else if (id) body.push(line.trim());
  }
  if (id) out.set(id, body.join('\n'));
  return out;
}

const [before, after] = [methods(process.argv[2]), methods(process.argv[3])];
const [typesBefore, typesAfter] = [operationTypes(process.argv[4]), operationTypes(process.argv[5])];
const opOf = (entry) => entry.route.match(/\((\w+)\)$/)?.[1];
const added = [...after.keys()].filter((k) => !before.has(k)).sort();
const removed = [...before.keys()].filter((k) => !after.has(k)).sort();
const changed = [...after.keys()].filter((k) => before.has(k) && before.get(k).signature !== after.get(k).signature).sort();
const retyped = [...after.keys()]
  .filter((k) => before.has(k) && !changed.includes(k))
  .filter((k) => {
    const [a, b] = [opOf(before.get(k)), opOf(after.get(k))];
    return typesBefore.has(a) && typesAfter.has(b) && typesBefore.get(a) !== typesAfter.get(b);
  })
  .sort();
const moved = [...after.keys()].filter((k) => before.has(k) && before.get(k).route !== after.get(k).route).sort();

const breaking = removed.length > 0 || changed.length > 0;
const lines = [];
lines.push(
  breaking
    ? '**Breaking:** a generated method was removed or changed its signature, which needs a major release (`feat(api)!:` or a `BREAKING-CHANGE:` footer).'
    : 'No generated method was removed or changed its signature.',
  '',
);
const list = (title, keys, note) => {
  lines.push(`#### ${title} (${keys.length})`, '');
  if (!keys.length) lines.push('None.', '');
  else {
    for (const k of keys) lines.push(`- \`client.${k}\`${note ? note(k) : ''}`);
    lines.push('');
  }
};
list('Added', added, (k) => `: ${after.get(k).route}`);
list('Removed', removed, (k) => `: was ${before.get(k).route}`);
list('Signature changed', changed, (k) => `: ${after.get(k).route}`);
if (typesBefore.size) list('Operation types changed (check: a newly required field breaks callers)', retyped, (k) => `: ${after.get(k).route}`);
if (moved.length) list('Route or operation changed', moved, (k) => `: ${before.get(k).route} → ${after.get(k).route}`);
process.stdout.write(lines.join('\n'));
