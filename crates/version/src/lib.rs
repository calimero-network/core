pub fn version_info() -> String {
    format!(
        "calimero {:?} (build {:?}, commit {:?}, rustc {:?})",
        std::env::var("CARGO_PKG_VERSION"),
        std::env::var("GIT_DESCRIBE"),
        std::env::var("GIT_COMMIT_HASH"),
        std::env::var("RUSTC_VERSION"),
    )
}
