# mero-sign

Ed25519 signing for Calimero application bundle manifests.

## About

`mero-sign` is the signing library and CLI behind `cargo mero key` and `cargo mero sign`.
It does three things:

- **Sign** a bundle `manifest.json` in place - adds `signerId` and a `signature` over the canonical (RFC 8785 JCS) manifest bytes.
- **Generate** an Ed25519 keypair for production signing.
- **Derive** the `did:key` signerId from a key file.

`mero-sign` does **not** build wasm or package bundles.
Packaging the signed manifest and its artifacts into a `.mpk` archive is done by `cargo mero bundle`, which calls this crate to sign the manifest as one step of the pipeline.
See [SIGNING.md](../cargo-mero/SIGNING.md) for the full signing model (dev vs production keys, `MERO_SIGN_KEY`, and how package + signer derive the `ApplicationId`).

## Installation

Most users get this through the `cargo mero` CLI and never install it directly.
To build the standalone binary:

```bash
# Run from the workspace without installing
cargo run -p mero-sign -- <COMMAND>

# Or install it globally
cargo install --path tools/mero-sign
mero-sign --version
```

## Commands

### `sign` - sign a manifest in place

Adds `signerId` and `signature` fields to an existing `manifest.json` (as produced by `cargo mero bundle`'s staging step).

```bash
# With a production key file:
mero-sign sign <MANIFEST_PATH> --key <KEY_FILE>

# Or with the well-known development key (no key file required):
mero-sign sign <MANIFEST_PATH> --dev
```

```
⚠  Signed with DEVELOPMENT key. This bundle cannot be published to the registry.
   signerId: did:key:z6MknF3p5L5FDHJQ7FREUapuX4Wmp4MtF6WrHYaXS2B3eZQd
```

What it does:

1. Reads the manifest file.
2. Canonicalizes the manifest (RFC 8785 JCS) and computes the SHA-256 signing payload.
3. Signs the payload with Ed25519.
4. Writes `signerId` and `signature` back into the manifest on disk.

It does not create or modify any bundle archive.

### `generate-key` - create an Ed25519 keypair

```bash
mero-sign generate-key --output my-key.json
```

```
Generated new keypair: my-key.json
  signerId: did:key:z6MkrV2imerTHzYtPyb2groFVNJSokGX7rpxnuJj8DSEQDnH
```

The output is a JSON key file holding the base64url-encoded Ed25519 private-key seed, its public key, and the derived signerId.
Keep it secret and never commit it (see the security notes below).

### `derive-signer-id` - read the signerId from a key file

```bash
mero-sign derive-signer-id --key my-key.json
```

```
did:key:z6MkrV2imerTHzYtPyb2groFVNJSokGX7rpxnuJj8DSEQDnH
```

Use this to check which signer a key belongs to before signing.

## Where signing fits

The normal path is not to call `mero-sign` by hand.
`cargo mero bundle` builds the app, stages the wasm/abi, writes `manifest.json`, signs it (via this crate), and packages the `.mpk`:

```bash
cargo mero bundle --key my-key.json     # production
cargo mero bundle --dev                  # local, not publishable
```

Reach for the `mero-sign` binary directly only to re-sign an existing `manifest.json`, or to generate and inspect keys outside a build.

## Security notes

1. **Never commit signing keys** to version control.
2. **Use `--dev` only for local development and CI** - it relies on a public, well-known key, so bundles signed with it are refused by the registry.
3. **Generate a unique production key** for anything you publish, and store it outside the repository.
4. **The signer is part of the app identity.** The `ApplicationId` is derived from `(package, signerId)`, so changing the signing key forks the app for every node. See [SIGNING.md](../cargo-mero/SIGNING.md).

## Key file format

Ed25519 keypair stored as JSON, produced by `generate-key`:

```json
{
  "private_key": "base64url-encoded-32-byte-private-key-seed",
  "public_key": "base64url-encoded-32-byte-public-key",
  "signer_id": "did:key:z6Mk..."
}
```

The public key is used to derive the `did:key` signerId in multibase base58btc form.
