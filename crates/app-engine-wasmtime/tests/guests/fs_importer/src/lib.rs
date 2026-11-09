//! Test guest: binds a TCP socket through std, so it imports wasi:sockets.

wit_bindgen::generate!({
    path: "../../../../app-wit/wit",
    world: "app",
    generate_all,
});

const ABI_JSON: &[u8] = br#"{"abiVersion":1,"world":"calimero:app@0.5.1","methods":{"bind":{"kind":"call"}}}"#;

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
        match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(_) => Ok(b"bound".to_vec()),
            Err(e) => Err(AppError { code: "Bind".into(), message: e.to_string() }),
        }
    }
}

export!(App);
