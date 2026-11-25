use calimero_sdk::prelude::*;

/// Plain serde, no shape: its `u64` would reach JSON as a number.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Totals {
    pub views: u64,
}

/// Described, but holding the type above.
#[derive(serde::Serialize, serde::Deserialize, AbiType)]
pub struct Report {
    pub name: String,
    pub totals: Totals,
}

#[app::state(version = 1)]
pub struct S {
    n: Counter,
}

#[app::logic]
impl S {
    #[app::view]
    pub fn report(&self) -> app::Result<Report> {
        Ok(Report {
            name: String::new(),
            totals: Totals { views: 0 },
        })
    }
}

fn main() {}
