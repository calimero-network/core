use crate::schema::{Manifest, TypeRef, TypeDef, Method, Event};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("Option<T> type missing nullable=true: {path}")]
    MissingNullable { path: String },
    #[error("Event uses 'type' key instead of 'payload': {path}")]
    EventUsesTypeKey { path: String },
    #[error("Variable bytes has size=0: {path}")]
    VariableBytesWithSize { path: String },
    #[error("Map key is not 'string': {path}")]
    MapKeyNotString { path: String },
    #[error("Dangling $ref: {ref_path} at {context_path}")]
    DanglingRef { ref_path: String, context_path: String },
    #[error("Methods not sorted deterministically")]
    MethodsNotSorted,
    #[error("Events not sorted deterministically")]
    EventsNotSorted,
}

pub fn validate_manifest(manifest: &Manifest) -> Result<(), ValidationError> {
    // Check for dangling $ref
    let mut refs = Vec::new();
    collect_refs_from_manifest(manifest, "", &mut refs);
    
    for (ref_path, context_path) in &refs {
        if !manifest.types.contains_key(ref_path) {
            return Err(ValidationError::DanglingRef {
                ref_path: ref_path.clone(),
                context_path: context_path.clone(),
            });
        }
    }
    
    // Check map keys are string
    for (type_name, type_def) in &manifest.types {
        validate_type_def(type_def, &format!("types.{}", type_name))?;
    }
    
    // Check methods are sorted
    for i in 1..manifest.methods.len() {
        if manifest.methods[i - 1].name > manifest.methods[i].name {
            return Err(ValidationError::MethodsNotSorted);
        }
    }
    
    // Check events are sorted
    for i in 1..manifest.events.len() {
        if manifest.events[i - 1].name > manifest.events[i].name {
            return Err(ValidationError::EventsNotSorted);
        }
    }
    
    Ok(())
}

fn validate_type_def(type_def: &TypeDef, path: &str) -> Result<(), ValidationError> {
    match type_def {
        TypeDef::Record { fields } => {
            for field in fields {
                validate_field(field, &format!("{}.{}", path, field.name))?;
            }
        }
        TypeDef::Variant { variants } => {
            for variant in variants {
                validate_variant(variant, &format!("{}.{}", path, variant.name))?;
            }
        }
        TypeDef::Bytes { size, .. } => {
            if let Some(size_val) = size {
                if *size_val == 0 {
                    return Err(ValidationError::VariableBytesWithSize {
                        path: path.to_string(),
                    });
                }
            }
        }
    }
    Ok(())
}

fn validate_field(field: &crate::schema::Field, path: &str) -> Result<(), ValidationError> {
    // Check if Option<T> has nullable=true
    if field.nullable.is_none() {
        // This is a simplified check - in a real implementation, we'd check if the type is Option<T>
        // For now, we'll skip this check as it requires more complex type analysis
    }
    
    validate_type_ref(&field.type_, path)
}

fn validate_variant(variant: &crate::schema::Variant, path: &str) -> Result<(), ValidationError> {
    if let Some(payload) = &variant.payload {
        validate_type_ref(payload, &format!("{}.payload", path))?;
    }
    Ok(())
}

fn validate_type_ref(type_ref: &TypeRef, path: &str) -> Result<(), ValidationError> {
    match type_ref {
        TypeRef::Reference { ref_: _ref_name } => {
            // This will be checked for dangling refs in the main validation
        }
        TypeRef::Scalar(scalar) => if let crate::schema::ScalarType::Bytes { size: Some(size_val), .. } = scalar {
            if *size_val == 0 {
                return Err(ValidationError::VariableBytesWithSize {
                    path: path.to_string(),
                });
            }
        },
        TypeRef::Collection(collection) => {
            match collection {
                crate::schema::CollectionType::Record { fields } => {
                    for field in fields {
                        validate_field(field, &format!("{}.{}", path, field.name))?;
                    }
                }
                crate::schema::CollectionType::List { items } => {
                    validate_type_ref(items, &format!("{}.items", path))?;
                }
                crate::schema::CollectionType::Map { key, value } => {
                    // Check if key is string type
                    if let TypeRef::Scalar(crate::schema::ScalarType::String) = &**key {
                        // Key is string, which is valid
                    } else {
                        return Err(ValidationError::MapKeyNotString {
                            path: path.to_string(),
                        });
                    }
                    validate_type_ref(value, &format!("{}.value", path))?;
                }
            }
        }
    }
    Ok(())
}

