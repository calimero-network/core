use calimero_app_sdk::prelude::*;

#[app::state(version = 1)]
pub struct S {
    n: Counter,
}

#[app::logic]
impl S {
    pub fn sum(&mut self, xs: Vec<u64>) -> app::Result<()> {
        let _ = xs;
        Ok(())
    }
}

fn main() {}
