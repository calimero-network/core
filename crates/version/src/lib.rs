use std::borrow::Cow;
use std::fmt;
use std::sync::LazyLock;

#[cfg(test)]
mod tests;

static CURRENT: LazyLock<CalimeroVersion<'static>> = LazyLock::new(|| CalimeroVersion {
    release: env!("CARGO_PKG_VERSION").into(),
    build: env!("CALIMERO_BUILD").into(),
    commit: env!("CALIMERO_COMMIT").into(),
    rustc: env!("CALIMERO_RUSTC_VERSION").into(),
});

static CURRENT_STRING: LazyLock<String> = LazyLock::new(|| CURRENT.to_string());

#[derive(Clone)]
pub struct CalimeroVersion<'a> {
    pub release: Cow<'a, str>,
    pub build: Cow<'a, str>,
    pub commit: Cow<'a, str>,
    pub rustc: Cow<'a, str>,
}

impl CalimeroVersion<'static> {
    pub fn current() -> Self {
        CURRENT.clone()
    }

    pub fn current_str() -> &'static str {
        &CURRENT_STRING
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
