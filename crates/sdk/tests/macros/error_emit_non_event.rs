//! Emitting a type that isn't `#[app::event]` must give a clear SDK message.

use calimero_sdk::app;

struct NotAnEvent;

#[app::state]
struct S;

#[app::logic]
impl S {
    #[app::init]
    pub fn init() -> S {
        S
    }

    pub fn go(&self) {
        app::emit!(NotAnEvent);
    }
}

fn main() {}
