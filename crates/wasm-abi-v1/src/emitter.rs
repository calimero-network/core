use crate::normalize::{normalize_type, ResolvedLocal, TypeResolver};
use crate::schema::{Field, Manifest, Method, Parameter, ScalarType, TypeDef, TypeRef, Variant};
use std::collections::HashMap;
use std::error;
use syn::{visit::Visit, Item, ItemEnum, ItemImpl, ItemStruct, Type};

/// ABI emitter that processes Rust source code
#[derive(Debug)]
pub struct AbiEmitter {
    type_definitions: HashMap<String, TypeDef>,
    local_types: HashMap<String, ResolvedLocal>,
}

impl AbiEmitter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            type_definitions: HashMap::new(),
            local_types: HashMap::new(),
        }
    }

    fn add_type_definition(&mut self, name: &str, type_def: TypeDef) {
        drop(self.type_definitions.insert(name.to_owned(), type_def));
    }

    fn add_local_type(&mut self, name: String, resolved: ResolvedLocal) {
        match resolved {
            ResolvedLocal::NewtypeBytes { size } => {
                let _ = self.local_types.insert(name, ResolvedLocal::NewtypeBytes { size });
            }
            ResolvedLocal::Record => {
                let _ = self.local_types.insert(name, ResolvedLocal::Record);
            }
            ResolvedLocal::Variant => {
                let _ = self.local_types.insert(name, ResolvedLocal::Variant);
            }
        }
    }
}

impl Default for AbiEmitter {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeResolver for AbiEmitter {
    fn resolve_local(&self, name: &str) -> Option<ResolvedLocal> {
        self.local_types.get(name).copied()
    }
}

/// Check if a struct is a newtype pattern (single unnamed field)
fn is_newtype_pattern(item_struct: &ItemStruct) -> bool {
    matches!(&item_struct.fields, syn::Fields::Unnamed(fields) if fields.unnamed.len() == 1)
}

/// Extract the target type from a newtype struct
fn extract_newtype_target(item_struct: &ItemStruct) -> Option<&Type> {
    if let syn::Fields::Unnamed(fields) = &item_struct.fields {
        if fields.unnamed.len() == 1 {
            return Some(&fields.unnamed[0].ty);
        }
    }
    None
}

/// Post-process a TypeRef to handle any special cases
fn post_process_type_ref(type_ref: TypeRef, _resolver: &dyn TypeResolver) -> TypeRef {
    type_ref
}

impl<'ast> Visit<'ast> for AbiEmitter {
    fn visit_item_struct(&mut self, item_struct: &'ast ItemStruct) {
        let struct_name = item_struct.ident.to_string();

        // Check if this is a newtype pattern
        if is_newtype_pattern(item_struct) {
            if let Some(target_type) = extract_newtype_target(item_struct) {
                let target_type_ref =
                    normalize_type(target_type, true, self).unwrap();
                let target_type_ref = post_process_type_ref(target_type_ref, self);

                // Add as an alias type definition
                self.add_type_definition(
                    &struct_name,
                    TypeDef::Alias {
                        target: target_type_ref,
                    },
                );
            }
        } else {
            // Process struct fields to generate type definitions
            let mut fields = Vec::new();

            for field in &item_struct.fields {
                let field_name = field
                    .ident
                    .as_ref()
                    .map_or_else(|| "unnamed".to_owned(), ToString::to_string);

                let field_type = normalize_type(&field.ty, true, self).unwrap();
                let field_type = post_process_type_ref(field_type, self);

                fields.push(Field {
                    name: field_name,
                    type_: field_type,
                    nullable: None, // Will be set based on Option<T>
                });
            }

            self.add_type_definition(&struct_name, TypeDef::Record { fields });
            self.add_local_type(struct_name, ResolvedLocal::Record);
        }
    }

    fn visit_item_enum(&mut self, item_enum: &'ast ItemEnum) {
        let enum_name = item_enum.ident.to_string();
        let mut variants = Vec::new();

        for variant in &item_enum.variants {
            let variant_name = variant.ident.to_string();
            let payload = if variant.fields.is_empty() {
                None
            } else {
                // For now, we'll use a simple approach for enum payloads
                // This could be enhanced to handle more complex cases
                Some(TypeRef::unit())
            };

            variants.push(Variant {
                name: variant_name,
                code: None,
                payload,
            });
        }

        self.add_type_definition(&enum_name, TypeDef::Variant { variants });
        self.add_local_type(enum_name, ResolvedLocal::Variant);
    }

    fn visit_item_impl(&mut self, item_impl: &'ast ItemImpl) {
        for item in &item_impl.items {
            if let syn::ImplItem::Fn(method) = item {
                // Only process public methods
                if matches!(method.vis, syn::Visibility::Public(_)) {
                    let method_name = method.sig.ident.to_string();

                    // Process parameters
                    let mut params = Vec::new();
                    for param in &method.sig.inputs {
                        if let syn::FnArg::Typed(pat_type) = param {
                            if let syn::Pat::Ident(pat_ident) = &*pat_type.pat {
                                let param_name = pat_ident.ident.to_string();
                                let param_type = normalize_type(&pat_type.ty, true, self).unwrap();
                                let param_type = post_process_type_ref(param_type, self);

                                // Check if it's Option<T> to set nullable
                                let nullable = if let Type::Path(type_path) = &*pat_type.ty {
                                    (type_path.path.segments.len() == 1
                                        && type_path.path.segments[0].ident == "Option")
                                        .then_some(true)
                                } else {
                                    None
                                };

                                params.push(Parameter {
                                    name: param_name,
                                    type_: param_type,
                                    nullable,
                                });
                            }
                        }
                    }

                    // Process return type
                    let returns = if let syn::ReturnType::Type(_, ty) = &method.sig.output {
                        let return_type = normalize_type(ty, true, self).unwrap();
                        let return_type = post_process_type_ref(return_type, self);

                        // Check if it's Option<T> to set nullable
                        let _returns_nullable = if let Type::Path(type_path) = &**ty {
                            (type_path.path.segments.len() == 1
                                && type_path.path.segments[0].ident == "Option")
                                .then_some(true)
                        } else {
                            None
                        };

                        Some(return_type)
                    } else {
                        Some(TypeRef::Scalar(ScalarType::Unit))
                    };

                    // For now, we'll create a simple method
                    let _method = Method {
                        name: method_name,
                        params,
                        returns,
                        returns_nullable: None, // Will be set based on Option<T>
                        errors: Vec::new(),
                    };

                    // Store the method (you might want to collect these differently)
                    // For now, we'll just process them
                }
            }
        }
    }
}

/// Emit ABI manifest from Rust source code
pub fn emit_manifest(source: &str) -> Result<Manifest, Box<dyn error::Error>> {
    let file = syn::parse_file(source)?;
    let mut emitter = AbiEmitter::new();

    // Visit all items in the file
    for item in &file.items {
        match item {
            Item::Struct(item_struct) => emitter.visit_item_struct(item_struct),
            Item::Enum(item_enum) => emitter.visit_item_enum(item_enum),
            Item::Impl(item_impl) => emitter.visit_item_impl(item_impl),
            _ => {}
        }
    }

    // Create the manifest
    let mut manifest = Manifest {
        schema_version: "wasm-abi/1".to_owned(),
        types: emitter.type_definitions.into_iter().collect(),
        methods: Vec::new(), // You'll need to collect methods differently
        events: Vec::new(),
    };

    // Remove any internal types that shouldn't be exposed
    drop(manifest.types.remove("AbiStateExposed"));
    drop(manifest.types.remove("Event"));

    Ok(manifest)
}
