use std::collections::BTreeMap;

use crate::schema::{
    CollectionType, Event, Field, Manifest, Method, Parameter, ScalarType, TypeDef, TypeRef,
    Variant,
};

/// Validation error types
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("invalid schema version: {0}")]
    InvalidSchemaVersion(String),
    #[error("invalid type reference: {ref_name} at {path}")]
    InvalidTypeReference { ref_name: String, path: String },
    #[error("invalid bytes size: {size} at {path}")]
    InvalidBytesSize { size: usize, path: String },
    #[error("invalid map key type at {path}")]
    InvalidMapKeyType { path: String },
    #[error("methods not sorted: {first} > {second}")]
    MethodsNotSorted { first: String, second: String },
    #[error("events not sorted: {first} > {second}")]
    EventsNotSorted { first: String, second: String },
}

/// Validate a manifest
pub fn validate_manifest(manifest: &Manifest) -> Result<(), ValidationError> {
    // Check schema version
    if manifest.schema_version != "wasm-abi/1" {
        return Err(ValidationError::InvalidSchemaVersion(
            manifest.schema_version.clone(),
        ));
    }

    // Check that methods are sorted
    for i in 1..manifest.methods.len() {
        if manifest.methods[i.saturating_sub(1)].name > manifest.methods[i].name {
            return Err(ValidationError::MethodsNotSorted {
                first: manifest.methods[i.saturating_sub(1)].name.clone(),
                second: manifest.methods[i].name.clone(),
            });
        }
    }

    // Check that events are sorted
    for i in 1..manifest.events.len() {
        if manifest.events[i.saturating_sub(1)].name > manifest.events[i].name {
            return Err(ValidationError::EventsNotSorted {
                first: manifest.events[i.saturating_sub(1)].name.clone(),
                second: manifest.events[i].name.clone(),
            });
        }
    }

    // Validate all types
    for (name, type_def) in &manifest.types {
        validate_type_def(type_def, name)?;
    }

    // Validate all methods
    for method in &manifest.methods {
        validate_method(method, &manifest.types)?;
    }

    // Validate all events
    for event in &manifest.events {
        validate_event(event, &manifest.types);
    }

    // Check for dangling references
    let mut refs = Vec::new();
    collect_refs_from_manifest(manifest, &mut refs);

    for (ref_name, path) in refs {
        if !manifest.types.contains_key(&ref_name) {
            return Err(ValidationError::InvalidTypeReference { ref_name, path });
        }
    }

    Ok(())
}

fn validate_field(field: &Field, path: &str) -> Result<(), ValidationError> {
    validate_type_ref(&field.type_, path)
}

fn validate_variant(variant: &Variant, path: &str) -> Result<(), ValidationError> {
    if let Some(payload) = &variant.payload {
        validate_type_ref(payload, path)?;
    }
    Ok(())
}

fn validate_type_def(type_def: &TypeDef, path: &str) -> Result<(), ValidationError> {
    match type_def {
        TypeDef::Record { fields } => {
            for field in fields {
                validate_field(field, path)?;
            }
        }
        TypeDef::Variant { variants } => {
            for variant in variants {
                validate_variant(variant, path)?;
            }
        }
        TypeDef::Bytes { size, .. } => {
            if let Some(size) = size {
                if *size == 0 {
                    return Err(ValidationError::InvalidBytesSize {
                        size: *size,
                        path: path.to_owned(),
                    });
                }
            }
        }
        TypeDef::Alias { target } => {
            validate_type_ref(target, path)?;
        }
    }
    Ok(())
}

fn validate_type_ref(type_ref: &TypeRef, path: &str) -> Result<(), ValidationError> {
    match type_ref {
        TypeRef::Scalar(scalar) => {
            if let ScalarType::Bytes {
                size: Some(size), ..
            } = scalar
            {
                if *size == 0 {
                    return Err(ValidationError::InvalidBytesSize {
                        size: *size,
                        path: path.to_owned(),
                    });
                }
            }
        }
        TypeRef::Reference { .. } => {
            // Will be checked for dangling refs later
        }
        TypeRef::Collection(collection) => match collection {
            CollectionType::Record { fields } => {
                for field in fields {
                    validate_type_ref(&field.type_, path)?;
                }
            }
            CollectionType::List { items } => {
                validate_type_ref(items, &format!("{path}.items"))?;
            }
            CollectionType::Map { key, value, .. } => {
                // Check that key is string
                if !matches!(&**key, TypeRef::Scalar(ScalarType::String)) {
                    return Err(ValidationError::InvalidMapKeyType {
                        path: path.to_owned(),
                    });
                }
                validate_type_ref(value, &format!("{path}.value"))?;
            }
        },
    }
    Ok(())
}

