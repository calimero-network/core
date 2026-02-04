//! Tests for valid complex generic patterns in SDK macros
//!
//! This file tests that the SDK macros correctly handle various valid
//! lifetime patterns and generic configurations.

use calimero_sdk::app;

// Test multiple lifetime parameters in methods
#[app::state]
struct MultiLifetimeState;

#[app::logic]
impl MultiLifetimeState {
    pub fn dual_lifetime_refs<'a, 'b>(&self, first: &'a str, second: &'b str) -> &'a str {
        first
    }

    pub fn triple_lifetime_refs<'a, 'b, 'c>(
        &self,
        a: &'a str,
        b: &'b str,
        c: &'c str,
    ) -> &'a str {
        a
    }

    // Test lifetime elision still works
    pub fn elided_lifetime(&self, data: &str) -> &str {
        data
    }
}

// Test static lifetime references
#[app::state]
struct StaticLifetimeState;

#[app::logic]
impl StaticLifetimeState {
    pub fn static_ref(&self, data: &'static str) -> &'static str {
        data
    }

    pub fn mixed_static_and_regular<'a>(&self, regular: &'a str, static_: &'static str) -> &'a str {
        regular
    }
}

// Test lifetimes in return types
#[app::state]
struct LifetimeReturnState {
    data: String,
}

#[app::logic]
impl LifetimeReturnState {
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
    pub fn process_slice<'a>(&self, data: &'a [u8]) -> &'a [u8] {
        data
    }

    pub fn process_str_slice<'a>(&self, items: &'a [&'a str]) -> usize {
        items.len()
    }
}

// Test where clause patterns (implicit through type bounds)
#[app::state]
struct BoundedTypesState;

#[app::logic]
impl BoundedTypesState {
    pub fn process_string(&self, s: String) -> String {
        s
    }

    pub fn process_vec(&self, v: Vec<u8>) -> Vec<u8> {
        v
    }
}

// Test Option and Result with lifetimes
#[app::state]
struct OptionResultState;

#[app::logic]
impl OptionResultState {
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
