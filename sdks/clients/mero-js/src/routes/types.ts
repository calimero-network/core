/**
 * Names for the generated types: a component of core's OpenAPI document, and
 * the success payload of one of its operations. The types themselves are
 * generated (`src/generated/api.ts`).
 */

import type { components, operations } from '../generated/api.js';

/** A shared shape of the node's API (`#/components/schemas/{name}`). */
export type ApiSchema<K extends keyof components['schemas']> = components['schemas'][K];

/** What operation `K` answers on success (its `200` JSON body). */
export type ApiAnswer<K extends keyof operations> = operations[K] extends {
  responses: { 200: { content: { 'application/json': infer T } } };
}
  ? T
  : never;
