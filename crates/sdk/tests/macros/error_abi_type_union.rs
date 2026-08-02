use calimero_sdk::abi::AbiType;

#[derive(AbiType)]
union Value {
    int: u32,
    float: f32,
}

fn main() {}
