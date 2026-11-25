//! Bench guest: compute that is mostly linear-memory loads and stores, so the cost of
//! a bounds check on each one shows (`benches/guest_memory.rs`). Each method returns a
//! checksum, so the work cannot be optimized away and a config change that altered a
//! result would show.

wit_bindgen::generate!({
    path: "../../../../app-wit/wit",
    world: "app",
    generate_all,
});

const ABI_JSON: &[u8] = br#"{"abiVersion":1,"world":"calimero:app@0.5.6","methods":{"noop":{"kind":"call"},"sort":{"kind":"call"},"sieve":{"kind":"call"},"matmul":{"kind":"call"}}}"#;

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

/// 1 Mi u32 (4 MiB) sorted.
const SORT_LEN: usize = 1 << 20;
/// Primes below 8 Mi, one byte per number (8 MiB).
const SIEVE_LEN: usize = 8 << 20;
/// Two 192 x 192 u32 matrices multiplied.
const MAT_N: usize = 192;

fn xorshift(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

fn sort() -> u64 {
    let mut state = 0x9e37_79b9;
    let mut v: Vec<u32> = (0..SORT_LEN).map(|_| xorshift(&mut state)).collect();
    v.sort_unstable();
    v.iter()
        .step_by(4096)
        .fold(0u64, |acc, &x| acc.wrapping_mul(31).wrapping_add(u64::from(x)))
}

fn sieve() -> u64 {
    let mut composite = vec![0u8; SIEVE_LEN];
    let mut count = 0u64;
    for i in 2..SIEVE_LEN {
        if composite[i] == 0 {
            count += 1;
            let mut j = i.saturating_mul(i);
            while j < SIEVE_LEN {
                composite[j] = 1;
                j += i;
            }
        }
    }
    count
}

fn matmul() -> u64 {
    let mut state = 0x2545_f491;
    let a: Vec<u32> = (0..MAT_N * MAT_N).map(|_| xorshift(&mut state) & 0xffff).collect();
    let b: Vec<u32> = (0..MAT_N * MAT_N).map(|_| xorshift(&mut state) & 0xffff).collect();
    let mut c = vec![0u32; MAT_N * MAT_N];
    for i in 0..MAT_N {
        for k in 0..MAT_N {
            let aik = a[i * MAT_N + k];
            for j in 0..MAT_N {
                c[i * MAT_N + j] = c[i * MAT_N + j].wrapping_add(aik.wrapping_mul(b[k * MAT_N + j]));
            }
        }
    }
    c.iter().fold(0u64, |acc, &x| acc.wrapping_add(u64::from(x)))
}

struct App;

impl Guest for App {
    fn handle(_kind: InvocationKind, method: String, _args: Vec<u8>) -> Result<Vec<u8>, AppError> {
        let sum = match method.as_str() {
            "noop" => 0,
            "sort" => sort(),
            "sieve" => sieve(),
            "matmul" => matmul(),
            other => {
                return Err(AppError {
                    code: "Unknown".into(),
                    message: other.into(),
                })
            }
        };
        Ok(sum.to_le_bytes().to_vec())
    }
}

export!(App);
