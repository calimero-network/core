//! Test guest for the compute world: `spin` never returns, `bomb` allocates 1 MiB chunks
//! until memory runs out, `id` returns the run id through the job interface, `wall` returns the wall clock, `draw` 32 random bytes, `random` as
//! many random bytes as `args` (decimal) says, `hashmap` the keys of a std `HashMap` in iteration order, `nan` the bits of NaNs made by arithmetic on NaN
//! inputs with payloads and sign bits set, `grow` grows linear memory to 65,536 pages.

use core::arch::wasm32::{f32x4, f32x4_add, f32x4_extract_lane};
use core::hint::black_box;
use std::collections::HashMap;

use calimero::app::job;
use wasi::clocks::wall_clock;
use wasi::random::random;

wit_bindgen::generate!({
    path: "../../../../app-wit/wit",
    world: "compute",
    generate_all,
});

const ABI_JSON: &[u8] = br#"{"abiVersion":1,"world":"calimero:app/compute@0.5.1","methods":{"spin":{"kind":"compute","compute":{"fuel":1,"result":"r"}},"bomb":{"kind":"compute","compute":{"fuel":1,"result":"r"}},"id":{"kind":"compute","compute":{"fuel":1,"result":"r"}},"wall":{"kind":"compute","compute":{"fuel":1,"result":"r"}},"draw":{"kind":"compute","compute":{"fuel":1,"result":"r"}},"random":{"kind":"compute","compute":{"fuel":1,"result":"r"}},"hashmap":{"kind":"compute","compute":{"fuel":1,"result":"r"}},"nan":{"kind":"compute","compute":{"fuel":1,"result":"r"}},"grow":{"kind":"compute","compute":{"fuel":1,"result":"r"}}}}"#;

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

const WASM_PAGES: usize = 65_536;

#[target_feature(enable = "simd128")]
fn lane(nan: f32) -> f32 {
    let one = f32x4(1.0, 1.0, 1.0, 1.0);
    f32x4_extract_lane::<2>(f32x4_add(f32x4(0.0, 0.0, nan, 0.0), one))
}

fn fail(code: &str) -> AppError {
    AppError { code: code.into(), message: String::new() }
}

struct App;

impl Guest for App {
    fn run(method: String, args: Vec<u8>) -> Result<Vec<u8>, AppError> {
        match method.as_str() {
            "spin" => {
                let mut x: u64 = 0;
                loop {
                    x = black_box(x.wrapping_add(1));
                }
            }
            "bomb" => {
                let mut hoard: Vec<Vec<u8>> = Vec::new();
                loop {
                    let mut chunk = vec![0u8; 1 << 20];
                    chunk[0] = 1;
                    hoard.push(black_box(chunk));
                }
            }
            "id" => Ok(job::run_id().unwrap_or_default().into_bytes()),
            "wall" => {
                let now = wall_clock::now();
                let mut out = now.seconds.to_le_bytes().to_vec();
                out.extend_from_slice(&now.nanoseconds.to_le_bytes());
                Ok(out)
            }
            "draw" => Ok(random::get_random_bytes(32)),
            "random" => {
                let len: u64 = String::from_utf8_lossy(&args)
                    .trim()
                    .parse()
                    .map_err(|_| fail("BadArgs"))?;
                Ok((random::get_random_bytes(len).len() as u64).to_le_bytes().to_vec())
            }
            "hashmap" => {
                let map: HashMap<u32, u32> = (0..64).map(|i| (i, i)).collect();
                Ok(map.keys().flat_map(|k| k.to_le_bytes()).collect())
            }
            "nan" => {
                let n32 = black_box(f32::from_bits(0xffa1_2345));
                let n64 = black_box(f64::from_bits(0xfff4_0000_dead_beef));
                let one32 = black_box(1.0f32);
                let one64 = black_box(1.0f64);
                let mut out = Vec::new();
                out.extend((n32 + one32).to_bits().to_le_bytes());
                out.extend((one32 * n32).to_bits().to_le_bytes());
                out.extend((n64 + one64).to_bits().to_le_bytes());
                out.extend((one64 * n64).to_bits().to_le_bytes());
                // SAFETY: simd128 is enabled for this call; the host allows SIMD.
                out.extend(unsafe { lane(n32) }.to_bits().to_le_bytes());
                Ok(out)
            }
            "grow" => {
                let before = core::arch::wasm32::memory_size(0);
                let grown = core::arch::wasm32::memory_grow(0, WASM_PAGES - before);
                let mut out = (before as u64).to_le_bytes().to_vec();
                out.extend((grown as u64).to_le_bytes());
                out.extend((core::arch::wasm32::memory_size(0) as u64).to_le_bytes());
                Ok(out)
            }
            _ => Err(fail("UnknownMethod")),
        }
    }
}

export!(App);
