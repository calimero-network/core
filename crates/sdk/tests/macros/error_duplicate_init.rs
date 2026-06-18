//! A type may declare at most one `#[app::init]`.
//!
//! Both initializers must be named `init` (the SDK requires it), so rustc also
//! reports a duplicate-definition error. That cascade is expected and
//! complementary: the `(calimero)>` message explains the *initializer* rule,
//! rustc explains the symbol collision. Two `#[app::init]` methods cannot be
//! isolated from one rustc/SDK companion error — a differently-named second
//! initializer would instead trip "must be named `init`".

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
