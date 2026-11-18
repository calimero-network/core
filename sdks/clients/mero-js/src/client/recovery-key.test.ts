import { describe, expect, it } from 'vitest';

import {
  parseRecoverySeed,
  recoveryMessage,
  recoveryPublicKey,
  recoverySeedText,
  signRecovery,
} from './recovery-key.js';

describe('recovery key', () => {
  // Pinned in core: `calimero_identity::recovery::the_wire_bytes_are_pinned_for_the_clients`.
  const seed = new Uint8Array(32).fill(1);

  it('derives and signs the bytes core verifies', async () => {
    expect(new TextDecoder().decode(recoveryMessage('o', 'a', 'n'))).toBe('calimero.org-recovery.v1\0o\0a\0n');
    expect(await recoveryPublicKey(seed)).toBe('iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w');
    expect(await signRecovery(seed, 'o', 'a', 'n')).toBe(
      'bbbHMaH2SC50zQADskIDS5xg8WxZFrtxdasJzhzCOMZh-aBrUxeDKro2_UUIyU4PzH5LmsS1gQRQmVfx3E1eDw',
    );
  });

  it('refuses an empty or long nonce', async () => {
    await expect(signRecovery(seed, 'o', 'a', '')).rejects.toThrow();
    await expect(signRecovery(seed, 'o', 'a', 'n'.repeat(65))).rejects.toThrow();
  });

  it('reads a seed back from its written form', () => {
    const s = Uint8Array.from({ length: 32 }, (_, i) => (i * 7) & 0xff);
    const text = recoverySeedText(s);
    expect(text).toHaveLength(71);
    expect(parseRecoverySeed(text)).toEqual(s);
    expect(parseRecoverySeed(text.toUpperCase().replace(/-/g, ' '))).toEqual(s);
    expect(() => parseRecoverySeed(text.slice(0, 70))).toThrow();
    expect(() => parseRecoverySeed(text.replace(/0/g, 'g'))).toThrow();
  });
});
