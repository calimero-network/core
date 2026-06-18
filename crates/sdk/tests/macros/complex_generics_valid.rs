//! Valid complex generic / lifetime patterns on `#[app::logic]` methods.
//!
//! State takes no generics or lifetimes; every interesting parameter lives on
//! the methods. Each state is a unit struct with an `#[app::init]` (both now
//! mandatory). Multiple `#[app::state]` types share this file — fine on the
//! host target, where the WASM entrypoint exports aren't generated.

use calimero_sdk::app;
use calimero_storage::collections::LwwRegister;

// Test multiple lifetime parameters in methods
#[app::state]
struct MultiLifetimeState;

#[app::logic]
impl MultiLifetimeState {
    #[app::init]
    pub fn init() -> MultiLifetimeState {
        MultiLifetimeState
    }

    pub fn dual_lifetime_refs<'a, 'b>(&self, first: &'a str, second: &'b str) -> &'a str {
        first
    }

    pub fn triple_lifetime_refs<'a, 'b, 'c>(&self, a: &'a str, b: &'b str, c: &'c str) -> &'a str {
        a
    }

    // Lifetime elision through the macro: with `&self` plus another reference
    // parameter, the elided output lifetime binds to `&self` (the receiver wins),
    // *not* to `_data` — so the body can't return `_data` and instead returns
    // `""`, which is `'static` and satisfies any lifetime. This checks the macro
    // accepts an elided-lifetime signature; borrowing a real field under the same
    // `&self`-tied elision is covered by `LifetimeReturnState::borrow_data` below.
    pub fn elided_lifetime(&self, _data: &str) -> &str {
        ""
    }
}

// Test static lifetime references
#[app::state]
struct StaticLifetimeState;

#[app::logic]
impl StaticLifetimeState {
    #[app::init]
    pub fn init() -> StaticLifetimeState {
        StaticLifetimeState
    }

    pub fn static_ref(&self, data: &'static str) -> &'static str {
        data
    }

    pub fn mixed_static_and_regular<'a>(&self, regular: &'a str, static_: &'static str) -> &'a str {
        regular
    }
}

// Test lifetimes in return types tied to the `&self` borrow. The `String` field
// is wrapped in `LwwRegister` (the CRDT form); `LwwRegister<String>` derefs to
// `String` to `str`, so the borrow accessors compile unchanged.
#[app::state]
struct LifetimeReturnState {
    data: LwwRegister<String>,
}

#[app::logic]
impl LifetimeReturnState {
    #[app::init]
    pub fn init() -> LifetimeReturnState {
        LifetimeReturnState {
            data: LwwRegister::new(String::new()),
        }
    }

    pub fn borrow_data(&self) -> &str {
        &self.data
    }

    pub fn borrow_with_lifetime<'a>(&'a self) -> &'a str {
        &self.data
    }
}

// Test slice references with lifetimes
#[app::state]
struct SliceState;

#[app::logic]
impl SliceState {
    #[app::init]
    pub fn init() -> SliceState {
        SliceState
    }

    pub fn process_slice<'a>(&self, data: &'a [u8]) -> &'a [u8] {
        data
    }

    pub fn process_str_slice<'a>(&self, items: &'a [&'a str]) -> usize {
        items.len()
    }
}

// Test owned-value arguments and returns
#[app::state]
struct BoundedTypesState;

#[app::logic]
impl BoundedTypesState {
    #[app::init]
    pub fn init() -> BoundedTypesState {
        BoundedTypesState
    }

    pub fn process_string(&self, s: String) -> String {
        s
    }

    pub fn process_vec(&self, v: Vec<u8>) -> Vec<u8> {
        v
    }
}

// Test Option with lifetimes
#[app::state]
struct OptionResultState;

#[app::logic]
impl OptionResultState {
    #[app::init]
    pub fn init() -> OptionResultState {
        OptionResultState
    }

    pub fn option_ref<'a>(&self, opt: Option<&'a str>) -> Option<&'a str> {
        opt
    }

    pub fn option_ref_inner<'a>(&self, opt: &'a Option<String>) -> Option<&'a str> {
        opt.as_deref()
    }
}

// Test complex nested lifetime patterns
#[app::state]
struct NestedLifetimeState;

#[app::logic]
impl NestedLifetimeState {
    #[app::init]
    pub fn init() -> NestedLifetimeState {
        NestedLifetimeState
    }

    pub fn nested_ref<'a, 'b>(&self, outer: &'a Vec<&'b str>) -> usize
    where
        'b: 'a,
    {
        outer.len()
    }

    pub fn slice_of_refs<'a>(&self, refs: &'a [&'a str]) -> &'a str {
        refs.first().copied().unwrap_or("")
    }
}

fn main() {}
