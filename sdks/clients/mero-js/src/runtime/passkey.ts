/**
 * Passkeys: the browser's WebAuthn API, or anything that answers like it.
 *
 * Every binary field crosses the wire as base64url, matching
 * `calimero-identity`'s `Registration` and `Assertion`.
 */

import type { components } from '../generated/api.js';
import { ConfirmCancelled, ConfirmPromptBlocked } from './errors.js';

export interface RegistrationJSON {
  attestationObject: string;
  clientDataJson: string;
}

export interface AssertionJSON {
  credentialId: string;
  authenticatorData: string;
  clientDataJson: string;
  signature: string;
}

/** How strongly an assertion must verify the user (WebAuthn's `UserVerificationRequirement`). */
export type PasskeyUserVerification = "required" | "preferred" | "discouraged";

/** What a confirmation asks of the passkey beyond the challenge. */
export interface PasskeyGetOptions {
  /**
   * `"required"` for a step-up confirmation (`412 CONFIRM_REQUIRED`): the
   * node accepts only an assertion with the user-verified flag (PIN or
   * biometric). `"preferred"` for the sign-up's confirmation, which the node
   * takes with the user present. Absent for a plain sign-in, where
   * `"preferred"` is enough.
   */
  userVerification?: PasskeyUserVerification;
  /**
   * Closes the open prompt when it aborts (a Cancel button, a page leaving).
   * The browser provider then rejects as a dismissed prompt does
   * (`NotAllowedError`), so the caller treats it as a cancel.
   */
  signal?: AbortSignal;
}

/** What a passkey creation takes beyond the node's options. */
export interface PasskeyCreateOptions {
  /** As {@link PasskeyGetOptions.signal}: aborting closes the prompt, rejecting as a cancel (`NotAllowedError`). */
  signal?: AbortSignal;
}

/** Something that holds a passkey. */
export interface PasskeyProvider {
  /**
   * Answer `navigator.credentials.create` for the node's options.
   * `createOptions.signal` closes the prompt when it aborts; a provider with
   * no prompt to close may ignore it.
   */
  create(options: Record<string, unknown>, createOptions?: PasskeyCreateOptions): Promise<RegistrationJSON>;
  /**
   * Sign `challenge` (an assertion), optionally limited to
   * `allowCredentials`. `options.userVerification` is `"required"` when the
   * client answers a step-up confirmation (`"preferred"` for the sign-up's);
   * a provider that always verifies the user may ignore it.
   * `options.signal` closes the prompt when it aborts.
   */
  get(
    challenge: Uint8Array,
    rpId: string,
    allowCredentials?: string[],
    options?: PasskeyGetOptions,
  ): Promise<AssertionJSON>;
}

