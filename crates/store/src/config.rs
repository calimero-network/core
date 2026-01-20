use camino::Utf8PathBuf;

#[derive(Debug)]
#[non_exhaustive]
pub struct StoreConfig {
    pub path: Utf8PathBuf,
    /// Optional encryption key for at-rest encryption.
    /// When set, all values stored in the database will be encrypted.
    pub encryption_key: Option<Vec<u8>>,
}

impl StoreConfig {
    #[must_use]
    pub fn new(path: Utf8PathBuf) -> Self {
        Self {
            path,
            encryption_key: None,
        }
    }

    /// Create a new config with encryption enabled.
    #[must_use]
    pub fn with_encryption(path: Utf8PathBuf, encryption_key: Vec<u8>) -> Self {
        Self {
            path,
            encryption_key: Some(encryption_key),
        }
    }
}
