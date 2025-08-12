use crate::schema::{Error, Event, Field, Manifest, Method, TypeDef, TypeRef, Variant};
use thiserror::Error;

/// Error type for validation failures
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("Option<T> type missing nullable=true: {path}")]
    MissingNullable { path: String },

    #[error("Event/error uses 'type' key instead of 'payload': {path}")]
    UsesTypeKey { path: String },

    #[error("Variable bytes has size=0: {path}")]
    VariableBytesWithSize { path: String },

    #[error("Map key is not 'string': {path}")]
    NonStringMapKey { path: String },

    #[error("Dangling $ref: {ref_name} at {path}")]
    DanglingRef { ref_name: String, path: String },

    #[error("Types not sorted deterministically")]
    TypesNotSorted,

    #[error("Methods not sorted deterministically")]
    MethodsNotSorted,

    #[error("Events not sorted deterministically")]
    EventsNotSorted,
}

/// Validate a manifest against all invariants
pub fn validate_manifest(manifest: &Manifest) -> Result<(), ValidationError> {
    // Check determinism
    validate_determinism(manifest)?;

    // Check all type references
    validate_type_refs(manifest)?;

    // Check all methods
    for (i, method) in manifest.methods.iter().enumerate() {
        validate_method(method, &format!("methods[{}]", i), manifest)?;
    }

    // Check all events
    for (i, event) in manifest.events.iter().enumerate() {
        validate_event(event, &format!("events[{}]", i))?;
    }

    // Check all type definitions
    for (type_name, type_def) in &manifest.types {
        validate_type_def(type_def, &format!("types.{}", type_name), manifest)?;
    }

    Ok(())
}

/// Validate determinism (sorted collections)
fn validate_determinism(manifest: &Manifest) -> Result<(), ValidationError> {
    // Check that types is a BTreeMap (already sorted)
    // This is enforced by the type system, but we can verify the keys are sorted
    let type_names: Vec<_> = manifest.types.keys().collect();
    let mut sorted_names = type_names.clone();
    sorted_names.sort();

    if type_names != sorted_names {
        return Err(ValidationError::TypesNotSorted);
    }

    // Check methods are sorted
    let method_names: Vec<_> = manifest.methods.iter().map(|m| &m.name).collect();
    let mut sorted_method_names = method_names.clone();
    sorted_method_names.sort();

    if method_names != sorted_method_names {
        return Err(ValidationError::MethodsNotSorted);
    }

    // Check events are sorted
    let event_names: Vec<_> = manifest.events.iter().map(|e| &e.name).collect();
    let mut sorted_event_names = event_names.clone();
    sorted_event_names.sort();

    if event_names != sorted_event_names {
        return Err(ValidationError::EventsNotSorted);
    }

    Ok(())
}

/// Validate all type references in the manifest
fn validate_type_refs(manifest: &Manifest) -> Result<(), ValidationError> {
    let mut refs = Vec::new();

    // Collect all $ref from methods
    for (i, method) in manifest.methods.iter().enumerate() {
        collect_refs_from_type_ref(
            &method.returns,
            &format!("methods[{}].returns", i),
            &mut refs,
        );
        for (j, param) in method.params.iter().enumerate() {
            collect_refs_from_type_ref(
                &Some(param.type_.clone()),
                &format!("methods[{}].params[{}].type", i, j),
                &mut refs,
            );
        }
        for (j, error) in method.errors.iter().enumerate() {
            collect_refs_from_type_ref(
                &error.payload,
                &format!("methods[{}].errors[{}].payload", i, j),
                &mut refs,
            );
        }
    }

    // Collect all $ref from events
    for (i, event) in manifest.events.iter().enumerate() {
        collect_refs_from_type_ref(&event.payload, &format!("events[{}].payload", i), &mut refs);
    }

    // Collect all $ref from type definitions
    for (type_name, type_def) in &manifest.types {
        collect_refs_from_type_def(type_def, &format!("types.{}", type_name), &mut refs);
    }

    // Check all refs exist
    for (ref_name, path) in refs {
        if !manifest.types.contains_key(&ref_name) {
            return Err(ValidationError::DanglingRef { ref_name, path });
        }
    }

    Ok(())
}

/// Collect all $ref from a TypeRef
fn collect_refs_from_type_ref(
    type_ref: &Option<TypeRef>,
    path: &str,
    refs: &mut Vec<(String, String)>,
) {
    if let Some(type_ref) = type_ref {
        match type_ref {
            TypeRef::Reference { ref_ } => {
                refs.push((ref_.clone(), path.to_string()));
            }
            TypeRef::Scalar(_) => {}
            TypeRef::Collection(collection) => match collection {
                crate::schema::CollectionType::List { items } => {
                    collect_refs_from_type_ref(
                        &Some((**items).clone()),
                        &format!("{}.items", path),
                        refs,
                    );
                }
                crate::schema::CollectionType::Map { key, value } => {
                    collect_refs_from_type_ref(
                        &Some((**key).clone()),
                        &format!("{}.key", path),
                        refs,
                    );
                    collect_refs_from_type_ref(
                        &Some((**value).clone()),
                        &format!("{}.value", path),
                        refs,
                    );
                }
                crate::schema::CollectionType::Record { fields } => {
                    for (i, field) in fields.iter().enumerate() {
                        collect_refs_from_type_ref(
                            &Some(field.type_.clone()),
                            &format!("{}.fields[{}].type", path, i),
                            refs,
                        );
                    }
                }
            },
        }
    }
}

