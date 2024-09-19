use camino::Utf8PathBuf;

#[derive(Debug)]
#[non_exhaustive]
pub struct StoreConfig {
    pub path: Utf8PathBuf,
}

impl StoreConfig {
    #[must_use]
    pub const fn new(path: Utf8PathBuf) -> Self {
        Self { path }
    }
}
