//! Test error message for trait implementations

use calimero_sdk::app;

#[app::state]
struct MyState;

trait MyTrait {
    fn do_something(&self);
}

#[app::logic]
impl MyTrait for MyState {
    fn do_something(&self) {}
}

fn main() {}
