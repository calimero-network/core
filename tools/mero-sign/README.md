# mero-sign

Ed25519 signing for Calimero application bundle manifests.

## About

`mero-sign` is the signing library and CLI for bundle manifests.
It does three things:

- **Sign** a bundle `manifest.json` in place - adds `signerId` and a `signature` over the canonical (RFC 8785 JCS) manifest bytes.
- **Generate** an Ed25519 keypair for production signing.
- **Derive** the `did:key` signerId from a key file.

`mero-sign` does **not** build wasm or package bundles.
Packaging the signed manifest and its component into a `.mpk` archive is done by `scripts/pack-component.py`, which calls this binary to sign the manifest as one step.

## Installation

Prebuilt binaries ship with core's releases (Linux and macOS). To build it:

```bash
# Run from the workspace without installing
cargo run -p mero-sign -- <COMMAND>

# Or install it globally
cargo install --path tools/mero-sign
mero-sign --version
```

## Commands

### `sign` - sign a manifest in place

Adds `signerId` and `signature` fields to an existing `manifest.json` (the one `scripts/pack-component.py` writes).

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
2. Canonicalizes the manifest (RFC 8785 JCS) and computes the signing payload: the SHA-256 of the domain tag `calimero.bundle.manifest.v1` and a NUL byte followed by the canonical bytes.
3. Signs the payload with Ed25519.
4. Writes `signerId` and `signature` back into the manifest on disk.

It does not create or modify any bundle archive.

#### Check the ABI against the last release

```bash
mero-sign sign <MANIFEST_PATH> --key <KEY_FILE> --previous <LAST_RELEASE.mpk>
```

`--previous` (an `.mpk`, a component `.wasm` or an ABI JSON) compares the ABI of every
component the manifest names (read beside it) with the last release's, as
`mero-app-build abi-diff` does, and prints the changes. A change that breaks the app's
callers is a warning; a state field removed or changed in kind while the state version
stayed the same is refused, and nothing is signed. `scripts/pack-component.py --previous`
passes it on.

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

Build the component, then pack and sign it:

```bash
scripts/install-mero-app-build.sh   # once: puts mero-app-build on PATH
mero-app-build apps/my-app --out my-app.wasm
python3 -I scripts/pack-component.py --wasm my-app.wasm --package com.example.my-app \
  --app-version 1.0.0 --out dist/com.example.my-app-1.0.0.mpk --key my-key.json   # or omit --key for --dev
```

## Security notes

1. **Never commit signing keys** to version control.
2. **Use `--dev` only for local development and CI** - it relies on a public, well-known key, so bundles signed with it are refused by the registry, and a node takes them only with `--dev` or `--allow-unsigned-bundles` (`403 DEV_SIGNED_PACKAGE` otherwise).
3. **Generate a unique production key** for anything you publish, and store it outside the repository.
4. **The signer is pinned to the package.** The first release of a package a cluster installs pins its `signerId`; a later release of that package signed with another key is refused, and the organization a key is bound to as publisher (`POST /orgs/{org}/publishers`) approves releases for every instance of the packages it signs. Losing the key means publishing under a new package id.

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

On unix, `sign` refuses a key file that group or others can access. `generate-key` creates the file with mode 0600; for a key written any other way, run `chmod 600 <file>`.
