# Embed ABI in wasm + L1 identity-downgrade gate — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make each app's state schema travel inside its wasm (tamper-evident, bound to `blob_id`), and add an emitter-side core gate that refuses an identity-stripping migration upgrade (`AuthoredMap → UnorderedMap`, etc.).

**Architecture:** A build step embeds `state-schema.json` as a `calimero_abi_v1` wasm custom section. A `calimero-wasm-abi` lib reads that section back and computes identity downgrades. At upgrade emit (`validate_upgrade` + `dispatch_cascade`), core reads the old + new app's embedded schema and rejects a downgrade (fail-open with a warning when a schema is absent).

**Tech Stack:** Rust (`calimero-wasm-abi`, `tools/calimero-abi`, `crates/context`), `wasmparser`/`wasm-encoder` for section I/O, eyre errors, merobox YAML for e2e.

**Companion spec:** `docs/superpowers/specs/2026-06-03-embed-abi-l1-downgrade-gate-design.md`

**Standing constraints:** TDD (failing test first, watch it fail). Keep this plan + the spec OUT of the feature PR (local `docs/superpowers/**` only). Describe the chosen approach singularly. Open ready-for-review. End commit messages with `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Branch off latest `master`. Land an approach comment on #2587 for format sign-off before the node-side work.

---

## File Structure

**`calimero-wasm-abi` (`crates/wasm-abi/`)** — the reusable core:
- `src/embed.rs` (new): `read_embedded_state_schema(&[u8]) -> Option<Manifest>`, `write_embedded_state_schema(&[u8], &Manifest) -> Vec<u8>` (append/replace the `calimero_abi_v1` section).
- `src/downgrade.rs` (new): `IdentityDowngrade`, `identity_downgrades(&Manifest, &Manifest) -> Vec<IdentityDowngrade>`.
- `src/lib.rs` (modify): `pub mod embed; pub mod downgrade;`.
- `Cargo.toml` (modify): add `wasm-encoder` + `wasmparser` if absent.

**`tools/calimero-abi`** — the CLI surface:
- `src/embed.rs` (new): `run_embed(wasm: &Path, schema: &Path) -> eyre::Result<()>` thin wrapper over the lib.
- `src/main.rs` (modify): register `Embed { wasm, schema }`.
- `src/diff.rs` (modify, optional Phase 8): delegate identity detection to `calimero_wasm_abi::downgrade::identity_downgrades`.

**`crates/context`** — the gate:
- `src/handlers/upgrade_group.rs` (modify): `resolve_embedded_schema(...)` helper + `verify_no_identity_downgrade(...)`, called in `validate_upgrade` and `dispatch_cascade`.
- `tests/identity_downgrade_gate.rs` (new): real-embedded-wasm integration test.

**Apps / CI:**
- `apps/migrations/scenario-identity-downgrade-v{1,2}/build.sh` (modify): add `mero-abi embed` after `wasm-opt`.
- `workflows/app-migration/21-scenario-identity-downgrade.yml` (new): merobox negative-path workflow.
- `.github/workflows/app-migration-e2e.yml` (modify): add `21-scenario-identity-downgrade` to the matrix.

---

## Task 0: Grounding spike (no code — record findings inline in the PR description)

Two node-side facts are not yet pinned and Task 5 depends on them. Resolve them first; write the answers into the PR description so reviewers see the basis.

- [ ] **Step 1: Locate the blob-bytes read API.** Given an `ApplicationId`/`BlobId`, how does the context crate read the wasm bytes? Start from how `get_module` / `update_application` loads bytecode.

Run:
```bash
rg -n "blob" crates/context/src --type rust | rg -i "get|read|fetch|bytes|load" | head -40
rg -n "fn get_module|BlobId|blob_manager|BlobManager|\.blobs\b" crates/context/src crates/node/src --type rust | head -40
```
Record: the exact call to get `Vec<u8>`/bytes from a `BlobId` (sync or async), and which struct/field on `ContextManager` exposes it.

- [ ] **Step 2: Locate the "old application id" for a group at upgrade time.** `validate_upgrade` knows `target_application_id` (new) and `group_id`; the old app is the current app of a representative/canary context in the group.

Run:
```bash
rg -n "canary|application_id|representative|contexts\b" crates/context/src/handlers/upgrade_group.rs | head -40
```
Record: how to obtain the current `ApplicationId` of the group's context(s) inside `validate_upgrade` and `dispatch_cascade`.

- [ ] **Step 3: Confirm error style.** Confirm both emit sites return `eyre::Result` and there is no `UpgradeError` enum.

Run:
```bash
rg -n "UpgradeError|eyre::eyre!|-> eyre::Result" crates/context/src/handlers/upgrade_group.rs | head
```
Expected: `eyre::eyre!` is the idiom; no `UpgradeError` enum. The gate returns `eyre::eyre!(...)`.

- [ ] **Step 4: Commit nothing.** This task only produces written findings used by Task 5.

---

## Task 1: `identity_downgrades` in the lib

**Files:**
- Create: `crates/wasm-abi/src/downgrade.rs`
- Modify: `crates/wasm-abi/src/lib.rs` (add `pub mod downgrade;`)

- [ ] **Step 1: Write the failing test**

Append to `crates/wasm-abi/src/downgrade.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(fields_json: &str) -> Manifest {
        let json = format!(
            r#"{{"schema_version":"wasm-abi/1","types":{{"Root":{{"kind":"record","fields":{fields_json}}}}},"methods":[],"events":[],"state_root":"Root"}}"#
        );
        serde_json::from_str(&json).expect("valid manifest json")
    }

    const AUTHORED_MAP: &str = r#"{"name":"wiki","type":{"kind":"map","key":{"kind":"string"},"value":{"kind":"string"},"crdt_type":"authored_map"}}"#;
    const UNORDERED_MAP: &str = r#"{"name":"wiki","type":{"kind":"map","key":{"kind":"string"},"value":{"kind":"string"},"crdt_type":"unordered_map"}}"#;
    const AUTHORED_VEC: &str = r#"{"name":"wiki","type":{"kind":"list","items":{"kind":"string"},"crdt_type":"authored_vector"}}"#;

    #[test]
    fn authored_map_to_unordered_is_downgrade() {
        let d = identity_downgrades(&manifest(&format!("[{AUTHORED_MAP}]")), &manifest(&format!("[{UNORDERED_MAP}]")));
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].field, "wiki");
        assert_eq!(d[0].from, "AuthoredMap");
        assert_eq!(d[0].to, "UnorderedMap");
    }

    #[test]
    fn carry_through_same_type_is_not_downgrade() {
        let m = manifest(&format!("[{AUTHORED_MAP}]"));
        assert!(identity_downgrades(&m, &m).is_empty());
    }

    #[test]
    fn dropped_identity_field_is_downgrade() {
        let d = identity_downgrades(&manifest(&format!("[{AUTHORED_MAP}]")), &manifest("[]"));
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].to, "(removed)");
    }

    #[test]
    fn both_identity_gated_different_type_is_not_downgrade() {
        // AuthoredMap -> AuthoredVector: still identity-gated, no provenance lost.
        let d = identity_downgrades(&manifest(&format!("[{AUTHORED_MAP}]")), &manifest(&format!("[{AUTHORED_VEC}]")));
        assert!(d.is_empty(), "{d:?}");
    }

    #[test]
    fn plain_to_plain_is_not_downgrade() {
        let d = identity_downgrades(&manifest(&format!("[{UNORDERED_MAP}]")), &manifest(&format!("[{UNORDERED_MAP}]")));
        assert!(d.is_empty());
    }

    #[test]
    fn plain_to_identity_gated_is_not_downgrade() {
        // Adding provenance is an upgrade, never flagged.
        let d = identity_downgrades(&manifest(&format!("[{UNORDERED_MAP}]")), &manifest(&format!("[{AUTHORED_MAP}]")));
        assert!(d.is_empty());
    }
}
```

- [ ] **Step 2: Run the test, watch it fail to compile** (`identity_downgrades` undefined)

Run: `cargo test -p calimero-wasm-abi downgrade:: 2>&1 | tail -20`
Expected: FAIL — `cannot find function identity_downgrades`.

- [ ] **Step 3: Write the implementation** (top of `crates/wasm-abi/src/downgrade.rs`)

```rust
//! Top-level identity-downgrade detection shared by the `calimero-abi diff`
//! CI lint and the core L1 upgrade gate.

