// `calimero_compute` is set by `calimero-app-build` for the compute component, and with
// `calimero_sandbox` for the sandbox component.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(calimero_compute)");
    println!("cargo::rustc-check-cfg=cfg(calimero_sandbox)");
}
