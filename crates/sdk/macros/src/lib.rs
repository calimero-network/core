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

/// Marks a function as an entry point to the migration function.
///
/// This function bypasses the state loading on function call, allowing
/// to read the raw state bytes, transform it into the new struct (new version of
/// the app state), and return the new struct representing the new state of the app.
///
/// The return value of this function will be serialized and written to the
/// application's root state key by the node.
///
/// # Example
///
/// ```rust
/// /// #[app::migrate]
/// pub fn migrate_v1_to_v2() -> NewState {
///     // Read raw bytes from the standardized root storage key
///     let old_bytes = calimero_sdk::state::read_raw().expect("No existing state found");
///     
///     // Deserialize using the old schema
///     let old = OldState::try_from_slice(&old_bytes).expect("Failed to deserialize old state");
///
///     env::log(&format!("Migrating state. Old count: {}", old.count));
///
///     // Return the new state structure
///     NewState {
///         count: old.count,
///         new_field: "Default Value".to_string(),
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn migrate(args: TokenStream, input: TokenStream) -> TokenStream {
    migration::migrate_impl(args.into(), input.into()).into()
}
