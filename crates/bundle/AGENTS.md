# calimero-bundle - `.mpk` Manifest Types & Signature Canonicalization

The `BundleManifest` schema and the RFC 8785 (JCS) canonicalization / Ed25519 signing and verification logic shared by every producer and consumer of a Calimero app bundle.

## Package Identity

- **Crate**: `calimero-bundle`
- **Entry**: `src/lib.rs` (types), `src/signature.rs` (canonicalization, signing, verification)
- **Key deps**: `serde`/`serde_json` ((de)serialization), `serde_json_canonicalizer` (RFC 8785 JCS), `ed25519-dalek` (sign/verify), `sha2` (bundle hash), `base64`/`bs58`/`hex` (encodings), `eyre` and `thiserror` (errors), `html5ever` (HTML tokenizer for the frontend check)

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
| `BundleManifest` | struct | The full `manifest.json` shape: `version`, `package`, `app_version`, `signer_id`, `min_runtime_version`, `metadata`, `handlers`, `interfaces`, `wasm`/`abi` (single-service) or `services` (multi-service), `frontend`, `links`, `frontend_aliases`, `outbound`, `signature` |
| `BundleManifest::artifacts()` | fn | Every artifact the manifest declares, paired with its field name; a `..`-free destructure, so a new `BundleManifest` field is a compile error here until classified |
| `BundleManifest::wasm_artifacts()` | fn | Iterates wasm artifacts uniformly across single- and multi-service bundles |
| `BundleManifest::service_names()` / `is_multi_service()` | fn | Shape queries over `services` |
| `BundleManifest::to_metadata_json()` | fn | Flattens `package`/`app_version`/`metadata.*`/`links.*`/`frontendAliases`/`outbound` to JSON for on-disk storage, keeping only web URLs (`is_web_url`) and, under `outbound`, only `is_outbound_url` ones |
| `is_outbound_url()` | fn | Whether a URL prefix may be declared under `outbound`: `https://` with a host, no credentials, query or fragment, no whitespace or control characters, at most `MAX_URL` bytes. `cargo mero` refuses a bundle with any other; `to_metadata_json()` drops it |
| `BundleArtifact`, `BundleMetadata`, `BundleInterfaces`, `BundleLinks`, `BundleHandlers`, `BundleSignature`, `BundleService` | structs | The manifest's nested shapes |
| `WasmArtifact` | struct | Borrowed `(name, wasm)` pair, the common type `wasm_artifacts()` returns |
| `BundleFrontend::validate()`, `verify_frontend_file()` | fn | What the node enforces at install and `cargo mero` shares: manifest-level (paths, case-insensitive duplicates, HTML entry, MIME allowlist, `MAX_FRONTEND_BYTES`), then per file size and hash |
| `check_frontend_html()` | fn (feature `html`) | Build-time HTML feedback for `cargo mero` only (UTF-8, no remote or bundle-escaping URL, no inline code); the node never calls it |
| `FrontendError` | enum | Each refusal, with a stable `code()` (`FRONTEND_*`) that `Display` also prints, `FRONTEND_MISSING_FILE` included for the node and CLI to raise; tooling matches the code, never the message |
| `FRONTEND_ROOT` | const | The `frontend/` prefix every frontend archive path starts with |
| `mime_type()` | fn | The `Content-Type` a frontend file is served with; `None` outside the allowlist |
| `MAX_MANIFEST_BYTES` | const | 1 MiB cap the node enforces on `manifest.json`, before any signature check |
| `canonicalize_manifest`, `compute_bundle_hash`, `compute_signing_payload` | fn | RFC 8785 canonical bytes (signature and `_`-prefixed transient fields stripped first), then their SHA-256; the signing payload is that SHA-256 over `MANIFEST_SIGNING_DOMAIN` followed by the canonical bytes |
| `sign_manifest_json`, `verify_manifest_signature`, `verify_ed25519` | fn | Sign/verify a manifest `Value` in place; `ManifestVerification` carries the result |
| `derive_signer_id_did_key`, `decode_public_key`, `decode_signature`, `format_bundle_hash` | fn | `did:key` derivation and base64url/hex codecs |

## Mental Model

This crate is the single source of truth for the bundle schema: `cargo-mero` builds a `BundleManifest` directly (no `..Default::default()`), `mero-sign` re-exports the canonicalization/signing functions for its CLI, and `calimero-node-primitives` (re-exported there as `bundle`) reads and verifies installed bundles against the same type. Because none of the three fill in fields with a wildcard, a new `BundleManifest` field must be handled at every one of those call sites before the workspace compiles - that's deliberate, to keep the node and the tool that builds bundles for it from silently drifting apart.

