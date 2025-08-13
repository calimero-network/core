use thiserror::Error;

use crate::schema::{CollectionType, Manifest, Method, ScalarType, TypeDef, TypeRef};

#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("Invalid schema version: expected {expected}, found {found}")]
    InvalidSchemaVersion { expected: String, found: String },
    #[error("Methods not sorted: {first} comes after {second}")]
    MethodsNotSorted { first: String, second: String },
    #[error("Events not sorted: {first} comes after {second}")]
    EventsNotSorted { first: String, second: String },
    #[error("Invalid type reference: {ref_name} at {path}")]
    InvalidTypeReference { ref_name: String, path: String },
    #[error("Invalid bytes size: {path}")]
    InvalidBytesSize { path: String },
    #[error("Invalid map key type: {path}")]
    InvalidMapKeyType { path: String },
}

pub fn validate_manifest(manifest: &Manifest) -> Result<(), ValidationError> {
    // Validate schema version
    if manifest.schema_version != "wasm-abi/1" {
        return Err(ValidationError::InvalidSchemaVersion {
            expected: "wasm-abi/1".to_owned(),
            found: manifest.schema_version.clone(),
        });
    }

    // Validate types
    for (type_name, type_def) in &manifest.types {
        validate_type_def(type_def, &format!("types.{type_name}"))?;
    }

    // Validate methods are sorted
    for i in 1..manifest.methods.len() {
        if manifest.methods[i - 1].name > manifest.methods[i].name {
            return Err(ValidationError::MethodsNotSorted {
                first: manifest.methods[i - 1].name.clone(),
                second: manifest.methods[i].name.clone(),
            });
        }
    }

    // Validate events are sorted
    for i in 1..manifest.events.len() {
        if manifest.events[i - 1].name > manifest.events[i].name {
            return Err(ValidationError::EventsNotSorted {
                first: manifest.events[i - 1].name.clone(),
                second: manifest.events[i].name.clone(),
            });
        }
    }

    // Validate methods
    for (i, method) in manifest.methods.iter().enumerate() {
        validate_method(method, &format!("methods[{i}]"))?;
    }

    // Validate events
    for (i, event) in manifest.events.iter().enumerate() {
        validate_event(event, &format!("events[{i}]"))?;
    }

    // Validate type references
    let mut refs = Vec::new();
    collect_refs_from_manifest(manifest, "manifest", &mut refs);

    for (ref_name, path) in refs {
        if !manifest.types.contains_key(&ref_name) {
            return Err(ValidationError::InvalidTypeReference {
                ref_name,
                path: path.to_owned(),
            });
        }
    }

    Ok(())
}

fn validate_field(field: &crate::schema::Field, path: &str) -> Result<(), ValidationError> {
    validate_type_ref(&field.type_, path)?;
    Ok(())
}

fn validate_variant(variant: &crate::schema::Variant, path: &str) -> Result<(), ValidationError> {
    if let Some(payload) = &variant.payload {
        validate_type_ref(payload, &format!("{path}.payload"))?;
    }
    Ok(())
}

fn validate_type_ref(type_ref: &TypeRef, path: &str) -> Result<(), ValidationError> {
    match type_ref {
        TypeRef::Scalar(scalar) => {
            if let ScalarType::Bytes {
                size: Some(size),
                ..
            } = scalar
            {
                if *size == 0 {
                    return Err(ValidationError::InvalidBytesSize {
                        path: path.to_owned(),
                    });
                }
            }
        }
        TypeRef::Collection(collection) => {
            match collection {
                CollectionType::Record { fields } => {
                    for field in fields {
                        validate_field(field, path)?;
                    }
                }
                CollectionType::List { items } => {
                    validate_type_ref(items, &format!("{path}.items"))?;
                }
                CollectionType::Map { key, value } => {
                    if let TypeRef::Scalar(ScalarType::String) = &**key {
                        // Valid key type
                    } else {
                        return Err(ValidationError::InvalidMapKeyType {
                            path: path.to_owned(),
                        });
                    }
                    validate_type_ref(value, &format!("{path}.value"))?;
                }
            }
        }
        TypeRef::Reference { .. } => {
            // References are validated separately
        }
    }
    Ok(())
}

