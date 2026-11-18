import { describe, expect, it } from 'vitest';

import type { Governance } from '../client/approvals.js';
import { hex } from '../crypto/internal.js';
import {
  backupPublicKey,
  holderRecoveryBody,
  newBackupKeyPair,
  openDataKey,
  receiptKeyBinding,
  recoveryAttestationNonce,
  verifyRecoveryAttestation,
  verifyRecoveryHeader,
} from './backup-files.js';
import binding from './rust-binding.json';
import fixture from './rust-fixture.json';

const fromHexStr = (s: string) => new Uint8Array(s.match(/../g)!.map((b) => parseInt(b, 16)));

// Made by core's `calimero_client::recovery` (seal_key, a header signed as a
// node signs one): what this module reads must be exactly what a node writes.
describe('recovery files sealed by a node', () => {
  it('checks the header and opens the data key with the holder secret', async () => {
    const header = await verifyRecoveryHeader(fixture);
    expect(header.kind).toBe('app');
    expect(header.holder).toEqual({ kind: 'org', id: 'org-1' });
    const key = await openDataKey(header, fromHexStr(fixture.secret));
    expect(hex(key)).toBe(fixture.dataKey);
  });

  it('refuses a header that is not the one signed', async () => {
    const changed = fixture.header.replace('"index":42', '"index":43');
    await expect(verifyRecoveryHeader({ ...fixture, header: changed })).rejects.toThrow(/signature/);
  });

  it('refuses another key', async () => {
    const header = await verifyRecoveryHeader(fixture);
    const { secret } = await newBackupKeyPair();
    await expect(openDataKey(header, secret)).rejects.toThrow(/not the one/);
  });

  it('derives the public key core derives', async () => {
    expect(await backupPublicKey(fromHexStr(fixture.secret))).toBe(fixture.public);
  });

  it('builds the holder recovery body only from that holder files', async () => {
    const listed = [{ name: 'app-app-1.0.0.rbak', header: fixture.header, signature: fixture.signature }];
    await expect(
      holderRecoveryBody({ kind: 'org', id: 'org-1' }, listed, fromHexStr(fixture.secret)),
    ).rejects.toThrow(/no holder file/);
    await expect(
      holderRecoveryBody({ kind: 'org', id: 'other' }, listed, fromHexStr(fixture.secret)),
    ).rejects.toThrow(/not org:other/);
  });
});

// Core's `attestation_nonce` and `receipt_key_binding` (v3) of the fixture's
// signing key under the default governance, for its cluster and node.
describe('a recovery file attestation', () => {
  it('binds the nonce and the receipt key as core does', async () => {
    expect(await recoveryAttestationNonce(binding.cluster, binding.node)).toBe(binding.nonce);
    const key = fromHexStr(binding.signingKey);
    expect(await receiptKeyBinding(key, binding.governance as Governance, binding.cluster, binding.node)).toBe(binding.binding);
    // Another node's quote binds other bytes.
    expect(await receiptKeyBinding(key, binding.governance as Governance, binding.cluster, binding.node + 1)).not.toBe(
      binding.binding,
    );
  });

  it('asks the quote verifier for that nonce and binding, and refuses another key', async () => {
    const header = await verifyRecoveryHeader(fixture);
    const attested = {
      ...header,
      cluster: binding.cluster,
      node: binding.node,
      signingKey: binding.signingKey,
      attestation: {
        quote: 'cXVvdGU=',
        nonce: binding.nonce,
        cluster: binding.cluster,
        node: binding.node,
        receiptKey: binding.signingKey,
        governance: binding.governance as Governance,
      },
    };
    const asked: unknown[] = [];
    const verifyQuote = async (args: unknown) => {
      asked.push(args);
      return true;
    };
    await verifyRecoveryAttestation(attested, verifyQuote);
    expect(asked).toEqual([{ quoteB64: 'cXVvdGU=', nonce: binding.nonce, reportDataSuffix: binding.binding }]);
    const otherKey = { ...attested, attestation: { ...attested.attestation, receiptKey: '00'.repeat(32) } };
    await expect(verifyRecoveryAttestation(otherKey, verifyQuote)).rejects.toThrow(/signed/);
    await expect(verifyRecoveryAttestation({ ...header, attestation: null }, verifyQuote)).rejects.toThrow(
      /no attestation/,
    );
    const { governance: _governance, ...noGovernance } = attested.attestation;
    await expect(verifyRecoveryAttestation({ ...attested, attestation: noGovernance }, verifyQuote)).rejects.toThrow(
      'names no governance',
    );
  });
});
