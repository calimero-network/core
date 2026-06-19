//! Valid receiver shapes accepted by `#[app::logic]`.
//!
//! `#[app::state]` no longer permits a lifetime parameter, so the receiver
//! forms that required a lifetime-parameterised `Self` (`self: &'a Self`, ...)
//! are gone. What remains is the full set of reference receivers — by `Self`
//! and by the concrete type name, with and without `mut` bindings and parens —
//! which is what an app actually writes.

use calimero_sdk::app;

#[app::state]
struct MyType;

#[app::logic]
impl MyType {
    #[app::init]
    pub fn init() -> MyType {
        MyType
    }

    pub fn method_01(&self) {}
    pub fn method_02(&mut self) {}
    pub fn method_09(self: &Self) {}
    pub fn method_10(self: &(Self)) {}
    pub fn method_11(self: &mut Self) {}
    pub fn method_12(self: &mut (Self)) {}
    pub fn method_13(mut self: &Self) {}
    pub fn method_14(mut self: &mut Self) {}
    pub fn method_19(self: &MyType) {}
    pub fn method_20(self: &(MyType)) {}
    pub fn method_23(self: &mut MyType) {}
    pub fn method_24(self: &mut (MyType)) {}
    pub fn method_25(mut self: &MyType) {}
    pub fn method_26(mut self: &mut MyType) {}
}

fn main() {}
