use calimero_sdk::abi::AbiType;
use calimero_sdk::serde::Serialize;

#[derive(Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct Base {
    id: u32,
}

// A flattened field splices another type's keys in; the ABI cannot say which.
#[derive(AbiType, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct Extended {
    #[serde(flatten)]
    base: Base,
    extra: String,
}

fn main() {}
