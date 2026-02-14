// Set version and build metadata env vars for the binary (NEAR-style).
// See https://github.com/near/nearcore/blob/master/neard/src/main.rs

fn main() {
    calimero_build_utils::set_version_env_vars("MEROD")
        .expect("failed to set MEROD_* build metadata env vars");
}
