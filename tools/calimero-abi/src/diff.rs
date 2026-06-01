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
    collection_category, CollectionCategory, CrdtCollectionType, Manifest, TypeDef, TypeRef,
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

fn is_identity_gated(ty: &TypeRef) -> bool {
    field_crdt(ty).is_some_and(|ct| collection_category(ct) == CollectionCategory::IdentityGated)
}

fn crdt_label(ty: &TypeRef) -> String {
    field_crdt(ty).map_or_else(|| "plain".to_owned(), |ct| format!("{ct:?}"))
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
                if is_identity_gated(base_ty) {
                    findings.push(Finding {
                        field: (*name).to_owned(),
                        class: FindingClass::UnsafeIdentityDowngrade,
                        detail: format!(
                            "identity-gated field '{name}' ({}) removed — strips authorship / writer-ACL network-wide",
                            crdt_label(base_ty)
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
                if *cur_ty != *base_ty {
                    if is_identity_gated(base_ty) && !is_identity_gated(cur_ty) {
                        findings.push(Finding {
                            field: (*name).to_owned(),
                            class: FindingClass::UnsafeIdentityDowngrade,
                            detail: format!(
                                "field '{name}' {} → {} — strips authorship / writer-ACL network-wide",
                                crdt_label(base_ty),
                                crdt_label(cur_ty)
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
    let findings = diff_state_schemas(&current, &baseline);

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
}
