//! Test guest: `put` writes one key; `write_n` writes N keys (N is u32 LE in args).

use calimero::app::storage;

wit_bindgen::generate!({
    path: "../../../../app-wit/wit",
    world: "app",
    generate_all,
});

const ABI_JSON: &[u8] = br#"{"abiVersion":1,"world":"calimero:app@0.5.0","methods":{"put":{"kind":"call"},"write_n":{"kind":"call"},"big":{"kind":"call"},"overwrite":{"kind":"call"},"shout":{"kind":"call"}}}"#;

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
    fn handle(_kind: InvocationKind, method: String, args: Vec<u8>) -> Result<Vec<u8>, AppError> {
        match method.as_str() {
            "put" => storage::put(b"k", b"v"),
            "write_n" => {
                let n = u32::from_le_bytes(args.try_into().map_err(|_| AppError { code: "BadArgs".into(), message: String::new() })?);
                for i in 0..n {
                    storage::put(&i.to_be_bytes(), b"v");
                }
            }
            // One 2 MiB value: over the run's 1 MiB cap at once.
            "big" => storage::put(b"big", &vec![7u8; 2 << 20]),
            // 64 KiB to one key, `n` times: each version counts.
            "overwrite" => {
                let n = u32::from_le_bytes(args.try_into().map_err(|_| AppError { code: "BadArgs".into(), message: String::new() })?);
                let value = vec![9u8; 64 << 10];
                for _ in 0..n {
                    storage::put(b"same", &value);
                }
            }
            // 100 events, each naming one 1 MiB account: the audience is kept too.
            "shout" => {
                let huge = "a".repeat(1 << 20);
                for _ in 0..100 {
                    let _ = calimero::app::events::emit(&calimero::app::types::Event {
                        name: "e".into(),
                        data: Vec::new(),
                        audience: calimero::app::types::Audience::Accounts(vec![huge.clone()]),
                    });
                }
            }
            _ => return Err(AppError { code: "NoMethod".into(), message: String::new() }),
        }
        Ok(Vec::new())
    }
}

export!(App);
