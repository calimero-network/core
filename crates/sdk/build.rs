// `calimero_compute` is set by `calimero-app-build` for the compute component.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(calimero_compute)");
}
