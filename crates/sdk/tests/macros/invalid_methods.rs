use calimero_sdk::app;

#[app::state]
struct MyType;

#[app::logic]
impl MyType {
    pub async fn method_00(&self) {}
    pub unsafe fn method_01(&self) {}
}

fn main() {}
