use std::fmt;

use once_cell::sync::Lazy;

pub struct CalimeroVersion {
    release_version: &'static str,
    build_version: &'static str,
    commit: &'static str,
    rustc_version: &'static str,
}

impl CalimeroVersion {
    fn new() -> Self {
        let pkg_version = env!("CARGO_PKG_VERSION");
        let git_describe = env!("GIT_DESCRIBE");

        let build_version = if git_describe == "unknown" {
            pkg_version.to_string()
        } else {
            git_describe.to_string()
        };

        Self {
            release_version: pkg_version,
            build_version: Box::leak(build_version.into_boxed_str()),
            commit: env!("GIT_COMMIT"),
            rustc_version: env!("RUSTC_VERSION"),
        }
    }
}

impl fmt::Display for CalimeroVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "(release {}) (build {}) (commit {}) (rustc {})",
            self.release_version, self.build_version, self.commit, self.rustc_version
        )
    }
}

static VERSION: Lazy<CalimeroVersion> = Lazy::new(CalimeroVersion::new);

pub fn get_version() -> &'static str {
    // Convert the Display output to a static str
    Box::leak(VERSION.to_string().into_boxed_str())
}
