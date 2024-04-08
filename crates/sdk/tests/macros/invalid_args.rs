use calimero_sdk::app;

#[app::state]
struct MyType;

trait MyTrait {}

#[app::logic]
impl MyType {
    pub fn method(&self, value: impl MyTrait) {}
}

fn main() {}
