/**
 * The `CalimeroClient` the provider holds, and the policy it verifies nodes
 * under.
 */

import { CalimeroClient } from '@calimero-network/mero-js';
import type {
  CalimeroClientOptions,
  CalimeroSession,
  NodePolicy,
} from '@calimero-network/mero-js';

/** How the provider decides which nodes to trust. */
export interface PolicyOptions {
  /** The full policy, as mero-js takes it. Wins over `minReleaseVersion`. */
  policy?: NodePolicy;
  /**
   * The oldest mero-tee release to trust (e.g. `2.3.109`): shorthand for
   * `policy: { release: { minReleaseVersion } }`.
   */
  minReleaseVersion?: string;
  /**
   * Accept mock attestation quotes. For `merod local` and other `--dev`
   * clusters only: never derive it from anything a visitor controls.
   */
  allowMock?: boolean;
}

/**
 * The policy for {@link PolicyOptions}. Throws when nothing says what to trust:
 * a client that trusted every quote would not be verifying anything.
 */
export function resolvePolicy({ policy, minReleaseVersion, allowMock }: PolicyOptions): NodePolicy {
  if (policy) return allowMock ? { ...policy, allowMock: true } : policy;
  if (minReleaseVersion) return { release: { minReleaseVersion }, ...(allowMock ? { allowMock: true } : {}) };
  if (allowMock) return { allowMock: true };
  throw new Error(
    'MeroProvider: pass `minReleaseVersion` (or a `policy`) so the client knows which node releases to trust, ' +
      'or `allowMock` for `merod local`.',
  );
}

/**
 * A `CalimeroClient` that reports every session it takes: a sign-in, a sign-up
 * and the refreshes the client makes by itself on a `401` (single-flight, any
 * node). The provider keeps the newest in its store, so a reload resumes with a
 * refresh token that has not been spent.
 */
export class SessionReportingClient extends CalimeroClient {
  private readonly report: ((session: CalimeroSession) => void) | undefined;

  constructor(options: CalimeroClientOptions, onSession?: (session: CalimeroSession) => void) {
    super(options);
    this.report = onSession;
  }

  override useSession(session: CalimeroSession): void {
    super.useSession(session);
    this.report?.(session);
  }

  /** Take a session kept from an earlier page load, without reporting it back. */
  restore(session: CalimeroSession): void {
    super.useSession(session);
  }
}

/**
 * {@link SessionReportingClient}'s reporting for a client mero-js built itself
 * (`CalimeroClient.connect`, a cluster by name): every session it takes from
 * now on goes to `onSession` too. Answers the unreported `useSession`, for a
 * session kept from an earlier page load.
 */
export function reportSessions(
  client: CalimeroClient,
  onSession: (session: CalimeroSession) => void,
): (session: CalimeroSession) => void {
  const take = client.useSession.bind(client);
  client.useSession = (session: CalimeroSession) => {
    take(session);
    onSession(session);
  };
  return take;
}