`handlers` (the deep-link `slug`) is a sibling of `metadata`, not nested inside it, so it never reaches `to_metadata_json()` and never reaches the display metadata an install stores; only `package` + `signerId` decide a bundle's identity (see `ApplicationId::for_bundle` in `calimero-primitives`).

Signing is two SHA-256 hashes over the same canonical bytes (the signing payload one is domain-tagged with `MANIFEST_SIGNING_DOMAIN`, so a signature over the bare hash is refused and a bundle's hash does not depend on the tag): `canonicalize_manifest` clones the manifest, drops `signature` and any `_`-prefixed field (the transient-field convention: `_binary`, `_overwrite`, and any future underscore-prefixed key), then RFC 8785-canonicalizes what's left. `compute_bundle_hash`/`compute_signing_payload` differ only by that tag. `sign_manifest_json` signs that hash with Ed25519 and writes both `signerId` (always overwritten to match the signing key) and `signature` back into the `Value`; `verify_manifest_signature` reverses this and additionally checks the manifest's declared `signerId` matches the one the public key derives to, so a manifest can't claim a different signer than the key that actually signed it.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | `BundleManifest` and its nested types, `MAX_MANIFEST_BYTES`, and the manifest-shape tests |
| `src/frontend.rs` | `BundleFrontend`, `FrontendError` and the shared frontend rules |
| `src/signature.rs` | Canonicalization, hashing, Ed25519 sign/verify, `did:key` derivation, and their tests |

## Invariants and Gotchas

- **No `..` in `BundleManifest::artifacts()`.** Keep the exhaustive destructure; it is the trip-wire that forces every new field through a deliberate classification instead of silently being skipped.
- **`_`-prefixed fields never reach the signed bytes.** `canonicalize_manifest` strips them unconditionally; don't add a real (signed) field with a leading underscore.
- **`outbound` is signed and is the app's half of the outbound gate.** A node reads it from the stored metadata (`calimero_context::outbound::app_outbound`) and lets a run send a request only to a URL under a prefix listed there (same host; the prefix's path, or one below it at a `/`; any query string), and only to a host its operator also allows. A prefix has no query or fragment. Empty is omitted from the manifest, so a bundle without it signs as before.
- **`frontend_aliases` is a sibling of `links`, on purpose.** The registry refuses any `links` value that is not one URL string, and clients iterate `links` as one URL each. The aliases do reach `to_metadata_json()`: a node's sign-in page reads them there to trust a login callback to an app's old origin.
- **A frontend path has one spelling.** `frontend/` then non-empty components of only `A-Za-z0-9._~@+-` (no `.` or `..`), compared ASCII-case-insensitively, so the archive entry, the manifest and a served URL cannot name different files.
- **The node enforces paths, sizes, hashes and the MIME allowlist.** The HTML rules are build-time feedback in `cargo mero`, not a security boundary.
  The serve-time boundary is the per-app origin (`{label}.{base_domain}`, serving only that app's files) plus the CSP below.
- **HTML is checked with the spec tokenizer, never by substring and never with a tree build.** `check_html` reads `html5ever`'s decoded attributes, so entity-encoded schemes are caught; a tree build is quadratic on deep nesting.
  The tokenizer's duplicate-attribute scan is quadratic in one tag's attributes and `check_frontend_html` has no size cap, so it is for trusted build output only.
  Bound the input before running it on anything untrusted.
- **The HTML lint is behind the `html` feature, which only `cargo-mero` enables.**
  `html5ever` is therefore absent from the node's dependency tree, so the node cannot parse HTML.
- **Raw-text elements hide their content, except after an `svg`, `math` or `select`.** There everything is scanned as markup, and any tag or comment inside a raw-text element (or any CDATA comment) is refused, so a browser's text reading and ours cannot disagree.
- **The static check mirrors the serving contract.** Each app is served from its own origin `{label}.{base_domain}` under `/{version}/`, so a root-absolute URL resolves outside that version's folder.
  Root-absolute or climbing resource URLs and any `<base>` (`FRONTEND_ESCAPING_URL`), remote resource URLs (`FRONTEND_REMOTE_RESOURCE`), and inline code including `srcdoc` (`FRONTEND_INLINE_SCRIPT`) are refused.
  SVG files are left to the CSP.
- **The server must send this CSP with the frontend.** `default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; img-src 'self' data: blob:; font-src 'self' data:; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'self'`.
  `'wasm-unsafe-eval'` lets an app compile WebAssembly (the SDK's WASM client) and does not allow `eval` or inline script.
  Loosening it without loosening `check_html` refuses bundles that would work, and tightening it without `check_html` ships pages that break.
- **`handlers` is a sibling of `metadata`, on purpose.** Keep it out of `to_metadata_json()` - see `handlers_does_not_affect_metadata` in `lib.rs`.

Part of [crates/](../AGENTS.md).
