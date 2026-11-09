//! Test guest: imports `wasi:random/insecure`, which no list allows.

wit_bindgen::generate!({
    inline: r#"
        package test:insecure;

        world insecure {
            include calimero:app/app@0.5.1;
            import wasi:random/insecure@0.2.12;
        }
    "#,
    path: "../../../../app-wit/wit",
    world: "test:insecure/insecure",
    generate_all,
});

const ABI_JSON: &[u8] = br#"{"abiVersion":1,"world":"calimero:app@0.5.1","methods":{"draw":{"kind":"call"}}}"#;

#[link_section = "calimero:abi"]
#[used]
static ABI: [u8; ABI_JSON.len()] = {
    let mut out = [0u8; ABI_JSON.len()];
    let mut i = 0;
    while i < ABI_JSON.len() {
        out[i] = ABI_JSON[i];
        i += 1;
    }
    out
};

struct App;

impl Guest for App {
    fn handle(_kind: InvocationKind, _method: String, _args: Vec<u8>) -> Result<Vec<u8>, AppError> {
        Ok(wasi::random::insecure::get_insecure_random_u64().to_le_bytes().to_vec())
    }
}

export!(App);
