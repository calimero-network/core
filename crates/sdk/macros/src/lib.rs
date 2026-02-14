//! Procedural macros for Calimero SDK applications.
//!
//! This module provides the core procedural macros that enable Calimero applications
//! to define their state, logic, events, and private data structures with minimal boilerplate.
//!
//! # Key Macros
//!
//! - **`#[app::state]`** - Defines the application's persistent state structure
//! - **`#[app::logic]`** - Defines the application's business logic implementation
//! - **`#[app::event]`** - Defines application events for external communication
//! - **`#[app::private]`** - Defines private data structures for internal use
//! - **`app::emit!`** - Emits events with optional callback handlers
//!
//! # Basic Usage
//!
//! The macros work together to define application state, logic, and events:
//! - Use `#[app::state]` to define persistent application state
//! - Use `#[app::logic]` to implement business logic methods
//! - Use `#[app::event]` to define events for external communication
//!
//! # Event Emission with Handlers
//!
//! Use `app::emit!` macro to emit events:
//! - Simple emission: `app::emit!(MyEvent::DataChanged { data: "hello" })`
//! - With callback handler: `app::emit!((MyEvent::CounterUpdated { value: 42 }, "counter_handler"))`

#![cfg_attr(
    all(test, feature = "nightly"),
    feature(non_exhaustive_omitted_patterns_lint)
)]

use macros::parse_macro_input;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, ToTokens};
use syn::{Expr, ItemImpl};

use crate::event::{EventImpl, EventImplInput};
use crate::items::{Empty, StructOrEnumItem};
use crate::logic::{LogicImpl, LogicImplInput};
use crate::private::{PrivateArgs, PrivateImpl, PrivateImplInput};
use crate::state::{StateArgs, StateImpl, StateImplInput};

mod errors;
mod event;
mod items;
mod logic;
mod macros;
mod migration;
mod private;
mod reserved;
mod sanitizer;
mod state;

// todo! use referenced lifetimes everywhere

// todo! permit #[app::logic(crate = "calimero_sdk")]

/// Defines the application's business logic implementation.
///
/// This macro transforms a regular `impl` block into a Calimero application logic block,
/// enabling the implementation to interact with the runtime, emit events, and manage state.
///
/// # Usage
///
/// Apply the `#[app::logic]` attribute to impl blocks to define business logic methods.
///
/// # Features
///
/// - **State Access**: Direct access to application state via `&mut self`
/// - **Event Emission**: Use `app::emit!` to emit events
/// - **Error Handling**: Return `app::Result<T>` for proper error propagation
/// - **Runtime Integration**: Automatic integration with Calimero runtime
#[proc_macro_attribute]
pub fn logic(args: TokenStream, input: TokenStream) -> TokenStream {
    reserved::init();
    let _args = parse_macro_input!({ input } => args as Empty);
    let block = parse_macro_input!(input as ItemImpl);

    let tokens = match LogicImpl::try_from(LogicImplInput { item: &block }) {
        Ok(data) => data.to_token_stream(),
        Err(err) => err.to_compile_error(),
    };

    tokens.into()
}

/// Defines the application's persistent state structure.
///
/// This macro transforms a struct or enum into a Calimero application state,
/// providing automatic serialization, persistence, and state management capabilities.
///
/// # Usage
///
/// Apply the `#[app::state]` attribute to a struct to make it the application state.
///
/// # Features
///
/// - **Persistence**: State is automatically persisted across application restarts
/// - **Serialization**: Automatic serialization/deserialization support
/// - **Type Safety**: Compile-time validation of state structure
/// - **Storage Integration**: Seamless integration with Calimero storage system
///
/// # Parameters
///
/// The macro accepts optional parameters to customize behavior:
/// - `storage_key` - Custom storage key for the state
/// - `version` - State version for migration support
#[proc_macro_attribute]
pub fn state(args: TokenStream, input: TokenStream) -> TokenStream {
    reserved::init();

    let args = parse_macro_input!({ input } => args as StateArgs);
    let item = parse_macro_input!(input as StructOrEnumItem);

    let tokens = match StateImpl::try_from(StateImplInput {
        item: &item,
        args: &args,
    }) {
        Ok(data) => data.to_token_stream(),
        Err(err) => err.to_compile_error(),
    };

    tokens.into()
}

