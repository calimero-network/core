//! A type may declare at most one `#[app::init]`.
//!
//! The second initializer is deliberately named differently (`init_with_value`)
//! rather than `init`: naming both `init` would add rustc's own
//! duplicate-definition error (E0592) to the golden, coupling it to rustc
//! wording. As written, both diagnostics are SDK-controlled `(calimero)>`
//! messages — `DuplicateInit` (a second `#[app::init]`) and
//! `AppInitMethodNotNamedInit` (the initializer must be named `init`) — so the
//! golden is stable across toolchain bumps.

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
