//! Top-level identity-downgrade detection shared by the `calimero-abi diff`
//! CI lint and the core L1 upgrade gate.

use crate::schema::{
    collection_category, CollectionCategory, CrdtCollectionType, Field, Manifest, TypeDef, TypeRef,
};

/// One top-level state field whose old type was identity-gated and whose new
/// type is not (changed to a non-gated type, or removed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityDowngrade {
    pub field: String,
    pub from: String,
    pub to: String,
}

const MAX_REF_DEPTH: u8 = 32;

/// Outcome of resolving a field's *top-level* type.
///
/// The third state is what keeps the L1 gate fail-CLOSED: `Unresolvable`
/// (a reference cycle, a dangling `$ref`, or an unknown `crdt_type`) is NOT
/// graded "plain". Folding it into "plain" — as the old `Option`-returning
/// resolver did — let an identity-gated field hidden behind a broken ref slip
/// past the gate (a missed downgrade). This mirrors the L2 `calimero-abi diff`
/// lint, which also fails closed on cyclic/dangling refs.
enum Resolution {
    /// Top level is a known CRDT collection.
    Crdt(CrdtCollectionType),
    /// Resolved to a genuine non-CRDT type (primitive, record, variant, …).
    Plain,
    /// Could not be resolved — treat conservatively (fail closed).
    Unresolvable,
}

/// Resolve a field's *top-level* type, following `$ref`/alias hops.
fn resolve_top_level(ty: &TypeRef, manifest: &Manifest, depth: u8) -> Resolution {
    if depth > MAX_REF_DEPTH {
        return Resolution::Unresolvable; // cycle: cannot determine
    }
    let Ok(value) = serde_json::to_value(ty) else {
        return Resolution::Unresolvable;
    };
    if let Some(ct) = value.get("crdt_type") {
        return match serde_json::from_value::<CrdtCollectionType>(ct.clone()) {
            Ok(ct) => Resolution::Crdt(ct),
            Err(_) => Resolution::Unresolvable, // unknown/new crdt type: fail closed
        };
    }
    if let Some(serde_json::Value::String(name)) = value.get("$ref") {
        return match manifest.types.get(name) {
            // An alias may wrap a CRDT collection — follow it.
            Some(TypeDef::Alias { target }) => resolve_top_level(target, manifest, depth + 1),
            // A ref to a record/variant/bytes is a genuine non-CRDT top level.
            Some(_) => Resolution::Plain,
            // Dangling ref: cannot determine — fail closed.
            None => Resolution::Unresolvable,
        };
    }
    Resolution::Plain
}

/// True when `ty` is a candidate downgrade *source*: definitely identity-gated,
/// OR unresolvable (fail closed — we cannot rule out that it is gated, so the
/// field must still be compared against its new shape rather than skipped).
fn old_gated_or_unknown(ty: &TypeRef, manifest: &Manifest) -> bool {
    match resolve_top_level(ty, manifest, 0) {
        Resolution::Crdt(ct) => collection_category(&ct) == CollectionCategory::IdentityGated,
        Resolution::Plain => false,
        Resolution::Unresolvable => true,
    }
}

/// True only when `ty` is *definitely* still identity-gated. An unresolvable
/// new type is NOT treated as gated, so it reads as a downgrade (fail closed).
fn new_still_gated(ty: &TypeRef, manifest: &Manifest) -> bool {
    matches!(
        resolve_top_level(ty, manifest, 0),
        Resolution::Crdt(ct) if collection_category(&ct) == CollectionCategory::IdentityGated
    )
}

fn label(ty: &TypeRef, manifest: &Manifest) -> String {
    match resolve_top_level(ty, manifest, 0) {
        Resolution::Crdt(ct) => format!("{ct:?}"),
        Resolution::Plain => "plain".to_owned(),
        Resolution::Unresolvable => "(unresolved)".to_owned(),
    }
}

/// True when `ty`'s top level cannot be resolved (cycle / dangling / unknown).
fn is_unresolvable(ty: &TypeRef, manifest: &Manifest) -> bool {
    matches!(resolve_top_level(ty, manifest, 0), Resolution::Unresolvable)
}

