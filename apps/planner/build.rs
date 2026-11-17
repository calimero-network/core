// The typed clients for the apps this release `uses` (`uses::calendar()`), generated from
// `calimero.lock` and `calimero/abi/` into `$OUT_DIR/uses.rs`.

fn main() {
    if let Err(e) = calimero_app_build::uses::generate() {
        // A build script reports a failure by panicking; the message names what to fix.
        panic!("{e:#}");
    }
}
