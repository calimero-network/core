import { describe, expect, it, vi } from 'vitest';

import { SealedTransportError } from '../sealed/sealed.js';

const refusal = vi.hoisted(() => ({ status: 413, code: 'too_large' as string | undefined }));

// The node answers an envelope past its 64 MiB cap with a 413 in the clear,
// before the envelope is opened, so it reaches the client as the sealed
// transport's refusal rather than as an API error.
vi.mock('../sealed/sealed.js', async (original) => {
  const actual = await original<typeof import('../sealed/sealed.js')>();
  return {
    ...actual,
    createSealedFetch: () => async () => {
      throw new actual.SealedTransportError(
        refusal.status,
        refusal.code,
        `Sealed request refused: ${refusal.code === 'too_large' ? 'a sealed request is limited to 64 MiB' : 'no'}`,
      );
    },
  };
});

const { CalimeroClient, CalimeroError } = await import('./calimero-client.js');

function client() {
  return new CalimeroClient({ baseUrl: 'http://node.test', policy: { allowMock: true } });
}

describe('CalimeroClient refusals', () => {
  it('answers an upload past the sealed cap with CalimeroError 413 TOO_LARGE', async () => {
    refusal.status = 413;
    refusal.code = 'too_large';
    const error = await client()
      .apps.blobUpload({ app: 'app', body: new Uint8Array(8) })
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CalimeroError);
    expect((error as InstanceType<typeof CalimeroError>).status).toBe(413);
    expect((error as InstanceType<typeof CalimeroError>).code).toBe('TOO_LARGE');
    expect((error as Error).message).toBe('413 TOO_LARGE: a sealed request is limited to 64 MiB');
  });

  it('leaves any other envelope refusal a SealedTransportError', async () => {
    refusal.status = 400;
    refusal.code = 'malformed';
    const error = await client()
      .apps.blobUpload({ app: 'app', body: new Uint8Array(8) })
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(SealedTransportError);
  });
});