/// Defines private data structures for internal application use.
///
/// This macro creates private data structures that are not exposed to external systems
/// but can be used internally by the application for temporary storage or processing.
///
/// # Usage
///
/// Apply the `#[app::private]` attribute to structs for internal data that won't be exposed externally.
///
/// # Features
///
/// - **Internal Use**: Data is not exposed to external systems
/// - **Temporary Storage**: Suitable for processing queues, caches, etc.
/// - **Type Safety**: Compile-time validation of private data structure
/// - **Memory Management**: Automatic memory management for private data
#[proc_macro_attribute]
pub fn private(args: TokenStream, input: TokenStream) -> TokenStream {
    reserved::init();

    let args = parse_macro_input!({ input } => args as PrivateArgs);
    let item = parse_macro_input!(input as StructOrEnumItem);

    let tokens = match PrivateImpl::try_from(PrivateImplInput {
        item: &item,
        args: &args,
    }) {
        Ok(data) => data.to_token_stream(),
        Err(err) => err.to_compile_error(),
    };

    tokens.into()
}

/// Marks a function as the application initialization function.
///
/// This macro marks a function that will be called when the application is first initialized.
/// It's a marker attribute that doesn't modify the function but indicates its purpose.
///
/// # Usage
///
/// Apply the `#[app::init]` attribute to a function to mark it as the application initialization function.
#[proc_macro_attribute]
pub fn init(_args: TokenStream, input: TokenStream) -> TokenStream {
    // this is a no-op, the attribute is just a marker
    input
}

/// Marks a function as the application cleanup function.
///
/// This macro marks a function that will be called when the application is being destroyed.
/// It's a marker attribute that doesn't modify the function but indicates its purpose.
///
/// # Usage
///
/// Apply the `#[app::destroy]` attribute to a function to mark it as the application cleanup function.
#[proc_macro_attribute]
pub fn destroy(_args: TokenStream, input: TokenStream) -> TokenStream {
    // this is a no-op, the attribute is just a marker
    input
}

/// Transforms a migration function into a WASM-compatible export.
///
/// This macro is used to define state migration functions that are called during
/// application upgrades. The migration function should read the old state using
/// `calimero_sdk::read_raw()`, transform it to the new schema, and return the new state.
///
/// # Usage
///
/// Apply the `#[app::migrate]` attribute to a function that performs state migration.
///
/// # Features
///
/// - **WASM Export**: Generates a `#[no_mangle] pub extern "C"` function for the runtime
/// - **Panic Hook**: Sets up proper panic handling with location information
/// - **Serialization**: Automatically serializes the returned state with borsh
/// - **Testing Support**: Preserves the original function signature for non-WASM testing
#[proc_macro_attribute]
pub fn migrate(args: TokenStream, input: TokenStream) -> TokenStream {
    migration::migrate_impl(args.into(), input.into()).into()
}

/// Defines application events for external communication.
///
/// This macro transforms a struct or enum into a Calimero application event,
/// enabling the application to emit events that can be consumed by external systems.
///
/// # Usage
///
/// Apply the `#[app::event]` attribute to enums to define application events.
///
/// # Features
///
/// - **External Communication**: Events are available to external systems
/// - **Serialization**: Automatic serialization for event transmission
/// - **Type Safety**: Compile-time validation of event structure
/// - **Event Emission**: Use with `app::emit!` macro for emission
#[proc_macro_attribute]
pub fn event(args: TokenStream, input: TokenStream) -> TokenStream {
    reserved::init();
    let _args = parse_macro_input!({ input } => args as Empty);
    let item = parse_macro_input!(input as StructOrEnumItem);
    let tokens = match EventImpl::try_from(EventImplInput { item: &item }) {
        Ok(data) => data.to_token_stream(),
        Err(err) => err.to_compile_error(),
    };
    tokens.into()
}

