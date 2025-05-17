pub const VERSION_INFO: &str = concat!(
    env!("CARGO_PKG_NAME"),
    " (release ",
    env!("CARGO_PKG_VERSION"),
    ") ",
    "(build ",
    env!("GIT_DESCRIBE"),
    ") ",
    "(commit ",
    env!("GIT_COMMIT_HASH"),
    ") ",
    "(rustc ",
    env!("RUSTC_VERSION"),
    ") ",
    "(protocol ",
    env!("CARGO_PKG_VERSION_MAJOR"),
    ")"
);
