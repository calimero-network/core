import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { hex } from '../crypto/internal.js';
import { x25519KeyPair } from '../sealed/noise.js';
import type { VerifyTransportQuote } from '../sealed/sealed.js';
import {
  AgentGateError,
  agentKeyBinding,
  attestAgent,
  generateOwnerKey,
  provisionAgent,
  provisionInfo,
  releaseAgent,
} from './gate.js';
import { hpkeOpenAuth, hpkeSealAuth } from './hpke.js';

const b64 = (bytes: Uint8Array) => Buffer.from(bytes).toString('base64');
const unb64 = (value: string) => new Uint8Array(Buffer.from(value, 'base64'));
const fill = (byte: number) => new Uint8Array(32).fill(byte);

describe('the report-data binding', () => {
  // The vectors mero-agent-gate pins (protocol.rs, the_binding_is_pinned).
  it('is the gate’s, byte for byte', async () => {
    expect(hex(await agentKeyBinding(fill(1), 'ana', fill(2), fill(4)))).toBe(
      '61c86ed9782e4b7f3d2a0d2146f66934ce5a2c99a79be440d8419c301dfeb1f5',
    );
    expect(hex(await agentKeyBinding(fill(1), ''))).toBe(
      '315a41492f5b7820a5dd9d72c5223a4a5efd7d4c96f9991d23c9c8b54fb18f6f',
    );
  });
});

describe('HPKE with the gate', () => {
  it('opens a bundle mero-agent-gate sealed', async () => {
    const fixture = JSON.parse(
      readFileSync(fileURLToPath(new URL('./fixtures/rust-sealed-bundle.json', import.meta.url)), 'utf8'),
    );
    const plaintext = await hpkeOpenAuth(
      unb64(fixture.recipientSecret),
      unb64(fixture.senderPublic),
      provisionInfo(unb64(fixture.recipientPublic), fixture.agentId),
      { enc: unb64(fixture.enc), ciphertext: unb64(fixture.ciphertext) },
    );
    expect(JSON.parse(new TextDecoder().decode(plaintext))).toEqual({ secrets: { NEAR_AI_API_KEY: 'sk-rust' } });
  });

  it('seals what the gate opens (written for mero-agent-gate when MERO_AGENT_GATE_INTEROP is set)', async () => {
    const recipient = await x25519KeyPair(fill(0x11));
    const message = { secrets: { NEAR_AI_API_KEY: 'sk-js' } };
    const sealed = await hpkeSealAuth(
      recipient.publicKey,
      fill(0x22),
      provisionInfo(recipient.publicKey, 'ana'),
      new TextEncoder().encode(JSON.stringify(message)),
    );
    const opened = await hpkeOpenAuth(fill(0x11), (await x25519KeyPair(fill(0x22))).publicKey, provisionInfo(recipient.publicKey, 'ana'), sealed);
    expect(JSON.parse(new TextDecoder().decode(opened))).toEqual(message);
    // `cargo run -p mero-agent-gate --example interop -- open <file>` opened this
    // with the gate's own code when this test was written.
    if (process.env.MERO_AGENT_GATE_INTEROP) {
      writeFileSync(
        process.env.MERO_AGENT_GATE_INTEROP,
        JSON.stringify({ agentId: 'ana', enc: b64(sealed.enc), ciphertext: b64(sealed.ciphertext) }),
      );
    }
  });
});

/**
 * mero-agent-gate in memory, as its HTTP tests run it: the quote is the report
 * data itself (an echo quoter), so the verifier below checks the binding the
 * client computed against the one the gate committed to.
 */
class FakeGate {
  provisioningSecret = crypto.getRandomValues(new Uint8Array(32));
  claims = new Map<string, { owner: Uint8Array; signing: Uint8Array; secrets: Record<string, string> }>();
  /** Answer for this agent id instead of the one asked about. */
  answerFor?: string;

  fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const path = new URL(String(input)).pathname;
    const body = JSON.parse(String(init?.body));
    try {
      const out = await this.handle(path, body);
      return new Response(JSON.stringify(out), { status: 200 });
    } catch (error) {
      const [status, code] = (error as Error).message.split(' ');
      return new Response(JSON.stringify({ error: code, details: code }), { status: Number(status) });
    }
  }) as typeof fetch;

  async publicKey() {
    return (await x25519KeyPair(this.provisioningSecret)).publicKey;
  }

  private async handle(path: string, body: Record<string, string>) {
    const provisioning = await this.publicKey();
    if (path === '/attest') {
      const agentId = this.answerFor ?? body.agentId ?? '';
      const claim = this.claims.get(agentId);
      const binding = await agentKeyBinding(provisioning, agentId, claim?.signing, claim?.owner);
      const quote = new Uint8Array([...unb64(body.nonceB64), ...binding]);
      return {
        quoteB64: b64(quote),
        provisioningPublicKeyB64: b64(provisioning),
        ...(agentId ? { agentId } : {}),
        ...(claim ? { signingPublicKeyB64: b64(claim.signing), ownerPublicKeyB64: b64(claim.owner) } : {}),
      };
    }
    const sender = unb64(body.senderPublicKeyB64);
    const claim = this.claims.get(body.agentId);
    if (claim && b64(claim.owner) !== b64(sender)) throw new Error('403 not_the_owner');
    let message: { secrets?: Record<string, string>; release?: boolean };
    try {
      const plaintext = await hpkeOpenAuth(this.provisioningSecret, sender, provisionInfo(provisioning, body.agentId), {
        enc: unb64(body.encappedKeyB64),
        ciphertext: unb64(body.ciphertextB64),
      });
      message = JSON.parse(new TextDecoder().decode(plaintext));
    } catch {
      throw new Error('403 not_from_sender');
    }
    if (path === '/release') {
      if (!claim) throw new Error('404 unknown_agent');
      this.claims.delete(body.agentId);
      return { released: body.agentId };
    }
    const now = claim ?? { owner: sender, signing: crypto.getRandomValues(new Uint8Array(32)), secrets: {} };
    Object.assign(now.secrets, message.secrets);
    this.claims.set(body.agentId, now);
    return { written: Object.keys(message.secrets ?? {}).sort(), claimed: !claim };
  }
}

/** Accepts an echo quote exactly when it is the nonce followed by the binding. */
const echoVerifier: VerifyTransportQuote = async ({ quoteB64, nonce, reportDataSuffix }) => {
  if (hex(unb64(quoteB64)) !== `${nonce}${reportDataSuffix}`) throw new Error('The quote does not commit to this nonce and binding');
  return true;
};

