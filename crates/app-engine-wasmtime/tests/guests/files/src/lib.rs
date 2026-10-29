//! Test guest: `write` stores a 1.25 MiB file and returns its id; `read` opens the file named
//! in `args`, reads it to the end and returns its SHA-256; `make` tries to create a file.

use calimero::app::files;
use sha2::{Digest, Sha256};

wit_bindgen::generate!({
    path: "../../../../app-wit/wit",
    world: "app",
    generate_all,
});

const ABI_JSON: &[u8] = br#"{"abiVersion":1,"world":"calimero:app@0.5.1","methods":{"write":{"kind":"call"},"read":{"kind":"view"},"make":{"kind":"view"}}}"#;

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

const FILE_BYTES: usize = 1024 * 1024 + 256 * 1024;
const WRITE_PIECE: usize = 500 * 1024;
const READ_CHUNK: u32 = 1024 * 1024;

fn fail(message: String) -> AppError {
    AppError { code: "Files".into(), message }
}

struct App;

impl Guest for App {
    fn handle(_kind: InvocationKind, method: String, args: Vec<u8>) -> Result<Vec<u8>, AppError> {
        match method.as_str() {
            "write" => {
                let writer = files::create().map_err(fail)?;
                let data: Vec<u8> = (0..FILE_BYTES).map(|i| (i % 251) as u8).collect();
                for piece in data.chunks(WRITE_PIECE) {
                    writer.write(piece).map_err(fail)?;
                }
                Ok(writer.finish().map_err(fail)?.into_bytes())
            }
            "read" => {
                let id = String::from_utf8(args).map_err(|e| fail(e.to_string()))?;
                let reader = files::open(&id).map_err(fail)?;
                let mut hash = Sha256::new();
                loop {
                    let chunk = reader.read(READ_CHUNK).map_err(fail)?;
                    if chunk.is_empty() {
                        break;
                    }
                    hash.update(&chunk);
                }
                Ok(hash.finalize().to_vec())
            }
            "make" => {
                let _ = files::create().map_err(fail)?;
                Ok(Vec::new())
            }
            _ => Err(fail("no such method".into())),
        }
    }
}

export!(App);
