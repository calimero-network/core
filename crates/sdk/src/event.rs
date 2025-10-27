//! Event emission system for Calimero applications.
//!
//! This module provides the core event emission functionality for Calimero applications,
//! including support for callback handlers that are automatically executed when events are emitted.
//!
//! # Basic Usage
//!
//! ```rust,no_run
//! use calimero_sdk::event::{emit, emit_with_handler};
//!
//! #[derive(serde::Serialize)]
//! struct MyEvent {
//!     data: String,
//! }
//!
//! impl calimero_sdk::event::AppEvent for MyEvent {
//!     fn kind(&self) -> std::borrow::Cow<'_, str> {
//!         "MyEvent".into()
//!     }
//!     fn data(&self) -> std::borrow::Cow<'_, [u8]> {
//!         serde_json::to_vec(self).unwrap().into()
//!     }
//! }
//!
//! impl calimero_sdk::event::AppEventExt for MyEvent {}
//!
//! // Emit a simple event
//! emit(MyEvent { data: "hello".to_string() });
//!
//! // Emit an event with a callback handler
//! emit_with_handler(MyEvent { data: "hello".to_string() }, "my_handler");
//! ```
//!
//! # Callback Handlers
//!
//! When using `emit_with_handler`, the specified handler method will be automatically
//! called by the runtime after the event is emitted. The handler method should be defined
//! in your application and will receive the event data as parameters.
//!
//! ```rust
//! struct MyApp;
//!
//! impl MyApp {
//!     fn my_handler(&mut self, data: &str) {
//!         // Handle the event
//!         println!("Received: {}", data);
//!     }
//! }
//! ```
//!
//! ## ⚠️ Handler Execution Model
//!
//! **IMPORTANT**: Event handlers currently execute **sequentially**, but future optimizations
//! may execute handlers **in parallel**. Your handlers **MUST** be written to handle both cases.
//!
//! ### Handler Requirements
//!
//! All event handlers MUST satisfy these properties:
//!
//! 1. **Commutative**: Handler order must not affect final state
//!    - ✅ SAFE: CRDT operations (`Counter::increment()`, `UnorderedMap::insert()`)
//!    - ❌ UNSAFE: Dependent operations (create → update → delete chains)
//!
//! 2. **Independent**: Handlers must not share mutable state
//!    - ✅ SAFE: Each handler modifies different CRDT keys
//!    - ❌ UNSAFE: Multiple handlers modifying the same entity
//!
//! 3. **Idempotent**: Re-execution must be safe (handlers may retry on failure)
//!    - ✅ SAFE: CRDT operations (naturally idempotent)
//!    - ❌ UNSAFE: External API calls (payment processing, email sending)
//!
//! 4. **No external side effects**: Handlers should only modify CRDT state
//!    - ✅ SAFE: Pure state updates using CRDTs
//!    - ❌ UNSAFE: HTTP requests, file I/O, blockchain transactions
//!
//! ### Examples
//!
//! **✅ SAFE Handler** (CRDT-only):
//! ```rust,ignore
//! fn my_handler(&mut self, user_id: &str) {
//!     // Each handler uses a unique key (safe for parallel execution)
//!     self.handler_counter.increment();  // G-Counter is commutative
//! }
//! ```
//!
//! **❌ UNSAFE Handler** (ordering dependency):
//! ```rust,ignore
//! fn create_handler(&mut self, id: &str) {
//!     self.items.insert(id, "created");
//! }
//! fn update_handler(&mut self, id: &str) {
//!     // DANGER: Assumes create_handler ran first!
//!     let item = self.items.get(id).expect("must exist");
//!     self.items.insert(id, format!("{} updated", item));
//! }
//! ```
//!
//! **❌ UNSAFE Handler** (external side effects):
//! ```rust,ignore
//! fn payment_handler(&mut self, amount: u64) {
//!     // DANGER: Non-idempotent external call!
//!     external_api::charge_payment(amount);
//! }
//! ```
//!
//! If your use case requires sequential execution or external side effects,
//! consider refactoring to use CRDTs or implementing a task queue pattern.

use core::any::TypeId;
use core::cell::RefCell;
use core::mem::transmute;
use std::borrow::Cow;

use crate::env;
use crate::state::AppState;

/// Trait for application events that can be emitted.
///
/// All events must implement this trait to be compatible with the event emission system.
pub trait AppEvent {
    /// Returns the event kind/type as a string.
    fn kind(&self) -> Cow<'_, str>;

    /// Returns the event data as bytes.
    fn data(&self) -> Cow<'_, [u8]>;
}

/// An encoded application event with its kind and data.
#[derive(Debug)]
#[non_exhaustive]
pub struct EncodedAppEvent<'a> {
    /// The event kind/type.
    pub kind: Cow<'a, str>,
    /// The event data as bytes.
    pub data: Cow<'a, [u8]>,
}

thread_local! {
    /// The event emission function that processes events through the runtime.
    /// This is set during app initialization and used by the `emit` function.
    static EVENT_EMITTER: RefCell<fn(Box<dyn AppEventExt>)> = panic!("uninitialized event emitter");

    /// The name of the callback handler method to call when emitting events with handlers.
    /// This is set temporarily by `emit_with_handler` and read by the runtime.
    static CURRENT_CALLBACK_HANDLER: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Internal handler function that processes events through the runtime.
///
/// This function is called by the event emission system to actually emit events
// through the WASM runtime.
#[track_caller]
#[inline(never)]
fn handler<E: AppEventExt + 'static>(event: Box<dyn AppEventExt>) {
    if let Ok(event) = E::downcast(event) {
        env::emit(&event);
    }
}

