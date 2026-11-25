use calimero_sdk::prelude::*;

/// Plain serde, no shape, answered by a method: its `u64` would reach JSON as a number.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Totals {
    pub views: u64,
}

#[app::state(version = 1)]
pub struct S {
    n: Counter,
}

#[app::logic]
impl S {
    #[app::view]
    pub fn totals(&self) -> app::Result<Totals> {
        Ok(Totals { views: 0 })
    }
}

fn main() {}
