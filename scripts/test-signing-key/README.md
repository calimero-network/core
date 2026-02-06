# Test Signing Key

This directory contains a **TEST-ONLY** Ed25519 signing key for development and CI purposes.

## ⚠️ WARNING

**DO NOT USE THIS KEY IN PRODUCTION!**

This key is:

- Committed to the repository and publicly visible
- Intended only for testing bundle signing in development/CI environments
- Not suitable for any production or security-sensitive use cases

## Key Details

- **Algorithm**: Ed25519
- **SignerId**: `did:key:z6MktDyUgjyGaEMxMyuZMs2v2L46zvVKNqB5K3KTvFxudtKL`

## File Format

The `test-key.json` file contains:

```json
{
  "private_key": "<base64url-encoded 32-byte seed>",
  "public_key": "<base64url-encoded 32-byte public key>",
  "signer_id": "did:key:z6Mk..."
}
```

## Usage

The key is used by `build-bundle.sh` scripts to sign manifests:

```bash
cargo run -p mero-sign --quiet -- sign manifest.json \
    --key ../../scripts/test-signing-key/test-key.json
```

## Generating a New Key

If you need to generate a new test key (e.g., for a fresh test environment):

```bash
cargo run -p mero-sign -- generate-key --output scripts/test-signing-key/test-key.json
```

## Deriving SignerId

To get the signerId from a key file:

```bash
cargo run -p mero-sign -- derive-signer-id --key scripts/test-signing-key/test-key.json
```
