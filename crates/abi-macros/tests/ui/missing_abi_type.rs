use calimero_sdk::app;

struct CustomType {
    value: u64,
}

#[app::state]
struct MyApp {
    custom: CustomType, //~ ERROR: missing `#[derive(AbiType)]` for `CustomType`
}

#[app::logic]
impl MyApp {
    #[app::init]
    pub fn init() -> Self {
        Self { custom: CustomType { value: 0 } }
    }
    
    pub fn process(&self, input: CustomType) -> app::Result<u64> {
        //~ ERROR: missing `#[derive(AbiType)]` for `CustomType`
        Ok(input.value)
    }
} 