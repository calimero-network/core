# calimero-bundle - `.mpk` Manifest Types & Signature Canonicalization

The `BundleManifest` schema and the RFC 8785 (JCS) canonicalization / Ed25519 signing and verification logic shared by every producer and consumer of a Calimero app bundle.

## Package Identity

- **Crate**: `calimero-bundle`
- **Entry**: `src/lib.rs` (types), `src/signature.rs` (canonicalization, signing, verification)
- **Key deps**: `serde`/`serde_json` ((de)serialization), `serde_json_canonicalizer` (RFC 8785 JCS), `ed25519-dalek` (sign/verify), `sha2` (bundle hash), `base64`/`bs58`/`hex` (encodings), `eyre` (errors)

## Commands

```bash
# Build
cargo build -p calimero-bundle

# Test
cargo test -p calimero-bundle
```

## Public API

| Item | Kind | Purpose |
| --- | --- | --- |
| `BundleManifest` | struct | The full `manifest.json` shape: `version`, `package`, `app_version`, `signer_id`, `min_runtime_version`, `metadata`, `handlers`, `interfaces`, `wasm`/`abi` (single-service) or `services` (multi-service), `links`, `signature` |
| `BundleManifest::artifacts()` | fn | Every artifact the manifest declares, paired with its field name; a `..`-free destructure, so a new `BundleManifest` field is a compile error here until classified |
| `BundleManifest::wasm_artifacts()` | fn | Iterates wasm artifacts uniformly across single- and multi-service bundles |
| `BundleManifest::service_names()` / `is_multi_service()` | fn | Shape queries over `services` |
| `BundleManifest::to_metadata_json()` | fn | Flattens `package`/`app_version`/`metadata.*`/`links.*` to JSON for on-disk storage |
| `BundleArtifact`, `BundleMetadata`, `BundleInterfaces`, `BundleLinks`, `BundleHandlers`, `BundleSignature`, `BundleService` | structs | The manifest's nested shapes |
| `WasmArtifact` | struct | Borrowed `(name, wasm)` pair, the common type `wasm_artifacts()` returns |
| `MAX_MANIFEST_BYTES` | const | 1 MiB cap the node enforces on `manifest.json`, before any signature check |
| `canonicalize_manifest`, `compute_bundle_hash`, `compute_signing_payload` | fn | RFC 8785 canonical bytes (signature and `_`-prefixed transient fields stripped first), then their SHA-256 |
| `sign_manifest_json`, `verify_manifest_signature`, `verify_ed25519` | fn | Sign/verify a manifest `Value` in place; `ManifestVerification` carries the result |
| `derive_signer_id_did_key`, `decode_public_key`, `decode_signature`, `format_bundle_hash` | fn | `did:key` derivation and base64url/hex codecs |

## Mental Model

This crate is the single source of truth for the bundle schema: `cargo-mero` builds a `BundleManifest` directly (no `..Default::default()`), `mero-sign` re-exports the canonicalization/signing functions for its CLI, and `calimero-node-primitives` (re-exported there as `bundle`) reads and verifies installed bundles against the same type. Because none of the three fill in fields with a wildcard, a new `BundleManifest` field must be handled at every one of those call sites before the workspace compiles - that's deliberate, to keep the node and the tool that builds bundles for it from silently drifting apart.

`handlers` (the deep-link `slug`) is a sibling of `metadata`, not nested inside it, so it never reaches `to_metadata_json()` and never reaches the display metadata an install stores; only `package` + `signerId` decide a bundle's identity (see `ApplicationId::for_bundle` in `calimero-primitives`).

Signing is two SHA-256 hashes over the same canonical bytes: `canonicalize_manifest` clones the manifest, drops `signature` and any `_`-prefixed field (the transient-field convention: `_binary`, `_overwrite`, and any future underscore-prefixed key), then RFC 8785-canonicalizes what's left. `compute_bundle_hash`/`compute_signing_payload` are the same SHA-256 in v0 - kept as two names because a future manifest version may split them. `sign_manifest_json` signs that hash with Ed25519 and writes both `signerId` (always overwritten to match the signing key) and `signature` back into the `Value`; `verify_manifest_signature` reverses this and additionally checks the manifest's declared `signerId` matches the one the public key derives to, so a manifest can't claim a different signer than the key that actually signed it.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | `BundleManifest` and its nested types, `MAX_MANIFEST_BYTES`, and the manifest-shape tests |
| `src/signature.rs` | Canonicalization, hashing, Ed25519 sign/verify, `did:key` derivation, and their tests |

## Invariants and Gotchas

- **No `..` in `BundleManifest::artifacts()`.** Keep the exhaustive destructure; it is the trip-wire that forces every new field through a deliberate classification instead of silently being skipped.
- **`_`-prefixed fields never reach the signed bytes.** `canonicalize_manifest` strips them unconditionally; don't add a real (signed) field with a leading underscore.
- **`handlers` is a sibling of `metadata`, on purpose.** Keep it out of `to_metadata_json()` - see `handlers_does_not_affect_metadata` in `lib.rs`.

Part of [crates/](../AGENTS.md).
