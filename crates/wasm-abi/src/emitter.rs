use std::collections::HashMap;
use std::error;

use syn::visit::Visit;
use syn::{Item, ItemEnum, ItemImpl, ItemStruct, Type};

use crate::normalize::{normalize_type, ResolvedLocal, TypeResolver};
use crate::schema::{
    Event, Field, Manifest, Method, MethodIntent, MigrationEdgeAbi, Parameter, ScalarType, TypeDef,
    TypeRef, Variant,
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
            match &variant.fields {
                syn::Fields::Unnamed(fields) => {
                    for field in &fields.unnamed {
                        self.collect_types_from_type(&field.ty, referenced_types);
                    }
                }
                syn::Fields::Named(fields) => {
                    for field in &fields.named {
                        self.collect_types_from_type(&field.ty, referenced_types);
                    }
                }
                syn::Fields::Unit => {
                    // No fields to collect
                }
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
                // Handle event payloads properly
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
                        let record_type = TypeDef::Record {
                            fields: record_fields,
                        };
                        let record_name = format!("Event_{event_name}");
                        self.add_type_definition(&record_name, record_type);
                        Some(TypeRef::reference(&record_name))
                    } else {
                        Some(TypeRef::unit())
                    }
                } else {
                    // Multiple fields - create a record type
                    let mut record_fields = Vec::new();
                    if let syn::Fields::Unnamed(fields) = &variant.fields {
                        // Tuple variant with multiple fields
                        for (i, field) in fields.unnamed.iter().enumerate() {
                            let field_name = format!("field_{i}");
                            let field_type = normalize_type(&field.ty, true, self).unwrap();
                            let field_type = post_process_type_ref(field_type, self);
                            record_fields.push(Field {
                                name: field_name,
                                type_: field_type,
                                nullable: None,
                            });
                        }
                    } else if let syn::Fields::Named(fields) = &variant.fields {
                        // Struct variant with multiple fields
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
                    }
                    let record_type = TypeDef::Record {
                        fields: record_fields,
                    };
                    let record_name = format!("Event_{event_name}");
                    self.add_type_definition(&record_name, record_type);
                    Some(TypeRef::reference(&record_name))
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

/// Returns `true` when the method carries `#[app::view]`, declaring it as
/// read-only — i.e. the app author guarantees it never mutates state.
fn has_app_view_attribute(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let path = attr.path();
        let segments: Vec<_> = path.segments.iter().collect();
        segments.len() == 2 && segments[0].ident == "app" && segments[1].ident == "view"
    })
}

/// Returns `true` when the method carries `#[app::xcall]`, declaring it as a
/// cross-context entry point — i.e. callable by another context via `xcall`.
fn has_app_xcall_attribute(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let path = attr.path();
        let segments: Vec<_> = path.segments.iter().collect();
        segments.len() == 2 && segments[0].ident == "app" && segments[1].ident == "xcall"
    })
}

/// Parse `version = N` out of `#[app::state(version = N, …)]`. `None` if no version arg.
fn app_state_version(attrs: &[syn::Attribute]) -> Option<u32> {
    for attr in attrs {
        let segs: Vec<_> = attr.path().segments.iter().collect();
        if segs.len() == 2 && segs[0].ident == "app" && segs[1].ident == "state" {
            let mut found = None;
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("version") {
                    let lit: syn::LitInt = meta.value()?.parse()?;
                    found = lit.base10_parse::<u32>().ok();
                } else if meta.input.peek(syn::Token![=]) {
                    // Consume `= <value>` for keys we don't use (emits, …) so
                    // parse_nested_meta advances past them regardless of order.
                    let _: syn::Expr = meta.value()?.parse()?;
                }
                Ok(())
            });
            return found;
        }
    }
    None
}

