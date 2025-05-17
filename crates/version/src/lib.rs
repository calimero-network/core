use std::fmt;
use std::sync::LazyLock;

pub struct CalimeroVersion {
    release: &'static str,
    build: &'static str,
    commit: &'static str,
    rustc: &'static str,
}

impl CalimeroVersion {
    fn new() -> Self {
        let release = env!("CARGO_PKG_VERSION");
        let describe = env!("GIT_DESCRIBE");
        let commit = env!("GIT_COMMIT");
        let rustc = env!("RUSTC_VERSION");

        let build = if describe == "unknown" {
            release
        } else {
            describe
        };

        Self {
            release,
            build: Box::leak(build.to_string().into_boxed_str()),
            commit,
            rustc,
        }
    }
}

impl fmt::Display for CalimeroVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "calimero {} (build {}, commit {}, rustc {})",
            self.release, self.build, self.commit, self.rustc
        )
    }
}

// Static initialization
static VERSION: LazyLock<CalimeroVersion> = LazyLock::new(CalimeroVersion::new);
static VERSION_STRING: LazyLock<String> = LazyLock::new(|| VERSION.to_string());

// Accessors
pub fn version() -> &'static CalimeroVersion {
    &*VERSION
}

pub fn version_str() -> &'static str {
    &*VERSION_STRING
}
