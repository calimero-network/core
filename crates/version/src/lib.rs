use std::fmt;
use std::sync::LazyLock;

#[cfg(test)]
mod tests;

pub struct CalimeroVersion {
    release: &'static str,
    build: &'static str,
    commit: &'static str,
    rustc: &'static str,
    protocol: &'static str,
}

impl CalimeroVersion {
    fn new() -> Self {
        let release = env!("CARGO_PKG_VERSION");
        let describe = env!("GIT_DESCRIBE");
        let commit = env!("GIT_COMMIT");
        let rustc = env!("RUSTC_VERSION");
        let protocol = env!("CARGO_PKG_VERSION_MAJOR");

        let build = if describe == "unknown" {
            release
        } else {
            Box::leak(describe.to_string().into_boxed_str())
        };

        Self {
            release,
            build,
            commit,
            rustc,
            protocol,
        }
    }
}

impl fmt::Display for CalimeroVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "(release {}) (build {}) (commit {}) (rustc {}) (protocol {})",
            self.release, self.build, self.commit, self.rustc, self.protocol
        )
    }
}

static VERSION: LazyLock<CalimeroVersion> = LazyLock::new(CalimeroVersion::new);
static VERSION_STRING: LazyLock<String> = LazyLock::new(|| VERSION.to_string());

pub fn version() -> &'static CalimeroVersion {
    &*VERSION
}

pub fn version_str() -> &'static str {
    &*VERSION_STRING
}