use crate::schema::{
    collection_category, CollectionCategory, CrdtCollectionType, Manifest, TypeDef, TypeRef,
};

/// One top-level state field whose old type was identity-gated and whose new
/// type is not (changed to a non-gated CRDT/plain type, or removed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityDowngrade {
    pub field: String,
    /// Old CRDT label, e.g. `AuthoredMap`.
    pub from: String,
    /// New label: a CRDT name, `plain`, or `(removed)`.
    pub to: String,
}

/// Resolve a field's *top-level* CRDT collection type, following `$ref`/alias
/// hops, if its top level is a CRDT collection. Cycle-guarded; returns `None`
/// for a non-CRDT (plain) type or an unresolvable ref.
fn top_level_crdt(ty: &TypeRef, manifest: &Manifest, depth: u8) -> Option<CrdtCollectionType> {
    if depth > 32 {
        return None; // fail-safe against a cyclic alias
    }
    // A re-serialized TypeRef carries `crdt_type` inline on a Collection.
    let value = serde_json::to_value(ty).ok()?;
    if let Some(ct) = value.get("crdt_type") {
        return serde_json::from_value::<CrdtCollectionType>(ct.clone()).ok();
    }
    // Follow a `$ref` to its (possibly aliased) definition.
    if let Some(serde_json::Value::String(name)) = value.get("$ref") {
        match manifest.types.get(name)? {
            TypeDef::Alias { target } => return top_level_crdt(target, manifest, depth + 1),
            _ => return None,
        }
    }
    None
}