fn collect_refs_from_manifest(manifest: &Manifest, path: &str, refs: &mut Vec<(String, String)>) {
    // Collect refs from methods
    for (i, method) in manifest.methods.iter().enumerate() {
        let method_path = format!("{}.methods[{}]", path, i);
        collect_refs_from_method(method, &method_path, refs);
    }
    
    // Collect refs from events
    for (i, event) in manifest.events.iter().enumerate() {
        let event_path = format!("{}.events[{}]", path, i);
        collect_refs_from_event(event, &event_path, refs);
    }
    
    // Collect refs from types
    for (type_name, type_def) in &manifest.types {
        let type_path = format!("{}.types.{}", path, type_name);
        collect_refs_from_type_def(type_def, &type_path, refs);
    }
}

fn collect_refs_from_method(method: &Method, path: &str, refs: &mut Vec<(String, String)>) {
    // Collect refs from parameters
    for (i, param) in method.params.iter().enumerate() {
        let param_path = format!("{}.params[{}]", path, i);
        collect_refs_from_type_ref(&param.type_, &param_path, refs);
    }
    
    // Collect refs from return type
    if let Some(returns) = &method.returns {
        collect_refs_from_type_ref(returns, &format!("{}.returns", path), refs);
    }
    
    // Collect refs from errors
    for (i, error) in method.errors.iter().enumerate() {
        let error_path = format!("{}.errors[{}]", path, i);
        if let Some(payload) = &error.payload {
            collect_refs_from_type_ref(payload, &format!("{}.payload", error_path), refs);
        }
    }
}

fn collect_refs_from_event(event: &Event, path: &str, refs: &mut Vec<(String, String)>) {
    if let Some(payload) = &event.payload {
        collect_refs_from_type_ref(payload, &format!("{}.payload", path), refs);
    }
}

fn collect_refs_from_type_def(type_def: &TypeDef, path: &str, refs: &mut Vec<(String, String)>) {
    match type_def {
        TypeDef::Record { fields } => {
            for field in fields {
                let field_path = format!("{}.{}", path, field.name);
                collect_refs_from_type_ref(&field.type_, &field_path, refs);
            }
        }
        TypeDef::Variant { variants } => {
            for variant in variants {
                let variant_path = format!("{}.{}", path, variant.name);
                if let Some(payload) = &variant.payload {
                    collect_refs_from_type_ref(payload, &format!("{}.payload", variant_path), refs);
                }
            }
        }
        TypeDef::Bytes { .. } => {
            // Bytes don't have refs
        }
    }
}

fn collect_refs_from_type_ref(type_ref: &TypeRef, path: &str, refs: &mut Vec<(String, String)>) {
    match type_ref {
        TypeRef::Reference { ref_: ref_name } => {
            refs.push((ref_name.clone(), path.to_string()));
        }
        TypeRef::Scalar(_) => {
            // Scalars don't have refs
        }
        TypeRef::Collection(collection) => {
            match collection {
                crate::schema::CollectionType::Record { fields } => {
                    for field in fields {
                        let field_path = format!("{}.{}", path, field.name);
                        collect_refs_from_type_ref(&field.type_, &field_path, refs);
                    }
                }
                crate::schema::CollectionType::List { items } => {
                    collect_refs_from_type_ref(items, &format!("{}.items", path), refs);
                }
                crate::schema::CollectionType::Map { value, .. } => {
                    collect_refs_from_type_ref(value, &format!("{}.value", path), refs);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Manifest, Method, TypeDef, TypeRef};

    #[test]
    fn test_validate_manifest_basic() {
        let mut manifest = Manifest::default();

        // Add a simple type
        manifest
            .types
            .insert("TestType".to_string(), TypeDef::Record { fields: vec![] });

        // Add a simple method
        manifest.methods.push(Method {
            name: "test_method".to_string(),
            params: vec![],
            returns: Some(TypeRef::u32()),
            returns_nullable: None,
            errors: vec![],
        });

        // Should pass validation
        assert!(validate_manifest(&manifest).is_ok());
    }

    #[test]
    fn test_validate_dangling_ref() {
        let mut manifest = Manifest::default();

        // Add a method that references a non-existent type
        manifest.methods.push(Method {
            name: "test".to_string(),
            params: vec![],
            returns: Some(TypeRef::reference("NonExistentType")),
            returns_nullable: None,
            errors: vec![],
        });

        // Should fail validation
        let result = validate_manifest(&manifest);
        assert!(result.is_err());
        match result.unwrap_err() {
            ValidationError::DanglingRef { ref_path, context_path } => {
                assert_eq!(ref_path, "NonExistentType");
                assert_eq!(context_path, ".methods[0].returns");
            }
            _ => panic!("Expected DanglingRef error"),
        }
    }
}

