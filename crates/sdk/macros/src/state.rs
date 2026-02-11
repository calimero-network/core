use proc_macro2::{Span, TokenStream};
use quote::{quote, ToTokens};
use syn::parse::{Parse, ParseStream};
use syn::{
    parse2, BoundLifetimes, Error as SynError, GenericParam, Generics, Ident, Lifetime,
    LifetimeParam, Result as SynResult, Token, Type,
};

use crate::errors::{Errors, ParseError, Pretty};
use crate::items::StructOrEnumItem;
use crate::macros::infallible;
use crate::reserved::idents;
use crate::sanitizer::{Action, Case, Func, Sanitizer};

#[derive(Clone, Copy)]
pub struct StateImpl<'a> {
    ident: &'a Ident,
    generics: &'a Generics,
    emits: &'a Option<MaybeBoundEvent>,
    orig: &'a StructOrEnumItem,
}

impl ToTokens for StateImpl<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let StateImpl {
            ident,
            generics,
            emits,
            orig,
        } = *self;

        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        let mut lifetime = quote! { 'a };
        let mut event = quote! { ::calimero_sdk::event::NoEvent };

        if let Some(emits) = emits {
            if let Some(lt) = &emits.lifetime {
                lifetime = quote! { #lt };
            }
            event = {
                let event = &emits.ty;
                quote! { #event }
            };
        }

        // Generate Mergeable implementation
        let merge_impl = generate_mergeable_impl(ident, generics, orig);

        // Generate registration hook
        let registration_hook = generate_registration_hook(ident, &ty_generics);

        // Generate deterministic ID assignment method
        let assign_ids_impl = generate_assign_deterministic_ids_impl(ident, generics, orig);

        quote! {
            #orig

            impl #impl_generics ::calimero_sdk::state::AppState for #ident #ty_generics #where_clause {
                type Event<#lifetime> = #event;
            }

            impl #impl_generics #ident #ty_generics #where_clause {
                fn external() -> ::calimero_sdk::env::ext::External {
                    ::calimero_sdk::env::ext::External {}
                }
            }

            // Auto-generated CRDT merge support
            #merge_impl

            // Auto-generated registration hook
            #registration_hook

            // Auto-generated deterministic ID assignment
            #assign_ids_impl
        }
        .to_tokens(tokens);
    }
}

struct MaybeBoundEvent {
    lifetime: Option<Lifetime>,
    ty: Type,
}

// todo! move all errors to ParseError