fn is_identity_gated(ty: &TypeRef, manifest: &Manifest) -> bool {
    top_level_crdt(ty, manifest)
        .is_some_and(|ct| collection_category(&ct) == CollectionCategory::IdentityGated)
}

fn label(ty: &TypeRef, manifest: &Manifest) -> String {
    top_level_crdt(ty, manifest).map_or_else(|| "plain".to_owned(), |ct| format!("{ct:?}"))
}

/// Top-level fields of a manifest's state root, or empty if it has none / is
/// not a record. (The L1 gate treats "no comparable root" as nothing to flag;
/// the CI lint's `diff_checked` fail-closes separately on a corrupt root.)
fn root_fields<'a>(m: &'a Manifest) -> &'a [crate::schema::Field] {
    m.state_root
        .as_deref()
        .and_then(|r| m.types.get(r))
        .and_then(|d| match d {
            TypeDef::Record { fields } => Some(fields.as_slice()),
            _ => None,
        })
        .unwrap_or(&[])
}

/// Every top-level state field whose old type is identity-gated and whose new
/// type is not (changed away or removed). Adding gating or plain→plain changes
/// are never flagged.
pub fn identity_downgrades(old: &Manifest, new: &Manifest) -> Vec<IdentityDowngrade> {
    let new_fields = root_fields(new);
    let mut out = Vec::new();
    for f in root_fields(old) {
        if !is_identity_gated(&f.type_, old) {
            continue;
        }
        match new_fields.iter().find(|nf| nf.name == f.name) {
            None => out.push(IdentityDowngrade {
                field: f.name.clone(),
                from: label(&f.type_, old),
                to: "(removed)".to_owned(),
            }),
            Some(nf) if !is_identity_gated(&nf.type_, new) => out.push(IdentityDowngrade {
                field: f.name.clone(),
                from: label(&f.type_, old),
                to: label(&nf.type_, new),
            }),
            Some(_) => {}
        }
    }
    out
}
```

Add to `crates/wasm-abi/src/lib.rs` (near the other `pub mod` lines):
```rust
pub mod downgrade;
```

- [ ] **Step 4: Run the tests, watch them pass**

Run: `cargo test -p calimero-wasm-abi downgrade:: 2>&1 | tail -15`
Expected: `test result: ok. 6 passed`.

- [ ] **Step 5: Sabotage check (prove the test is meaningful)** — temporarily change `is_identity_gated` in the new-side check to `true`, re-run, confirm `authored_map_to_unordered_is_downgrade` now FAILS, then revert.

- [ ] **Step 6: Commit**

```bash
git add crates/wasm-abi/src/downgrade.rs crates/wasm-abi/src/lib.rs
git commit -m "feat(wasm-abi): identity_downgrades — shared top-level identity-downgrade check

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Embed/read the `calimero_abi_v1` custom section (lib)

**Files:**
- Create: `crates/wasm-abi/src/embed.rs`
- Modify: `crates/wasm-abi/src/lib.rs` (`pub mod embed;`), `crates/wasm-abi/Cargo.toml`

- [ ] **Step 1: Add deps** to `crates/wasm-abi/Cargo.toml` `[dependencies]` (skip any already present — check first with `rg 'wasmparser|wasm-encoder' crates/wasm-abi/Cargo.toml`):
```toml
wasmparser = "0.221"
wasm-encoder = "0.221"
```
(Match versions already used elsewhere in the workspace if pinned: `rg -n 'wasmparser|wasm-encoder' Cargo.lock | head`.)

