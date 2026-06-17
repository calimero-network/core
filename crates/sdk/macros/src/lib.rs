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
use syn::{DeriveInput, Expr, ItemImpl};

use crate::event::{EventImpl, EventImplInput};
use crate::items::{Empty, StructOrEnumItem};
use crate::logic::{LogicImpl, LogicImplInput};
use crate::private::{PrivateArgs, PrivateImpl, PrivateImplInput};
use crate::state::{StateArgs, StateImpl, StateImplInput};

mod errors;
mod event;
mod forbidden_types;
mod items;
mod logic;
mod macros;
mod mergeable;
mod migrate_derive;
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
    let mut item = parse_macro_input!(input as StructOrEnumItem);

    // Hand `version = N` to a `#[derive(app::Migrate)]` below: this attribute
    // is consumed before that derive expands, so the version is re-emitted as
    // a `#[migrate(state_version = N)]` helper the derive can read.
    state::inject_migrate_state_version(&mut item, &args);

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

/// Marks a method as a cross-context (`xcall`) entry point.
///
/// A marker consumed by `#[app::logic]` and recorded in the ABI as
/// `Method.xcall_callable`; on its own it does not modify the method. The node
/// uses the flag to restrict `xcall` dispatch to declared entry points.
///
/// Must be a registered attribute (not just read syntactically like
/// `#[app::view]`) so it resolves at the method site when `#[app::logic]`
/// re-emits the impl verbatim.
///
/// # Usage
///
/// Apply `#[app::xcall]` to a public logic method to allow other contexts in
/// the same namespace to invoke it via `env::xcall`. Mutually exclusive with
/// `#[app::init]` and `#[app::view]` (xcall is fire-and-forget, so a read-only
/// target's return value would go nowhere).
#[proc_macro_attribute]
pub fn xcall(_args: TokenStream, input: TokenStream) -> TokenStream {
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

/// Transforms a migration-check predicate into a WASM-compatible export.
///
/// This is the sibling of [`migrate`]: the runtime invokes the generated
/// `__calimero_migration_check` export on the produced v2 root **before** it is
/// committed. The author writes `fn check(old: OldState, new: NewState) -> bool`;
/// returning `false` (or panicking) lets the runtime *logically abort* the
/// migration, leaving the still-untouched v1 root intact.
///
/// # Usage
///
/// Apply the `#[app::migration_check]` attribute to a two-parameter predicate
/// that returns `bool`.
///
/// # Features
///
/// - **WASM Export**: Generates a `#[no_mangle] pub extern "C" fn
///   __calimero_migration_check` the runtime calls
/// - **Old + New Access**: Reads the old v1 root via `calimero_sdk::read_raw()`
///   and the produced new v2 root via `env::input()`
/// - **Read-only**: Pure predicate — never assigns deterministic ids and is not
///   wrapped in merge mode (no state is produced)
/// - **Backwards Compatible**: Apps without this export migrate unchecked (the
///   runtime treats a missing export as `Ok(true)`)
/// - **Testing Support**: Preserves the original function signature for non-WASM testing
#[proc_macro_attribute]
pub fn migration_check(args: TokenStream, input: TokenStream) -> TokenStream {
    migration::migration_check_impl(args.into(), input.into()).into()
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

/// Derives `Mergeable` for a user-defined struct so it can be used as a value
/// inside Calimero CRDT collections (`UnorderedMap<_, V>`, `Vector<V>`, ...).
///
/// The derive applies the same forbidden-type lint as `#[app::state]` to every
/// field — `std::collections::*`, bare `Vec`, bare `String`, and bare
/// primitives are rejected because they have no merge semantics and would
/// silently diverge across replicas. Use SDK CRDT types or `LwwRegister<T>`
/// instead.
///
/// # Generated impl
///
/// `merge()` calls each field's own `Mergeable::merge()` in declaration order.
/// Every field must therefore implement `Mergeable`. If you really need a
/// non-CRDT field, skip the derive and implement `Mergeable` by hand.
///
/// # Limitations
///
/// Enums are rejected — there's no canonical way to merge values from
/// different variants. Wrap in `LwwRegister<MyEnum>` for last-write-wins, or
/// implement `Mergeable` by hand.
#[proc_macro_derive(Mergeable)]
pub fn derive_mergeable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    mergeable::derive(input).into()
}

/// Generates a `#[app::migrate]` migration function from the new state struct,
/// so app authors write only the *diff* instead of the full
/// read-deserialize-carry skeleton.
///
/// ```ignore
/// #[app::state(emits = for<'a> MigrateEvent<'a>)]
/// #[derive(Migrate)]
/// #[migrate(from = AppV1, method = migrate_v1_to_v2,
///           emit = MigrateEvent::Migrated { from: "1.0.0", to: "2.0.0" })]
/// pub struct AppV2 {
///     items: UnorderedMap<String, LwwRegister<String>>, // carried from old.items
///     #[migrate(new = LwwRegister::new("note".to_owned()))]
///     notes: LwwRegister<String>,                       // additive: seeded
///     #[migrate(from = legacy)]
///     renamed: LwwRegister<String>,                     // rename: old.legacy
///     #[migrate(from = count, with = u64_reg_to_string)]
///     count: LwwRegister<String>,                       // type change via `with`
/// }
/// ```
///
/// Every field is carried through by name from `from`'s borsh layout unless a
/// `#[migrate(...)]` attribute overrides it:
/// - `new = EXPR` — additive seed for a field absent from the old state;
/// - `from = old` — renamed source field;
/// - `with = EXPR` — transform: `EXPR(old.field)` (combine with `from`); covers
///   type changes / struct→enum / single-field content transforms;
/// - struct-level `emit = EXPR` — emit an app event from the migration.
///
/// Fields absent from the new struct are dropped automatically; the generated
/// body runs under the same merge-mode + deterministic-id machinery as a
/// hand-written `#[app::migrate]`.
///
/// # Limitations
///
/// Non-generic structs with named fields only. **Cross-field** transforms still
/// need a hand-written body — splitting one field into several, or deriving a new
/// field from a field you also carry (the source would move twice). The `from`
/// type is the developer-supplied borsh shadow of the old layout (field order
/// must match the old `#[app::state]`). The method name defaults to `migrate`;
/// give each derive an explicit `method = ...` when a module has more than one.
#[proc_macro_derive(Migrate, attributes(migrate))]
pub fn derive_migrate(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    migrate_derive::derive(input).into()
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
