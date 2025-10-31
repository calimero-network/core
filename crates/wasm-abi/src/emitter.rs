use std::collections::HashMap;
use std::error;

use syn::visit::Visit;
use syn::{Item, ItemEnum, ItemImpl, ItemStruct, Type};

use crate::normalize::{normalize_type, ResolvedLocal, TypeResolver};
use crate::schema::{
    Event, Field, Manifest, Method, Parameter, ScalarType, TypeDef, TypeRef, Variant,
};

/// ABI emitter that processes Rust source code
#[derive(Debug)]
pub struct AbiEmitter {
    type_definitions: HashMap<String, TypeDef>,
    local_types: HashMap<String, ResolvedLocal>,
    methods: Vec<Method>,
    events: Vec<Event>,
    state_type: Option<String>,
}

impl<'ast> AbiEmitter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            type_definitions: HashMap::new(),
            local_types: HashMap::new(),
            methods: Vec::new(),
            events: Vec::new(),
            state_type: None,
        }
    }

    fn record_state_type(&mut self, name: &str) {
        if self.state_type.is_none() {
            self.state_type = Some(name.to_owned());
        }
    }

    fn collect_referenced_types(
        &self,
        item_impl: &'ast ItemImpl,
        referenced_types: &mut std::collections::HashSet<String>,
    ) {
        for item in &item_impl.items {
            if let syn::ImplItem::Fn(method) = item {
                // Only process public methods
                if matches!(method.vis, syn::Visibility::Public(_)) {
                    let method_name = method.sig.ident.to_string();

                    // Skip init methods since they return void
                    if method_name == "init" {
                        continue;
                    }

                    // Process parameters
                    for param in &method.sig.inputs {
                        if let syn::FnArg::Typed(pat_type) = param {
                            self.collect_types_from_type(&pat_type.ty, referenced_types);
                        }
                    }

                    // Process return type
                    if let syn::ReturnType::Type(_, ty) = &method.sig.output {
                        self.collect_types_from_type(ty, referenced_types);
                    }
                }
            }
        }
    }

    fn collect_types_from_enum_variants(
        &self,
        item_enum: &'ast ItemEnum,
        referenced_types: &mut std::collections::HashSet<String>,
    ) {
        for variant in &item_enum.variants {
            for field in &variant.fields {
                self.collect_types_from_type(&field.ty, referenced_types);
            }
        }
    }

    fn collect_types_from_struct_fields(
        &self,
        item_struct: &'ast ItemStruct,
        referenced_types: &mut std::collections::HashSet<String>,
    ) {
        for field in &item_struct.fields {
            self.collect_types_from_type(&field.ty, referenced_types);
        }
    }

    fn collect_types_from_type(
        &self,
        ty: &Type,
        referenced_types: &mut std::collections::HashSet<String>,
    ) {
        match ty {
            Type::Path(type_path) => {
                if let Some(segment) = type_path.path.segments.last() {
                    let type_name = segment.ident.to_string();
                    let _ = referenced_types.insert(type_name);

                    // Also collect generic type arguments
                    if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                        for arg in &args.args {
                            if let syn::GenericArgument::Type(ty) = arg {
                                self.collect_types_from_type(ty, referenced_types);
                            }
                        }
                    }
                }
            }
            Type::Reference(type_ref) => {
                self.collect_types_from_type(&type_ref.elem, referenced_types);
            }
            Type::Ptr(type_ptr) => {
                self.collect_types_from_type(&type_ptr.elem, referenced_types);
            }
            Type::Array(type_array) => {
                self.collect_types_from_type(&type_array.elem, referenced_types);
            }
            Type::Slice(type_slice) => {
                self.collect_types_from_type(&type_slice.elem, referenced_types);
            }
            Type::Tuple(type_tuple) => {
                for elem in &type_tuple.elems {
                    self.collect_types_from_type(elem, referenced_types);
                }
            }
            _ => {}
        }
    }

    fn add_type_definition(&mut self, name: &str, type_def: TypeDef) {
        drop(self.type_definitions.insert(name.to_owned(), type_def));
    }

    fn add_local_type(&mut self, name: String, resolved: ResolvedLocal) {
        match resolved {
            ResolvedLocal::NewtypeBytes { size } => {
                let _ = self
                    .local_types
                    .insert(name, ResolvedLocal::NewtypeBytes { size });
            }
            ResolvedLocal::Record => {
                let _ = self.local_types.insert(name, ResolvedLocal::Record);
            }
            ResolvedLocal::Variant => {
                let _ = self.local_types.insert(name, ResolvedLocal::Variant);
            }
        }
    }

    fn process_events(&mut self, item_enum: &ItemEnum) {
        for variant in &item_enum.variants {
            let event_name = variant.ident.to_string();
            let payload = if variant.fields.is_empty() {
                None
            } else {
                // Handle event payloads
                if variant.fields.len() == 1 {
                    // Single field variant
                    if let syn::Fields::Unnamed(fields) = &variant.fields {
                        let field_type = normalize_type(&fields.unnamed[0].ty, true, self).unwrap();
                        let field_type = post_process_type_ref(field_type, self);
                        Some(field_type)
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            self.events.push(Event {
                name: event_name,
                payload,
            });
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

/// Returns true when the struct has the `#[app::state]` marker attribute so it can
/// be treated as the contract's root state during ABI emission.
fn has_app_state_attribute(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let path = attr.path();
        let segments: Vec<_> = path.segments.iter().collect();
        segments.len() == 2 && segments[0].ident == "app" && segments[1].ident == "state"
    })
}

impl<'ast> Visit<'ast> for AbiEmitter {
    fn visit_item_struct(&mut self, item_struct: &'ast ItemStruct) {
        let struct_name = item_struct.ident.to_string();

        // Check if this is a newtype pattern
        if is_newtype_pattern(item_struct) {
            if let Some(target_type) = extract_newtype_target(item_struct) {
                let target_type_ref = normalize_type(target_type, true, self).unwrap();
                let target_type_ref = post_process_type_ref(target_type_ref, self);

                // Add as an alias type definition
                self.add_type_definition(
                    &struct_name,
                    TypeDef::Alias {
                        target: target_type_ref,
                    },
                );

                // Add to local types for resolution
                self.add_local_type(struct_name.clone(), ResolvedLocal::Record);
            }
        } else {
            // Process struct fields to generate type definitions
            let mut fields = Vec::new();

            for field in &item_struct.fields {
                let field_name = field
                    .ident
                    .as_ref()
                    .map_or_else(|| "unnamed".to_owned(), ToString::to_string);

                let field_type = normalize_type(&field.ty, true, self).unwrap_or_else(|e| {
                    eprintln!("Failed to normalize type for field: {field_name}");
                    eprintln!("Error: {e:?}");
                    panic!("Type normalization failed");
                });
                let field_type = post_process_type_ref(field_type, self);

                // Check if it's Option<T> to set nullable
                let nullable = if let Type::Path(type_path) = &field.ty {
                    (type_path.path.segments.len() == 1
                        && type_path.path.segments[0].ident == "Option")
                        .then_some(true)
                } else {
                    None
                };

                fields.push(Field {
                    name: field_name,
                    type_: field_type,
                    nullable,
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
                // Handle enum payloads properly
                if variant.fields.len() == 1 {
                    // Single field variant
                    if let syn::Fields::Unnamed(fields) = &variant.fields {
                        let field_type = normalize_type(&fields.unnamed[0].ty, true, self).unwrap();
                        let field_type = post_process_type_ref(field_type, self);
                        Some(field_type)
                    } else if let syn::Fields::Named(fields) = &variant.fields {
                        // Named fields - create a record type
                        let mut record_fields = Vec::new();
                        for field in &fields.named {
                            let field_name = field.ident.as_ref().unwrap().to_string();
                            let field_type = normalize_type(&field.ty, true, self).unwrap();
                            let field_type = post_process_type_ref(field_type, self);
                            record_fields.push(Field {
                                name: field_name,
                                type_: field_type,
                                nullable: None,
                            });
                        }
                        // Create a temporary record type for the variant payload
                        let record_type = TypeDef::Record {
                            fields: record_fields,
                        };
                        let record_name = format!("{enum_name}_{variant_name}");
                        self.add_type_definition(&record_name, record_type);
                        Some(TypeRef::reference(&record_name))
                    } else {
                        Some(TypeRef::unit())
                    }
                } else {
                    // Multiple fields - create a record type
                    let mut record_fields = Vec::new();
                    for (i, field) in variant.fields.iter().enumerate() {
                        let field_name = field
                            .ident
                            .as_ref()
                            .map_or_else(|| format!("field_{i}"), ToString::to_string);
                        let field_type = normalize_type(&field.ty, true, self).unwrap();
                        let field_type = post_process_type_ref(field_type, self);
                        record_fields.push(Field {
                            name: field_name,
                            type_: field_type,
                            nullable: None,
                        });
                    }
                    // Create a temporary record type for the variant payload
                    let record_type = TypeDef::Record {
                        fields: record_fields,
                    };
                    let record_name = format!("{enum_name}_{variant_name}");
                    self.add_type_definition(&record_name, record_type);
                    Some(TypeRef::reference(&record_name))
                }
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
                    let (returns, returns_nullable) = if method_name == "init" {
                        // Init methods should always return void
                        (Some(TypeRef::Scalar(ScalarType::Unit)), None)
                    } else if let syn::ReturnType::Type(_, ty) = &method.sig.output {
                        let return_type = normalize_type(ty, true, self).unwrap();
                        let return_type = post_process_type_ref(return_type, self);

                        // Check if it's Option<T> to set nullable
                        let returns_nullable = if let Type::Path(type_path) = &**ty {
                            (type_path.path.segments.len() == 1
                                && type_path.path.segments[0].ident == "Option")
                                .then_some(true)
                        } else {
                            None
                        };

                        (Some(return_type), returns_nullable)
                    } else {
                        (Some(TypeRef::Scalar(ScalarType::Unit)), None)
                    };

                    // Create and store the method
                    let method = Method {
                        name: method_name,
                        params,
                        returns,
                        returns_nullable,
                        errors: Vec::new(),
                    };

                    self.methods.push(method);
                }
            }
        }
    }
}

/// Emit ABI manifest from multiple source files (lib.rs + modules)
pub fn emit_manifest_from_crate(
    sources: &[(String, String)],
) -> Result<Manifest, Box<dyn error::Error>> {
    // Parse all files
    let mut files = Vec::new();
    for (name, content) in sources {
        let file =
            syn::parse_file(content).map_err(|e| format!("Failed to parse {}: {}", name, e))?;
        files.push(file);
    }

    let mut emitter = AbiEmitter::new();

    // Pre-scan: Register all struct and enum names as local types from all files
    // This ensures type resolution works even for types defined in other modules
    for file in &files {
        for item in &file.items {
            match item {
                Item::Struct(item_struct) => {
                    let struct_name = item_struct.ident.to_string();
                    emitter.add_local_type(struct_name, ResolvedLocal::Record);
                }
                Item::Enum(item_enum) => {
                    let enum_name = item_enum.ident.to_string();
                    emitter.add_local_type(enum_name, ResolvedLocal::Variant);
                }
                _ => {}
            }
        }
    }

    // First pass: collect all referenced types from public methods and state definitions
    let mut referenced_types = std::collections::HashSet::new();
    for file in &files {
        for item in &file.items {
            if let Item::Struct(item_struct) = item {
                if has_app_state_attribute(&item_struct.attrs) {
                    let struct_name = item_struct.ident.to_string();
                    let _ = referenced_types.insert(struct_name.clone());
                    emitter.record_state_type(&struct_name);
                }
            }
        }
    }

    for file in &files {
        for item in &file.items {
            if let Item::Impl(item_impl) = item {
                emitter.collect_referenced_types(item_impl, &mut referenced_types);
            }
        }
    }

    // Also collect types from event payloads
    for file in &files {
        for item in &file.items {
            if let Item::Enum(item_enum) = item {
                if item_enum.ident == "Event" {
                    emitter.collect_types_from_enum_variants(item_enum, &mut referenced_types);
                }
            }
        }
    }

    // Second pass: iteratively collect all transitively referenced types
    let mut changed = true;
    while changed {
        changed = false;
        let initial_size = referenced_types.len();

        for file in &files {
            for item in &file.items {
                match item {
                    Item::Enum(item_enum) => {
                        let enum_name = item_enum.ident.to_string();
                        if referenced_types.contains(&enum_name) {
                            emitter
                                .collect_types_from_enum_variants(item_enum, &mut referenced_types);
                        }
                    }
                    Item::Struct(item_struct) => {
                        if !is_newtype_pattern(item_struct) {
                            let struct_name = item_struct.ident.to_string();
                            if referenced_types.contains(&struct_name) {
                                emitter.collect_types_from_struct_fields(
                                    item_struct,
                                    &mut referenced_types,
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        if referenced_types.len() > initial_size {
            changed = true;
        }
    }

    // Third pass: process types (newtypes first, then referenced types)
    // Process newtypes first from all files
    for file in &files {
        for item in &file.items {
            if let Item::Struct(item_struct) = item {
                if is_newtype_pattern(item_struct) {
                    emitter.visit_item_struct(item_struct);
                }
            }
        }
    }

    // Then process referenced types from all files
    for file in &files {
        for item in &file.items {
            match item {
                Item::Struct(item_struct) => {
                    if !is_newtype_pattern(item_struct) {
                        let struct_name = item_struct.ident.to_string();
                        if referenced_types.contains(&struct_name) {
                            emitter.visit_item_struct(item_struct);
                        }
                    }
                }
                Item::Enum(item_enum) => {
                    let enum_name = item_enum.ident.to_string();
                    if enum_name == "Event" {
                        emitter.process_events(item_enum);
                    } else if referenced_types.contains(&enum_name) {
                        emitter.visit_item_enum(item_enum);
                    }
                }
                _ => {}
            }
        }
    }

    // Fourth pass: process methods (after all types are defined) - only from lib.rs
    if let Some(lib_file) = files.first() {
        for item in &lib_file.items {
            if let Item::Impl(item_impl) = item {
                emitter.visit_item_impl(item_impl);
            }
        }
    }

    // Create the manifest
    let mut manifest = Manifest {
        schema_version: "wasm-abi/1".to_owned(),
        types: emitter.type_definitions.into_iter().collect(),
        methods: emitter.methods,
        events: emitter.events,
        state_root: emitter.state_type,
    };

    // Remove any internal types that shouldn't be exposed
    drop(manifest.types.remove("AbiStateExposed"));
    drop(manifest.types.remove("Event"));

    Ok(manifest)
}

/// Emit ABI manifest from Rust source code (single file - for backward compatibility)
pub fn emit_manifest(source: &str) -> Result<Manifest, Box<dyn error::Error>> {
    emit_manifest_from_crate(&[("lib.rs".to_owned(), source.to_owned())])
}
