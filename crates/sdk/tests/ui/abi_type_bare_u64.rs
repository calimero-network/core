use calimero_sdk::prelude::*;

#[derive(serde::Serialize, serde::Deserialize, AbiType)]
pub struct Totals {
    pub name: String,
    pub views: u64,
}

fn main() {}