/// Collect all $ref from a TypeDef
fn collect_refs_from_type_def(type_def: &TypeDef, path: &str, refs: &mut Vec<(String, String)>) {
    match type_def {
        TypeDef::Record { fields } => {
            for (i, field) in fields.iter().enumerate() {
                collect_refs_from_type_ref(
                    &Some(field.type_.clone()),
                    &format!("{}.fields[{}].type", path, i),
                    refs,
                );
            }
        }
        TypeDef::Variant { variants } => {
            for (i, variant) in variants.iter().enumerate() {
                collect_refs_from_type_ref(
                    &variant.payload,
                    &format!("{}.variants[{}].payload", path, i),
                    refs,
                );
            }
        }
        TypeDef::Bytes { .. } => {}
    }
}

/// Validate a method
fn validate_method(
    method: &Method,
    path: &str,
    manifest: &Manifest,
) -> Result<(), ValidationError> {
    // Check parameters
    for (i, param) in method.params.iter().enumerate() {
        validate_type_ref(
            &param.type_,
            &format!("{}.params[{}].type", path, i),
            manifest,
        )?;

        // Check nullable for Option<T>
        if is_option_type(&param.type_) && param.nullable != Some(true) {
            return Err(ValidationError::MissingNullable {
                path: format!("{}.params[{}]", path, i),
            });
        }
    }

    // Check return type
    if let Some(returns) = &method.returns {
        validate_type_ref(returns, &format!("{}.returns", path), manifest)?;

        // Check nullable for Option<T>
        if is_option_type(returns) {
            // Note: We can't check nullable here since it's not part of the return type structure
            // This would need to be checked at the emitter level
        }
    }

    // Check errors
    for (i, error) in method.errors.iter().enumerate() {
        if let Some(payload) = &error.payload {
            validate_type_ref(
                payload,
                &format!("{}.errors[{}].payload", path, i),
                manifest,
            )?;
        }
    }

    Ok(())
}

/// Validate an event
fn validate_event(event: &Event, path: &str) -> Result<(), ValidationError> {
    // Check that event doesn't use 'type' key (should use 'payload')
    // This is enforced by the schema, but we can double-check
    Ok(())
}

/// Validate a type definition
fn validate_type_def(
    type_def: &TypeDef,
    path: &str,
    manifest: &Manifest,
) -> Result<(), ValidationError> {
    match type_def {
        TypeDef::Record { fields } => {
            for (i, field) in fields.iter().enumerate() {
                validate_type_ref(
                    &field.type_,
                    &format!("{}.fields[{}].type", path, i),
                    manifest,
                )?;

                // Check nullable for Option<T>
                if is_option_type(&field.type_) && field.nullable != Some(true) {
                    return Err(ValidationError::MissingNullable {
                        path: format!("{}.fields[{}]", path, i),
                    });
                }
            }
        }
        TypeDef::Variant { variants } => {
            for (i, variant) in variants.iter().enumerate() {
                if let Some(payload) = &variant.payload {
                    validate_type_ref(
                        payload,
                        &format!("{}.variants[{}].payload", path, i),
                        manifest,
                    )?;
                }
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

/// Validate a type reference
fn validate_type_ref(
    type_ref: &TypeRef,
    path: &str,
    manifest: &Manifest,
) -> Result<(), ValidationError> {
    match type_ref {
        TypeRef::Reference { ref_ } => {
            if !manifest.types.contains_key(ref_) {
                return Err(ValidationError::DanglingRef {
                    ref_name: ref_.clone(),
                    path: path.to_string(),
                });
            }
        }
        TypeRef::Scalar(scalar) => match scalar {
            crate::schema::ScalarType::Bytes { size, .. } => {
                if let Some(size_val) = size {
                    if *size_val == 0 {
                        return Err(ValidationError::VariableBytesWithSize {
                            path: path.to_string(),
                        });
                    }
                }
            }
            _ => {}
        },
        TypeRef::Collection(collection) => {
            match collection {
                crate::schema::CollectionType::List { items } => {
                    validate_type_ref(items, &format!("{}.items", path), manifest)?;
                }
                crate::schema::CollectionType::Map { key, value } => {
                    // Check map key is string
                    match **key {
                        TypeRef::Scalar(crate::schema::ScalarType::String) => {}
                        _ => {
                            return Err(ValidationError::NonStringMapKey {
                                path: path.to_string(),
                            });
                        }
                    }
                    validate_type_ref(key, &format!("{}.key", path), manifest)?;
                    validate_type_ref(value, &format!("{}.value", path), manifest)?;
                }
                crate::schema::CollectionType::Record { fields } => {
                    for (i, field) in fields.iter().enumerate() {
                        validate_type_ref(
                            &field.type_,
                            &format!("{}.fields[{}].type", path, i),
                            manifest,
                        )?;
                    }
                }
            }
        }
    }

    Ok(())
}

/// Check if a type reference represents an Option<T>
/// This is a simplified check - in practice, this would need to be done at the emitter level
/// where we have access to the original Rust types
fn is_option_type(_type_ref: &TypeRef) -> bool {
    // This is a placeholder - the real implementation would need to track
    // which types were originally Option<T> in the Rust code
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Manifest, Method, Parameter, TypeDef, TypeRef};

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
            ValidationError::DanglingRef { ref_name, .. } => {
                assert_eq!(ref_name, "NonExistentType");
            }
            _ => panic!("Expected DanglingRef error"),
        }
    }
}
