//! Test guest: `handle` increments a static and returns it (u32 LE).

use std::sync::atomic::{AtomicU32, Ordering};

wit_bindgen::generate!({
    path: "../../../../app-wit/wit",
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

static COUNT: AtomicU32 = AtomicU32::new(0);

impl Guest for App {
    fn handle(_kind: InvocationKind, _method: String, _args: Vec<u8>) -> Result<Vec<u8>, AppError> {
        let n = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(n.to_le_bytes().to_vec())
    }
}

export!(App);
