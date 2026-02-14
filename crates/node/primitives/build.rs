// Expose package version as env var for runtime version checks (e.g. bundle min_runtime_version).
// NEAR-style: version comes from the crate's CARGO_PKG_VERSION (workspace version).

fn main() {
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION set by Cargo");
    println!("cargo:rustc-env=CALIMERO_RELEASE_VERSION={}", version);
}
