//! Semantic diff of two `state-schema.json` versions.
//!
//! Classifies each top-level state-field change as additive, breaking
//! (migration required), or — the security-relevant one —
//! `UNSAFE_IDENTITY_DOWNGRADE`: an identity-gated CRDT (`AuthoredMap`,
//! `AuthoredVector`, `SharedStorage`) replaced by a non-identity-gated type or
//! dropped, which silently strips per-entry authorship / the writer-ACL
//! network-wide. This is the CI (L2) layer of the migration safety rail; it
//! consumes the authoritative `collection_category` classifier from
//! `calimero-wasm-abi`.

use std::collections::BTreeMap;
use std::path::Path;

use calimero_wasm_abi::schema::{
    collection_category, CollectionCategory, CrdtCollectionType, Field, Manifest, TypeDef, TypeRef,
};

/// Classification of a single field-level change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingClass {
    /// A new field that an old state can default-fill — no migration needed.
    Additive,
    /// A change that requires a migration (type change, field removed).
    Breaking,
    /// An identity-gated field downgraded to a non-identity-gated type (or
    /// dropped) — strips authorship / writer-ACL with no error. Requires an
    /// explicit, reasoned opt-in.
    UnsafeIdentityDowngrade,
}

impl FindingClass {
    /// Short tag used in CLI output.
    pub fn tag(self) -> &'static str {
        match self {
            FindingClass::Additive => "ADDITIVE",
            FindingClass::Breaking => "BREAKING",
            FindingClass::UnsafeIdentityDowngrade => "UNSAFE_IDENTITY_DOWNGRADE",
        }
    }

    /// Whether this finding should fail CI by default.
    pub fn is_failure(self) -> bool {
        matches!(
            self,
            FindingClass::Breaking | FindingClass::UnsafeIdentityDowngrade
        )
    }
}

/// A single classified change to a state field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub field: String,
    pub class: FindingClass,
    pub detail: String,
}

/// Map a state-root's top-level fields by name.
fn state_root_fields(manifest: &Manifest) -> BTreeMap<&str, &TypeRef> {
    let mut out = BTreeMap::new();
    if let Some(root) = &manifest.state_root {
        if let Some(TypeDef::Record { fields }) = manifest.types.get(root) {
            for field in fields {
                let _ = out.insert(field.name.as_str(), &field.type_);
            }
        }
    }
    out
}

/// Follow `$ref` → `Alias` chains to the effective type. An identity-gated CRDT
/// can hide behind a newtype alias (`field: $ref Wiki`, `Wiki = alias → AuthoredMap`),
/// so the identity check must resolve before inspecting the `crdt_type`. Stops at
/// the first non-alias type (collection / scalar / record-ref / missing); cycle-guarded.
fn resolve_aliases<'a>(ty: &'a TypeRef, manifest: &'a Manifest) -> &'a TypeRef {
    let mut cur = ty;
    for _ in 0..64 {
        match cur {
            TypeRef::Reference { ref_ } => match manifest.types.get(ref_) {
                Some(TypeDef::Alias { target }) => cur = target,
                _ => return cur,
            },
            _ => return cur,
        }
    }
    cur
}

/// The CRDT tag of a field's top-level type, if it is a CRDT collection.
fn field_crdt(ty: &TypeRef) -> Option<&CrdtCollectionType> {
    match ty {
        TypeRef::Collection {
            crdt_type: Some(ct),
            ..
        } => Some(ct),
        _ => None,
    }
}

fn is_identity_gated(ty: &TypeRef, manifest: &Manifest) -> bool {
    field_crdt(resolve_aliases(ty, manifest))
        .is_some_and(|ct| collection_category(ct) == CollectionCategory::IdentityGated)
}

fn crdt_label(ty: &TypeRef, manifest: &Manifest) -> String {
    field_crdt(resolve_aliases(ty, manifest))
        .map_or_else(|| "plain".to_owned(), |ct| format!("{ct:?}"))
}

