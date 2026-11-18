// The route layer: core's node API as one method per operation.
//
// Layering, bottom up:
//   runtime/    the transport (attestation, the sealed channel, the session,
//               retries, errors, server-sent events): everything that is not a
//               route; also `@calimero-network/mero-js/runtime`.
//   generated/  what `pnpm gen:api` writes from core's OpenAPI document: the
//               types (`api.ts`) and one method per operation (`methods.ts`).
//   routes/     (here) what the generated methods run on: a typed
//               `openapi-fetch` client whose fetch is the transport
//               (`route-client.ts`), the step-up and operator-signature answers
//               (`step-up.ts`), and the runner that turns one argument object
//               into a request and its answer or typed refusal (`method.ts`).
//   client/     `CalimeroClient`: the generated groups (`client.orgs.get(...)`)
//               on one transport, plus the flows and streams no single route is.
// This layer stays apart from generated/ (hand-written, reviewed code next to
// code nothing should edit) and from client/ (which only composes it).
//
// Public here: the method groups and their types, `createRoutes` and
// `createRouteClient` to build them on a transport of your own, the passkey
// answer to a step-up, and names for the document's types.

export { createRouteClient } from './route-client.js';
export type { ApiPaths, RouteClient, RouteClientOptions, RouteTransport } from './route-client.js';
export type { OperatorProof, OperatorProofCallback, OperatorStatement } from './step-up.js';
export type { ApiAnswer, ApiSchema } from './types.js';
export type { MethodOptions, RawBody } from './method.js';
export { createRoutes } from '../generated/methods.js';
export type {
  AppsRoutes,
  AuthRoutes,
  ConsentRoutes,
  DeploymentsRoutes,
  OpsRoutes,
  OrgsRoutes,
  PackagesRoutes,
  PublicRoutes,
  Routes,
} from '../generated/methods.js';
export type { components as ApiComponents, operations as ApiOperations } from '../generated/api.js';
