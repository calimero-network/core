// Writes src/generated/methods.ts: one method per operation of core's node
// API document (openapi/core-openapi.json), grouped by the operation's first
// tag (its first path segment when untagged), each taking one flattened,
// typed argument object and answering the typed success payload. Run by
// `pnpm gen:api` after openapi-typescript; every type it names is an index
// into that output's `operations`.
//
//   node scripts/gen-methods.mjs [openapi.json] [out.ts]
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const input = process.argv[2] ?? `${root}openapi/core-openapi.json`;
const output = process.argv[3] ?? `${root}src/generated/methods.ts`;
const doc = JSON.parse(readFileSync(input, 'utf8'));

// Operations whose id names the route rather than what a caller does.
const RENAMES = { intents: 'call', views: 'view' };

const camel = (words) => words.map((w, i) => (i === 0 ? w : w[0].toUpperCase() + w.slice(1))).join('');
/** The operation's group: the document's `x-calimero-group`, else its tag, else its first path segment. */
const groupOf = (op, path) => {
  if (op['x-calimero-group']) return op['x-calimero-group'];
  const tag = op.tags?.[0];
  if (tag) return camel(tag.toLowerCase().split(/[^a-z0-9]+/).filter(Boolean));
  return camel(path.split('/').filter(Boolean)[0].split(/[^a-zA-Z0-9]+/));
};
/** The operationId in camelCase, without a first word that only repeats the group (`org_get` in `orgs`: `get`). */
const nameOf = (group, id) => {
  if (RENAMES[id]) return RENAMES[id];
  let words = id.split('_');
  const repeats = new Set([group, group.replace(/s$/, '')]);
  if (words.length > 1 && repeats.has(words[0])) words = words.slice(1);
  return camel(words);
};

const deref = (schema) => {
  if (schema?.$ref) return deref(doc.components.schemas[schema.$ref.replace('#/components/schemas/', '')]);
  return schema ?? {};
};
/** The property names of a JSON object schema, or null when the body is not a plain object. */
const objectFields = (schema) => {
  const s = deref(schema);
  if (s.type === 'object' && s.properties && !s.anyOf && !s.oneOf) return { names: Object.keys(s.properties), required: s.required ?? [] };
  if (s.allOf) {
    const parts = s.allOf.map(objectFields);
    if (parts.some((p) => p === null)) return null;
    return { names: parts.flatMap((p) => p.names), required: parts.flatMap((p) => p.required) };
  }
  return null;
};

