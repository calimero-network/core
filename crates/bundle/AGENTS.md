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
| `BundleManifest` | struct | The full `manifest.json` shape: `version`, `package`, `app_version`, `signer_id`, `min_runtime_version`, `upgrades`, `metadata`, `handlers`, `interfaces`, `wasm`/`abi`/`compute`/`pipelines` (single-service) or `services` (multi-service), `frontend`, `links`, `frontend_aliases`, `outbound`, `uses`, `instance_org`, `guests`, `signature` |
| `BundleManifest::artifacts()` | fn | Every artifact the manifest declares, paired with its field name; a `..`-free destructure, so a new `BundleManifest` field is a compile error here until classified |
| `BundleManifest::wasm_artifacts()` | fn | Iterates wasm artifacts, each with its compute component, uniformly across single- and multi-service bundles |
| `BundleManifest::service_names()` / `is_multi_service()` | fn | Shape queries over `services` |
| `BundleManifest::to_metadata_json()` | fn | Flattens `package`/`app_version`/`metadata.*`/`links.*`/`frontendAliases`/`outbound` to JSON, keeping only web URLs (`is_web_url`) and, under `outbound`, only `is_outbound_url` ones. The context-era node stored it; nothing in the workspace calls it now |
| `InstanceOrg` | enum | The manifest's `instanceOrg`: `standalone` (an organization made with each instance), `org` (one its creator is in) or `either` (default, omitted); the node keeps it on `PackageVersion::instance_org` and places an instance by it. An older node ignores the field |
| `GuestPolicy` | enum | The manifest's `guests`: `allowed`, or `forbidden` (default, omitted); the node keeps it on `PackageVersion::guests` and admits an instance's listed outsiders only when it says `allowed` and the organization allows guests. An older node ignores the field |
| `UpgradeAuthority` | enum | The manifest's `upgrades`: `owner` (default, omitted) or `publisher`; a later version may only move toward `owner` (the node keeps it on `PackageVersion::upgrades`) |
| `VerifiedBundle` (`archive.rs`) | struct | An `.mpk` whose manifest signature verified (`open(data, runtime_version)`, which also refuses a `minRuntimeVersion` above it); every artifact accessor (`wasm`, `contents` → `VerifiedContents`/`VerifiedWasm`) checks the digest first, so unverified bytes have no representation. `manifest_hash`, `signer_id` |
| `extract_bundle_manifest`, `is_bundle_blob`, `validate_artifact_path`, `MAX_ARCHIVE_BYTES` (`archive.rs`) | fn/const | Reading the archive: bounded, no path traversal; `fuzz_api` for `fuzz/fuzz_targets/bundle_open.rs` |
| `BundleSecret`, `SecretPlacement`, `BundleManifest::check_secrets()`, `is_secret_name()`, `is_secret_header()`, `is_secret_format()`, `is_secret_body_field()`, `is_forbidden_header()`, `outbound_host()`, `SecretsError`, `MAX_SECRETS` (`secrets.rs`) | struct/fn | The manifest's `secrets`, credential bindings: name -> `{ host, paths, header + format? \| bodyField, description, required }`, signed. Each is one host (an `outbound` prefix's), 1 to `MAX_SECRET_PATHS` (8) path prefixes there each under an `outbound` prefix (`path_covers`: whole segments; `["/"]` for the whole host, said explicitly) and one place, where `calimero-outbound` attaches it; the app never places or reads it. `check_secrets` is what a node enforces at install and `meroctl app pack` checks first: names `A-Z0-9_` starting with a letter, at most `MAX_SECRETS` (16), exactly one of a lowercase header that is not `FORBIDDEN_HEADERS`/`proxy-`/`content-type` and a body field (`A-Za-z0-9_.-`), each at most `MAX_SECRET_FIELD_BYTES` (64), a `format` with `{}` once and no other brace (`MAX_SECRET_FORMAT_BYTES`, 64), paths well formed (`is_secret_path`: `/`, printable ASCII, no `?`/`#`/`%`/space, no `.`/`..` segment, `MAX_SECRET_PATH_BYTES`), no two on one host's same place on a shared path, a description of at most 256 bytes. `describe` words a binding for the consent page and the classifier; `narrows` (fewer paths, same host and place) keeps an update green. The shape replaced `{ hosts }` and `{{secret:NAME}}` placeholders in 1.0.0-rc.36, a hard cutover: a manifest in the earlier shape does not parse (`deny_unknown_fields`) |
| `BundleUse`, `UseMode`, `BundleManifest::check_uses()`, `is_use_name()`, `UsesError`, `MAX_USES` | struct/enum/fn | The manifest's `uses`: name -> `{ package, range, mode: create\|attach, version?, abi? }`, signed. `check_uses` is what a node enforces at install and `meroctl app pack` checks first: names `a-z0-9_` starting with a letter, at most `MAX_USES` (16), not the release's own package, a semver range, a locked version inside it, an ABI digest `sha256:<64 hex>`. An older node ignores the field |
| `update::{classify, Release, Unit, Classification, Class, Reason, EgressDiff, covers}` (`update.rs`) | mod | The update classifier the node and `meroctl` share: a unit's releases (`Release::new` from a manifest, its signer and embedded ABI) before and after give `green` or `red` with `reasons` (signer changed, egress added, state version changed, a member with egress added or removed, a new method called as the person, a secret added or bound elsewhere (`secretBindingChanged`); `unclassified`, a release no longer installed, is the node's to say, never `classify`'s), the egress diff (`before`/`after`/`added`/`removed`; an empty list means no data leaves Calimero, and an external endpoint, a TEE included, is just a destination) and new/removed methods. Pure; the rules are in the module docs |
| `is_outbound_url()` | fn | Whether a URL prefix may be declared under `outbound`: `https://` with a host, no credentials, query or fragment, no whitespace or control characters, at most `MAX_URL` bytes. `calimero-outbound` (`merge_approval`) refuses an approval of any other; `to_metadata_json()` drops it |
| `BundleArtifact`, `BundleMetadata`, `BundleInterfaces`, `BundleLinks`, `BundleHandlers`, `BundleSignature`, `BundleService` | structs | The manifest's nested shapes |
| `WasmArtifact` | struct | Borrowed `(name, wasm)` pair, the common type `wasm_artifacts()` returns |
| `BundleFrontend::validate()`, `verify_frontend_file()` | fn | What the node enforces at install: manifest-level (paths, case-insensitive duplicates, HTML entry, MIME allowlist, `MAX_FRONTEND_BYTES`), then per file size and hash |
| `check_frontend_html()` | fn (feature `html`) | Build-time HTML feedback (UTF-8, no remote or bundle-escaping URL, no inline code) for a bundling tool; the node never calls it, `meroctl app pack` does |
| `FrontendError` | enum | Each refusal, with a stable `code()` (`FRONTEND_*`) that `Display` also prints, `FRONTEND_MISSING_FILE` included for the node and CLI to raise; tooling matches the code, never the message |
| `FRONTEND_ROOT` | const | The `frontend/` prefix every frontend archive path starts with |
| `mime_type()` | fn | The `Content-Type` a frontend file is served with; `None` outside the allowlist |
| `MAX_MANIFEST_BYTES` | const | 1 MiB cap the node enforces on `manifest.json`, before any signature check |
| `canonicalize_manifest`, `compute_bundle_hash`, `compute_signing_payload` | fn | RFC 8785 canonical bytes (signature and `_`-prefixed transient fields stripped first), then their SHA-256; the signing payload is that SHA-256 over `MANIFEST_SIGNING_DOMAIN` followed by the canonical bytes |
| `sign_manifest_json`, `verify_manifest_signature`, `verify_ed25519` | fn | Sign/verify a manifest `Value` in place; `ManifestVerification` carries the result |
| `DEV_SIGNER_ID` | const | The signerId of the development key (`meroctl app sign --dev`, `meroctl app pack` without `--key`); a node without `--allow-unsigned-bundles` (on with `--dev`) refuses releases it signed |
| `derive_signer_id_did_key`, `decode_public_key`, `decode_signature`, `format_bundle_hash` | fn | `did:key` derivation and base64url/hex codecs |