/// Emits an event with optional callback handler support.
///
/// This macro provides a convenient way to emit events from Calimero applications.
/// It supports both simple event emission and event emission with callback handlers.
///
/// # Usage
///
/// ## Simple Event Emission
///
/// Use `app::emit!(MyEvent::Variant { field: value })` for simple event emission.
///
/// ## Event Emission with Callback Handler
///
/// Use `app::emit!((MyEvent::Variant { field: value }, "handler_name"))` for events with callbacks.
///
/// # Features
///
/// - **Flexible Syntax**: Supports both simple events and events with handlers
/// - **Type Safety**: Compile-time validation of event structure
/// - **Handler Support**: Optional callback handler execution
/// - **Backward Compatibility**: Existing code continues to work unchanged
///
/// # Handler Methods
///
/// When using callback handlers, the specified handler method should be defined
/// in your application logic by implementing handler methods in your `#[app::logic]` impl block.
#[proc_macro]
pub fn emit(input: TokenStream) -> TokenStream {
    // Try to parse as a tuple first to check for handler parameter
    if let Ok(parsed) = syn::parse::<syn::ExprTuple>(input.clone()) {
        if parsed.elems.len() == 2 {
            let event = &parsed.elems[0];
            let handler = &parsed.elems[1];

            quote!(::calimero_sdk::event::emit_with_handler(#event, #handler)).into()
        } else if parsed.elems.len() == 1 {
            // Single element tuple - just the event
            let event = &parsed.elems[0];
            quote!(::calimero_sdk::event::emit(#event)).into()
        } else {
            // Fallback to regular emit if not 1 or 2 arguments
            let event = &parsed.elems[0];
            quote!(::calimero_sdk::event::emit(#event)).into()
        }
    } else {
        // Simple case - just the event (not a tuple)
        let input = parse_macro_input!(input as Expr);
        quote!(::calimero_sdk::event::emit(#input)).into()
    }
}

/// Creates an error result with the given message.
///
/// This macro provides a convenient way to create error results in Calimero applications.
/// It's equivalent to `Err(app::Error::new(message))`.
///
/// # Usage
///
/// Use `app::err!("error message")` to return an error result with a formatted message.
#[proc_macro]
pub fn err(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as TokenStream2);

    quote!(::calimero_sdk::__err__!(#input)).into()
}

/// Returns early with an error result.
///
/// This macro provides a convenient way to return early from a function with an error.
/// It's equivalent to `return Err(app::Error::new(message))`.
///
/// # Usage
///
/// Use `app::bail!("error message")` to immediately return an error with a formatted message.
#[proc_macro]
pub fn bail(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as TokenStream2);

    quote!(::calimero_sdk::__bail__!(#input)).into()
}

/// Logs a message to the runtime's logging system.
///
/// This macro provides a convenient way to log messages from Calimero applications.
/// It supports format strings and arguments similar to `println!`.
///
/// # Usage
///
/// Use `app::log!("message")` or `app::log!("formatted {}", value)` for logging messages.
#[proc_macro]
pub fn log(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as TokenStream2);

    quote!(::calimero_sdk::__log__!(#input)).into()
}

/// Generates a WASM merge export for a custom type with `Mergeable` implementation.
///
/// This macro is applied to types that implement `Mergeable` and need to be merged
/// via WASM callback during sync. It generates a `__calimero_merge_{TypeName}` export
/// that the runtime calls when entities with `CrdtType::Custom("TypeName")` conflict.
///
/// # Usage
///
/// Apply to types that implement `Mergeable`:
///
/// ```ignore
/// use calimero_sdk::app;
/// use calimero_storage::collections::{Mergeable, crdt_meta::MergeError};
///
/// #[derive(BorshSerialize, BorshDeserialize)]
/// pub struct TeamStats {
///     wins: Counter,
///     losses: Counter,
/// }
///
/// #[app::mergeable]
/// impl Mergeable for TeamStats {
///     fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
///         self.wins.merge(&other.wins)?;
///         self.losses.merge(&other.losses)?;
///         Ok(())
///     }
/// }
/// ```
///
/// # Generated Code
///
/// This generates a WASM export `__calimero_merge_TeamStats` that:
/// 1. Deserializes both local and remote values as `TeamStats`
/// 2. Calls `Mergeable::merge()` on local with remote
/// 3. Serializes the merged result
/// 4. Returns a pointer to `MergeResult` struct
///
/// # When Is This Called?
///
/// During sync, when an entity has `CrdtType::Custom("TeamStats")` in its metadata
/// and conflicts with another version, the storage layer calls `merge_custom()` which
/// invokes this WASM export.
#[proc_macro_attribute]
pub fn mergeable(args: TokenStream, input: TokenStream) -> TokenStream {
    reserved::init();
    let _args = parse_macro_input!({ input } => args as Empty);
    let impl_block = parse_macro_input!(input as ItemImpl);

    // Extract the type name from `impl Mergeable for TypeName`
    let type_path = match &*impl_block.self_ty {
        syn::Type::Path(type_path) => type_path,
        _ => {
            return syn::Error::new_spanned(&impl_block.self_ty, "Expected a type path")
                .to_compile_error()
                .into();
        }
    };

    // Get just the type name (last segment)
    let type_name = match type_path.path.segments.last() {
        Some(seg) => &seg.ident,
        None => {
            return syn::Error::new_spanned(&impl_block.self_ty, "Expected a type name")
                .to_compile_error()
                .into();
        }
    };

    let type_name_str = type_name.to_string();
    let export_name = syn::Ident::new(
        &format!("__calimero_merge_{type_name_str}"),
        type_name.span(),
    );

    // Generate a unique module name to avoid conflicts
    let mod_name = syn::Ident::new(
        &format!("__calimero_merge_{}_impl", type_name_str.to_lowercase()),
        type_name.span(),
    );

    let output = quote! {
        #impl_block

        // ============================================================================
        // AUTO-GENERATED WASM Export for Custom Type Merge
        // ============================================================================
        //
        // This function is called by the runtime during sync when entities with
        // CrdtType::Custom("#type_name_str") need to be merged.
        //
        #[cfg(target_arch = "wasm32")]
        #[doc(hidden)]
        mod #mod_name {
            use super::*;

            fn alloc(size: u64) -> u64 {
                // Guard against zero-size allocation (UB per GlobalAlloc contract)
                if size == 0 {
                    return ::std::ptr::NonNull::dangling().as_ptr() as u64;
                }
                let layout = ::std::alloc::Layout::from_size_align(size as usize, 8)
                    .expect("Invalid allocation size");
                unsafe { ::std::alloc::alloc(layout) as u64 }
            }

            fn make_success(data: ::std::vec::Vec<u8>) -> u64 {
                let data_len = data.len() as u64;
                let data_ptr = alloc(data_len);
                unsafe {
                    ::std::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr as *mut u8, data.len());
                }
                let result_ptr = alloc(33);
                unsafe {
                    let ptr = result_ptr as *mut u8;
                    *ptr = 1; // success
                    ::std::ptr::copy_nonoverlapping(data_ptr.to_le_bytes().as_ptr(), ptr.add(1), 8);
                    ::std::ptr::copy_nonoverlapping(data_len.to_le_bytes().as_ptr(), ptr.add(9), 8);
                    ::std::ptr::copy_nonoverlapping(0u64.to_le_bytes().as_ptr(), ptr.add(17), 8);
                    ::std::ptr::copy_nonoverlapping(0u64.to_le_bytes().as_ptr(), ptr.add(25), 8);
                }
                result_ptr
            }

            fn make_error(error: ::std::string::String) -> u64 {
                let error_bytes = error.into_bytes();
                let error_len = error_bytes.len() as u64;
                let error_ptr = alloc(error_len);
                unsafe {
                    ::std::ptr::copy_nonoverlapping(error_bytes.as_ptr(), error_ptr as *mut u8, error_bytes.len());
                }
                let result_ptr = alloc(33);
                unsafe {
                    let ptr = result_ptr as *mut u8;
                    *ptr = 0; // failure
                    ::std::ptr::copy_nonoverlapping(0u64.to_le_bytes().as_ptr(), ptr.add(1), 8);
                    ::std::ptr::copy_nonoverlapping(0u64.to_le_bytes().as_ptr(), ptr.add(9), 8);
                    ::std::ptr::copy_nonoverlapping(error_ptr.to_le_bytes().as_ptr(), ptr.add(17), 8);
                    ::std::ptr::copy_nonoverlapping(error_len.to_le_bytes().as_ptr(), ptr.add(25), 8);
                }
                result_ptr
            }

            #[no_mangle]
            pub extern "C" fn #export_name(
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

                // Deserialize local value
                let mut local_value: #type_name = match ::calimero_sdk::borsh::from_slice(local_slice) {
                    Ok(v) => v,
                    Err(e) => {
                        return make_error(::std::format!("Failed to deserialize local {}: {}", #type_name_str, e));
                    }
                };

                // Deserialize remote value
                let remote_value: #type_name = match ::calimero_sdk::borsh::from_slice(remote_slice) {
                    Ok(v) => v,
                    Err(e) => {
                        return make_error(::std::format!("Failed to deserialize remote {}: {}", #type_name_str, e));
                    }
                };

                // Merge using the Mergeable implementation
                if let Err(e) = ::calimero_storage::collections::Mergeable::merge(&mut local_value, &remote_value) {
                    return make_error(::std::format!("Merge failed for {}: {}", #type_name_str, e));
                }

                // Serialize the merged value
                let merged_bytes = match ::calimero_sdk::borsh::to_vec(&local_value) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        return make_error(::std::format!("Failed to serialize merged {}: {}", #type_name_str, e));
                    }
                };

                make_success(merged_bytes)
            }
        }
    };

    output.into()
}
