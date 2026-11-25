//! Test guest: host work for little guest fuel. `reads` (a view) reads the key `big` as many
//! times as `args` (u32 LE) says and returns the bytes it was handed (u64 LE); `searches` (a
//! view) runs that many searches of the table `notes` and returns how many answered and the
//! hits they carried (two u64 LE).

use calimero::app::{search, storage};

wit_bindgen::generate!({
    path: "../../../../app-wit/wit",
    world: "app",
    generate_all,
});

const ABI_JSON: &[u8] = br#"{"abiVersion":1,"world":"calimero:app@0.5.7","methods":{"reads":{"kind":"view"},"searches":{"kind":"view"}}}"#;

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

fn count(args: Vec<u8>) -> Result<u32, AppError> {
    let bytes: [u8; 4] = args
        .try_into()
        .map_err(|_| AppError { code: "BadArgs".into(), message: String::new() })?;
    Ok(u32::from_le_bytes(bytes))
}

struct App;

impl Guest for App {
    fn handle(_kind: InvocationKind, method: String, args: Vec<u8>) -> Result<Vec<u8>, AppError> {
        let n = count(args)?;
        match method.as_str() {
            "reads" => {
                let mut bytes = 0u64;
                for _ in 0..n {
                    bytes += storage::get(b"big").map_or(0, |v| v.len() as u64);
                }
                Ok(bytes.to_le_bytes().to_vec())
            }
            "searches" => {
                let (mut answered, mut hits) = (0u64, 0u64);
                for _ in 0..n {
                    if let Ok(page) = search::query("notes", "eggs", search::SearchMode::Words, 0, 100) {
                        answered += 1;
                        hits += page.hits.len() as u64;
                    }
                }
                let mut out = answered.to_le_bytes().to_vec();
                out.extend(hits.to_le_bytes());
                Ok(out)
            }
            _ => Err(AppError { code: "NoMethod".into(), message: String::new() }),
        }
    }
}

export!(App);
