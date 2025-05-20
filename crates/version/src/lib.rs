use std::{borrow::Cow, fmt, sync::LazyLock};

#[cfg(test)]
mod tests;


static CURRENT: LazyLock<CalimeroVersion<'static>> = LazyLock::new(|| CalimeroVersion {
    release: Cow::Borrowed(env!("CARGO_PKG_VERSION")),
    build: Cow::Borrowed(env!("CALIMERO_BUILD")),
    commit: Cow::Borrowed(env!("CALIMERO_COMMIT")),
    rustc: Cow::Borrowed(env!("CALIMERO_RUSTC_VERSION")),
});

static CURRENT_STRING: LazyLock<String> = LazyLock::new(|| VERSION.to_string());

#[derive(Clone)]
pub struct CalimeroVersion<'a> {
    pub release: Cow<'a, str>,
    pub build: Cow<'a, str>,
    pub commit: Cow<'a, str>,
    pub rustc: Cow<'a, str>,
}

impl CalimeroVersion<'_> {
    pub fn current() -> CalimeroVersion<'static> {
        CURRENT.clone()
    }

    pub fn current_str() -> &'static str {
        &*CURRENT_STRING
    }
}

impl fmt::Display for CalimeroVersion<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "(release {}) (build {}) (commit {}) (rustc {})",
            self.release, self.build, self.commit, self.rustc,
        )
    }
}

