/**
 * A client for mero-agent-gate, the gate in a private agent TD (mero-tee,
 * `mero-agent-gate/`): attest an agent, claim it with your key, give it its
 * secrets sealed to the TD, and release it. It runs where the owner is (a
 * browser, a CLI), so whoever carries the requests, mdma included, sees only
 * quotes and ciphertext.
 *
 * One agent TD hosts several owners' agents, each named by an agent id. For
 * an agent the gate's quote binds, in its report data:
 *
 * `nonce ‖ SHA-256("mero-agent-gate/attest/v3" ‖ provisioning_pub ‖ len(id) ‖ id ‖ signing_pub ‖ owner_pub)`
 *
 * (signing and owner keys 32 zero bytes while unclaimed). {@link attestAgent}
 * checks that and the quote itself before returning anything; every function
 * that sends refuses an agent whose quote names another owner.
 *
 * ```ts
 * import { verify as dcapVerify, getCollateral } from '@phala/dcap-qvl';
 *
 * const policy = await verifySignedAgentPolicy(policyJson, bundleJson, { profile: 'locked-read-only' });
 * const verifier = agentQuoteVerifier(policy, { dcapVerify, fetchCollateral: (q) => getCollateral(PCCS, q) });
 * const owner = await generateOwnerKey(); // keep owner.secret: it is the only key that can change the agent
 * const agent = await attestAgent({ gateUrl, agentId: 'a-1f2e', verifier });
 * await provisionAgent({ gateUrl, agent, ownerSecret: owner.secret, verifier, secrets: { NEAR_AI_API_KEY: key } });
 * ```
 */

import { concat, hex } from '../crypto/internal.js';
import { x25519KeyPair } from '../sealed/noise.js';
import type { VerifyTransportQuote } from '../sealed/sealed.js';
import { hpkeSealAuth } from './hpke.js';

export const AGENT_ATTEST_DOMAIN = 'mero-agent-gate/attest/v3';
export const AGENT_PROVISION_INFO_DOMAIN = 'mero-agent-gate/provision/v2';

const AGENT_ID = /^[a-z0-9][a-z0-9-]{0,62}$/;
const SECRET_NAME = /^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$/;
const MAX_SECRETS = 64;
const MAX_SECRET_LEN = 64 * 1024;

/** A gate's refusal, with its `error` code (`not_the_owner`, `no_free_slot`, …) when it gave one. */
export class AgentGateError extends Error {
  constructor(
    message: string,
    readonly status?: number,
    readonly code?: string,
  ) {
    super(message);
    this.name = 'AgentGateError';
  }
}

/** An agent's keys, once its quote passed {@link attestAgent}'s checks. */
export interface VerifiedAgent {
  agentId: string;
  /** Base64; secrets are sealed to it. Valid until the gate restarts. */
  provisioningPublicKey: string;
  /** Base64 Ed25519 key the agent signs with; absent while unclaimed. */
  signingPublicKey?: string;
  /** Base64 X25519 key of the owner that claimed it; absent while unclaimed. */
  ownerPublicKey?: string;
}

export interface OwnerKey {
  /** 32 raw X25519 bytes. The only key that can provision or release the agent: keep it. */
  secret: Uint8Array;
  /** Base64 of the public half: what the gate's quote names as owner. */
  publicKey: string;
}

type Fetch = typeof fetch;

export function validateAgentId(agentId: string): void {
  if (!AGENT_ID.test(agentId)) {
    throw new Error(`Agent id ${JSON.stringify(agentId)} must match [a-z0-9][a-z0-9-]{0,62}`);
  }
}

function utf8(value: string): Uint8Array {
  return new TextEncoder().encode(value);
}

function lengthPrefixed(agentId: string): Uint8Array {
  const id = utf8(agentId);
  return concat(new Uint8Array([id.length]), id);
}

