# Signing Calimero bundles

Every `.mpk` bundle carries an Ed25519 signature over its `manifest.json`.
The signature identifies the publisher and lets a node verify the bundle was not tampered with in transit.
`cargo mero bundle` signs as part of packaging; `cargo mero sign` signs an existing `manifest.json` in place.

This document explains the two kinds of signing key, how a signer becomes a `signerId`, and how the package and signer together fix an app's on-node identity.

## Dev key vs production key

There are two ways to sign, and the choice determines whether the bundle can leave your machine.

**Development key (`--dev`).**
A single well-known key baked into the tool.
Its seed is derived deterministically as `SHA-256("calimero-dev-signing-key-v1")`, so every `--dev` bundle everywhere is signed by the same key and resolves to the same signer:

```
did:key:z6MknF3p5L5FDHJQ7FREUapuX4Wmp4MtF6WrHYaXS2B3eZQd
```

This is the analogue of Android's `debug.keystore`: it needs no key file and is fine for local installs and CI, but because the key is public it proves nothing about provenance.
The registry **refuses** bundles signed with the dev key, and `cargo mero bundle --dev` prints a warning saying so.

**Production key (`--key <file>`).**
A private Ed25519 key that only you hold.
Use it for anything you publish or install against a registry.
Generate one with `cargo mero key generate` (below).

`cargo mero bundle` also accepts `--unsigned`, which skips signing entirely.
An unsigned bundle cannot be published to or installed from a registry.

## Generating and storing a production key

```bash
cargo mero key generate -o my-key.json
```

This writes a JSON key file holding the base64url-encoded Ed25519 private-key seed (plus the public key and the derived `signerId`) and prints the `signerId`:

```
Generated new keypair: my-key.json
  signerId: did:key:z6MkrV2imerTHzYtPyb2groFVNJSokGX7rpxnuJj8DSEQDnH
```

The private key is a secret.
Treat the key file the way you would treat any signing credential:

- **Never commit it.**
  Keep key files out of the repository (the scaffold's `.gitignore` already ignores them; if you store one elsewhere, add its path).
- **In CI, inject it, do not check it in.**
  Point `cargo mero bundle` at a key file through the `MERO_SIGN_KEY` environment variable instead of `--key`.
  When none of `--key` / `--dev` / `--unsigned` is passed, `bundle` reads `MERO_SIGN_KEY` as the path to a key file, so a CI job can materialize the key from a secret at build time and set the variable:

  ```bash
  export MERO_SIGN_KEY="$RUNNER_TEMP/mero-key.json"
  echo "$MERO_SIGN_KEY_JSON" > "$MERO_SIGN_KEY"   # from a CI secret
  cargo mero bundle
  ```

- **Back it up.**
  Losing the key means you can no longer publish updates under the same app identity (see below).

To see the `signerId` for an existing key file without signing anything:

```bash
cargo mero key derive-signer-id -k my-key.json
```

## The signerId (`did:key`)

A signer is identified not by a raw public key but by a `did:key` string derived from it.
The derivation, for an Ed25519 public key, is:

1. Prefix the 32-byte public key with the `ed25519-pub` multicodec indicator (`0xed01`).
2. Encode the result with base58btc.
3. Prepend the multibase `z` marker and the `did:key:` scheme.

The result looks like `did:key:z6Mk...`.
The same public key always yields the same `signerId`, and the node recomputes it from the signature's embedded public key to reject a spoofed `signerId` field.

## How package + signer become the ApplicationId

The node does **not** hash the wasm to identify an app.
It derives the `ApplicationId` from the manifest's `package` string and the bundle's `signerId`.

> Rule, from core `crates/node/primitives/src/client/application/install.rs:194-196`:
>
> ```rust
> let application_id = {
>     let components = (&application.package, &application.signer_id);
>     ApplicationId::from(*Hash::hash_borsh(&components)?)
> };
> ```
>
> `Hash::hash_borsh` is `SHA-256` over the borsh serialization of the value
> (`crates/primitives/src/hash.rs`), so
> **`ApplicationId = SHA-256(borsh((package, signerId)))`** - the wasm bytes and the app version are not part of it.

Two consequences follow directly from this rule:

- **The ApplicationId is version-stable.**
  Publishing a new version of the same app - same `package`, same signing key - produces the *same* `ApplicationId`.
  The node treats the newer bundle as an update to the existing application row rather than a different app.
  (What a given context actually executes is decided by its per-context binding, not by the row alone.)

- **Changing either half forks the identity.**
  A different `package` string, or the same package signed with a *different* key, hashes to a *different* `ApplicationId` - a distinct app as far as every node is concerned.

## What changing the signer means

Because the signer is half of the `ApplicationId`, the signing key is part of your app's identity, not just a provenance stamp.

- Keep signing every release with the **same production key** to keep shipping upgrades under one `ApplicationId`.
- **Do not release with `--dev` and later switch to a production key** (or swap between keys): each signer yields a different `ApplicationId`, so existing installs will not recognize the re-signed bundle as an upgrade - it is a new app that peers must install and adopt from scratch.
- If you lose the production key, you cannot publish an in-place upgrade; a replacement key means a new app identity and a migration for existing users.

Pick the production key before your first published release and guard it accordingly.

## See also

- [README.md](README.md) - the full `cargo mero` workflow and the `.mpk` bundle layout.
- Core documentation: <https://calimero-network.github.io/core/>
