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

/// Resolve a field's *top-level* CRDT collection type, following `$ref`/alias
/// hops, if its top level is a CRDT collection. Cycle-guarded.
fn top_level_crdt(ty: &TypeRef, manifest: &Manifest, depth: u8) -> Option<CrdtCollectionType> {
    if depth > 32 {
        return None;
    }
    let value = serde_json::to_value(ty).ok()?;
    if let Some(ct) = value.get("crdt_type") {
        return serde_json::from_value::<CrdtCollectionType>(ct.clone()).ok();
    }
    if let Some(serde_json::Value::String(name)) = value.get("$ref") {
        match manifest.types.get(name)? {
            TypeDef::Alias { target } => return top_level_crdt(target, manifest, depth + 1),
            _ => return None,
        }
    }
    None
}

fn is_identity_gated(ty: &TypeRef, manifest: &Manifest) -> bool {
    top_level_crdt(ty, manifest, 0)
        .is_some_and(|ct| collection_category(&ct) == CollectionCategory::IdentityGated)
}

fn label(ty: &TypeRef, manifest: &Manifest) -> String {
    top_level_crdt(ty, manifest, 0).map_or_else(|| "plain".to_owned(), |ct| format!("{ct:?}"))
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
}
