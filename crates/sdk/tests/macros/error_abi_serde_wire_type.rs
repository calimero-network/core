use calimero_sdk::abi::AbiType;
use calimero_sdk::serde::{Serialize, Serializer};

fn as_hex<S: Serializer>(bytes: &[u8; 2], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format!("{:02x}{:02x}", bytes[0], bytes[1]))
}

// `serialize_with` hides the wire type; the ABI needs it spelled out.
#[derive(AbiType, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct Blob {
    #[serde(serialize_with = "as_hex")]
    id: [u8; 2],
}

fn main() {}