/// The migrate method declared by `#[migrate(method = ident, …)]` on the state
/// struct (the `#[derive(app::Migrate)]` form). Presence of a `#[migrate(...)]`
/// attribute IS the migration declaration; `method` is optional and **defaults
/// to `migrate`** (matching the derive macro), so a `#[migrate(from = ...)]`
/// without an explicit method still yields the entrypoint name.
fn migrate_method_from_attrs(
    attrs: &[syn::Attribute],
    state_version: Option<u32>,
) -> Option<String> {
    for attr in attrs {
        if attr.path().is_ident("migrate") {
            let mut method = None;
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("method") {
                    let p: syn::Path = meta.value()?.parse()?;
                    method = p.get_ident().map(|i| i.to_string());
                } else if meta.input.peek(syn::Token![=]) {
                    // Consume `= <value>` for keys we don't use (from, emit, …)
                    // so parse_nested_meta advances past them to reach `method`.
                    let _: syn::Expr = meta.value()?.parse()?;
                }
                Ok(())
            });
            // A `#[migrate(...)]` attr present ⇒ this is a migration. The
            // default name mirrors the derive macro: versioned
            // (`migrate_v{N-1}_to_v{N}`) when the state declares a version
            // past 1 — a bare `migrate` collides across releases — else
            // `migrate`.
            return Some(method.unwrap_or_else(|| match state_version {
                Some(to) if to > 1 => format!("migrate_v{}_to_v{}", to - 1, to),
                _ => "migrate".to_owned(),
            }));
        }
    }
    None
}

/// The name of a free `#[app::migrate] fn …()` (the attribute-macro form).
fn free_migrate_fn_name(items: &[syn::Item]) -> Option<String> {
    for item in items {
        if let syn::Item::Fn(f) = item {
            let has = f.attrs.iter().any(|a| {
                let s: Vec<_> = a.path().segments.iter().collect();
                s.len() == 2 && s[0].ident == "app" && s[1].ident == "migrate"
            });
            if has {
                return Some(f.sig.ident.to_string());
            }
        }
    }
    None
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
                    panic!("ABI emit: failed to normalize type for field `{field_name}`: {e:?}");
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
                    if let syn::Fields::Unnamed(fields) = &variant.fields {
                        // Tuple variant with multiple fields
                        for (i, field) in fields.unnamed.iter().enumerate() {
                            let field_name = format!("field_{i}");
                            let field_type = normalize_type(&field.ty, true, self).unwrap();
                            let field_type = post_process_type_ref(field_type, self);
                            record_fields.push(Field {
                                name: field_name,
                                type_: field_type,
                                nullable: None,
                            });
                        }
                    } else if let syn::Fields::Named(fields) = &variant.fields {
                        // Struct variant with multiple fields
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

                    // Read/write intent from #[app::view]; default Unspecified.
                    // init is always mutating regardless of any attribute — the
                    // SDK macro rejects #[app::view] + #[app::init] at compile
                    // time, but a non-SDK toolchain could produce both; guard
                    // here so a rogue annotation can't cause the node to take a
                    // shared read lock for an initializer that always writes.
                    let intent = if method_name == "init" {
                        MethodIntent::Unspecified
                    } else if has_app_view_attribute(&method.attrs) {
                        MethodIntent::ReadOnly
                    } else {
                        MethodIntent::Unspecified
                    };

                    // Cross-context entry point from #[app::xcall]; never on
                    // init (an initializer is not an xcall target).
                    let xcall_callable =
                        method_name != "init" && has_app_xcall_attribute(&method.attrs);

                    // Create and store the method
                    let method = Method {
                        name: method_name,
                        params,
                        returns,
                        returns_nullable,
                        errors: Vec::new(),
                        intent,
                        xcall_callable,
                    };

                    self.methods.push(method);
                }
            }
        }
    }
}

