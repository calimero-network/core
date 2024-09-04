use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlobStoreConfig {
    pub path: Utf8PathBuf,
}
