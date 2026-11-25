use calimero_sdk::prelude::*;

/// An alias hides the `u64` from the macros' reading of the source.
pub type Millis = u64;

#[derive(serde::Serialize, serde::Deserialize, AbiType)]
pub struct Window {
    pub until: Millis,
}

#[app::state(version = 1)]
pub struct S {
    n: Counter,
}

#[app::logic]
impl S {
    #[app::view]
    pub fn window(&self) -> app::Result<Window> {
        Ok(Window { until: 0 })
    }
}

fn main() {}