impl Parse for MaybeBoundEvent {
    #[expect(
        clippy::unwrap_in_result,
        reason = "Error handling verified - errors exist when !fine"
    )]
    fn parse(input: ParseStream<'_>) -> SynResult<Self> {
        let mut lifetime = None;

        let errors = Errors::default();

        'bounds: {
            if input.peek(Token![for]) {
                let bounds = match input.parse::<BoundLifetimes>() {
                    Ok(bounds) => bounds,
                    Err(err) => {
                        errors.subsume(err);
                        break 'bounds;
                    }
                };

                let fine = if bounds.lifetimes.is_empty() {
                    errors.subsume(SynError::new_spanned(
                        bounds.gt_token,
                        "non-empty lifetime bounds expected",
                    ));
                    false
                } else if input.is_empty() {
                    errors.subsume(SynError::new_spanned(
                        &bounds,
                        "expected an event type to immediately follow",
                    ));
                    false
                } else {
                    true
                };

                if !fine {
                    return Err(errors.take().expect("not fine, so we must have errors"));
                }

                for param in bounds.lifetimes {
                    if let GenericParam::Lifetime(LifetimeParam { lifetime: lt, .. }) = param {
                        if lifetime.is_some() {
                            errors.subsume(SynError::new(
                                lt.span(),
                                "only one lifetime can be specified",
                            ));

                            continue;
                        }
                        lifetime = Some(lt);
                    }
                }
            }
        }

        let ty = match input.parse::<Type>() {
            Ok(ty) => ty,
            Err(err) => return Err(errors.subsumed(err)),
        };

        let mut sanitizer = match parse2::<Sanitizer<'_>>(ty.to_token_stream()) {
            Ok(sanitizer) => sanitizer,
            Err(err) => return Err(errors.subsumed(err)),
        };

        let mut cases = vec![];

        if let Some(lt) = &lifetime {
            cases.push((Case::Lifetime(Some(lt)), Action::Ignore));
        }

        let mut unexpected_lifetime = |span: Span| {
            let lifetime = span
                .source_text()
                .unwrap_or_else(|| "'{lifetime}".to_owned());

            // todo! source text is unreliable
            let error = if matches!(lifetime.as_str(), "&" | "'_") {
                ParseError::MustSpecifyLifetime
            } else {
                ParseError::UseOfUndeclaredLifetime {
                    append: format!(
                        "\n\nuse the `for<{}> {}` directive to declare it",
                        lifetime,
                        Pretty::Type(&ty)
                    ),
                }
            };

            Action::Forbid(error)
        };

        let static_lifetime = Lifetime::new("'static", Span::call_site());

        cases.extend([
            (Case::Lifetime(Some(&static_lifetime)), Action::Ignore),
            (
                Case::Lifetime(None),
                Action::Custom(Func::new(&mut unexpected_lifetime)),
            ),
        ]);

        let mut outcome = sanitizer.sanitize(&cases);

        if let Some(lifetime) = &lifetime {
            if 0 == outcome.count(&Case::Lifetime(Some(lifetime)))
                && !(lifetime == &static_lifetime
                    || matches!(lifetime.ident.to_string().as_str(), "_"))
            {
                outcome
                    .errors()
                    .subsume(SynError::new(lifetime.span(), "unused lifetime specified"));
            }
        }

        outcome.check()?;

        let ty = infallible!({ parse2(sanitizer.into_token_stream()) });

        input
            .is_empty()
            .then(|| Self { lifetime, ty })
            .ok_or_else(|| input.error("unexpected token"))
    }
}

pub struct StateArgs {
    emits: Option<MaybeBoundEvent>,
}

impl Parse for StateArgs {
    fn parse(input: ParseStream<'_>) -> SynResult<Self> {
        let mut emits = None;

        if !input.is_empty() {
            if !input.peek(Ident) {
                return Err(input.error("expected an identifier"));
            }

            let ident = input.parse::<Ident>()?;

            if !input.peek(Token![=]) {
                let span = if let Some((tt, _)) = input.cursor().token_tree() {
                    tt.span()
                } else {
                    ident.span()
                };
                return Err(SynError::new(
                    span,
                    format_args!("expected `=` after `{ident}`"),
                ));
            }

            let eq = input.parse::<Token![=]>()?;

            match ident.to_string().as_str() {
                "emits" => {
                    if input.is_empty() {
                        return Err(SynError::new_spanned(
                            eq,
                            "expected an event type after `=`",
                        ));
                    }
                    emits = Some(input.parse::<MaybeBoundEvent>()?);
                }
                _ => {
                    return Err(SynError::new_spanned(
                        &ident,
                        format_args!("unexpected `{ident}`"),
                    ));
                }
            }

            if !input.is_empty() {
                return Err(input.error("unexpected token"));
            }
        }

        Ok(Self { emits })
    }
}

pub struct StateImplInput<'a> {
    pub item: &'a StructOrEnumItem,
    pub args: &'a StateArgs,
}

impl<'a> TryFrom<StateImplInput<'a>> for StateImpl<'a> {
    type Error = Errors<'a, StructOrEnumItem>;

    fn try_from(input: StateImplInput<'a>) -> Result<Self, Self::Error> {
        let errors = Errors::new(input.item);

        let (ident, generics) = match input.item {
            StructOrEnumItem::Struct(item) => (&item.ident, &item.generics),
            StructOrEnumItem::Enum(item) => (&item.ident, &item.generics),
        };

        if ident == &*idents::input() {
            errors.subsume(SynError::new_spanned(ident, ParseError::UseOfReservedIdent));
        }

        for generic in &generics.params {
            match generic {
                GenericParam::Lifetime(params) => {
                    errors.subsume(SynError::new(
                        params.lifetime.span(),
                        ParseError::NoGenericLifetimeSupport,
                    ));
                }
                GenericParam::Type(params) => {
                    if params.ident == *idents::input() {
                        errors.subsume(SynError::new_spanned(
                            &params.ident,
                            ParseError::UseOfReservedIdent,
                        ));
                    }
                }
                GenericParam::Const(_) => {}
            }
        }

        errors.check()?;

        Ok(StateImpl {
            ident,
            generics,
            emits: &input.args.emits,
            orig: input.item,
        })
    }
}