export function b64url(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function fromB64url(s: string): Uint8Array {
  const pad =
    s.replace(/-/g, "+").replace(/_/g, "/") + "===".slice((s.length + 3) % 4);
  const bin = atob(pad);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

const buf = (b: Uint8Array): ArrayBuffer =>
  b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength) as ArrayBuffer;

/** A dismissed prompt, named as WebAuthn names one (`NotAllowedError`), so a confirmation reads it as declined. */
function cancelled(message: string): Error {
  const error = new Error(message);
  error.name = "NotAllowedError";
  return error;
}

/**
 * Ask the browser's own WebAuthn API (`CredentialsContainer.prototype[method]`),
 * not whatever a password manager extension put on `navigator.credentials`.
 * Calimero's passkeys live in the platform's authenticators: iCloud Keychain
 * and Touch ID, Google Password Manager, Windows Hello, a phone over a QR code,
 * a security key. Extensions sit in between and broke sign-up: LastPass, after
 * creating a passkey, opened a hidden request of its own on the next prompt,
 * so the browser refused it ("A request is already pending") and every try
 * left a passkey with no account. A passkey kept only in an extension's vault
 * is therefore not offered here. Where the browser has no
 * `CredentialsContainer` (tests, older runtimes), `navigator.credentials` is used.
 */
type Ask = (
  options: CredentialCreationOptions | CredentialRequestOptions,
) => Promise<Credential | null>;

async function ask(
  method: "create" | "get",
  options: CredentialCreationOptions | CredentialRequestOptions,
  signal: AbortSignal | undefined,
  what: string,
): Promise<Credential | null> {
  const container = navigator.credentials;
  const own = (
    globalThis as unknown as {
      CredentialsContainer?: { prototype: Record<string, unknown> };
    }
  ).CredentialsContainer?.prototype?.[method] as Ask | undefined;
  const call = own ?? (container[method] as Ask);
  try {
    return await call.call(container, signal ? { ...options, signal } : options);
  } catch (error) {
    // An aborted `signal` rejects with its reason (an `AbortError` unless the
    // caller gave another): the prompt was closed on purpose, so it reads as
    // a dismissed one.
    if (signal?.aborted || (error as { name?: unknown } | null)?.name === "AbortError") {
      throw Object.assign(cancelled(`${what} was cancelled`), { cause: error });
    }
    throw error;
  }
}

/**
 * The node's algorithms with ES256 alone when it is among them. Some password
 * managers make an Ed25519 (EdDSA, -8) passkey when the site offers it and
 * then fail to sign with it: the account's create succeeds and its very next
 * prompt fails. LastPass did this on Calimero's sign-up (create, then approve
 * at once), refusing the approval as "A request is already pending"; Proton
 * Pass answers UnsupportedAlgorithm. ES256 is what every authenticator makes,
 * and the node checks it as it checks EdDSA, so a passkey already made with
 * EdDSA still signs in.
 */
export function es256First(
  params: Array<{ type: "public-key"; alg: number }>,
): Array<{ type: "public-key"; alg: number }> {
  const es256 = params.filter((p) => p.alg === -7);
  return es256.length ? es256 : params;
}

/** The platform authenticator (Touch ID, Face ID, Windows Hello, a security key, a synced passkey). */
export function browserPasskey(): PasskeyProvider {
  return {
    async create(options, createOptions) {
      const o = options as {
        challenge: string;
        rp: { id: string; name: string };
        user: { id: string; name: string; displayName: string };
        pubKeyCredParams: Array<{ type: "public-key"; alg: number }>;
        excludeCredentials?: Array<{ type: "public-key"; id: string }>;
        authenticatorSelection?: AuthenticatorSelectionCriteria;
        timeout?: number;
      };
      const cred = (await ask("create", {
        publicKey: {
          challenge: buf(fromB64url(o.challenge)),
          rp: o.rp,
          user: { ...o.user, id: buf(fromB64url(o.user.id)) },
          pubKeyCredParams: es256First(o.pubKeyCredParams),
          excludeCredentials: (o.excludeCredentials ?? []).map((c) => ({
            type: c.type,
            id: buf(fromB64url(c.id)),
          })),
          authenticatorSelection: o.authenticatorSelection,
          attestation: "none",
          timeout: o.timeout,
        },
      }, createOptions?.signal, "passkey creation")) as PublicKeyCredential | null;
      if (!cred) throw cancelled("passkey creation was cancelled");
      const r = cred.response as AuthenticatorAttestationResponse;
      return {
        attestationObject: b64url(new Uint8Array(r.attestationObject)),
        clientDataJson: b64url(new Uint8Array(r.clientDataJSON)),
      };
    },
    async get(challenge, rpId, allowCredentials, options) {
      const cred = (await ask("get", {
        publicKey: {
          challenge: buf(challenge),
          rpId,
          userVerification: options?.userVerification ?? "preferred",
          allowCredentials: (allowCredentials ?? []).map((id) => ({
            type: "public-key",
            id: buf(fromB64url(id)),
          })),
        },
      }, options?.signal, "passkey sign-in")) as PublicKeyCredential | null;
      if (!cred) throw cancelled("passkey sign-in was cancelled");
      const r = cred.response as AuthenticatorAssertionResponse;
      return {
        credentialId: b64url(new Uint8Array(cred.rawId)),
        authenticatorData: b64url(new Uint8Array(r.authenticatorData)),
        clientDataJson: b64url(new Uint8Array(r.clientDataJSON)),
        signature: b64url(new Uint8Array(r.signature)),
      };
    },
  };
}

// ---------------------------------------------------------------- the step-up
/**
 * `details` of a `412 CONFIRM_REQUIRED`: the WebAuthn challenge to sign.
 * `userVerification` is read when a node names it (core does not today).
 */
export type ConfirmChallenge = components['schemas']['ConfirmRequired'] & { userVerification?: unknown };

/** What a confirmation answers a `412` with: an assertion over the challenge. */
export type ConfirmCallback = (challenge: ConfirmChallenge, context: { request: Request }) => Promise<AssertionJSON>;

/**
 * The user verification a confirmation asks the passkey for: what the node's
 * `412` details name, if it names one WebAuthn knows, else `fallback` (the
 * flow's own requirement: `required` for a step-up, which the node takes only
 * with the user verified; `preferred` for the sign-up confirmation, which it
 * takes with the user present).
 */
export function userVerificationOf(named: unknown, fallback: PasskeyUserVerification): PasskeyUserVerification {
  return named === 'required' || named === 'preferred' || named === 'discouraged' ? named : fallback;
}

/**
 * What a failed confirmation prompt means to the caller. The browser or a
 * password manager refusing to show the prompt (the page not focused because
 * an extension's popup still holds focus, or another prompt still open) is
 * {@link ConfirmPromptBlocked}; the user dismissing it (`NotAllowedError`, or
 * `AbortError` when it was aborted) is {@link ConfirmCancelled}; anything
 * else is a real failure, passed through.
 */
export function promptFailure(error: unknown): unknown {
  const e = error as { name?: unknown; message?: unknown } | null;
  const name = e?.name;
  const message = typeof e?.message === 'string' ? e.message : '';
  if (name === 'InvalidStateError' || /not focused|already pending/i.test(message)) {
    return new ConfirmPromptBlocked(error);
  }
  if (name === 'NotAllowedError' || name === 'AbortError') return new ConfirmCancelled(error);
  return error;
}

/** How {@link passkeyConfirm} prompts. */
export interface PasskeyConfirmOptions {
  /** The user verification asked for when the node names none. Default `required` (a step-up). */
  userVerification?: PasskeyUserVerification;
  /** Aborts the passkey prompt. */
  signal?: AbortSignal;
}

/**
 * `passkey`'s assertion over a `412` challenge: the {@link ConfirmCallback}
 * of a passkey. A prompt the user dismissed rejects with
 * {@link ConfirmCancelled}, one the browser would not show with
 * {@link ConfirmPromptBlocked}.
 */
export function passkeyConfirm(passkey: PasskeyProvider, options: PasskeyConfirmOptions = {}): ConfirmCallback {
  return async (challenge) => {
    try {
      // A step-up is taken only with the user verified (PIN or biometric);
      // `preferred` lets a password manager skip it, and the node then
      // refuses. Only the sign-up confirmation passes `preferred`. A
      // requirement the node names itself wins.
      return await passkey.get(fromB64url(challenge.challenge), challenge.rpId, challenge.allowCredentials, {
        userVerification: userVerificationOf(challenge.userVerification, options.userVerification ?? 'required'),
        ...(options.signal ? { signal: options.signal } : {}),
      });
    } catch (declined) {
      throw promptFailure(declined);
    }
  };
}