- [ ] **Step 2: Write the failing test** (append to `crates/wasm-abi/src/embed.rs`)
```rust
#[cfg(test)]
mod tests {
    use super::*;

    const SECTION: &str = "calimero_abi_v1";

    fn sample_manifest() -> Manifest {
        serde_json::from_str(
            r#"{"schema_version":"wasm-abi/1","types":{"Root":{"kind":"record","fields":[]}},"methods":[],"events":[],"state_root":"Root"}"#,
        ).unwrap()
    }

    /// Minimal valid empty module: magic + version, no sections.
    fn empty_module() -> Vec<u8> {
        vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]
    }

    #[test]
    fn round_trip() {
        let m = sample_manifest();
        let wasm = write_embedded_state_schema(&empty_module(), &m);
        let got = read_embedded_state_schema(&wasm).expect("section present");
        assert_eq!(got.state_root.as_deref(), Some("Root"));
    }

    #[test]
    fn read_absent_is_none() {
        assert!(read_embedded_state_schema(&empty_module()).is_none());
    }

    #[test]
    fn re_embed_is_idempotent_replace() {
        let wasm1 = write_embedded_state_schema(&empty_module(), &sample_manifest());
        let wasm2 = write_embedded_state_schema(&wasm1, &sample_manifest());
        // Exactly one calimero_abi_v1 section after re-embed.
        let count = wasmparser::Parser::new(0)
            .parse_all(&wasm2)
            .filter_map(Result::ok)
            .filter(|p| matches!(p, wasmparser::Payload::CustomSection(c) if c.name() == SECTION))
            .count();
        assert_eq!(count, 1);
        assert!(read_embedded_state_schema(&wasm2).is_some());
    }
}
```

- [ ] **Step 3: Run, watch it fail** — `cargo test -p calimero-wasm-abi embed:: 2>&1 | tail`. Expected: FAIL (functions undefined).

- [ ] **Step 4: Implement** (top of `crates/wasm-abi/src/embed.rs`)
```rust
//! Embed/read the app state schema as a `calimero_abi_v1` wasm custom section,
//! so the schema travels inside the bytecode and is covered by `blob_id`.

use wasm_encoder::{CustomSection, Encode as _};
use wasmparser::{Parser, Payload};

use crate::schema::Manifest;

const SECTION_NAME: &str = "calimero_abi_v1";

/// Read the embedded state-schema `Manifest`, or `None` if the section is
/// absent or malformed (drives fail-open at the gate).
pub fn read_embedded_state_schema(wasm: &[u8]) -> Option<Manifest> {
    for payload in Parser::new(0).parse_all(wasm).flatten() {
        if let Payload::CustomSection(reader) = payload {
            if reader.name() == SECTION_NAME {
                return serde_json::from_slice::<Manifest>(reader.data()).ok();
            }
        }
    }
    None
}

/// Return a copy of `wasm` carrying a single `calimero_abi_v1` section with
/// `manifest` (replacing any existing one — idempotent).
pub fn write_embedded_state_schema(wasm: &[u8], manifest: &Manifest) -> Vec<u8> {
    let json = serde_json::to_vec(manifest).expect("Manifest serializes");
    // 1. Copy the module, dropping any pre-existing calimero_abi_v1 section.
    let mut out = Vec::with_capacity(wasm.len() + json.len() + 32);
    out.extend_from_slice(&wasm[..8]); // magic + version
    for payload in Parser::new(0).parse_all(wasm).flatten() {
        if let Payload::CustomSection(reader) = &payload {
            if reader.name() == SECTION_NAME {
                continue; // strip old section
            }
        }
        // Re-emit every other section verbatim by byte range.
        if let Some(range) = section_range(&payload) {
            out.extend_from_slice(&wasm[range]);
        }
    }
    // 2. Append the fresh custom section.
    CustomSection { name: SECTION_NAME.into(), data: json.as_slice().into() }.encode(&mut out);
    out
}

/// The byte range of a top-level section payload+header, if this payload is a
/// section (not the version/end markers).
fn section_range(payload: &Payload) -> Option<std::ops::Range<usize>> {
    use wasmparser::Payload as P;
    let range = match payload {
        P::CustomSection(r) => r.range(),
        P::TypeSection(r) => r.range(),
        P::ImportSection(r) => r.range(),
        P::FunctionSection(r) => r.range(),
        P::TableSection(r) => r.range(),
        P::MemorySection(r) => r.range(),
        P::TagSection(r) => r.range(),
        P::GlobalSection(r) => r.range(),
        P::ExportSection(r) => r.range(),
        P::ElementSection(r) => r.range(),
        P::DataSection(r) => r.range(),
        P::CodeSectionStart { range, .. } => range.clone(),
        P::StartSection { range, .. } => range.clone(),
        P::DataCountSection { range, .. } => range.clone(),
        _ => return None,
    };
    // wasmparser section `range` covers the *payload*; the 1-byte id + LEB size
    // header sits just before `range.start`. Re-emit from the id byte: walk back
    // over the size LEB and the id. Simplest robust approach: use the encoder.
    Some(range)
}
```

