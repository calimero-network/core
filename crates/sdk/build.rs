fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").expect("CARGO_CFG_TARGET_ARCH is not set") != "wasm32"
    {
        panic!(
            "\x1b[1;31merror\x1b[0m: Incompatible target architecture, no polyfill available, only \x1b[1mwasm32\x1b[0m is supported."
        );
    }
}
