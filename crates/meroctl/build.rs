// Set version and build metadata env vars for the binary.

fn main() {
    calimero_build_utils::set_version_env_vars("MEROCTL")
        .expect("failed to set MEROCTL_* build metadata env vars");
}
