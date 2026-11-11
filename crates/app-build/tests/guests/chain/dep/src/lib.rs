//! Test guest: exports `dep_only`.

wit_bindgen::generate!({
    path: "../../../../../app-wit/wit",
    world: "app",
    generate_all,
});

#[link_section = "calimero:abi"]
#[used]
static ABI: [u8; 69] = *b"{\"world\":\"calimero:app@0.5.1\",\"methods\":{\"dep_only\":{\"kind\":\"call\"}}}";

struct App;

impl Guest for App {
    fn handle(_kind: InvocationKind, _method: String, args: Vec<u8>) -> Result<Vec<u8>, AppError> {
        Ok(args)
    }
}

export!(App);
