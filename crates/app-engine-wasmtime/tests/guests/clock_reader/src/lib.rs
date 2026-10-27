//! Test guest: reads clocks, randomness and context; prints; tries outgoing HTTP.

use std::time::Duration;

use calimero::app::context;
use wasi::clocks::{monotonic_clock, wall_clock};
use wasi::http::{outgoing_handler, types};
use wasi::random::random;

wit_bindgen::generate!({
    path: "../../../../app-wit/wit",
    world: "app",
    generate_all,
});

const ABI_JSON: &[u8] = br#"{"abiVersion":1,"world":"calimero:app@0.5.0","methods":{"wall":{"kind":"view"},"monotonic":{"kind":"view"},"random":{"kind":"call"},"org":{"kind":"view"},"print":{"kind":"call"},"fetch":{"kind":"call"}}}"#;

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
    fn handle(_kind: InvocationKind, method: String, _args: Vec<u8>) -> Result<Vec<u8>, AppError> {
        match method.as_str() {
            "wall" => {
                let now = wall_clock::now();
                let mut out = now.seconds.to_le_bytes().to_vec();
                out.extend_from_slice(&now.nanoseconds.to_le_bytes());
                Ok(out)
            }
            "monotonic" => {
                let before = monotonic_clock::now();
                std::thread::sleep(Duration::from_secs(3_600));
                let after = monotonic_clock::now();
                let mut out = before.to_le_bytes().to_vec();
                out.extend_from_slice(&after.to_le_bytes());
                Ok(out)
            }
            "random" => Ok(random::get_random_bytes(32)),
            "org" => Ok(context::org().unwrap_or_default().into_bytes()),
            "print" => {
                println!("first line");
                eprintln!("second line");
                print!("partial");
                let _ = std::io::Write::flush(&mut std::io::stdout());
                Ok(Vec::new())
            }
            "fetch" => {
                let request = types::OutgoingRequest::new(types::Fields::new());
                let _ = request.set_authority(Some("example.com"));
                match outgoing_handler::handle(request, None) {
                    Ok(_) => Ok(b"sent".to_vec()),
                    Err(_) => Err(AppError { code: "HttpError".into(), message: String::new() }),
                }
            }
            _ => Err(AppError { code: "NoMethod".into(), message: String::new() }),
        }
    }
}

export!(App);