## Mental Model

This crate is the single source of truth for the bundle schema: `meroctl-sign` re-exports the canonicalization/signing functions for `meroctl`, and `calimero-node` reads and verifies installed bundles against the same type (the node's tests build a `BundleManifest` directly, with no `..Default::default()`, so a new field must be handled there before the workspace compiles). `meroctl app pack` (`crates/meroctl/src/make.rs`) writes the same manifest as JSON.

`handlers` (the deep-link `slug`) is a sibling of `metadata`, not nested inside it, so it never reaches `to_metadata_json()` and never reaches the display metadata an install stores; only `package` + `signerId` decide a bundle's identity (see `ApplicationId::for_bundle` in `calimero-primitives`).

Signing is two SHA-256 hashes over the same canonical bytes (the signing payload one is domain-tagged with `MANIFEST_SIGNING_DOMAIN`, so a signature over the bare hash is refused and a bundle's hash does not depend on the tag): `canonicalize_manifest` clones the manifest, drops `signature` and any `_`-prefixed field (the transient-field convention: `_binary`, `_overwrite`, and any future underscore-prefixed key), then RFC 8785-canonicalizes what's left. `compute_bundle_hash`/`compute_signing_payload` differ only by that tag. `sign_manifest_json` signs that hash with Ed25519 and writes both `signerId` (always overwritten to match the signing key) and `signature` back into the `Value`; `verify_manifest_signature` reverses this and additionally checks the manifest's declared `signerId` matches the one the public key derives to, so a manifest can't claim a different signer than the key that actually signed it.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | `BundleManifest` and its nested types, `UpgradeAuthority`, `MAX_MANIFEST_BYTES`, and the manifest-shape tests |
| `src/archive.rs` | Reading an `.mpk`: `VerifiedBundle`, `extract_bundle_manifest`, path checks; tests in `src/archive/` (compute, pipelines, sparse entries) |
| `src/html.rs` | `check_frontend_html` (feature `html`) |
| `src/frontend.rs` | `BundleFrontend`, `FrontendError` and the shared frontend rules |
| `src/signature.rs` | Canonicalization, hashing, Ed25519 sign/verify, `did:key` derivation, and their tests |
| `src/secrets.rs` | `BundleSecret` (credential bindings), their checks, limits and words |
| `src/update.rs` | The update classifier (green/red over a unit's egress, signer, state versions) and its tests |

## Invariants and Gotchas

- **No `..` in `BundleManifest::artifacts()`.** Keep the exhaustive destructure; it is the trip-wire that forces every new field through a deliberate classification instead of silently being skipped.
- **A service's compute component is an artifact like the rest.**
  `BundleManifest::compute` and `BundleService::compute` (`Option<BundleArtifact>`) are signed, listed by `artifacts()` and digest-checked by `VerifiedBundle::contents` (`VerifiedWasm::compute`).
  An older node ignores the field, and a manifest without it verifies as before.
  What the compute component may contain is the node's and the engine's rule, not this crate's.
- **Pipelines are artifacts too.**
  `BundleManifest::pipelines` and `BundleService::pipelines` list YAML files, signed, in `artifacts()` and digest-checked by `contents()` (`VerifiedWasm::pipelines`, by archive path), each read bounded by its declared size.
  At most `MAX_PIPELINES_PER_PACKAGE` (16) per package and `MAX_PIPELINE_BYTES` (64 KiB) each, refused when the manifest is read.
  What a pipeline may say is the pipeline crate's and the node's rule, not this crate's.
- **`_`-prefixed fields never reach the signed bytes.** `canonicalize_manifest` strips them unconditionally; don't add a real (signed) field with a leading underscore.
- **`outbound` is signed and is the app's half of the outbound gate.** A node keeps it on the release record at install (`PackageVersion::outbound`) and hands it to `calimero-outbound` (`Node::outbound_for`), which lets a run send a request only to a URL under a prefix listed there (same host; the prefix's path, or one below it at a `/`; any query string), and only to a host its operator also allows. A prefix has no query or fragment. Empty is omitted from the manifest, so a bundle without it signs as before.
- **`frontend_aliases` is a sibling of `links`, on purpose.** The registry refuses any `links` value that is not one URL string, and clients iterate `links` as one URL each. The node's consent page reads them from the verified manifest: with the frontend link they are the origins a sign-in callback may land on (`login_origins` in `consent_page/diff.rs`), and an upgrade's page shows a change to them.
- **A frontend path has one spelling.** `frontend/` then non-empty components of only `A-Za-z0-9._~@+-` (no `.` or `..`), compared ASCII-case-insensitively, so the archive entry, the manifest and a served URL cannot name different files.
- **The node enforces paths, sizes, hashes and the MIME allowlist.** The HTML rules are build-time feedback, not a security boundary.
  The serve-time boundary is the per-app origin (`{label}.{base_domain}`, serving only that app's files) plus the CSP below.
- **HTML is checked with the spec tokenizer, never by substring and never with a tree build.** `check_html` reads `html5ever`'s decoded attributes, so entity-encoded schemes are caught; a tree build is quadratic on deep nesting.
  The tokenizer's duplicate-attribute scan is quadratic in one tag's attributes and `check_frontend_html` has no size cap, so it is for trusted build output only.
  Bound the input before running it on anything untrusted.
- **The HTML lint is behind the `html` feature, which only `meroctl` enables.**
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