fn validate_method(method: &Method, path: &str) -> Result<(), ValidationError> {
    // Validate parameters
    for (i, param) in method.params.iter().enumerate() {
        let param_path = format!("{path}.params[{i}]");
        validate_type_ref(&param.type_, &param_path)?;
    }

    // Validate return type
    if let Some(returns) = &method.returns {
        collect_refs_from_type_ref(returns, &format!("{path}.returns"), &mut Vec::new());
    }

    // Validate errors
    for (i, error) in method.errors.iter().enumerate() {
        let error_path = format!("{path}.errors[{i}]");
        if let Some(payload) = &error.payload {
            collect_refs_from_type_ref(payload, &format!("{error_path}.payload"), &mut Vec::new());
        }
    }

    Ok(())
}

fn validate_event(event: &crate::schema::Event, path: &str) -> Result<(), ValidationError> {
    if let Some(payload) = &event.payload {
        collect_refs_from_type_ref(payload, &format!("{path}.payload"), &mut Vec::new());
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
        TypeDef::Bytes { .. } => {
            // Bytes type is always valid
        }
    }
    Ok(())
}

fn collect_refs_from_manifest(manifest: &Manifest, path: &str, refs: &mut Vec<(String, String)>) {
    // Collect from methods
    for (i, method) in manifest.methods.iter().enumerate() {
        let method_path = format!("{path}.methods[{i}]");
        collect_refs_from_method(method, &method_path, refs);
    }

    // Collect from events
    for (i, event) in manifest.events.iter().enumerate() {
        let event_path = format!("{path}.events[{i}]");
        collect_refs_from_event(event, &event_path, refs);
    }

    // Collect from types
    for (type_name, type_def) in &manifest.types {
        let type_path = format!("{path}.types.{type_name}");
        collect_refs_from_type_def(type_def, &type_path, refs);
    }
}

fn collect_refs_from_method(method: &Method, path: &str, refs: &mut Vec<(String, String)>) {
    // Collect from parameters
    for (i, param) in method.params.iter().enumerate() {
        let param_path = format!("{path}.params[{i}]");
        collect_refs_from_type_ref(&param.type_, &param_path, refs);
    }

    // Collect from return type
    if let Some(returns) = &method.returns {
        collect_refs_from_type_ref(returns, &format!("{path}.returns"), refs);
    }

    // Collect from errors
    for (i, error) in method.errors.iter().enumerate() {
        let error_path = format!("{path}.errors[{i}]");
        if let Some(payload) = &error.payload {
            collect_refs_from_type_ref(payload, &format!("{error_path}.payload"), refs);
        }
    }
}

fn collect_refs_from_event(event: &crate::schema::Event, path: &str, refs: &mut Vec<(String, String)>) {
    if let Some(payload) = &event.payload {
        collect_refs_from_type_ref(payload, &format!("{path}.payload"), refs);
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
            for (i, variant) in variants.iter().enumerate() {
                let variant_path = format!("{path}.variants[{i}]");
                if let Some(payload) = &variant.payload {
                    collect_refs_from_type_ref(payload, &format!("{variant_path}.payload"), refs);
                }
            }
        }
        TypeDef::Bytes { .. } => {
            // No references in bytes type
        }
    }
}

fn collect_refs_from_type_ref(type_ref: &TypeRef, path: &str, refs: &mut Vec<(String, String)>) {
    match type_ref {
        TypeRef::Reference { ref_ } => {
            refs.push((ref_.clone(), path.to_owned()));
        }
        TypeRef::Collection(collection) => {
            match collection {
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
            }
        }
        TypeRef::Scalar(_) => {
            // No references in scalar types
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Event, Field, Parameter, Variant};

    #[test]
    fn test_valid_manifest() {
        let mut manifest = Manifest::new();
        
        // Add a test type
        manifest
            .types
            .insert("TestType".to_owned(), TypeDef::Record { fields: vec![] });

        // Add a test method
        manifest.methods.push(Method {
            name: "test_method".to_owned(),
            params: vec![Parameter {
                name: "param1".to_owned(),
                type_: TypeRef::string(),
                nullable: None,
            }],
            returns: Some(TypeRef::string()),
            returns_nullable: None,
            errors: Vec::new(),
        });

        // Add a test event
        manifest.events.push(Event {
            name: "test".to_owned(),
            payload: None,
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
            errors: Vec::new(),
        });

        assert!(validate_manifest(&manifest).is_err());
    }
}