/// Canonical form of a field type with all `$ref`s expanded inline, so two fields
/// compare equal iff their *fully resolved* shapes match. Without this, a field
/// whose `$ref` name is stable while the referenced `types` entry changes would
/// look unchanged and hide a downgrade. Cycle-guarded.
fn canonical(ty: &TypeRef, manifest: &Manifest) -> serde_json::Value {
    let value = serde_json::to_value(ty).unwrap_or(serde_json::Value::Null);
    expand_refs(value, manifest, &mut Vec::new())
}

fn expand_refs(
    value: serde_json::Value,
    manifest: &Manifest,
    seen: &mut Vec<String>,
) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            if map.len() == 1 {
                if let Some(Value::String(name)) = map.get("$ref") {
                    if seen.iter().any(|n| n == name) {
                        return serde_json::json!({ "$ref": name, "$cycle": true });
                    }
                    seen.push(name.clone());
                    let expanded = match manifest.types.get(name) {
                        Some(def) => {
                            let def_value = serde_json::to_value(def).unwrap_or(Value::Null);
                            expand_refs(def_value, manifest, seen)
                        }
                        None => serde_json::json!({ "$ref": name, "$missing": true }),
                    };
                    let _ = seen.pop();
                    return expanded;
                }
            }
            Value::Object(
                map.into_iter()
                    .map(|(k, v)| (k, expand_refs(v, manifest, seen)))
                    .collect(),
            )
        }
        Value::Array(arr) => Value::Array(
            arr.into_iter()
                .map(|v| expand_refs(v, manifest, seen))
                .collect(),
        ),
        other => other,
    }
}

/// Resolve a manifest's state root to its record fields, or fail. Fail-closed: a
/// missing / non-record `state_root` must NOT be treated as "zero fields" (that
/// would make a flawed baseline silently pass a downgrade as merely additive).
fn root_record_fields<'a>(manifest: &'a Manifest, which: &str) -> eyre::Result<&'a [Field]> {
    let root = manifest
        .state_root
        .as_deref()
        .ok_or_else(|| eyre::eyre!("{which} schema has no state_root"))?;
    match manifest.types.get(root) {
        Some(TypeDef::Record { fields }) => Ok(fields),
        Some(_) => eyre::bail!("{which} state_root '{root}' is not a record type"),
        None => eyre::bail!("{which} state_root '{root}' is not defined in `types`"),
    }
}

/// Diff `current` (the new build) against `baseline` (the previous version),
/// returning one [`Finding`] per changed top-level state field.
pub fn diff_state_schemas(current: &Manifest, baseline: &Manifest) -> Vec<Finding> {
    let cur = state_root_fields(current);
    let base = state_root_fields(baseline);
    let mut findings = Vec::new();

    // Removed or changed fields (walk the baseline).
    for (name, base_ty) in &base {
        match cur.get(name) {
            None => {
                if is_identity_gated(base_ty, baseline) {
                    findings.push(Finding {
                        field: (*name).to_owned(),
                        class: FindingClass::UnsafeIdentityDowngrade,
                        detail: format!(
                            "identity-gated field '{name}' ({}) removed — strips authorship / writer-ACL network-wide",
                            crdt_label(base_ty, baseline)
                        ),
                    });
                } else {
                    findings.push(Finding {
                        field: (*name).to_owned(),
                        class: FindingClass::Breaking,
                        detail: format!("field '{name}' removed — migration required"),
                    });
                }
            }
            Some(cur_ty) => {
                if canonical(cur_ty, current) != canonical(base_ty, baseline) {
                    if is_identity_gated(base_ty, baseline) && !is_identity_gated(cur_ty, current) {
                        findings.push(Finding {
                            field: (*name).to_owned(),
                            class: FindingClass::UnsafeIdentityDowngrade,
                            detail: format!(
                                "field '{name}' {} → {} — strips authorship / writer-ACL network-wide",
                                crdt_label(base_ty, baseline),
                                crdt_label(cur_ty, current)
                            ),
                        });
                    } else {
                        findings.push(Finding {
                            field: (*name).to_owned(),
                            class: FindingClass::Breaking,
                            detail: format!("field '{name}' type changed — migration required"),
                        });
                    }
                }
            }
        }
    }

    // Added fields (walk the current).
    for name in cur.keys() {
        if !base.contains_key(name) {
            findings.push(Finding {
                field: (*name).to_owned(),
                class: FindingClass::Additive,
                detail: format!("field '{name}' added"),
            });
        }
    }

    findings
}

