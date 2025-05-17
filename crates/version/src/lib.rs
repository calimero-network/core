pub fn version_info() -> String {
    format!(
        "calimero {} (build {}, commit {}, rustc {})",
        env!("CARGO_PKG_VERSION"),
        env!("GIT_DESCRIBE"),
        env!("GIT_COMMIT_HASH"),
        env!("RUSTC_VERSION"),
    )
}
