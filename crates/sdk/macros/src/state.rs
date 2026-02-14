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

/// Generate registration hook for automatic merge during sync
fn generate_registration_hook(ident: &Ident, ty_generics: &syn::TypeGenerics<'_>) -> TokenStream {
    quote! {
        // ============================================================================
        // AUTO-GENERATED WASM Export for Merge Registration
        // ============================================================================
        //
        // This function is called ONCE when the WASM module is loaded by the node runtime.
        // It registers the app's merge function so that sync can automatically call it.
        //
        // Lifecycle:
        // 1. WASM loads → runtime calls __calimero_register_merge()
        // 2. Registration → stores merge function in global registry
        // 3. Sync → automatically uses registered merge
        //
        // Developer impact: ZERO - this is completely automatic!
        //
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn __calimero_register_merge() {
            ::calimero_storage::register_crdt_merge::<#ident #ty_generics>();
        }

        // ============================================================================
        // AUTO-GENERATED WASM Export for Memory Allocation
        // ============================================================================
        //
        // This function is called by the runtime to allocate memory in the WASM module
        // for passing data to merge functions.
        //
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn __calimero_alloc(size: u64) -> u64 {
            // Guard against zero-size allocation (UB per GlobalAlloc contract)
            if size == 0 {
                return ::std::ptr::NonNull::dangling().as_ptr() as u64;
            }
            let layout = ::std::alloc::Layout::from_size_align(size as usize, 8)
                .expect("Invalid allocation size");
            // SAFETY: Layout is valid (size > 0), and we're in WASM where the allocator is available
            let ptr = unsafe { ::std::alloc::alloc(layout) };
            ptr as u64
        }

        // ============================================================================
        // AUTO-GENERATED WASM Export for Root State Merge
        // ============================================================================
        //
        // This function is called by the runtime during sync when two nodes have
        // concurrent updates to the root state and need to merge them.
        //
        // Protocol:
        // 1. Runtime writes local and remote state to WASM memory
        // 2. Runtime calls __calimero_merge_root_state()
        // 3. This function deserializes, merges, serializes
        // 4. Returns pointer to MergeResult struct
        //
        // MergeResult struct layout (33 bytes):
        //   - success: u8 (0 = failure, 1 = success)
        //   - data_ptr: u64 (pointer to merged data if success)
        //   - data_len: u64 (length of merged data if success)
        //   - error_ptr: u64 (pointer to error message if failure)
        //   - error_len: u64 (length of error message if failure)
        //
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn __calimero_merge_root_state(
            local_ptr: u64,
            local_len: u64,
            remote_ptr: u64,
            remote_len: u64,
        ) -> u64 {
            // SAFETY: The runtime guarantees these pointers are valid
            let local_slice = unsafe {
                ::std::slice::from_raw_parts(local_ptr as *const u8, local_len as usize)
            };
            let remote_slice = unsafe {
                ::std::slice::from_raw_parts(remote_ptr as *const u8, remote_len as usize)
            };

            // Deserialize local state
            let mut local_state: #ident #ty_generics = match ::calimero_sdk::borsh::from_slice(local_slice) {
                Ok(state) => state,
                Err(e) => {
                    return __calimero_make_merge_error(format!("Failed to deserialize local state: {}", e));
                }
            };

            // Deserialize remote state
            let remote_state: #ident #ty_generics = match ::calimero_sdk::borsh::from_slice(remote_slice) {
                Ok(state) => state,
                Err(e) => {
                    return __calimero_make_merge_error(format!("Failed to deserialize remote state: {}", e));
                }
            };

            // Merge using the auto-generated Mergeable implementation
            if let Err(e) = ::calimero_storage::collections::Mergeable::merge(&mut local_state, &remote_state) {
                return __calimero_make_merge_error(format!("Merge failed: {}", e));
            }

            // Serialize the merged state
            let merged_bytes = match ::calimero_sdk::borsh::to_vec(&local_state) {
                Ok(bytes) => bytes,
                Err(e) => {
                    return __calimero_make_merge_error(format!("Failed to serialize merged state: {}", e));
                }
            };

            // Allocate and copy the result
            __calimero_make_merge_success(merged_bytes)
        }

        /// Helper to create a success MergeResult
        #[cfg(target_arch = "wasm32")]
        fn __calimero_make_merge_success(data: Vec<u8>) -> u64 {
            let data_len = data.len() as u64;
            let data_ptr = __calimero_alloc(data_len);
            // SAFETY: We just allocated this memory
            unsafe {
                ::std::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr as *mut u8, data.len());
            }

            // Allocate result struct (33 bytes)
            let result_ptr = __calimero_alloc(33);
            // SAFETY: We just allocated this memory
            unsafe {
                let ptr = result_ptr as *mut u8;
                // success = 1
                *ptr = 1;
                // data_ptr
                ::std::ptr::copy_nonoverlapping(data_ptr.to_le_bytes().as_ptr(), ptr.add(1), 8);
                // data_len
                ::std::ptr::copy_nonoverlapping(data_len.to_le_bytes().as_ptr(), ptr.add(9), 8);
                // error_ptr = 0
                ::std::ptr::copy_nonoverlapping(0u64.to_le_bytes().as_ptr(), ptr.add(17), 8);
                // error_len = 0
                ::std::ptr::copy_nonoverlapping(0u64.to_le_bytes().as_ptr(), ptr.add(25), 8);
            }
            result_ptr
        }

        /// Helper to create a failure MergeResult
        #[cfg(target_arch = "wasm32")]
        fn __calimero_make_merge_error(error: String) -> u64 {
            let error_bytes = error.into_bytes();
            let error_len = error_bytes.len() as u64;
            let error_ptr = __calimero_alloc(error_len);
            // SAFETY: We just allocated this memory
            unsafe {
                ::std::ptr::copy_nonoverlapping(error_bytes.as_ptr(), error_ptr as *mut u8, error_bytes.len());
            }

            // Allocate result struct (33 bytes)
            let result_ptr = __calimero_alloc(33);
            // SAFETY: We just allocated this memory
            unsafe {
                let ptr = result_ptr as *mut u8;
                // success = 0
                *ptr = 0;
                // data_ptr = 0
                ::std::ptr::copy_nonoverlapping(0u64.to_le_bytes().as_ptr(), ptr.add(1), 8);
                // data_len = 0
                ::std::ptr::copy_nonoverlapping(0u64.to_le_bytes().as_ptr(), ptr.add(9), 8);
                // error_ptr
                ::std::ptr::copy_nonoverlapping(error_ptr.to_le_bytes().as_ptr(), ptr.add(17), 8);
                // error_len
                ::std::ptr::copy_nonoverlapping(error_len.to_le_bytes().as_ptr(), ptr.add(25), 8);
            }
            result_ptr
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