fn validate_method(
    method: &Method,
    types: &BTreeMap<String, TypeDef>,
) -> Result<(), ValidationError> {
    for param in &method.params {
        validate_parameter(param, types)?;
    }

    if let Some(returns) = &method.returns {
        validate_type_ref(returns, &format!("method {}.returns", method.name))?;
    }

    Ok(())
}

fn validate_parameter(
    param: &Parameter,
    _types: &BTreeMap<String, TypeDef>,
) -> Result<(), ValidationError> {
    validate_type_ref(&param.type_, &format!("parameter {}", param.name))
}

fn validate_event(event: &Event, _types: &BTreeMap<String, TypeDef>) {
    if let Some(payload) = &event.payload {
        collect_refs_from_type_ref(
            payload,
            &format!("event {}.payload", event.name),
            &mut Vec::new(),
        );
    }
}

fn collect_refs_from_manifest(manifest: &Manifest, refs: &mut Vec<(String, String)>) {
    for (name, type_def) in &manifest.types {
        collect_refs_from_type_def(type_def, name, refs);
    }

    for method in &manifest.methods {
        for param in &method.params {
            collect_refs_from_type_ref(
                &param.type_,
                &format!("method {}.param {}", method.name, param.name),
                refs,
            );
        }

        if let Some(returns) = &method.returns {
            collect_refs_from_type_ref(returns, &format!("method {}.returns", method.name), refs);
        }
    }

    for event in &manifest.events {
        collect_refs_from_event(event, &format!("event {}", event.name), refs);
    }
}

fn collect_refs_from_type_def(type_def: &TypeDef, path: &str, refs: &mut Vec<(String, String)>) {
    match type_def {
        TypeDef::Record { fields } => {
            for field in fields {
                collect_refs_from_type_ref(&field.type_, path, refs);
            }
        }
        TypeDef::Variant { variants } => {
            for variant in variants {
                if let Some(payload) = &variant.payload {
                    collect_refs_from_type_ref(payload, path, refs);
                }
            }
        }
        TypeDef::Bytes { .. } => {
            // No references in bytes types
        }
        TypeDef::Alias { target } => {
            collect_refs_from_type_ref(target, path, refs);
        }
    }
}

fn collect_refs_from_type_ref(type_ref: &TypeRef, path: &str, refs: &mut Vec<(String, String)>) {
    match type_ref {
        TypeRef::Scalar(_) => {
            // No references in scalar types
        }
        TypeRef::Reference { ref_ } => {
            refs.push((ref_.clone(), path.to_owned()));
        }
        TypeRef::Collection(collection) => match collection {
            CollectionType::Record { fields } => {
                for field in fields {
                    collect_refs_from_type_ref(&field.type_, path, refs);
                }
            }
            CollectionType::List { items } => {
                collect_refs_from_type_ref(items, &format!("{path}.items"), refs);
            }
            CollectionType::Map { value, .. } => {
                collect_refs_from_type_ref(value, &format!("{path}.value"), refs);
            }
        },
    }
}

fn collect_refs_from_event(event: &Event, path: &str, refs: &mut Vec<(String, String)>) {
    if let Some(payload) = &event.payload {
        collect_refs_from_type_ref(payload, &format!("{path}.payload"), refs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_manifest() {
        let mut manifest = Manifest::new();

        // Add a test type
        let _ = manifest
            .types
            .insert("TestType".to_owned(), TypeDef::Record { fields: vec![] });

        // Add a test method
        manifest.methods.push(Method {
            name: "test_method".to_owned(),
            params: vec![],
            returns: Some(TypeRef::u32()),
            returns_nullable: None,
            errors: vec![],
        });

        assert!(validate_manifest(&manifest).is_ok());
    }

    #[test]
    fn test_invalid_schema_version() {
        let mut manifest = Manifest::new();
        manifest.schema_version = "invalid".to_owned();

        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn test_invalid_type_reference() {
        let mut manifest = Manifest::new();

        // Add a method that references a non-existent type
        manifest.methods.push(Method {
            name: "test_method".to_owned(),
            params: vec![],
            returns: Some(TypeRef::reference("NonExistentType")),
            returns_nullable: None,
            errors: vec![],
        });

        assert!(validate_manifest(&manifest).is_err());
    }
}
