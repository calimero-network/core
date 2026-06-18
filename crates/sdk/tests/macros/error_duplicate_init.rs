//! A type may declare at most one `#[app::init]`.

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
    pub fn init(value: u64) -> S {
        let _ = value;
        S
    }
}

fn main() {}
