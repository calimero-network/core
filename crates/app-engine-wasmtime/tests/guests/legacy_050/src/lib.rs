//! Test guest built against the 0.5.0 world: `bump` increments a counter in storage and
//! returns it (u32 LE).

use calimero::app::storage;

wit_bindgen::generate!({
    path: "wit",
    world: "app",
    generate_all,
});

const ABI_JSON: &[u8] = br#"{"abiVersion":1,"world":"calimero:app@0.5.0","methods":{"bump":{"kind":"call"}}}"#;

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
        let n = storage::get(b"n")
            .and_then(|v| v.try_into().ok())
            .map_or(0, u32::from_le_bytes)
            + 1;
        storage::put(b"n", &n.to_le_bytes());
        Ok(n.to_le_bytes().to_vec())
    }
}

export!(App);
