//! `#[app::state(emits = T)]` where `T` isn't an `#[app::event]` type.

use calimero_sdk::app;

struct NotAnEvent;

#[app::state(emits = NotAnEvent)]
struct S;

#[app::logic]
impl S {
    #[app::init]
    pub fn init() -> S {
        S
    }
}

fn main() {}