function toBase64(bytes: Uint8Array): string {
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function fromBase64(value: string, label: string, length?: number): Uint8Array {
  let bytes: Uint8Array;
  try {
    bytes = Uint8Array.from(atob(value.trim()), (c) => c.charCodeAt(0));
  } catch {
    throw new Error(`${label} is not base64`);
  }
  if (length !== undefined && bytes.length !== length) throw new Error(`${label} is not ${length} bytes`);
  return bytes;
}

/** The 32 bytes after the nonce in a gate's quote for `agentId` (empty: the TD alone). */
export async function agentKeyBinding(
  provisioningPublic: Uint8Array,
  agentId: string,
  signingPublic?: Uint8Array,
  ownerPublic?: Uint8Array,
): Promise<Uint8Array> {
  const zero = new Uint8Array(32);
  const data = concat(
    utf8(AGENT_ATTEST_DOMAIN),
    provisioningPublic,
    lengthPrefixed(agentId),
    signingPublic ?? zero,
    ownerPublic ?? zero,
  );
  return new Uint8Array(await crypto.subtle.digest('SHA-256', data));
}

/** HPKE `info` for a message to `agentId` on the gate whose provisioning key is `recipientPublic`. */
export function provisionInfo(recipientPublic: Uint8Array, agentId: string): Uint8Array {
  return concat(utf8(AGENT_PROVISION_INFO_DOMAIN), recipientPublic, lengthPrefixed(agentId));
}

/** A fresh owner key, made here; it never needs to leave the owner. */
export async function generateOwnerKey(): Promise<OwnerKey> {
  const secret = crypto.getRandomValues(new Uint8Array(32));
  return { secret, publicKey: await ownerPublicKey(secret) };
}

/** The base64 public half of an owner's X25519 secret. */
export async function ownerPublicKey(secret: Uint8Array): Promise<string> {
  if (secret.length !== 32) throw new Error('An owner key is 32 bytes');
  return toBase64((await x25519KeyPair(secret)).publicKey);
}

function url(gateUrl: string, path: string): string {
  return `${gateUrl.replace(/\/+$/, '')}${path}`;
}

async function postJson(fetchImpl: Fetch, target: string, body: unknown): Promise<unknown> {
  const response = await fetchImpl(target, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  const text = await response.text();
  let parsed: unknown = undefined;
  try {
    parsed = text ? JSON.parse(text) : undefined;
  } catch {
    // Not JSON: reported below with the status.
  }
  if (!response.ok) {
    const error = parsed as { error?: string; details?: string } | undefined;
    throw new AgentGateError(
      `The gate answered ${response.status}${error?.error ? ` ${error.error}` : ''}: ${error?.details ?? text.slice(0, 200)}`,
      response.status,
      error?.error,
    );
  }
  return parsed;
}

export interface AttestAgentOptions {
  /** The gate's base URL, or a relay of it (mdma's): only verified quotes are trusted. */
  gateUrl: string;
  /** The agent to attest; omit for the TD alone (no keys, nothing to send to). */
  agentId?: string;
  /** Checks the quote's signature, TCB, image and report data: {@link agentQuoteVerifier}. */
  verifier: VerifyTransportQuote;
  fetch?: Fetch;
}

interface AttestResponseBody {
  quoteB64: string;
  provisioningPublicKeyB64: string;
  agentId?: string;
  signingPublicKeyB64?: string;
  ownerPublicKeyB64?: string;
}

/**
 * Ask the gate for a quote for `agentId` over a fresh nonce and verify it:
 * a genuine quote from a measured agent image, for this nonce, committing to
 * the agent id asked about and to the keys the gate names. Returns those
 * keys only once every check has passed.
 */
export async function attestAgent(options: AttestAgentOptions): Promise<VerifiedAgent> {
  const agentId = options.agentId ?? '';
  if (agentId) validateAgentId(agentId);
  const nonce = crypto.getRandomValues(new Uint8Array(32));
  const body = (await postJson(options.fetch ?? fetch, url(options.gateUrl, '/attest'), {
    nonceB64: toBase64(nonce),
    ...(agentId ? { agentId } : {}),
  })) as AttestResponseBody;
  if ((body?.agentId ?? '') !== agentId) {
    throw new Error(`Asked about agent ${JSON.stringify(agentId)}, the gate answered for ${JSON.stringify(body?.agentId)}`);
  }
  const provisioning = fromBase64(body.provisioningPublicKeyB64 ?? '', 'provisioningPublicKeyB64', 32);
  const signing = body.signingPublicKeyB64 ? fromBase64(body.signingPublicKeyB64, 'signingPublicKeyB64', 32) : undefined;
  const owner = body.ownerPublicKeyB64 ? fromBase64(body.ownerPublicKeyB64, 'ownerPublicKeyB64', 32) : undefined;
  if (!signing !== !owner) {
    throw new Error('A claimed agent has both a signing key and an owner; the gate named one');
  }
  // The id asked about, not the one answered: a quote for another agent fails here.
  const binding = await agentKeyBinding(provisioning, agentId, signing, owner);
  await options.verifier({ quoteB64: body.quoteB64, nonce: hex(nonce), reportDataSuffix: hex(binding) });
  return {
    agentId,
    provisioningPublicKey: toBase64(provisioning),
    ...(signing ? { signingPublicKey: toBase64(signing) } : {}),
    ...(owner ? { ownerPublicKey: toBase64(owner) } : {}),
  };
}

/** The owner key `ownerSecret` may act on `agent`: it is unclaimed, or claimed by that key. */
export async function checkOwner(agent: VerifiedAgent, ownerSecret: Uint8Array): Promise<string> {
  const mine = await ownerPublicKey(ownerSecret);
  if (agent.ownerPublicKey && agent.ownerPublicKey !== mine) {
    throw new AgentGateError(
      `This agent is claimed by another key (${agent.ownerPublicKey}); it is not yours. Send it nothing and do not authorize its signing key`,
      undefined,
      'not_yours',
    );
  }
  return mine;
}

function validateSecrets(secrets: Record<string, string>): void {
  const names = Object.keys(secrets);
  if (names.length === 0) throw new Error('There are no secrets to provision');
  if (names.length > MAX_SECRETS) throw new Error(`At most ${MAX_SECRETS} secrets at once`);
  for (const name of names) {
    if (!SECRET_NAME.test(name)) throw new Error(`Secret name ${JSON.stringify(name)} must match [A-Za-z0-9][A-Za-z0-9_.-]{0,63}`);
    if (typeof secrets[name] !== 'string') throw new Error(`Secret ${name} is not a string`);
    if (utf8(secrets[name]).length > MAX_SECRET_LEN) throw new Error(`Secret ${name} is over ${MAX_SECRET_LEN} bytes`);
  }
}

async function sendSealed(
  fetchImpl: Fetch,
  gateUrl: string,
  path: '/provision' | '/release',
  agent: VerifiedAgent,
  ownerSecret: Uint8Array,
  message: unknown,
): Promise<unknown> {
  if (!agent.agentId) throw new Error('This quote names no agent; attest with the agent id');
  const mine = await checkOwner(agent, ownerSecret);
  const recipient = fromBase64(agent.provisioningPublicKey, 'provisioningPublicKey', 32);
  const sealed = await hpkeSealAuth(
    recipient,
    ownerSecret,
    provisionInfo(recipient, agent.agentId),
    utf8(JSON.stringify(message)),
  );
  return postJson(fetchImpl, url(gateUrl, path), {
    agentId: agent.agentId,
    provisioningPublicKeyB64: agent.provisioningPublicKey,
    senderPublicKeyB64: mine,
    encappedKeyB64: toBase64(sealed.enc),
    ciphertextB64: toBase64(sealed.ciphertext),
  });
}

export interface ProvisionAgentOptions {
  gateUrl: string;
  /** What {@link attestAgent} returned for this agent, just now. */
  agent: VerifiedAgent;
  ownerSecret: Uint8Array;
  /** Name to value; a name replaces what was there, others stay. */
  secrets: Record<string, string>;
  /** The verifier `agent` was attested with: the claim is checked by a fresh quote. */
  verifier: VerifyTransportQuote;
  fetch?: Fetch;
}

export interface ProvisionedAgent extends Required<VerifiedAgent> {
  written: string[];
  /** Whether this claimed the agent for your key. */
  claimed: boolean;
}

/**
 * Seal `secrets` to the agent as its owner and send them. The first bundle for
 * an unclaimed agent claims it for `ownerSecret`'s key. The gate's answer is
 * not taken as proof: a fresh quote must name your key as owner, and the
 * agent's signing key must not have changed.
 */
export async function provisionAgent(options: ProvisionAgentOptions): Promise<ProvisionedAgent> {
  validateSecrets(options.secrets);
  const fetchImpl = options.fetch ?? fetch;
  const answer = (await sendSealed(fetchImpl, options.gateUrl, '/provision', options.agent, options.ownerSecret, {
    secrets: options.secrets,
  })) as { written?: string[]; claimed?: boolean };
  const after = await attestAgent({ gateUrl: options.gateUrl, agentId: options.agent.agentId, verifier: options.verifier, fetch: fetchImpl });
  const mine = await ownerPublicKey(options.ownerSecret);
  if (after.ownerPublicKey !== mine || !after.signingPublicKey) {
    throw new AgentGateError('The gate accepted the secrets, but its quote does not name your key as owner');
  }
  if (options.agent.signingPublicKey && options.agent.signingPublicKey !== after.signingPublicKey) {
    throw new AgentGateError("The agent's signing key changed while provisioning");
  }
  return {
    agentId: after.agentId,
    provisioningPublicKey: after.provisioningPublicKey,
    signingPublicKey: after.signingPublicKey,
    ownerPublicKey: mine,
    written: answer?.written ?? [],
    claimed: answer?.claimed === true,
  };
}

export interface ReleaseAgentOptions {
  gateUrl: string;
  agent: VerifiedAgent;
  ownerSecret: Uint8Array;
  verifier: VerifyTransportQuote;
  fetch?: Fetch;
}

/**
 * Release the agent as its owner: the gate stops it and erases its claim,
 * keys, secrets and state. Checked by a fresh quote that names no owner.
 */
export async function releaseAgent(options: ReleaseAgentOptions): Promise<void> {
  if (!options.agent.ownerPublicKey) throw new Error('This agent is not claimed; there is nothing to release');
  const fetchImpl = options.fetch ?? fetch;
  await sendSealed(fetchImpl, options.gateUrl, '/release', options.agent, options.ownerSecret, { release: true });
  const after = await attestAgent({ gateUrl: options.gateUrl, agentId: options.agent.agentId, verifier: options.verifier, fetch: fetchImpl });
  if (after.ownerPublicKey) {
    throw new AgentGateError('The gate says it released the agent, but its quote still names an owner');
  }
}