/// Validate both manifests have a resolvable state-root record, then diff.
/// Fail-closed: a missing/broken root errors out (non-zero) rather than silently
/// producing no findings.
pub fn diff_checked(current: &Manifest, baseline: &Manifest) -> eyre::Result<Vec<Finding>> {
    let _ = root_record_fields(current, "current")?;
    let _ = root_record_fields(baseline, "baseline")?;
    Ok(diff_state_schemas(current, baseline))
}

fn load_manifest(path: &Path) -> eyre::Result<Manifest> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| eyre::eyre!("failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| {
        eyre::eyre!(
            "failed to parse {} as a state-schema manifest: {e}",
            path.display()
        )
    })
}

/// CLI entry point: diff two `state-schema.json` files, print findings, and (by
/// default) exit non-zero if any breaking / unsafe change is present.
pub fn run_diff(current: &Path, baseline: &Path, exit_zero: bool) -> eyre::Result<()> {
    let current = load_manifest(current)?;
    let baseline = load_manifest(baseline)?;
    let findings = diff_checked(&current, &baseline)?;

    if findings.is_empty() {
        println!("✓ No state-schema changes.");
        return Ok(());
    }

    let mut fail = false;
    for finding in &findings {
        let marker = match finding.class {
            FindingClass::Additive => "+",
            FindingClass::Breaking => "⚠",
            FindingClass::UnsafeIdentityDowngrade => "⛔",
        };
        println!("{marker} [{}] {}", finding.class.tag(), finding.detail);
        if finding.class == FindingClass::UnsafeIdentityDowngrade {
            println!(
                "    override requires #[migrate(unsafe_strip_identity = \"…\")] + governance allowance (see #2534)"
            );
        }
        fail |= finding.class.is_failure();
    }

    if fail && !exit_zero {
        std::process::exit(1);
    }
    Ok(())
}

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
    const SHARED_STORAGE: &str = r#"{"name":"acl","type":{"kind":"record","fields":[],"crdt_type":"shared_storage","inner_type":{"kind":"string"}}}"#;
    const COUNTER_U64: &str = r#"{"name":"counter","type":{"kind":"record","fields":[],"crdt_type":"lww_register","inner_type":{"kind":"u64"}}}"#;
    const COUNTER_STR: &str = r#"{"name":"counter","type":{"kind":"record","fields":[],"crdt_type":"lww_register","inner_type":{"kind":"string"}}}"#;

    #[test]
    fn authored_map_to_unordered_map_is_unsafe_downgrade() {
        let baseline = manifest(&format!("[{AUTHORED_MAP}]"));
        let current = manifest(&format!("[{UNORDERED_MAP}]"));
        let findings = diff_state_schemas(&current, &baseline);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].field, "wiki");
        assert_eq!(findings[0].class, FindingClass::UnsafeIdentityDowngrade);
    }

    #[test]
    fn shared_storage_removed_is_unsafe_downgrade() {
        let baseline = manifest(&format!("[{SHARED_STORAGE}]"));
        let current = manifest("[]");
        let findings = diff_state_schemas(&current, &baseline);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].field, "acl");
        assert_eq!(findings[0].class, FindingClass::UnsafeIdentityDowngrade);
    }

    #[test]
    fn added_field_is_additive() {
        let baseline = manifest(&format!("[{COUNTER_U64}]"));
        let current = manifest(&format!("[{COUNTER_U64},{SHARED_STORAGE}]"));
        let findings = diff_state_schemas(&current, &baseline);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].field, "acl");
        assert_eq!(findings[0].class, FindingClass::Additive);
    }

    #[test]
    fn non_identity_type_change_is_breaking() {
        let baseline = manifest(&format!("[{COUNTER_U64}]"));
        let current = manifest(&format!("[{COUNTER_STR}]"));
        let findings = diff_state_schemas(&current, &baseline);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].class, FindingClass::Breaking);
    }

    #[test]
    fn identical_schema_has_no_findings() {
        let m = manifest(&format!("[{COUNTER_U64}]"));
        assert!(diff_state_schemas(&m, &m).is_empty());
    }

    #[test]
    fn authored_to_authored_value_change_is_breaking_not_downgrade() {
        // AuthoredMap<…,String> -> AuthoredMap<…,u64> stays identity-gated, so it
        // is a normal breaking change, NOT an unsafe downgrade.
        let base = r#"{"name":"wiki","type":{"kind":"map","key":{"kind":"string"},"value":{"kind":"string"},"crdt_type":"authored_map"}}"#;
        let cur = r#"{"name":"wiki","type":{"kind":"map","key":{"kind":"string"},"value":{"kind":"u64"},"crdt_type":"authored_map"}}"#;
        let findings = diff_state_schemas(
            &manifest(&format!("[{cur}]")),
            &manifest(&format!("[{base}]")),
        );
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].class, FindingClass::Breaking);
    }

    fn manifest_raw(json: &str) -> Manifest {
        serde_json::from_str(json).expect("valid manifest json")
    }

    #[test]
    fn alias_to_authored_map_downgrade_is_unsafe() {
        // The field is a `$ref` to a newtype alias; the alias target downgrades from
        // AuthoredMap to UnorderedMap. The `$ref` name is stable, so naive top-level
        // equality would miss it — alias resolution must catch it as an unsafe downgrade.
        let baseline = manifest_raw(
            r#"{"schema_version":"wasm-abi/1","types":{
                "Wiki":{"kind":"alias","target":{"kind":"map","key":{"kind":"string"},"value":{"kind":"string"},"crdt_type":"authored_map"}},
                "Root":{"kind":"record","fields":[{"name":"wiki","type":{"$ref":"Wiki"}}]}
            },"methods":[],"events":[],"state_root":"Root"}"#,
        );
        let current = manifest_raw(
            r#"{"schema_version":"wasm-abi/1","types":{
                "Wiki":{"kind":"alias","target":{"kind":"map","key":{"kind":"string"},"value":{"kind":"string"},"crdt_type":"unordered_map"}},
                "Root":{"kind":"record","fields":[{"name":"wiki","type":{"$ref":"Wiki"}}]}
            },"methods":[],"events":[],"state_root":"Root"}"#,
        );
        let findings = diff_state_schemas(&current, &baseline);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].field, "wiki");
        assert_eq!(findings[0].class, FindingClass::UnsafeIdentityDowngrade);
    }

    #[test]
    fn ref_target_record_change_is_breaking() {
        // The field's `$ref` name is unchanged, but the referenced record gains a
        // field. Canonical (ref-expanding) comparison must see the structural change.
        let baseline = manifest_raw(
            r#"{"schema_version":"wasm-abi/1","types":{
                "Inner":{"kind":"record","fields":[{"name":"a","type":{"kind":"u64"}}]},
                "Root":{"kind":"record","fields":[{"name":"data","type":{"$ref":"Inner"}}]}
            },"methods":[],"events":[],"state_root":"Root"}"#,
        );
        let current = manifest_raw(
            r#"{"schema_version":"wasm-abi/1","types":{
                "Inner":{"kind":"record","fields":[{"name":"a","type":{"kind":"u64"}},{"name":"b","type":{"kind":"string"}}]},
                "Root":{"kind":"record","fields":[{"name":"data","type":{"$ref":"Inner"}}]}
            },"methods":[],"events":[],"state_root":"Root"}"#,
        );
        let findings = diff_state_schemas(&current, &baseline);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].class, FindingClass::Breaking);
    }

    #[test]
    fn missing_state_root_is_error_not_silent_pass() {
        // A baseline with no resolvable state_root must error (fail-closed), never be
        // treated as zero fields (which would mask a downgrade as merely additive).
        let baseline =
            manifest_raw(r#"{"schema_version":"wasm-abi/1","types":{},"methods":[],"events":[]}"#);
        let current = manifest(&format!("[{AUTHORED_MAP}]"));
        assert!(diff_checked(&current, &baseline).is_err());
    }

    #[test]
    fn diff_checked_passes_through_valid_manifests() {
        let baseline = manifest(&format!("[{COUNTER_U64}]"));
        let current = manifest(&format!("[{COUNTER_U64},{SHARED_STORAGE}]"));
        let findings = diff_checked(&current, &baseline).expect("valid roots");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].class, FindingClass::Additive);
    }
}
