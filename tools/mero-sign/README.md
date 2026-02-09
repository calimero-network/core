# mero-sign

CLI tool for signing Calimero application bundle manifests with Ed25519 keys.

## About

`mero-sign` is a cryptographic signing tool used in the Calimero application build pipeline. It signs bundle manifests with Ed25519 keys and generates `.mpk` (Mero PacKage) files that contain your application's WASM binary, ABI, and cryptographically signed metadata.

**Purpose:**
- Sign application manifests for verification and provenance
- Generate Ed25519 keypairs for signing
- Create `.mpk` bundle files ready for deployment
- Derive DID (Decentralized Identifier) keys in `did:key` format

**Why signing?** Signed manifests ensure application integrity and provide a chain of trust when deploying to Calimero nodes. The signature verifies that the application bundle hasn't been tampered with and identifies the publisher.

## Installation

```bash
# Run from workspace (no installation needed)
$: cargo run -p mero-sign -- <COMMAND>

# Install globally (accessible system-wide)
$: cd tools/mero-sign
$: cargo install --path .
> ...
>     Installed package `mero-sign v0.1.0 (/path/to/core/tools/mero-sign)` (executable `mero-sign`)
# Or from workspace root:
$: cargo install --path tools/mero-sign
> ...
> Installed package `mero-sign v0.1.0 (/path/to/core/tools/mero-sign)` (executable `mero-sign`)

# Verify installation
$: mero-sign --version
> mero-sign 0.1.0
```

After global installation, `mero-sign` is available as a system command from any directory.

## Bundle Structure

A Calimero application bundle (`.mpk` file) contains:

```
bundle-temp/
├── app.wasm          # Compiled WASM binary
├── abi.json          # Application Binary Interface
├── state-schema.json # State structure definition (optional)
└── manifest.json     # Signed metadata
```

After signing, these files are packaged into a single `.mpk` file (e.g., `kv-store-1.0.0.mpk`).

## Workflow

### 1. Generate a Signing Key (One-time)

```bash
# Generate new Ed25519 keypair
$: mero-sign generate-key --output my-signing-key.json
> Generated new keypair: my-signing-key.json
>   signerId: did:key:z6Mkrb81NKkv7Mw4dZAWA5PTuwBhbq9u9eCPuby3icBUdirg
```

**Security:** Store your signing key securely. Do NOT commit it to version control. Add `*.json` key files to `.gitignore`.

### 2. Build Your WASM Application

```bash
# Example: kv-store app
$: cd apps/kv-store
$: ./build.sh
> ...
> Compiling proc-macro2 v1.0.102
> ...
>  Finished `app-release` profile [optimized] target(s) in 19.47s
```

### 3. Sign the Manifest and Create Bundle

```bash
# Sign manifest in-place and package into .mpk
$: mero-sign sign res/bundle-temp/manifest.json \
  --key ../../scripts/test-signing-key/test-key.json
> Signed manifest: res/bundle-temp/manifest.json
>    signerId: did:key:z6Mkm9KCceaDHiwAuYM7y3HteaCHSEkPzACySDkqkXTK6nWd
>  Bundle created: res/kv-store-1.0.0.mpk
```

### 4. Deploy Bundle (using meroctl)

```bash
# Install application
$: meroctl --node node1 app install \
  --path res/kv-store-1.0.0.mpk \
  --package com.calimero.kv-store \
  --version 1.0.0
> ╭───────────────────────────────────────────────────────────────────────────────────╮
> │ Application Installed                                                             │
> ╞═══════════════════════════════════════════════════════════════════════════════════╡
> │ Successfully installed application '8CtFJJ8GohJLhatFZwfHN8ccyWuUcCTHnDHeZiA2xqHn' │
> ╰───────────────────────────────────────────────────────────────────────────────────╯
# Create context
$: meroctl --node node1 context create --application-id <app_id> --protocol <protocol>

$: meroctl --node node1 context create --application-id 8CtFJJ8GohJLhatFZwfHN8ccyWuUcCTHnDHeZiA2xqHn --protocol near
> +------------------------------+
> | Context Created              |
> +==============================+
> | Successfully created context |
> +------------------------------+

```

See [meroctl README](../../crates/meroctl/README.md) for deployment details.

## Commands

### `sign` - Sign manifest and create MPK

Signs a `manifest.json` file in-place and packages the bundle directory into an `.mpk` file.

```bash
$: mero-sign sign <MANIFEST_PATH> --key <KEY_FILE>

# Example:
$: mero-sign sign apps/kv-store/res/bundle-temp/manifest.json \
  --key scripts/test-signing-key/test-key.json
> Signed manifest: apps/kv-store/res/bundle-temp/manifest.json
>    signerId: did:key:z6Mkm9KCceaDHiwAuYM7y3HteaCHSEkPzACySDkqkXTK6nWd
>  Bundle created: res/kv-store-1.0.0.mpk
```

