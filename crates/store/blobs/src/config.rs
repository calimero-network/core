use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BlobStoreConfig {
    pub path: Utf8PathBuf,
}

impl BlobStoreConfig {
    #[must_use]
    pub const fn new(path: Utf8PathBuf) -> Self {
        Self { path }
    }
}
