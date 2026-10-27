use calimero_app_sdk::prelude::*;

#[app::state(version = 1)]
pub struct S {
    n: Counter,
}

#[app::logic]
impl S {
    #[app::view]
    #[app::public(rate = "lots")]
    pub fn ping(&self) -> app::Result<()> {
        Ok(())
    }
}

fn main() {}
