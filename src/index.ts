// Mero.js: the JavaScript client for Calimero 1.0, the confidential cloud.
//
// `CalimeroClient` verifies a node's TDX attestation, seals every request to
// it, and signs people in with passkeys and machines with API keys; its
// groups (`client.orgs.get({ org })`, ...) are every route of the node,
// generated from core's OpenAPI document. Everything below it is the pieces it
// is built from, exported for callers who need one on its own. The layering
// (runtime/ → generated/ + routes/ → client/) is described in
// src/routes/index.ts.

// The client, its errors and its types
export * from './client/index.js';

// The node's routes as generated methods (`createRoutes`), the passkey answer
// to a step-up (`passkeyConfirm`), and names for the document's types.
export * from './routes/index.js';

// The runtime the client sends every route through (also its own entry point,
// `@calimero-network/mero-js/runtime`): one `Transport.request` over the
// attested, sealed channel, with the session, step-up and error mapping.
export {
  DEFAULT_RETRY,
  NodeTransport,
  directoryTransport,
  newIdempotencyKey,
  queryString,
  refusal,
  retryDelayMs,
  routePath,
  sseFrames,
} from './runtime/transport.js';
export type {
  DirectorySource,
  HttpMethod,
  NodeTransportOptions,
  NodeVerificationState,
  QueryValue,
  RequestOptions,
  Transport,
} from './runtime/transport.js';

// Sealed transport: requests encrypted end to end to a TEE node's attested key,
// and the checks that decide whether a quote is trusted.
export * from './sealed/index.js';

// Private agents (mero-tee's agent gate): attest one, claim it with your key,
// give it its secrets sealed to its TD.
export * from './agent-gate/index.js';
// The recovery disk's files as their holder's device opens them.
export * from './recovery/backup-files.js';

// Building and signing a compute-only pipeline package (.mpk), byte-compatible with core.
export {
  MAX_ARCHIVE_BYTES,
  MAX_MANIFEST_BYTES,
  MAX_MPK_BYTES,
  MAX_PIPELINE_BYTES,
  MAX_PIPELINES_PER_PACKAGE,
  canonicalizeManifest,
  exportPublisherKey,
  generatePublisherKey,
  importPublisherKey,
  isDevPublisherKey,
  packComputePackage,
  signerIdOf,
  signingPayload,
  webCryptoPublisherKey,
} from './package/pack.js';
export type {
  ComputePackageInput,
  PackKey,
  PackageMetadata,
  PublisherKey,
  WebCryptoPublisherKey,
} from './package/pack.js';
