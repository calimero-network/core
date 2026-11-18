import { afterEach, describe, expect, it, vi } from "vitest";
import { browserPasskey, b64url, es256First } from "./passkey.js";

// A page where an extension (LastPass) has replaced navigator.credentials:
// after one prompt it answers every later one in the tab with "A request is
// already pending", though the browser itself has none open. A sign-up takes
// two prompts in a row (create, then approve), so its second always failed.

const pending = () =>
  Object.assign(new Error("A request is already pending."), {
    name: "OperationError",
  });

function assertion(): PublicKeyCredential {
  const bytes = new Uint8Array([1, 2, 3]).buffer;
  return {
    rawId: bytes,
    response: {
      authenticatorData: bytes,
      clientDataJSON: bytes,
      signature: bytes,
    },
  } as unknown as PublicKeyCredential;
}

function page({
  extension,
  browser,
}: {
  extension?: (o: unknown) => Promise<unknown>;
  browser: (o: unknown) => Promise<unknown>;
}) {
  class CredentialsContainer {}
  Object.assign(CredentialsContainer.prototype, {
    get: browser,
    create: browser,
  });
  const container = new CredentialsContainer() as Record<string, unknown>;
  if (extension)
    Object.assign(container, { get: extension, create: extension });
  vi.stubGlobal("CredentialsContainer", CredentialsContainer);
  vi.stubGlobal("navigator", { credentials: container });
}

afterEach(() => vi.unstubAllGlobals());

describe("browserPasskey", () => {
  it("asks the browser's own API, never an extension's replacement of navigator.credentials", async () => {
    const browser = vi.fn(async () => assertion());
    const extension = vi.fn(async () => assertion());
    page({ extension, browser });
    const out = await browserPasskey().get(
      new Uint8Array([9]),
      "calimero.network",
    );
    expect(out.credentialId).toBe(b64url(new Uint8Array([1, 2, 3])));
    expect(browser).toHaveBeenCalledTimes(1);
    expect(extension).not.toHaveBeenCalled();
  });

  it("passes the browser's refusal through, asked once", async () => {
    const declined = Object.assign(
      new Error("The operation either timed out or was not allowed."),
      { name: "NotAllowedError" },
    );
    const browser = vi.fn(async () => Promise.reject(declined));
    page({ browser });
    await expect(
      browserPasskey().get(new Uint8Array([9]), "calimero.network"),
    ).rejects.toBe(declined);
    expect(browser).toHaveBeenCalledTimes(1);
  });

  it("asks for user verification when told to, and prefers it otherwise", async () => {
    const browser = vi.fn(async (_o: unknown) => assertion());
    page({ browser });
    await browserPasskey().get(new Uint8Array([9]), "calimero.network");
    await browserPasskey().get(
      new Uint8Array([9]),
      "calimero.network",
      ["AQID"],
      {
        userVerification: "required",
      },
    );
    const uv = browser.mock.calls.map(
      ([o]) =>
        (o as { publicKey: { userVerification: string } }).publicKey
          .userVerification,
    );
    expect(uv).toEqual(["preferred", "required"]);
  });

  it("uses navigator.credentials where there is no CredentialsContainer", async () => {
    const get = vi.fn(async () => assertion());
    vi.stubGlobal("CredentialsContainer", undefined);
    vi.stubGlobal("navigator", { credentials: { get } });
    await browserPasskey().get(new Uint8Array([9]), "calimero.network");
    expect(get).toHaveBeenCalledTimes(1);
  });
});

describe("browserPasskey.create", () => {
  it("asks the authenticator for ES256 only, never Ed25519 a password manager cannot sign with later", async () => {
    let asked: Array<{ alg: number }> = [];
    const bytes = new Uint8Array([1]).buffer;
    const made = {
      response: { attestationObject: bytes, clientDataJSON: bytes },
    };
    page({
      browser: async (o) => {
        asked = (
          o as { publicKey: { pubKeyCredParams: Array<{ alg: number }> } }
        ).publicKey.pubKeyCredParams;
        return made;
      },
    });
    await browserPasskey().create({
      challenge: "AA",
      rp: { id: "calimero.network", name: "Calimero" },
      user: { id: "AA", name: "ana", displayName: "ana" },
      pubKeyCredParams: [
        { type: "public-key", alg: -7 },
        { type: "public-key", alg: -8 },
      ],
    });
    expect(asked.map((p) => p.alg)).toEqual([-7]);
  });

  it("keeps what the node offers when ES256 is not among it", () => {
    expect(
      es256First([{ type: "public-key", alg: -8 }]).map((p) => p.alg),
    ).toEqual([-8]);
  });
});

describe("browserPasskey with an AbortSignal", () => {
  /** A browser prompt that stays open until its `signal` aborts, then rejects as WebAuthn does. */
  const openPrompt = (seen: unknown[]) => (o: unknown) => {
    seen.push(o);
    const signal = (o as { signal?: AbortSignal }).signal;
    return new Promise((_, reject) => {
      signal?.addEventListener("abort", () => reject(signal.reason));
    });
  };
  const creation = {
    challenge: "AA",
    rp: { id: "calimero.network", name: "Calimero" },
    user: { id: "AA", name: "ana", displayName: "ana" },
    pubKeyCredParams: [{ type: "public-key" as const, alg: -7 }],
  };

  it("passes the signal to the browser's get, and an abort rejects as cancelled", async () => {
    const seen: unknown[] = [];
    page({ browser: openPrompt(seen) });
    const controller = new AbortController();
    const pending = browserPasskey()
      .get(new Uint8Array([9]), "calimero.network", undefined, {
        signal: controller.signal,
      })
      .catch((e: unknown) => e);
    controller.abort();
    // The source attaches `cause` with Object.assign: the ES2020 lib's Error has none.
    const error = (await pending) as Error & { cause?: unknown };
    expect((seen[0] as { signal?: AbortSignal }).signal).toBe(controller.signal);
    expect(error.name).toBe("NotAllowedError");
    expect(error.message).toBe("passkey sign-in was cancelled");
    expect((error.cause as Error).name).toBe("AbortError");
  });

  it("passes the signal to the browser's create, and an abort rejects as cancelled", async () => {
    const seen: unknown[] = [];
    page({ browser: openPrompt(seen) });
    const controller = new AbortController();
    const pending = browserPasskey()
      .create(creation, { signal: controller.signal })
      .catch((e: unknown) => e);
    controller.abort(new Error("closed by the page"));
    const error = (await pending) as Error;
    expect((seen[0] as { signal?: AbortSignal }).signal).toBe(controller.signal);
    // ES256-first still applies with a signal.
    expect(
      (seen[0] as { publicKey: { pubKeyCredParams: unknown[] } }).publicKey
        .pubKeyCredParams,
    ).toEqual([{ type: "public-key", alg: -7 }]);
    expect(error.name).toBe("NotAllowedError");
    expect(error.message).toBe("passkey creation was cancelled");
  });

  it("maps a bare AbortError to the same cancel, and sends no signal key when none is given", async () => {
    const seen: unknown[] = [];
    page({
      browser: async (o) => {
        seen.push(o);
        throw Object.assign(new Error("aborted"), { name: "AbortError" });
      },
    });
    const error = (await browserPasskey()
      .get(new Uint8Array([9]), "calimero.network")
      .catch((e: unknown) => e)) as Error;
    expect(error.name).toBe("NotAllowedError");
    expect(seen[0]).not.toHaveProperty("signal");
  });
});
