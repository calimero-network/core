//! Test guest: `handle` returns its arguments, after touching the clock and the RNG.

wit_bindgen::generate!({
    path: "../../../../app-wit/wit",
    world: "app",
    generate_all,
});

#[link_section = "calimero:abi"]
#[used]
static ABI: [u8; 65] = *b"{\"world\":\"calimero:app@0.5.1\",\"methods\":{\"echo\":{\"kind\":\"call\"}}}";

struct App;

impl Guest for App {
    fn handle(_kind: InvocationKind, _method: String, args: Vec<u8>) -> Result<Vec<u8>, AppError> {
        let _now = std::time::SystemTime::now();
        let mut buf = [0u8; 8];
        getrandom::fill(&mut buf).map_err(|e| AppError {
            code: "Rand".into(),
            message: e.to_string(),
        })?;
        Ok(args)
    }
}

export!(App);