/// Generate Mergeable trait implementation for the state struct
fn generate_mergeable_impl(
    ident: &Ident,
    generics: &Generics,
    orig: &StructOrEnumItem,
) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Extract fields from the struct
    let fields = match orig {
        StructOrEnumItem::Struct(s) => &s.fields,
        StructOrEnumItem::Enum(_) => {
            // Enums don't have fields to merge
            return quote! {
                // No Mergeable impl for enums
            };
        }
    };

    // Generate merge calls for each field
    // Only merge fields that are known CRDT types
    let merge_calls: Vec<_> = fields
        .iter()
        .enumerate()
        .filter_map(|(idx, field)| {
            let field_type = &field.ty;

            // Check if this is a known CRDT type by examining the type path
            let type_str = quote! { #field_type }.to_string();

            // Only generate merge for CRDT collections
            // Non-CRDT fields (String, u64, etc.) are handled by storage layer's LWW
            let is_crdt = type_str.contains("UnorderedMap")
                || type_str.contains("Vector")
                || type_str.contains("UnorderedSet")
                || type_str.contains("Counter")
                || type_str.contains("ReplicatedGrowableArray")
                || type_str.contains("LwwRegister")
                || type_str.contains("UserStorage")
                || type_str.contains("FrozenStorage");

            if !is_crdt {
                // Skip non-CRDT fields
                return None;
            }

            // Handle both named fields and tuple struct fields
            if let Some(field_name) = &field.ident {
                // Named field
                Some(quote! {
                    ::calimero_storage::collections::Mergeable::merge(
                        &mut self.#field_name,
                        &other.#field_name
                    ).map_err(|e| {
                        ::calimero_storage::collections::crdt_meta::MergeError::StorageError(
                            format!(
                                "Failed to merge field '{}': {:?}",
                                stringify!(#field_name),
                                e
                            )
                        )
                    })?;
                })
            } else {
                // Tuple struct field
                let field_index = syn::Index::from(idx);
                Some(quote! {
                    ::calimero_storage::collections::Mergeable::merge(
                        &mut self.#field_index,
                        &other.#field_index
                    ).map_err(|e| {
                        ::calimero_storage::collections::crdt_meta::MergeError::StorageError(
                            format!(
                                "Failed to merge field {}: {:?}",
                                #idx,
                                e
                            )
                        )
                    })?;
                })
            }
        })
        .collect();

    quote! {
        // ============================================================================
        // AUTO-GENERATED by #[app::state] macro
        // ============================================================================
        //
        // This Mergeable implementation enables automatic conflict resolution during sync.
        //
        // When is this called?
        // - ONLY during remote synchronization (not on local operations)
        // - ONLY when root state conflicts occur (rare)
        // - NOT on every state change (local ops are O(1))
        //
        // Performance:
        // - Local ops: O(1) - this is NOT called
        // - Remote sync: O(N) where N = number of CRDT fields (typically 3-10)
        // - Happens during network sync (already slow), so overhead is negligible
        //
        // What it does:
        // - Merges each CRDT field (Map, Counter, RGA, etc.)
        // - Skips non-CRDT fields (String, u64, etc.) - handled by storage LWW
        // - Recursive merging for nested CRDTs
        // - Guarantees no divergence!
        //
        impl #impl_generics ::calimero_storage::collections::Mergeable for #ident #ty_generics #where_clause {
            fn merge(&mut self, other: &Self)
                -> ::core::result::Result<(), ::calimero_storage::collections::crdt_meta::MergeError>
            {
                #(#merge_calls)*
                ::core::result::Result::Ok(())
            }
        }
    }
}

