//! Multi-writer / sync-semantic Calimero collections have no coherent
//! meaning in a single-writer private namespace, so `#[app::private]`
//! must reject them with a targeted diagnostic rather than silently
//! leaving them pinned to `MainStorage` (which re-introduces the leak).
//!
//! The collection names below are stand-ins defined locally — the macro
//! matches on the *last path segment ident*, so it fires the diagnostic
//! without the real `calimero_storage` types in scope. This keeps the
//! captured stderr to just the macro errors we're asserting on.

use calimero_sdk::app;

#[allow(dead_code)]
struct AuthoredVector<T>(core::marker::PhantomData<T>);
#[allow(dead_code)]
struct Counter;
#[allow(dead_code)]
struct SharedStorage<T>(core::marker::PhantomData<T>);

#[app::private]
struct Secrets {
    history: AuthoredVector<String>,
    hits: Counter,
    // Nested inside a wrapper — must still be caught.
    shared: Option<SharedStorage<String>>,
}

fn main() {}
