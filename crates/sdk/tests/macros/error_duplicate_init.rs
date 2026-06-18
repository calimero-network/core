//! A type may declare at most one `#[app::init]`.
//!
//! The second initializer is deliberately named differently (`init_with_value`)
//! rather than `init`: naming both `init` would add rustc's own
//! duplicate-definition error (E0592) to the golden, coupling it to rustc
//! wording. As written, every diagnostic is an SDK-controlled `(calimero)>`
//! message — `DuplicateInit` (a second `#[app::init]`),
//! `AppInitMethodNotNamedInit` (the initializer must be named `init`), and the
//! `AppStateInit` `on_unimplemented` note (the macro bails on error, so no
//! initializer is registered) — so the golden is stable across toolchain bumps.
//!
//! `DuplicateInit` is detected at the impl level (counting `#[app::init]`
//! attributes), not from the parsed-method list, so it fires here even though
//! the misnamed second initializer fails its own per-method validation.

use calimero_sdk::app;

#[app::state]
struct S;

#[app::logic]
impl S {
    #[app::init]
    pub fn init() -> S {
        S
    }

    #[app::init]
    pub fn init_with_value(value: u64) -> S {
        let _ = value;
        S
    }
}

fn main() {}
