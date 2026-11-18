import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { verify as dcapVerify } from '@phala/dcap-qvl';
import { describe, expect, it } from 'vitest';

import { hex } from '../crypto/internal.js';
import type { DcapCollateral } from '../sealed/verify.js';
import { agentQuoteVerifier, verifySignedAgentPolicy, type AgentPolicy } from './policy.js';

const sealedFixture = (name: string) => fileURLToPath(new URL(`../sealed/fixtures/${name}`, import.meta.url));
const QUOTE = new Uint8Array(readFileSync(sealedFixture('tdx_quote.bin')));
const COLLATERAL = JSON.parse(readFileSync(sealedFixture('tdx_quote_collateral.json'), 'utf8')) as DcapCollateral;
// Inside that collateral's validity window, as the sealed tests use it.
const AT = 1_751_000_000_000;
const sample = dcapVerify(QUOTE, COLLATERAL, AT / 1000);
const td = sample.report.data as Record<'mrTd' | 'rtMr0' | 'rtMr1' | 'rtMr2' | 'rtMr3' | 'reportData', Uint8Array>;

/** The sample quote's image as an agent allowlist, written the way a release may write it. */
const policy = (): AgentPolicy => ({
  schema_version: 1,
  role: 'agent',
  profile: 'locked-read-only',
  allowed_tcb_statuses: [sample.status.toLowerCase()],
  allowed_mrtd: [`0x${hex(td.mrTd).toUpperCase()}`],
  allowed_rtmr0: [hex(td.rtMr0)],
  allowed_rtmr1: [hex(td.rtMr1)],
  allowed_rtmr2: [hex(td.rtMr2)],
  allowed_rtmr3: [hex(td.rtMr3)],
});

const args = () => ({
  quoteB64: Buffer.from(QUOTE).toString('base64'),
  nonce: hex(td.reportData.slice(0, 32)),
  reportDataSuffix: hex(td.reportData.slice(32)),
});

describe('an agent quote verifier', () => {
  const verifier = (p: AgentPolicy) =>
    agentQuoteVerifier(p, { dcapVerify, fetchCollateral: async () => COLLATERAL, now: () => AT });

  it('accepts a genuine quote of an image the policy lists, fetching its collateral', async () => {
    await expect(verifier(policy())(args())).resolves.toBe(true);
  });

  it('refuses a register outside the policy', async () => {
    const p = policy();
    p.allowed_rtmr3 = ['a'.repeat(96)];
    await expect(verifier(p)(args())).rejects.toThrow('RTMR3');
  });

  it('refuses a TCB status the policy does not allow', async () => {
    const p = policy();
    p.allowed_tcb_statuses = sample.status === 'OutOfDate' ? ['UpToDate'] : ['OutOfDate'];
    await expect(verifier(p)(args())).rejects.toThrow('TCB status');
  });

  it('accepts nothing from an empty list, as the gate’s own verifier', () => {
    const p = policy();
    p.allowed_rtmr0 = [];
    expect(() => verifier(p)).toThrow('allowed_rtmr0 is empty');
  });

  it('refuses a status no DCAP verifier reports', () => {
    const p = policy();
    p.allowed_tcb_statuses = ['Fine'];
    expect(() => verifier(p)).toThrow('unknown TCB status');
  });
});

describe('a signed agent policy', () => {
  const kms = {
    body: readFileSync(sealedFixture('mero-kms-v2.3.87.kms-attestation-policy.json'), 'utf8'),
    bundle: readFileSync(sealedFixture('mero-kms-v2.3.87.kms-attestation-policy.json.bundle.json'), 'utf8'),
  };
  const node = {
    body: readFileSync(sealedFixture('mero-tee-v2.3.87.published-mrtds.json'), 'utf8'),
    bundle: readFileSync(sealedFixture('mero-tee-v2.3.87.published-mrtds.json.bundle.json'), 'utf8'),
  };

  it('is a file the KMS release workflow signed, and an agent allowlist', async () => {
    // Genuinely signed by the KMS workflow, but the KMS's own policy, not an agent one.
    await expect(verifySignedAgentPolicy(kms.body, kms.bundle)).rejects.toThrow('not an agent one');
  });

  it('names the role it asked for: an inference allowlist is not the KMS policy either', async () => {
    await expect(verifySignedAgentPolicy(kms.body, kms.bundle, { role: 'inference' })).rejects.toThrow(
      'not an inference one',
    );
  });

  it('refuses a file signed by any other workflow', async () => {
    await expect(verifySignedAgentPolicy(node.body, node.bundle)).rejects.toThrow('release-kms.yaml');
  });

  it('refuses a file whose bytes are not the signed ones', async () => {
    await expect(verifySignedAgentPolicy(`${kms.body} `, kms.bundle)).rejects.toThrow();
  });
});
