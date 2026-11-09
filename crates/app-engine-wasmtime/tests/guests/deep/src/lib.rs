//! Test guest: `handle` recurses without bound on the wasm stack.

wit_bindgen::generate!({
    path: "../../../../app-wit/wit",
    world: "app",
    generate_all,
});

const ABI_JSON: &[u8] = br#"{"abiVersion":1,"world":"calimero:app@0.5.1","methods":{"deep":{"kind":"call"}}}"#;

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

#[inline(never)]
fn depth(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let below = depth(core::hint::black_box(n - 1));
    core::hint::black_box(below) ^ n
}

impl Guest for App {
    fn handle(_kind: InvocationKind, _method: String, _args: Vec<u8>) -> Result<Vec<u8>, AppError> {
        Ok(depth(u64::MAX).to_le_bytes().to_vec())
    }
}

export!(App);
