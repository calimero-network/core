import { describe, expect, it } from 'vitest';
import { ConfirmCancelled, DirectoryError, errorFrom } from '@calimero-network/mero-js';
import { describeError } from '../errors';

describe('describeError', () => {
  it('reads an app error with its code and named fields', () => {
    const f = describeError(errorFrom(422, { error: 'APP_ERROR', message: 'no', details: { code: 'Taken', fields: { key: 'a' } } }));
    expect(f).toMatchObject({ kind: 'app', appCode: 'Taken', fields: { key: 'a' }, retryable: false, status: 422 });
  });

  it.each([
    [401, 'UNAUTHENTICATED', 'login'],
    [412, 'CONFIRM_REQUIRED', 'confirm'],
    [421, 'WRONG_NODE', 'wrong-node'],
    [421, 'NOT_LEADER', 'wrong-node'],
    [404, 'NOT_FOUND', 'not-found'],
    [409, 'CONSENT_STALE', 'consent-stale'],
    [409, 'IDEMPOTENCY_MISMATCH', 'idempotency'],
    [413, 'ARGS_TOO_LARGE', 'too-large'],
    [413, 'TOO_LARGE', 'too-large'],
    [403, 'UPLOADS_DISABLED', 'forbidden'],
    [428, 'OPERATOR_SIGNATURE_REQUIRED', 'operator-signature'],
    [429, 'RATE_LIMITED', 'rate-limited'],
    [422, 'TRAP', 'trap'],
  ])('%i %s is %s', (status, code, kind) => {
    expect(describeError(errorFrom(status, { error: code, message: 'm' })).kind).toBe(kind);
  });

  it('keeps the release a deadline names', () => {
    const f = describeError(errorFrom(409, { error: 'RELEASE_DEADLINE', details: { release: '2.3.110', after: 'x' } }));
    expect(f).toMatchObject({ kind: 'release-deadline', release: '2.3.110' });
  });

  it('says whether an unavailable call may have committed', () => {
    expect(describeError(errorFrom(503, { error: 'TIMEOUT' }))).toMatchObject({ kind: 'unavailable', committed: false, retryable: true });
    expect(describeError(errorFrom(503, { error: 'UNAVAILABLE' }))).toMatchObject({ committed: 'unknown' });
  });

  it('reads a cluster the directory does not know', () => {
    expect(describeError(new DirectoryError('eu-x', 'not-found', 'no such cluster', 404))).toMatchObject({
      kind: 'directory',
      code: 'not-found',
      retryable: false,
      message: 'There is no cluster named eu-x.',
    });
    expect(describeError(new DirectoryError('eu-x', 'unreachable', 'down')).retryable).toBe(true);
  });

  it('treats a dismissed passkey prompt as cancelled', () => {
    expect(describeError(new ConfirmCancelled()).kind).toBe('cancelled');
  });

  it('passes anything else through as unknown', () => {
    expect(describeError(new Error('boom'))).toMatchObject({ kind: 'unknown', message: 'boom' });
    expect(describeError('plain')).toMatchObject({ kind: 'unknown', message: 'plain' });
  });
});
