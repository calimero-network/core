use calimero_sdk::abi::AbiType;
use calimero_sdk::serde::Serialize;

// serde writes a tagged struct's name into its JSON; the ABI has no place for it.
#[derive(AbiType, Serialize)]
#[serde(crate = "calimero_sdk::serde", tag = "kind")]
pub struct Tagged {
    id: u32,
}

fn main() {}