/// Emit ABI manifest from multiple source files (lib.rs + modules)
///
/// # Requirements
/// - The `sources` slice must include a file named "lib.rs" which contains the main
///   implementation with `#[app::state]` and public methods. Method processing only
///   happens on lib.rs.
/// - Additional module files can be included to resolve types defined in separate modules.
/// - The order of files in the slice doesn't matter - lib.rs is found by name.
///
/// # Errors
/// Returns an error if lib.rs is not found in the sources or if parsing fails.
pub fn emit_manifest_from_crate(
    sources: &[(String, String)],
) -> Result<Manifest, Box<dyn error::Error>> {
    // Parse all files
    let mut files = Vec::new();
    for (name, content) in sources {
        let file = syn::parse_file(content).map_err(|e| format!("Failed to parse {name}: {e}"))?;
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
    // Find lib.rs explicitly by name to avoid silent failures if files are in wrong order
    let lib_index = sources
        .iter()
        .position(|(name, _)| name == "lib.rs")
        .ok_or("lib.rs not found in sources - ABI generation requires lib.rs as the main implementation file")?;

    let lib_file = &files[lib_index];
    for item in &lib_file.items {
        if let Item::Impl(item_impl) = item {
            emitter.visit_item_impl(item_impl);
        }
    }

    // Create the manifest
    // State version from `#[app::state(version = N)]` (default 1 — always
    // recorded so version comparison is total even on code-only releases).
    // Migration method from `#[migrate(method = …)]` (derive form, whose
    // versioned default mirrors the derive macro) or a free
    // `#[app::migrate] fn`; declared as the `from_version = N-1` edge.
    let mut state_version = None;
    let mut migrations = Vec::new();
    for file in &files {
        for item in &file.items {
            if let Item::Struct(s) = item {
                if has_app_state_attribute(&s.attrs) {
                    let to = app_state_version(&s.attrs).unwrap_or(1);
                    state_version = Some(to);
                    let method = migrate_method_from_attrs(&s.attrs, Some(to))
                        .or_else(|| free_migrate_fn_name(&file.items));
                    if let Some(method) = method {
                        if to > 1 {
                            migrations.push(MigrationEdgeAbi {
                                method,
                                from_version: to - 1,
                            });
                        }
                    }
                }
            }
        }
    }

    let mut manifest = Manifest {
        schema_version: "wasm-abi/1".to_owned(),
        types: emitter.type_definitions.into_iter().collect(),
        methods: emitter.methods,
        events: emitter.events,
        state_root: emitter.state_type,
        state_version,
        migrations,
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

#[cfg(test)]
mod xcall_tests {
    use super::*;

    #[test]
    fn emits_xcall_callable_only_for_annotated_methods() {
        let lib = r#"
            #[app::state(version = 1)]
            pub struct State { x: u32 }
            #[app::logic]
            impl State {
                #[app::init] pub fn init() -> State { State { x: 0 } }
                #[app::xcall] pub fn on_event(&mut self) {}
                pub fn plain(&mut self) {}
            }
        "#;
        let m = emit_manifest_from_crate(&[("lib.rs".to_owned(), lib.to_owned())]).unwrap();

        let on_event = m
            .methods
            .iter()
            .find(|mm| mm.name == "on_event")
            .expect("on_event present");
        assert!(
            on_event.xcall_callable,
            "#[app::xcall] method must be xcall_callable"
        );

        let plain = m
            .methods
            .iter()
            .find(|mm| mm.name == "plain")
            .expect("plain present");
        assert!(
            !plain.xcall_callable,
            "unannotated method must not be xcall_callable"
        );
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    #[test]
    fn emits_migration_edge_for_derive_form() {
        let lib = r#"
            #[app::state(version = 2, emits = Event)]
            #[derive(app::Migrate)]
            #[migrate(from = OldState, method = migrate_v1_to_v2, emit = Event::Migrated)]
            pub struct State { x: u32 }
            #[app::logic]
            impl State { #[app::init] pub fn init() -> State { State { x: 0 } } }
        "#;
        let m = emit_manifest_from_crate(&[("lib.rs".to_owned(), lib.to_owned())]).unwrap();
        let edge = m.edge_from(1).expect("edge emitted");
        assert_eq!(edge.method, "migrate_v1_to_v2");
        assert_eq!(m.state_version_or_default(), 2);
    }

    #[test]
    fn emits_migration_edge_for_free_fn_form() {
        let lib = r#"
            #[app::state(version = 3)]
            pub struct State { x: u32 }
            #[app::migrate]
            pub fn migrate_v2_to_v3() -> State { State { x: 0 } }
            #[app::logic]
            impl State { #[app::init] pub fn init() -> State { State { x: 0 } } }
        "#;
        let m = emit_manifest_from_crate(&[("lib.rs".to_owned(), lib.to_owned())]).unwrap();
        let edge = m.edge_from(2).expect("edge emitted");
        assert_eq!(edge.method, "migrate_v2_to_v3");
        assert_eq!(m.state_version_or_default(), 3);
    }

    // Omitted `method = …` defaults to the VERSIONED name (mirrors the derive
    // macro): a bare `migrate` would collide across releases and between two
    // derives in one module.
    #[test]
    fn derive_without_explicit_method_defaults_to_versioned_name() {
        let lib = r#"
            #[app::state(version = 2)]
            #[derive(app::Migrate)]
            #[migrate(from = OldState)]
            pub struct State { x: u32 }
            #[app::logic]
            impl State { #[app::init] pub fn init() -> State { State { x: 0 } } }
        "#;
        let m = emit_manifest_from_crate(&[("lib.rs".to_owned(), lib.to_owned())]).unwrap();
        assert_eq!(
            m.edge_from(1).expect("edge emitted").method,
            "migrate_v1_to_v2"
        );
    }

    #[test]
    fn version_read_regardless_of_key_order() {
        // `version` after `emits` — parse_nested_meta must consume `emits`'s value.
        let lib = r#"
            #[app::state(emits = Event, version = 5)]
            #[derive(app::Migrate)]
            #[migrate(from = OldState, method = migrate_v4_to_v5)]
            pub struct State { x: u32 }
            #[app::logic]
            impl State { #[app::init] pub fn init() -> State { State { x: 0 } } }
        "#;
        let m = emit_manifest_from_crate(&[("lib.rs".to_owned(), lib.to_owned())]).unwrap();
        assert_eq!(m.state_version, Some(5));
        assert_eq!(
            m.edge_from(4).expect("edge emitted").method,
            "migrate_v4_to_v5"
        );
    }

    // `state_version` is always emitted (default 1) so the upgrade decision
    // table can compare versions even across code-only releases.
    #[test]
    fn state_version_and_edges_always_emitted() {
        // Versioned migration build: version + one edge.
        let lib = r#"
            #[app::state(version = 2)]
            #[derive(app::Migrate)]
            #[migrate(from = OldState, method = migrate_v1_to_v2)]
            pub struct State { x: u32 }
            #[app::logic]
            impl State { #[app::init] pub fn init() -> State { State { x: 0 } } }
        "#;
        let m = emit_manifest_from_crate(&[("lib.rs".to_owned(), lib.to_owned())]).unwrap();
        assert_eq!(m.state_version, Some(2));
        assert_eq!(m.migrations.len(), 1);
        assert_eq!(m.migrations[0].method, "migrate_v1_to_v2");
        assert_eq!(m.migrations[0].from_version, 1);

        // Code-only build (no migration declared): version still present,
        // no edges.
        let lib = r#"
            #[app::state(version = 2)]
            pub struct State { x: u32 }
            #[app::logic]
            impl State { #[app::init] pub fn init() -> State { State { x: 0 } } }
        "#;
        let m = emit_manifest_from_crate(&[("lib.rs".to_owned(), lib.to_owned())]).unwrap();
        assert_eq!(m.state_version, Some(2));
        assert!(m.migrations.is_empty());

        // Unversioned `#[app::state]`: defaults to 1, no edges even with a
        // declared migrate (there is no prior version to hop from).
        let lib = r#"
            #[app::state]
            #[derive(app::Migrate)]
            #[migrate(from = OldState)]
            pub struct State { x: u32 }
            #[app::logic]
            impl State { #[app::init] pub fn init() -> State { State { x: 0 } } }
        "#;
        let m = emit_manifest_from_crate(&[("lib.rs".to_owned(), lib.to_owned())]).unwrap();
        assert_eq!(m.state_version, Some(1));
        assert!(m.migrations.is_empty());
    }

    #[test]
    fn no_migration_when_absent() {
        let lib = r#"
            #[app::state(version = 1)]
            pub struct State { x: u32 }
            #[app::logic]
            impl State { #[app::init] pub fn init() -> State { State { x: 0 } } }
        "#;
        let m = emit_manifest_from_crate(&[("lib.rs".to_owned(), lib.to_owned())]).unwrap();
        assert!(m.migrations.is_empty());
    }
}