> **NOTE for the implementer:** re-emitting sections by `range` alone drops their 1-byte id + size header. If preserving exact bytes proves fiddly, replace the copy loop with a `wasm_encoder::Module` re-encode: iterate `Parser` payloads and push each non-target section via `module.section(&RawSection { id, data: &wasm[range] })`, computing `id` from the payload kind. Either way the **round-trip + idempotent tests in Step 2 are the contract** — make them pass.

Add to `crates/wasm-abi/src/lib.rs`:
```rust
pub mod embed;
```

- [ ] **Step 5: Run, watch pass** — `cargo test -p calimero-wasm-abi embed:: 2>&1 | tail`. Expected: 3 passed. If the byte-range copy corrupts the module, switch to the `RawSection` re-encode per the NOTE until green.

- [ ] **Step 6: Commit**
```bash
git add crates/wasm-abi/src/embed.rs crates/wasm-abi/src/lib.rs crates/wasm-abi/Cargo.toml Cargo.lock
git commit -m "feat(wasm-abi): embed/read calimero_abi_v1 state-schema wasm section

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: `mero-abi embed` subcommand

**Files:**
- Create: `tools/calimero-abi/src/embed.rs`
- Modify: `tools/calimero-abi/src/main.rs`
- Test: `tools/calimero-abi/tests/embed_cli.rs`

- [ ] **Step 1: Write the failing integration test** (`tools/calimero-abi/tests/embed_cli.rs`)
```rust
use std::process::Command;

