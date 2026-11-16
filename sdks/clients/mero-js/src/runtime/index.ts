// The mero-js runtime (`@calimero-network/mero-js/runtime`): everything a
// route layer sends through, and nothing that is a route.
//
// `Transport.request(method, path, options)` reaches one node route over the
// attested, sealed channel with the credential held now; the runtime handles
// attestation, sealing, the session and its refresh, `421 WRONG_NODE` moves,
// D70 retries, the `412 CONFIRM_REQUIRED` passkey step-up, error mapping and
// server-sent events. Its pieces live here too: the node's attestation
// (`attestation.ts`), the node directory (`directory.ts`), passkeys
// (`passkey.ts`) and the typed refusals (`errors.ts`). This is the bottom layer:
// it imports nothing from routes/, generated/ or client/. The route layer on
// top (src/routes/index.ts describes the layering) sends through
// `NodeTransport.exchange`; `client.transport` is a `CalimeroClient`'s runtime.

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
} from './transport.js';
export type {
  CalimeroSession,
  NodeEvent,
  RetryOptions,
  DirectorySource,
  HttpMethod,
  NodeTransportOptions,
  NodeVerificationState,
  QueryValue,
  RequestOptions,
  Transport,
} from './transport.js';

// Attestation: what a node must prove before anything is sealed to it.
export { claimedRelease, fetchAttestedKey, freshCollateral, MOCK_QUOTE_HEADER, verifyNode } from './attestation.js';
export type { AttestationDoc, NodePolicy, NodeReleasePolicy, NodeVerification } from './attestation.js';

// The node directory a cluster's node list is read from.
export { DEFAULT_DIRECTORY, DirectoryError } from './directory.js';
export type { DirectoryFailure } from './directory.js';

// Passkeys (WebAuthn) for the step-up and the ceremonies.
export { browserPasskey, b64url, fromB64url, passkeyConfirm } from './passkey.js';
export type {
  AssertionJSON,
  ConfirmCallback,
  ConfirmChallenge,
  PasskeyConfirmOptions,
  PasskeyCreateOptions,
  PasskeyGetOptions,
  PasskeyProvider,
  PasskeyUserVerification,
  RegistrationJSON,
} from './passkey.js';

// A refusal as a typed error.
export * from './errors.js';

// The sealed channel and the quote and release checks under it.
export * from '../sealed/index.js';