**What it does:**
1. Reads the manifest file
2. Creates a canonical signature using Ed25519
3. Adds `signature` and `signerId` fields to manifest
4. Writes signed manifest back to disk
5. Packages `bundle-temp/` directory into `.mpk` file

**Arguments:**
- `<MANIFEST_PATH>`: Path to `manifest.json` file
- `--key <PATH>`: Path to Ed25519 private key (JSON format)

### `generate-key` - Generate Ed25519 keypair

Creates a new Ed25519 keypair for signing bundles.

```bash
$: mero-sign generate-key --output <OUTPUT_PATH>

# Example:
$: mero-sign generate-key --output my-key.json
> Generated new keypair: my-key.json
>   signerId: did:key:z6MkuWd7fnCXYaiLNTwf7v9kdyLJjUsRvMAcH7VwvtGNY38N
```

**Output format** (JSON):
```json
{
  "privateKey": "base64-encoded-private-key",
  "publicKey": "base64-encoded-public-key"
}
```

**Security:** The generated key should be kept secret. Use it only for signing your own applications.

### `derive-signer-id` - Get DID from key

Derives the `did:key` identifier from an Ed25519 keypair file.

```bash
$: mero-sign derive-signer-id --key <KEY_FILE>

# Example:
$: mero-sign derive-signer-id --key my-key.json
> Output: did:key:z6Mkrb81NKkv7Mw4dZAWA5PTuwBhbq9u9eCPuby3icBUdirg
```

Use this to get your signer ID before signing, or to verify which key signed a manifest.

## Complete Example: Build and Sign kv-store

```bash
# 1. Build WASM binary
$: cd apps/kv-store
$: ./build.sh

# 2. Generate ABI and bundle (typically done by build-bundle.sh)
$: cargo run -p calimero-abi-cli -- generate \
  --input src/lib.rs \
  --output res/bundle-temp/abi.json

# 3. Create manifest.json (example structure)
$: cat > res/bundle-temp/manifest.json <<EOF
{
  "name": "kv-store",
  "version": "1.0.0",
  "description": "Key-value storage application",
  "repository": "https://github.com/calimero-network/core",
  "author": "Calimero Network"
}
EOF

# 4. Sign and package
$: mero-sign sign res/bundle-temp/manifest.json \
  --key ../../scripts/test-signing-key/test-key.json

# Result: res/kv-store-1.0.0.mpk created
```

Or simply run the provided build script:

```bash
$: ./build-bundle.sh
```

## Integration with Build Scripts

Most applications include a `build-bundle.sh` script that automates the entire process:

1. **`build.rs`** - Cargo build script (generates ABI at compile time)
2. **`build.sh`** - Shell script to build WASM with optimizations
3. **`build-bundle.sh`** - Complete bundle creation pipeline:
   - Builds WASM binary
   - Generates ABI
   - Creates state schema
   - **Signs manifest** (calls `mero-sign`)
   - Packages into `.mpk` file

Example: See [`apps/kv-store/build-bundle.sh`](../../apps/kv-store/build-bundle.sh) for a reference implementation.

## Security Best Practices

1. **Never commit signing keys** to version control
2. **Use test keys for development** (see `scripts/test-signing-key/`)
3. **Generate unique keys for production** applications
4. **Store keys securely** outside the repository for production
5. **Verify signer IDs** after signing to ensure correctness

## Key File Format

Ed25519 keypair stored as JSON:

```json
{
  "privateKey": "base64-encoded-32-byte-private-key",
  "publicKey": "base64-encoded-32-byte-public-key"
}
```

The public key is used to derive the `did:key` identifier in multibase format.

## Troubleshooting

**Command not found after global install:**
```bash
# Ensure cargo bin directory is in PATH
export PATH="$HOME/.cargo/bin:$PATH"

# Or reinstall
cargo install --path tools/mero-sign --force
```

**"Invalid key format" error:**
- Ensure key file is valid JSON with `privateKey` and `publicKey` fields
- Keys must be base64-encoded 32-byte values
- Use `generate-key` to create a properly formatted key

**"Manifest not found" error:**
- Ensure `manifest.json` exists at the specified path
- The manifest must be in a `bundle-temp/` directory alongside `app.wasm` and `abi.json`

## Related Tools

- **[meroctl](../../crates/meroctl/)** - CLI for deploying signed bundles to Calimero nodes
- **[calimero-abi-cli](../calimero-abi/)** - Generate ABI from Rust source code
- **[merod](../../crates/merod/)** - Calimero node daemon

## See Also

- [Application build examples](../../apps/) - Reference implementations for various app types
- [Test signing key](../../scripts/test-signing-key/README.md) - Pre-generated key for development