/// Registers the event emission system for the given application state.
///
/// This function must be called during app initialization to set up the event emission system.
/// It configures the internal event emitter to work with the application's event types.
pub fn register<S: AppState>()
where
    for<'a> S::Event<'a>: AppEventExt,
{
    EVENT_EMITTER.set(handler::<S::Event<'static>>);
}

/// Emits an event without any callback handler.
///
/// This is the standard event emission function that simply emits the event
/// through the runtime without any additional processing.
///
/// # Example
///
/// ```rust,no_run
/// use calimero_sdk::event::emit;
///
/// #[derive(serde::Serialize)]
/// struct MyEvent {
///     data: String,
/// }
///
/// impl calimero_sdk::event::AppEvent for MyEvent {
///     fn kind(&self) -> std::borrow::Cow<'_, str> {
///         "MyEvent".into()
///     }
///     fn data(&self) -> std::borrow::Cow<'_, [u8]> {
///         serde_json::to_vec(self).unwrap().into()
///     }
/// }
///
/// impl calimero_sdk::event::AppEventExt for MyEvent {}
///
/// emit(MyEvent { data: "hello".to_string() });
/// ```
#[track_caller]
pub fn emit<'a, E: AppEventExt + 'a>(event: E) {
    let f = EVENT_EMITTER.with_borrow(|emitter| *emitter);
    let f: fn(Box<dyn AppEventExt + 'a>) = unsafe { transmute::<_, _>(f) };
    f(Box::new(event));
}

/// Emits an event with a callback handler that will be automatically executed.
///
/// This function emits the event and arranges for the specified handler method
/// to be called by the runtime after the event is processed. The handler method
/// should be defined in your application and will receive the event data as parameters.
///
/// # ⚠️ Important: Handler Execution Guarantees
///
/// **Handlers may execute in parallel** (not guaranteed sequential). Your handler MUST be:
/// - **Commutative**: Order-independent (use CRDTs, not sequential operations)
/// - **Independent**: No shared mutable state between handlers
/// - **Idempotent**: Safe to retry (no external API calls)
/// - **Side-effect free**: Only modify CRDT state
///
/// See module-level documentation for detailed requirements and examples.
///
/// # Parameters
///
/// * `event` - The event to emit
/// * `handler` - The name of the handler method to call (e.g., "my_handler")
///
/// # Example
///
/// ```rust,no_run
/// use calimero_sdk::event::emit_with_handler;
///
/// #[derive(serde::Serialize)]
/// struct MyEvent {
///     data: String,
/// }
///
/// impl calimero_sdk::event::AppEvent for MyEvent {
///     fn kind(&self) -> std::borrow::Cow<'_, str> {
///         "MyEvent".into()
///     }
///     fn data(&self) -> std::borrow::Cow<'_, [u8]> {
///         serde_json::to_vec(self).unwrap().into()
///     }
/// }
///
/// impl calimero_sdk::event::AppEventExt for MyEvent {}
///
/// emit_with_handler(MyEvent { data: "hello".to_string() }, "my_handler");
/// ```
///
/// The handler method should be defined in your app:
/// ```rust,no_run
/// struct MyApp;
///
/// impl MyApp {
///     fn my_handler(&mut self, data: &str) {
///         // Handle the event
///     }
/// }
/// ```
#[track_caller]
pub fn emit_with_handler<'a, E: AppEventExt + 'a>(event: E, handler: &str) {
    env::log(&format!("Emitting event with handler: {handler}"));

    // Call env::emit_with_handler directly
    env::emit_with_handler(&event, handler);
}

mod reflect {
    use core::any::{type_name, TypeId};

    pub trait Reflect {
        fn id(&self) -> TypeId
        where
            Self: 'static,
        {
            TypeId::of::<Self>()
        }

        fn name(&self) -> &'static str {
            type_name::<Self>()
        }
    }

    impl<T> Reflect for T {}
}

use reflect::Reflect;

pub trait AppEventExt: AppEvent + Reflect {
    // todo! experiment with &dyn AppEventExt downcast_ref to &Self
    // yes, this will mean delegated downcasting would have to be referential
    // but that's not bad, not one bit
    fn downcast(event: Box<dyn AppEventExt>) -> Result<Self, Box<dyn AppEventExt>>
    where
        Self: Sized + 'static,
    {
        downcast(event)
    }
}

impl dyn AppEventExt {
    pub fn is<T: AppEventExt + 'static>(&self) -> bool {
        self.id() == TypeId::of::<T>()
    }
}

pub fn downcast<T: AppEventExt + 'static>(
    event: Box<dyn AppEventExt>,
) -> Result<T, Box<dyn AppEventExt>> {
    if event.is::<T>() {
        Ok(*unsafe { Box::from_raw(Box::into_raw(event).cast::<T>()) })
    } else {
        Err(event)
    }
}

#[derive(Clone, Copy, Debug)]
#[expect(clippy::exhaustive_enums, reason = "This will never have variants")]
pub enum NoEvent {}
impl AppEvent for NoEvent {
    fn kind(&self) -> Cow<'_, str> {
        unreachable!()
    }

    fn data(&self) -> Cow<'_, [u8]> {
        unreachable!()
    }
}
impl AppEventExt for NoEvent {}