#[test]
fn embed_then_inspect_finds_section() {
    let dir = std::env::temp_dir().join(format!("mero_abi_embed_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wasm = dir.join("m.wasm");
    let schema = dir.join("s.json");
    std::fs::write(&wasm, [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]).unwrap();
    std::fs::write(
        &schema,
        r#"{"schema_version":"wasm-abi/1","types":{"Root":{"kind":"record","fields":[]}},"methods":[],"events":[],"state_root":"Root"}"#,
    ).unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_mero-abi"))
        .args(["embed", wasm.to_str().unwrap(), schema.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());

    let bytes = std::fs::read(&wasm).unwrap();
    let found = wasmparser::Parser::new(0)
        .parse_all(&bytes)
        .filter_map(Result::ok)
        .any(|p| matches!(p, wasmparser::Payload::CustomSection(c) if c.name() == "calimero_abi_v1"));
    assert!(found, "calimero_abi_v1 section present after embed");
    let _ = std::fs::remove_dir_all(&dir);
}
```
Add `wasmparser` to `tools/calimero-abi/Cargo.toml` `[dev-dependencies]` if not present.

- [ ] **Step 2: Run, watch fail** — `cargo test -p mero-abi --test embed_cli 2>&1 | tail`. Expected: FAIL (no `embed` subcommand → non-zero exit).

- [ ] **Step 3: Implement** `tools/calimero-abi/src/embed.rs`
```rust
use std::path::Path;

use calimero_wasm_abi::embed::write_embedded_state_schema;
use calimero_wasm_abi::schema::Manifest;

/// Embed `schema` (a state-schema.json) into `wasm` as the `calimero_abi_v1`
/// custom section, in place. Idempotent (replaces any existing section).
pub fn run_embed(wasm: &Path, schema: &Path) -> eyre::Result<()> {
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(schema)?)
        .map_err(|e| eyre::eyre!("failed to parse {} as a state-schema manifest: {e}", schema.display()))?;
    let original = std::fs::read(wasm)?;
    let updated = write_embedded_state_schema(&original, &manifest);
    std::fs::write(wasm, updated)?;
    println!("✓ embedded calimero_abi_v1 ({} bytes schema) into {}", manifest.types.len(), wasm.display());
    Ok(())
}
```
Wire into `tools/calimero-abi/src/main.rs` — add to the `Commands` enum and dispatch:
```rust
    /// Embed a state-schema.json into a wasm as the calimero_abi_v1 section.
    Embed {
        /// The wasm to modify in place.
        wasm: std::path::PathBuf,
        /// The state-schema.json to embed.
        schema: std::path::PathBuf,
    },
```
```rust
        Commands::Embed { wasm, schema } => embed::run_embed(&wasm, &schema)?,
```
Add `mod embed;` near the other `mod` lines in `main.rs`.

- [ ] **Step 4: Run, watch pass** — `cargo test -p mero-abi --test embed_cli 2>&1 | tail`. Expected: 1 passed.

- [ ] **Step 5: Commit**
```bash
git add tools/calimero-abi/src/embed.rs tools/calimero-abi/src/main.rs tools/calimero-abi/tests/embed_cli.rs tools/calimero-abi/Cargo.toml Cargo.lock
git commit -m "feat(calimero-abi): embed subcommand — write calimero_abi_v1 into a wasm

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: Wire `embed` into the downgrade-scenario builds

**Files:**
- Modify: `apps/migrations/scenario-identity-downgrade-v1/build.sh`, `apps/migrations/scenario-identity-downgrade-v2/build.sh`

- [ ] **Step 1: Add the embed step after `wasm-opt`** in each build.sh. After the existing `wasm-opt` block, append (v1 shown; use `_v2` for v2):
```bash
# Embed the emitted state schema into the wasm as the calimero_abi_v1 custom
# section (AFTER wasm-opt, which would otherwise strip an unknown section), so
# the node can read the schema at upgrade time for the L1 downgrade gate.
cargo run -q -p mero-abi -- embed \
  ./res/scenario_identity_downgrade_v1.wasm \
  ./res/state-schema.json
```

- [ ] **Step 2: Build + verify the section is present**

Run:
```bash
bash apps/migrations/scenario-identity-downgrade-v1/build.sh
cargo run -q -p mero-abi -- inspect apps/migrations/scenario-identity-downgrade-v1/res/scenario_identity_downgrade_v1.wasm | rg calimero_abi_v1
```
Expected: `✓ 'calimero_abi_v1' section found`.

- [ ] **Step 3: Commit**
```bash
git add apps/migrations/scenario-identity-downgrade-v1/build.sh apps/migrations/scenario-identity-downgrade-v2/build.sh
git commit -m "build(app-migration): embed calimero_abi_v1 in the downgrade-scenario wasms

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: The L1 gate in `upgrade_group.rs`

**Files:**
- Modify: `crates/context/src/handlers/upgrade_group.rs`

Uses Task 0 findings: `BLOB_READ` = the call that yields wasm bytes from an app id / blob id; `OLD_APP_ID` = how to get the group's current application id.

- [ ] **Step 1: Write the failing unit test** for the pure gate fn (in the `mod tests` at `upgrade_group.rs:1278`)
```rust
    use calimero_wasm_abi::schema::Manifest;

    fn m(fields: &str) -> Manifest {
        serde_json::from_str(&format!(
            r#"{{"schema_version":"wasm-abi/1","types":{{"Root":{{"kind":"record","fields":[{fields}]}}}},"methods":[],"events":[],"state_root":"Root"}}"#
        )).unwrap()
    }
    const AUTH: &str = r#"{"name":"wiki","type":{"kind":"map","key":{"kind":"string"},"value":{"kind":"string"},"crdt_type":"authored_map"}}"#;
    const PLAIN: &str = r#"{"name":"wiki","type":{"kind":"map","key":{"kind":"string"},"value":{"kind":"string"},"crdt_type":"unordered_map"}}"#;

    #[test]
    fn gate_refuses_identity_downgrade() {
        let err = verify_no_identity_downgrade(Some(&m(AUTH)), Some(&m(PLAIN))).unwrap_err();
        assert!(err.to_string().contains("identity downgrade forbidden"), "{err}");
        assert!(err.to_string().contains("wiki"));
    }
    #[test]
    fn gate_allows_carry_through() {
        assert!(verify_no_identity_downgrade(Some(&m(AUTH)), Some(&m(AUTH))).is_ok());
    }
    #[test]
    fn gate_fails_open_when_schema_absent() {
        assert!(verify_no_identity_downgrade(None, Some(&m(PLAIN))).is_ok());
        assert!(verify_no_identity_downgrade(Some(&m(AUTH)), None).is_ok());
    }
```

- [ ] **Step 2: Run, watch fail** — `cargo test -p calimero-context --lib verify_no_identity 2>&1 | tail` (adjust crate name to the actual package: `rg '^name' crates/context/Cargo.toml | head -1`). Expected: FAIL (fn undefined).

- [ ] **Step 3: Implement the pure gate fn** (module-level in `upgrade_group.rs`)
```rust
use calimero_wasm_abi::downgrade::identity_downgrades;
use calimero_wasm_abi::schema::Manifest;

/// L1 identity-downgrade gate. Refuse a migration upgrade that strips identity
/// from a top-level state field. Fail-open (allow) when either schema is absent
/// — legacy apps built before ABI embedding cannot be checked.
fn verify_no_identity_downgrade(
    old: Option<&Manifest>,
    new: Option<&Manifest>,
) -> eyre::Result<()> {
    let (Some(old), Some(new)) = (old, new) else {
        tracing::warn!(
            "cannot verify identity downgrade: one side has no embedded ABI (legacy app); allowing upgrade"
        );
        // metric: see Step 5
        return Ok(());
    };
    if let Some(d) = identity_downgrades(old, new).into_iter().next() {
        eyre::bail!(
            "identity downgrade forbidden: field '{}' {} → {} strips authorship/writer-ACL network-wide \
             (use owner-driven rewrite; see #2534)",
            d.field, d.from, d.to
        );
    }
    Ok(())
}
```

- [ ] **Step 4: Run, watch pass** — `cargo test -p calimero-context --lib verify_no_identity 2>&1 | tail`. Expected: 3 passed.

- [ ] **Step 5: Add the metric + schema resolver** (module-level). Use the Task-0 `BLOB_READ` API. Template:
```rust
/// Read a context application's embedded state schema, or None if unavailable
/// (no blob, no embedded section, or a read error — all fail-open).
fn resolve_embedded_schema(actor: &ContextManager, application_id: &ApplicationId) -> Option<Manifest> {
    let app = actor.applications.get(application_id)?;          // confirm field in Task 0
    let bytes = /* BLOB_READ: read wasm bytes for app.blob.bytecode */;
    calimero_wasm_abi::embed::read_embedded_state_schema(&bytes)
}
```
Replace `/* BLOB_READ */` with the exact call recorded in Task 0. If it is async/actor-bound, resolve the bytes in the calling context before invoking `verify_no_identity_downgrade` (the gate fn itself stays pure and sync).

- [ ] **Step 6: Call the gate at both emit sites — `validate_upgrade`.** After the existing `has_migration` policy check (the `2a.` block), add:
```rust
    if has_migration {
        let old = /* OLD_APP_ID → resolve_embedded_schema */ ;
        let new = resolve_embedded_schema(actor, target_application_id);
        verify_no_identity_downgrade(old.as_ref(), new.as_ref())?;
    }
```
> If `validate_upgrade` lacks `&ContextManager`/actor access (it takes `&Store`), resolve both schemas in the **caller** (`upgrade_group` handler, which has `actor`) and pass them in, or move the gate call to the handler right after `validate_upgrade` returns and before the op is emitted. Keep it strictly before any op emission.

- [ ] **Step 7: Call the gate in `dispatch_cascade`.** After its `has_migration` is computed and the admin/preflight checks pass, before emitting the cascade op:
```rust
    if has_migration {
        let old = /* representative descendant/group current app → resolve_embedded_schema */;
        let new = resolve_embedded_schema(actor, &target_application_id);
        if let Err(err) = verify_no_identity_downgrade(old.as_ref(), new.as_ref()) {
            return ActorResponse::reply(Err(err));
        }
    }
```

- [ ] **Step 8: Run the full handler test module + build**

Run: `cargo test -p calimero-context 2>&1 | tail -20`
Expected: all green (new gate tests + existing upgrade tests unaffected).

- [ ] **Step 9: Commit**
```bash
git add crates/context/src/handlers/upgrade_group.rs
git commit -m "feat(context): L1 identity-downgrade gate at upgrade emit (fail-open, emitter-only)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: Core integration test (real embedded wasm → handler)

**Files:**
- Create: `crates/context/tests/identity_downgrade_gate.rs`

- [ ] **Step 1: Write the test.** Model it on an existing `crates/context/tests/*.rs` (e.g. `cascade_atomic_apply.rs`) for harness setup (node/context/store bootstrap). Build the real embedded scenario wasms first, load their bytes as the app blobs, then drive the upgrade handler.
```rust
// Pseudocode-level structure — fill harness specifics from cascade_atomic_apply.rs.
// 1. Build v1 + v2 scenario wasms (assume CI/build already produced them; read from
//    apps/migrations/scenario-identity-downgrade-v{1,2}/res/*.wasm).
// 2. Bootstrap a context whose current application is v1 (AuthoredMap), blob = v1 bytes.
// 3. Call the upgrade path (validate_upgrade / upgrade_group handler) targeting v2
//    (UnorderedMap) WITH a migration.
// 4. Assert it errors with "identity downgrade forbidden" and "wiki".
// 5. Negative control: upgrading v1 -> v1' (same AuthoredMap shape) WITH a migration
//    is allowed (Ok).
```
- [ ] **Step 2: Run, watch the downgrade assertion fail first if the gate is bypassed** (temporarily comment the gate call) to confirm the test exercises it; then restore.
- [ ] **Step 3: Run, watch pass** — `cargo test -p calimero-context --test identity_downgrade_gate 2>&1 | tail`.
- [ ] **Step 4: Commit**
```bash
git add crates/context/tests/identity_downgrade_gate.rs
git commit -m "test(context): integration — real embedded wasm rejected at upgrade as identity downgrade

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: Merobox negative-path workflow + CI matrix

**Files:**
- Create: `workflows/app-migration/21-scenario-identity-downgrade.yml`
- Modify: `.github/workflows/app-migration-e2e.yml`

- [ ] **Step 1: Author the workflow.** Copy the structure of `workflows/app-migration/00-single-group-migration-baseline.yml` (deploy v1, set LazyOnAccess, attempt upgrade to v2 with `migrate_method`). Change the upgrade step to expect failure and assert via logs:
```yaml
  - name: Attempt identity-downgrade upgrade (must be refused by L1 gate)
    type: upgrade_group
    node: node-a
    group_id: "{{group_id}}"
    target_application_id: "{{app_v2_id}}"
    migrate_method: migrate_v1_to_v2
    expected_failure: true        # gate returns an error; step passes on expected failure

  - name: Assert the gate refused it
    type: assert_log_present
    node: node-a
    pattern: "identity downgrade forbidden"

  - name: Assert the migration never ran
    type: assert_log_absent
    node: node-a
    pattern: "Executing migration"
```
(Use the same dynamic-value wiring for `group_id`/app ids as `00-…baseline.yml`.)

- [ ] **Step 2: Add to the CI matrix** — in `.github/workflows/app-migration-e2e.yml` `scenario.strategy.matrix.workflow`, append:
```yaml
          - 21-scenario-identity-downgrade
```
Also add `apps/migrations/scenario-identity-downgrade-v1` and `-v2` to `workflows/app-migration/build-wasms.sh` `SUITES` so the matrix's `build` job produces their (embedded) wasms.

- [ ] **Step 3: Local dry-run** (if merobox available locally)
```bash
merobox bootstrap run workflows/app-migration/21-scenario-identity-downgrade.yml --image merod:local --e2e-mode --verbose 2>&1 | tail -30
```
Expected: the upgrade step logs `✓ Expected failure occurred: …identity downgrade forbidden…`; assert steps pass.

- [ ] **Step 4: Commit**
```bash
git add workflows/app-migration/21-scenario-identity-downgrade.yml workflows/app-migration/build-wasms.sh .github/workflows/app-migration-e2e.yml
git commit -m "test(app-migration): merobox e2e — L1 gate refuses an identity-downgrade upgrade

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 8 (optional, DRY): `diff` tool delegates to `identity_downgrades`

Only after Tasks 1–7 are green. Keep #2586's tests as the regression guard.

**Files:**
- Modify: `tools/calimero-abi/src/diff.rs`

- [ ] **Step 1:** Replace `diff.rs`'s inline identity detection (`is_identity_gated`/`canonical_crdt` usage inside `diff_checked`'s downgrade branch) with a call to `calimero_wasm_abi::downgrade::identity_downgrades`, mapping each `IdentityDowngrade` to `FindingClass::UnsafeIdentityDowngrade`. Keep the Additive/Breaking classification as-is.
- [ ] **Step 2: Run the full #2586 suite** — `cargo test -p mero-abi 2>&1 | tail`. Expected: all existing `diff::tests::*` + `tests/diff_cli.rs` still pass (behaviour unchanged).
- [ ] **Step 3: Commit**
```bash
git add tools/calimero-abi/src/diff.rs
git commit -m "refactor(calimero-abi): diff delegates identity-downgrade detection to the shared lib

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Final verification

- [ ] `cargo test -p calimero-wasm-abi -p mero-abi -p calimero-context 2>&1 | tail -30` — all green.
- [ ] `cargo build -p merod` — node builds with the new dep edge.
- [ ] Re-run the `schema-downgrade-guard` CI job locally (Task in #2586) — still green.
- [ ] PR description records the Task-0 findings (blob read API, old-app-id source) and links #2587; approach comment posted for format sign-off before requesting review.
- [ ] Open the PR ready-for-review; single PR, layered commits (Tasks 1→7, optional 8).

## Out of scope (follow-ups, do NOT include)
- `#[migrate(unsafe_strip_identity = "…")]` override (macro attr + `MigrationParams` field + governance allowance).
- Rolling the `embed` build step out to every real app (here: only the downgrade scenarios).
- Receiver-side re-validation; macro-based auto-embed (Option B); nested identity-gated CRDTs.