const comment = (text) =>
  text
    .replace(/\*\//g, '*\\/')
    .split('\n')
    .map((line) => `   * ${line}`.trimEnd())
    .join('\n');

const groups = new Map();
const retrySafe = [];
const skipped = [];
for (const [path, item] of Object.entries(doc.paths)) {
  for (const [verb, op] of Object.entries(item)) {
    if (!['get', 'post', 'put', 'delete', 'patch'].includes(verb)) continue;
    const id = op.operationId;
    const ok = op.responses?.['200']?.content ?? {};
    const replyMedia = Object.keys(ok)[0];
    if (replyMedia === 'text/event-stream') {
      skipped.push(`${verb.toUpperCase()} ${path}`);
      continue;
    }
    const group = groupOf(op, path);
    const name = nameOf(group, id);
    const params = op.parameters ?? [];
    const pathParams = params.filter((p) => p.in === 'path').map((p) => p.name);
    const query = params.filter((p) => p.in === 'query');
    const queryNames = query.map((p) => p.name);
    const O = `operations['${id}']`;

    const parts = [];
    if (pathParams.length) parts.push(`${O}['parameters']['path']`);
    // A query parameter named like a path parameter (none in core 92ac297a1's document;
    // `GET /apps/{app}/attestation?app=` was one until it documented only `nonce`)
    // cannot be flattened beside it: the query goes whole, under `query`.
    const queryWhole = queryNames.some((n) => pathParams.includes(n));
    if (query.length) {
      const type = `NonNullable<${O}['parameters']['query']>`;
      const needed = query.some((p) => p.required);
      parts.push(queryWhole ? (needed ? `{ query: ${type} }` : `{ query?: ${type} }`) : type);
    }
    let required = pathParams.length > 0 || query.some((p) => p.required);

    let body = 'none';
    let media;
    const rb = op.requestBody;
    const optionalBody = !!rb && !rb.required;
    if (rb) {
      media = Object.keys(rb.content)[0];
      const schema = rb.content[media].schema;
      const bodyType = optionalBody ? `NonNullable<${O}['requestBody']>['content']['${media}']` : `${O}['requestBody']['content']['${media}']`;
      // Bytes (`format: binary`, or any media type but JSON) go as they are.
      if (media !== 'application/json' || deref(schema).format === 'binary') {
        body = 'raw';
        parts.push(`RawBody`);
        required = true;
      } else {
        const fields = objectFields(schema);
        const taken = new Set([...pathParams, ...(queryWhole ? ['query'] : queryNames)]);
        if (fields && !fields.names.some((n) => taken.has(n) || n === 'body')) {
          body = 'fields';
          parts.push(optionalBody ? `Partial<${bodyType}>` : bodyType);
          if (!optionalBody && fields.required.length) required = true;
        } else {
          body = 'whole';
          parts.push(optionalBody ? `{ body?: ${bodyType} }` : `{ body: ${bodyType} }`);
          if (!optionalBody) required = true;
        }
      }
    }
    const replySchema = deref(ok[replyMedia]?.schema);
    const reply =
      replySchema.format === 'binary' || replyMedia === 'application/octet-stream'
        ? 'bytes'
        : replyMedia === 'text/plain'
          ? 'text'
          : 'json';
    const answer =
      reply === 'bytes' ? 'Blob' : reply === 'text' ? 'string' : `${O}['responses'][200]['content']['application/json']`;
    const args = parts.length ? parts.join(' & ') : 'Record<string, never>';
    const shape = {
      method: verb.toUpperCase(),
      path,
      pathParams,
      query: queryNames,
      ...(queryWhole ? { queryWhole } : {}),
      body,
      optionalBody,
      // A wildcard (`*/*`: any type the caller names) sends bytes as octet-stream by default.
      ...(body === 'raw' ? { media: media.includes('*') ? 'application/octet-stream' : media } : {}),
      reply,
      ...(op['x-calimero-step-up'] ? { stepUp: true } : {}),
      ...(op['x-calimero-user-verification'] ? { userVerification: op['x-calimero-user-verification'] } : {}),
      ...(op['x-calimero-operator'] ? { operator: true } : {}),
    };
    if (op['x-calimero-retry-safe']) {
      retrySafe.push({ method: shape.method, path, ...(op['x-calimero-retry-requires'] ? { requires: op['x-calimero-retry-requires'] } : {}) });
    }
    const extras = [];
    if (op['x-calimero-step-up']) {
      extras.push(
        `Answers \`412 CONFIRM_REQUIRED\` first${op['x-calimero-step-up-when'] ? ` when ${op['x-calimero-step-up-when']}` : ''}: give \`confirm\` (here or on the client); the passkey must verify the user \`${op['x-calimero-user-verification'] ?? 'required'}\`.`,
      );
    }
    if (op['x-calimero-operator']) extras.push('Answers `428 OPERATOR_SIGNATURE_REQUIRED` first: give `operatorProof` (here or on the client).');
    if (op['x-calimero-retry-safe']) {
      extras.push(
        `Retried on \`503\` (D70)${op['x-calimero-retry-requires'] ? `, with its \`${op['x-calimero-retry-requires']}\` (one is made when none is given)` : ''}.`,
      );
    }
    if (op.responses['421']?.description?.includes('WRONG_NODE')) {
      extras.push('A `421 WRONG_NODE` moves the client to the next node of the cluster.');
    }
    const doc_ = [`${op.summary ?? id}.`.replace(/\.\.$/, '.'), '', `\`${shape.method} ${path}\` (\`${id}\`). **Who:** ${op['x-calimero-who']}.`];
    if (extras.length) doc_.push(...extras);
    const list = groups.get(group) ?? [];
    if (list.some((m) => m.name === name)) throw new Error(`two methods named ${group}.${name}`);
    list.push({ name, id, args, answer, required, shape, doc: doc_.join('\n') });
    groups.set(group, list);
  }
}

const out = [];
out.push(`// Generated by scripts/gen-methods.mjs from openapi/core-openapi.json (\`pnpm gen:api\`). Do not edit.
//
// One method per operation of the node's API, grouped by the operation's tag.
// Each takes one argument object (path parameters, query and body fields
// merged; the body under \`body\` when it is raw bytes or its fields would
// collide) and answers the route's success payload; a refusal is thrown as the
// typed \`CalimeroError\`. Event streams are not here: read them with
// \`transport.events\`.${skipped.length ? `\n// Streams left out: ${skipped.join(', ')}.` : ''}

import type { operations } from './api.js';
import type { RouteClient } from '../routes/route-client.js';
import { route, type MethodOptions, type RawBody } from '../routes/method.js';
`);
const groupNames = [...groups.keys()].sort();
out.push('/** The generated methods, by group, sending through `api`. */');
out.push('export function createRoutes(api: RouteClient) {');
out.push('  return {');
for (const group of groupNames) {
  out.push(`    ${group}: {`);
  for (const m of groups.get(group).sort((a, b) => a.name.localeCompare(b.name))) {
    out.push('      /**');
    out.push(comment(m.doc).replace(/^ {3}/gm, '       '));
    out.push('       */');
    const param = m.required ? `args: ${m.args}` : `args: ${m.args} = {}`;
    out.push(
      `      ${m.name}: (${param}, options?: MethodOptions): Promise<${m.answer}> =>`,
      `        route(api, ${JSON.stringify(m.shape)}, args as unknown as Record<string, unknown>, options) as Promise<${m.answer}>,`,
    );
  }
  out.push('    },');
}
out.push('  };');
out.push('}');
out.push('');
out.push('/**');
out.push(" * The routes the document marks `x-calimero-retry-safe`: the transport sends them again on a `503`");
out.push(" * (D70), and one that `requires` an `idempotencyKey` is given one before it is first sent.");
out.push(' */');
out.push(`export const RETRY_SAFE_ROUTES: ReadonlyArray<{ method: string; path: string; requires?: string }> = ${JSON.stringify(retrySafe)};`);
out.push('');
out.push('/** Every generated group. */');
out.push('export type Routes = ReturnType<typeof createRoutes>;');
for (const group of groupNames) {
  out.push(`/** The \`${group}\` methods. */`);
  out.push(`export type ${group[0].toUpperCase() + group.slice(1)}Routes = Routes['${group}'];`);
}
out.push('');
writeFileSync(output, out.join('\n'));
const count = [...groups.values()].reduce((n, l) => n + l.length, 0);
console.log(`${output.replace(root, '')}: ${count} methods in ${groupNames.length} groups (${groupNames.join(', ')})`);