/// Generate WASM exports for merge operations during sync.
///
/// This generates three exports:
/// 1. `__calimero_register_merge` - Called once at module load to register the merge function
/// 2. `__calimero_merge_root_state` - Called to merge root state during sync
/// 3. `__calimero_merge` - Called to merge custom types (for CrdtType::Custom)
fn generate_registration_hook(ident: &Ident, ty_generics: &syn::TypeGenerics<'_>) -> TokenStream {
    quote! {
        // ============================================================================
        // AUTO-GENERATED WASM Exports for CRDT Merge
        // ============================================================================
        //
        // These functions enable automatic conflict resolution during state sync.
        //
        // Developer impact: ZERO - this is completely automatic!
        //

        // ----------------------------------------------------------------------------
        // Registration Hook
        // ----------------------------------------------------------------------------
        // Called ONCE when the WASM module is loaded by the node runtime.
        // Registers the app's merge function in the global registry.
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn __calimero_register_merge() {
            ::calimero_storage::register_crdt_merge::<#ident #ty_generics>();
        }

        // ----------------------------------------------------------------------------
        // Root State Merge Export
        // ----------------------------------------------------------------------------
        // Called by the runtime to merge root state during sync.
        // Uses the registered Mergeable impl to perform the merge.
        //
        // Memory Protocol:
        // - Input: Two pointers to Borsh-serialized state (local, remote)
        // - Output: Pointer to MergeResult struct with status and data
        // - Caller owns all input memory; callee owns output memory
        //
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn __calimero_merge_root_state(
            local_ptr: u64,
            local_len: u64,
            remote_ptr: u64,
            remote_len: u64,
        ) -> u64 {
            // Safety: The runtime guarantees valid pointers and lengths
            let local_data = unsafe {
                ::core::slice::from_raw_parts(local_ptr as *const u8, local_len as usize)
            };
            let remote_data = unsafe {
                ::core::slice::from_raw_parts(remote_ptr as *const u8, remote_len as usize)
            };

            // Deserialize states
            let local_result = ::calimero_sdk::borsh::from_slice::<#ident #ty_generics>(local_data);
            let remote_result = ::calimero_sdk::borsh::from_slice::<#ident #ty_generics>(remote_data);

            let merge_result = match (local_result, remote_result) {
                (Ok(mut local), Ok(remote)) => {
                    // Merge using the auto-generated Mergeable impl
                    // CRITICAL: Use merge mode to prevent timestamp generation during merge.
                    // Without this, different nodes generate different timestamps, causing
                    // hash divergence even when logical state is identical.
                    let merge_outcome = ::calimero_storage::env::with_merge_mode(|| {
                        ::calimero_storage::collections::Mergeable::merge(&mut local, &remote)
                    });
                    match merge_outcome {
                        Ok(()) => {
                            match ::calimero_sdk::borsh::to_vec(&local) {
                                Ok(bytes) => __MergeResultInternal::Success(bytes),
                                Err(e) => __MergeResultInternal::Error(
                                    ::std::format!("Serialization failed: {}", e)
                                ),
                            }
                        }
                        Err(e) => __MergeResultInternal::Error(
                            ::std::format!("Merge failed: {:?}", e)
                        ),
                    }
                }
                (Err(e), _) => __MergeResultInternal::Error(
                    ::std::format!("Failed to deserialize local state: {}", e)
                ),
                (_, Err(e)) => __MergeResultInternal::Error(
                    ::std::format!("Failed to deserialize remote state: {}", e)
                ),
            };

            // Serialize the result and return pointer
            let result_bytes = ::calimero_sdk::borsh::to_vec(&merge_result)
                .expect("__MergeResultInternal serialization should not fail");
            let ptr = result_bytes.as_ptr() as u64;
            let len = result_bytes.len() as u64;
            ::core::mem::forget(result_bytes);

            // Return packed pointer (high 32 bits = ptr, low 32 bits = len)
            (ptr << 32) | len
        }

        // ----------------------------------------------------------------------------
        // Custom Type Merge Export
        // ----------------------------------------------------------------------------
        // Called by the runtime to merge custom types (CrdtType::Custom) during sync.
        // Looks up the merge function by type name in the global registry.
        //
        // Memory Protocol:
        // - Input: type_name (ptr, len) + local_data (ptr, len) + remote_data (ptr, len)
        // - Output: Pointer to MergeResult struct with status and data
        // - Caller owns all input memory; callee owns output memory
        //
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn __calimero_merge(
            type_name_ptr: u64,
            type_name_len: u64,
            local_ptr: u64,
            local_len: u64,
            remote_ptr: u64,
            remote_len: u64,
        ) -> u64 {
            // Safety: The runtime guarantees valid pointers and lengths
            let type_name_bytes = unsafe {
                ::core::slice::from_raw_parts(type_name_ptr as *const u8, type_name_len as usize)
            };
            let local_data = unsafe {
                ::core::slice::from_raw_parts(local_ptr as *const u8, local_len as usize)
            };
            let remote_data = unsafe {
                ::core::slice::from_raw_parts(remote_ptr as *const u8, remote_len as usize)
            };

            // Parse type name from UTF-8 bytes
            let type_name = match ::core::str::from_utf8(type_name_bytes) {
                Ok(name) => name,
                Err(e) => {
                    let error = __MergeResultInternal::Error(
                        ::std::format!("Invalid UTF-8 in type name: {}", e)
                    );
                    let result_bytes = ::calimero_sdk::borsh::to_vec(&error)
                        .expect("__MergeResultInternal serialization should not fail");
                    let ptr = result_bytes.as_ptr() as u64;
                    let len = result_bytes.len() as u64;
                    ::core::mem::forget(result_bytes);
                    return (ptr << 32) | len;
                }
            };

            // Look up merge function by type name in the global registry
            // Use timestamp 0 for both since CRDT merge is timestamp-independent
            let merge_result = match ::calimero_storage::try_merge_by_type_name(
                type_name,
                local_data,
                remote_data,
                0,
                0,
            ) {
                Some(Ok(merged_bytes)) => __MergeResultInternal::Success(merged_bytes),
                Some(Err(e)) => __MergeResultInternal::Error(
                    ::std::format!("Merge failed: {}", e)
                ),
                None => __MergeResultInternal::Error(
                    ::std::format!("Type '{}' not found in merge registry", type_name)
                ),
            };

            // Serialize the result and return pointer
            let result_bytes = ::calimero_sdk::borsh::to_vec(&merge_result)
                .expect("__MergeResultInternal serialization should not fail");
            let ptr = result_bytes.as_ptr() as u64;
            let len = result_bytes.len() as u64;
            ::core::mem::forget(result_bytes);

            // Return packed pointer (high 32 bits = ptr, low 32 bits = len)
            (ptr << 32) | len
        }

        // Internal merge result type for WASM boundary
        // Prefixed with __ to avoid conflicts with user types
        // Manual BorshSerialize/BorshDeserialize to avoid requiring borsh in app's Cargo.toml
        #[cfg(target_arch = "wasm32")]
        #[doc(hidden)]
        pub enum __MergeResultInternal {
            Success(::std::vec::Vec<u8>),
            Error(::std::string::String),
        }

        #[cfg(target_arch = "wasm32")]
        impl ::calimero_sdk::borsh::BorshSerialize for __MergeResultInternal {
            fn serialize<W: ::std::io::Write>(&self, writer: &mut W) -> ::std::io::Result<()> {
                match self {
                    __MergeResultInternal::Success(data) => {
                        writer.write_all(&[0u8])?;
                        // Write Vec<u8>: length (u32) + bytes
                        let len = data.len() as u32;
                        writer.write_all(&len.to_le_bytes())?;
                        writer.write_all(data)?;
                    }
                    __MergeResultInternal::Error(msg) => {
                        writer.write_all(&[1u8])?;
                        // Write String: length (u32) + bytes
                        let bytes = msg.as_bytes();
                        let len = bytes.len() as u32;
                        writer.write_all(&len.to_le_bytes())?;
                        writer.write_all(bytes)?;
                    }
                }
                Ok(())
            }
        }

        #[cfg(target_arch = "wasm32")]
        impl ::calimero_sdk::borsh::BorshDeserialize for __MergeResultInternal {
            fn deserialize_reader<R: ::std::io::Read>(reader: &mut R) -> ::std::io::Result<Self> {
                let mut tag = [0u8; 1];
                reader.read_exact(&mut tag)?;
                match tag[0] {
                    0 => {
                        // Success: read Vec<u8>
                        let mut len_bytes = [0u8; 4];
                        reader.read_exact(&mut len_bytes)?;
                        let len = u32::from_le_bytes(len_bytes) as usize;
                        let mut data = ::std::vec![0u8; len];
                        reader.read_exact(&mut data)?;
                        Ok(__MergeResultInternal::Success(data))
                    }
                    1 => {
                        // Error: read String
                        let mut len_bytes = [0u8; 4];
                        reader.read_exact(&mut len_bytes)?;
                        let len = u32::from_le_bytes(len_bytes) as usize;
                        let mut bytes = ::std::vec![0u8; len];
                        reader.read_exact(&mut bytes)?;
                        let msg = ::std::string::String::from_utf8(bytes)
                            .map_err(|e| ::std::io::Error::new(::std::io::ErrorKind::InvalidData, e))?;
                        Ok(__MergeResultInternal::Error(msg))
                    }
                    _ => Err(::std::io::Error::new(
                        ::std::io::ErrorKind::InvalidData,
                        "Invalid variant tag for __MergeResultInternal"
                    )),
                }
            }
        }
    }
}

