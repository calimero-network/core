#[derive(Debug)]
#[non_exhaustive]
pub struct StoreConfig {
    pub path: camino::Utf8PathBuf,
}

impl StoreConfig {
    #[must_use]
    pub const fn new(path: camino::Utf8PathBuf) -> Self {
        Self { path }
    }
}
