/**
 * What the package root actually exposes.
 *
 * A symbol exported from a subdirectory's `index.ts` but not from here is
 * unreachable to anyone installing the package. The names below are the ones
 * consumers import today (mdma's console loads the package at run time and
 * checks for `CalimeroClient`), so dropping one fails here, not in their build.
 */
import { describe, expect, it } from 'vitest';

import * as root from './index.js';

describe('the package root', () => {
  it('exposes the Calimero 1.0 client and what its callers name', () => {
    for (const name of [
      'CalimeroClient',
      'browserPasskey',
      'fetchAttestedKey',
      'verifyNode',
      'verifyNodeUrl',
      'claimedRelease',
      'MOCK_QUOTE_HEADER',
      'b64url',
      'fromB64url',
      'errorFrom',
      'CalimeroError',
      'LoginRequired',
      'PasskeyUnknownHere',
      'NotFound',
      'ConfirmRequired',
      'ConfirmCancelled',
      'ConfirmPromptBlocked',
      'ArgsTooLarge',
      'IdempotencyMismatch',
      'AppError',
      'TrapError',
      'UnavailableError',
      'WrongNode',
      'NotServing',
      'ClusterNotUpgraded',
      'FeatureNotActive',
      'ORG_CAPABILITIES',
      'orgCapabilities',
      'createSealedFetch',
      'transportKeyBinding',
      'createQuoteVerifier',
      'createSignedReleaseVerifier',
      'trustSignedRelease',
      'passkeyConfirm',
      'presenceState',
      'createRoutes',
      'createRouteClient',
      'DEFAULT_DIRECTORY',
      'DEFAULT_ORG_CAPABILITIES',
    ]) {
      expect(root, name).toHaveProperty(name);
    }
    expect(typeof root.CalimeroClient).toBe('function');
  });

  it("keeps the route client's plumbing internal: write against the generated methods", () => {
    for (const name of ['createApiClient', 'API_BASE_URL', 'transportFetch', 'unwrap', 'refusalOf', 'idempotencyKeys', 'stepUp', 'operatorSignature', 'route']) {
      expect(root, name).not.toHaveProperty(name);
    }
  });

  it('no longer carries the v1 client', () => {
    for (const name of ['MeroJs', 'createMeroJs', 'AdminApiClient', 'AuthApiClient', 'RpcClient', 'SseClient', 'CloudClient']) {
      expect(root, name).not.toHaveProperty(name);
    }
  });
});