/// Generate method to assign deterministic IDs to all collection fields.
///
/// This method is called by the init wrapper to ensure all top-level collections
/// have deterministic IDs based on their field names, regardless of how they were
/// created in the user's init() function.
fn generate_assign_deterministic_ids_impl(
    ident: &Ident,
    generics: &Generics,
    orig: &StructOrEnumItem,
) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Extract fields from the struct
    let fields = match orig {
        StructOrEnumItem::Struct(s) => &s.fields,
        StructOrEnumItem::Enum(_) => {
            // Enums don't have fields
            return quote! {};
        }
    };

    // Helper function to check if a type is a collection that needs ID assignment
    fn is_collection_type(type_str: &str) -> bool {
        type_str.contains("UnorderedMap")
            || type_str.contains("Vector")
            || type_str.contains("UnorderedSet")
            || type_str.contains("Counter")
            || type_str.contains("ReplicatedGrowableArray")
            || type_str.contains("UserStorage")
            || type_str.contains("FrozenStorage")
    }

    // Generate reassign calls for each collection field
    let reassign_calls: Vec<_> = fields
        .iter()
        .enumerate()
        .filter_map(|(idx, field)| {
            let field_type = &field.ty;
            let type_str = quote! { #field_type }.to_string();

            if !is_collection_type(&type_str) {
                return None;
            }

            // Handle both named fields and tuple struct fields
            if let Some(field_name) = &field.ident {
                // Named field: use field name for both access and ID
                let field_name_str = field_name.to_string();
                Some(quote! {
                    self.#field_name.reassign_deterministic_id(#field_name_str);
                })
            } else {
                // Tuple struct field: use index for access, index string for ID
                let field_index = syn::Index::from(idx);
                let field_name_str = idx.to_string();
                Some(quote! {
                    self.#field_index.reassign_deterministic_id(#field_name_str);
                })
            }
        })
        .collect();

    quote! {
        // ============================================================================
        // AUTO-GENERATED Deterministic ID Assignment
        // ============================================================================
        //
        // This method is called after init() to ensure all top-level collections have
        // deterministic IDs. This allows users to use `UnorderedMap::new()` in init()
        // while still getting deterministic IDs for proper sync behavior.
        //
        // CIP Invariant I9: Deterministic Entity IDs
        // > Given the same application code and field names, all nodes MUST generate
        // > identical entity IDs for the same logical entities.
        //
        // Note: This method is always generated (even if empty) because the init wrapper
        // unconditionally calls it. For apps without CRDT collections, this is a no-op.
        //
        impl #impl_generics #ident #ty_generics #where_clause {
            /// Assigns deterministic IDs to all collection fields based on their field names.
            ///
            /// This is called automatically by the init wrapper. Users should not call this directly.
            #[doc(hidden)]
            pub fn __assign_deterministic_ids(&mut self) {
                #(#reassign_calls)*
            }
        }
    }
}