fn root_fields(m: &Manifest) -> &[Field] {
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
        if !old_gated_or_unknown(&f.type_, old) {
            continue;
        }
        match new_fields.iter().find(|nf| nf.name == f.name) {
            None => out.push(IdentityDowngrade {
                field: f.name.clone(),
                from: label(&f.type_, old),
                to: "(removed)".to_owned(),
            }),
            // Carry-through carve-out: when BOTH old and new are unresolvable,
            // the field is unchanged-and-unknowable (e.g. a legacy app that has
            // always had a dangling `$ref` and never used identity-gated state).
            // Nothing downgraded, so don't fail-closed on it. A genuine
            // alias-target downgrade keeps the ref name stable but resolves
            // old→gated / new→plain, so it is NOT both-unresolvable and still
            // flags. An unresolvable→plain transition also still flags.
            Some(nf)
                if !(new_still_gated(&nf.type_, new)
                    || is_unresolvable(&f.type_, old) && is_unresolvable(&nf.type_, new)) =>
            {
                out.push(IdentityDowngrade {
                    field: f.name.clone(),
                    from: label(&f.type_, old),
                    to: label(&nf.type_, new),
                })
            }
            Some(_) => {}
        }
    }
    out
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
    const AUTHORED_VEC: &str = r#"{"name":"wiki","type":{"kind":"list","items":{"kind":"string"},"crdt_type":"authored_vector"}}"#;

    #[test]
    fn authored_map_to_unordered_is_downgrade() {
        let d = identity_downgrades(
            &manifest(&format!("[{AUTHORED_MAP}]")),
            &manifest(&format!("[{UNORDERED_MAP}]")),
        );
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
        let d = identity_downgrades(
            &manifest(&format!("[{AUTHORED_MAP}]")),
            &manifest(&format!("[{AUTHORED_VEC}]")),
        );
        assert!(d.is_empty(), "{d:?}");
    }
    #[test]
    fn plain_to_plain_is_not_downgrade() {
        let d = identity_downgrades(
            &manifest(&format!("[{UNORDERED_MAP}]")),
            &manifest(&format!("[{UNORDERED_MAP}]")),
        );
        assert!(d.is_empty());
    }
    #[test]
    fn plain_to_identity_gated_is_not_downgrade() {
        let d = identity_downgrades(
            &manifest(&format!("[{UNORDERED_MAP}]")),
            &manifest(&format!("[{AUTHORED_MAP}]")),
        );
        assert!(d.is_empty());
    }

    /// Manifest with extra named types. `extra_types_json` must be comma-prefixed
    /// (e.g. `,"Foo":{...}`) or empty.
    fn manifest_with_types(fields_json: &str, extra_types_json: &str) -> Manifest {
        let json = format!(
            r#"{{"schema_version":"wasm-abi/1","types":{{"Root":{{"kind":"record","fields":{fields_json}}}{extra_types_json}}},"methods":[],"events":[],"state_root":"Root"}}"#
        );
        serde_json::from_str(&json).expect("valid manifest json")
    }

    #[test]
    fn alias_wrapped_authored_map_to_plain_is_downgrade() {
        // OLD field is a $ref to an alias wrapping authored_map (gated, behind a
        // ref hop the old fail-open resolver could mis-grade); NEW is plain.
        let old = manifest_with_types(
            r#"[{"name":"wiki","type":{"$ref":"WikiT"}}]"#,
            r#","WikiT":{"kind":"alias","target":{"kind":"map","key":{"kind":"string"},"value":{"kind":"string"},"crdt_type":"authored_map"}}"#,
        );
        let new = manifest(&format!("[{UNORDERED_MAP}]"));
        let d = identity_downgrades(&old, &new);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].from, "AuthoredMap");
        assert_eq!(d[0].to, "UnorderedMap");
    }

    #[test]
    fn ref_to_record_is_plain_not_downgrade() {
        // A $ref to a record is a genuine non-CRDT top level: never flagged.
        let m = manifest_with_types(
            r#"[{"name":"meta","type":{"$ref":"Meta"}}]"#,
            r#","Meta":{"kind":"record","fields":[{"name":"x","type":{"kind":"string"}}]}"#,
        );
        assert!(identity_downgrades(&m, &m).is_empty());
    }

    #[test]
    fn dangling_ref_old_fails_closed() {
        // OLD field references a missing type: unresolvable must NOT be graded
        // "plain" and skipped — it fails closed and the move to a plain new type
        // is flagged.
        let old = manifest(r#"[{"name":"wiki","type":{"$ref":"Missing"}}]"#);
        let new = manifest(&format!("[{UNORDERED_MAP}]"));
        let d = identity_downgrades(&old, &new);
        assert_eq!(d.len(), 1, "unresolvable old must fail closed");
        assert_eq!(d[0].from, "(unresolved)");
    }

    #[test]
    fn cyclic_alias_fails_closed() {
        // A self-referential alias exhausts the depth budget: unresolvable, not
        // plain, so a downgrade to a plain new type is still caught.
        let old = manifest_with_types(
            r#"[{"name":"wiki","type":{"$ref":"CycleT"}}]"#,
            r#","CycleT":{"kind":"alias","target":{"$ref":"CycleT"}}"#,
        );
        let new = manifest(&format!("[{UNORDERED_MAP}]"));
        let d = identity_downgrades(&old, &new);
        assert_eq!(d.len(), 1, "cyclic old must fail closed");
    }

    #[test]
    fn unresolvable_carried_through_unchanged_is_not_downgrade() {
        // A field that is unresolvable in BOTH old and new (e.g. a legacy app
        // that has always had a dangling `$ref` and never used identity-gated
        // state) is unchanged — nothing downgraded — so it must NOT be flagged.
        // Fail-closed applies to the directions that could *lose* gating
        // (gated→plain, unresolvable→plain), not to an unchanged unknowable field.
        let m = manifest(r#"[{"name":"wiki","type":{"$ref":"Missing"}}]"#);
        assert!(identity_downgrades(&m, &m).is_empty());
    }

    #[test]
    fn unresolvable_old_to_resolvable_plain_still_fails_closed() {
        // The dangerous direction is preserved: an unresolvable old field moving
        // to a resolvably-plain new type can't be ruled out as a downgrade, so it
        // is still flagged (not exempted by the carry-through carve-out).
        let old = manifest(r#"[{"name":"wiki","type":{"$ref":"Missing"}}]"#);
        let new = manifest(&format!("[{UNORDERED_MAP}]"));
        let d = identity_downgrades(&old, &new);
        assert_eq!(d.len(), 1, "unresolvable old → plain new must fail closed");
        assert_eq!(d[0].from, "(unresolved)");
    }
}
