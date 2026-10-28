use calimero_app_sdk::prelude::*;

#[app::state(version = 1)]
pub struct S {
    n: Counter,
}

#[app::logic]
impl S {
    #[app::init]
    #[app::allow(app = "com.x")]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }
}

fn main() {}