describe('an agent gate client', () => {
  const gateUrl = 'http://gate.test:8090';

  it('claims an unclaimed agent, and a fresh quote names its owner', async () => {
    const gate = new FakeGate();
    const owner = await generateOwnerKey();
    const agent = await attestAgent({ gateUrl, agentId: 'a-1', verifier: echoVerifier, fetch: gate.fetch });
    expect(agent).toEqual({ agentId: 'a-1', provisioningPublicKey: b64(await gate.publicKey()) });
    const done = await provisionAgent({
      gateUrl,
      agent,
      ownerSecret: owner.secret,
      verifier: echoVerifier,
      fetch: gate.fetch,
      secrets: { NEAR_AI_API_KEY: 'sk-1', MERO_AGENTS_JSON: '{}' },
    });
    expect(done.claimed).toBe(true);
    expect(done.written).toEqual(['MERO_AGENTS_JSON', 'NEAR_AI_API_KEY']);
    expect(done.ownerPublicKey).toBe(owner.publicKey);
    expect(gate.claims.get('a-1')?.secrets.NEAR_AI_API_KEY).toBe('sk-1');
  });

  it('refuses, before sending anything, an agent another key claimed', async () => {
    const gate = new FakeGate();
    const first = await generateOwnerKey();
    const agent = await attestAgent({ gateUrl, agentId: 'a-1', verifier: echoVerifier, fetch: gate.fetch });
    await provisionAgent({ gateUrl, agent, ownerSecret: first.secret, verifier: echoVerifier, fetch: gate.fetch, secrets: { A: '1' } });

    const second = await generateOwnerKey();
    const claimed = await attestAgent({ gateUrl, agentId: 'a-1', verifier: echoVerifier, fetch: gate.fetch });
    await expect(
      provisionAgent({ gateUrl, agent: claimed, ownerSecret: second.secret, verifier: echoVerifier, fetch: gate.fetch, secrets: { A: 'evil' } }),
    ).rejects.toThrow('not yours');
    // Trusting a stale quote instead, the gate refuses it.
    await expect(
      provisionAgent({ gateUrl, agent, ownerSecret: second.secret, verifier: echoVerifier, fetch: gate.fetch, secrets: { A: 'evil' } }),
    ).rejects.toMatchObject({ status: 403, code: 'not_the_owner' } satisfies Partial<AgentGateError>);
    expect(gate.claims.get('a-1')?.secrets.A).toBe('1');
  });

  it('refuses a quote for another agent than the one asked about', async () => {
    const gate = new FakeGate();
    gate.answerFor = 'a-2';
    await expect(attestAgent({ gateUrl, agentId: 'a-1', verifier: echoVerifier, fetch: gate.fetch })).rejects.toThrow(
      'the gate answered for "a-2"',
    );
  });

  it('catches keys the quote does not commit to', async () => {
    const gate = new FakeGate();
    const swapped: typeof fetch = async (input, init) => {
      const response = await gate.fetch(input, init);
      const body = await response.json();
      body.provisioningPublicKeyB64 = b64(fill(9));
      return new Response(JSON.stringify(body), { status: 200 });
    };
    await expect(attestAgent({ gateUrl, agentId: 'a-1', verifier: echoVerifier, fetch: swapped })).rejects.toThrow(
      'does not commit',
    );
  });

  it('releases an agent as its owner, and a fresh quote names no owner', async () => {
    const gate = new FakeGate();
    const owner = await generateOwnerKey();
    let agent = await attestAgent({ gateUrl, agentId: 'a-1', verifier: echoVerifier, fetch: gate.fetch });
    await provisionAgent({ gateUrl, agent, ownerSecret: owner.secret, verifier: echoVerifier, fetch: gate.fetch, secrets: { A: '1' } });
    agent = await attestAgent({ gateUrl, agentId: 'a-1', verifier: echoVerifier, fetch: gate.fetch });
    await releaseAgent({ gateUrl, agent, ownerSecret: owner.secret, verifier: echoVerifier, fetch: gate.fetch });
    expect(gate.claims.has('a-1')).toBe(false);
  });

  it('checks agent ids and secret names before anything is sent', async () => {
    const gate = new FakeGate();
    await expect(attestAgent({ gateUrl, agentId: 'A_1', verifier: echoVerifier, fetch: gate.fetch })).rejects.toThrow(
      'must match',
    );
    const owner = await generateOwnerKey();
    const agent = await attestAgent({ gateUrl, agentId: 'a-1', verifier: echoVerifier, fetch: gate.fetch });
    await expect(
      provisionAgent({ gateUrl, agent, ownerSecret: owner.secret, verifier: echoVerifier, fetch: gate.fetch, secrets: { '../x': '1' } }),
    ).rejects.toThrow('must match');
    await expect(
      provisionAgent({ gateUrl, agent, ownerSecret: owner.secret, verifier: echoVerifier, fetch: gate.fetch, secrets: {} }),
    ).rejects.toThrow('no secrets');
    expect(gate.claims.size).toBe(0);
  });
});
