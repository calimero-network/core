use calimero_sdk::abi::AbiType;

// `pattern` describes a newtype's values; a record has none to describe.
#[derive(AbiType)]
#[abi(pattern = "^x")]
struct NotANewtype {
    field: String,
}

fn main() {}
